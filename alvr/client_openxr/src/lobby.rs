use crate::{
    anchor_ui::{AnchorUi, PointerInput, QrStatus},
    graphics::{self, ProjectionLayerAlphaConfig, ProjectionLayerBuilder},
    interaction::{self, ButtonAction, InteractionContext},
};
use alvr_common::{
    LEFT_TRIGGER_VALUE_ID, Pose, RIGHT_TRIGGER_VALUE_ID, ViewParams,
    glam::{UVec2, Vec3},
    parking_lot::RwLock,
};
use alvr_graphics::{GraphicsContext, LobbyRenderer, LobbyViewParams, SDR_FORMAT_GL};
use alvr_system_info::Platform;
use openxr as xr;
use std::{rc::Rc, sync::Arc, time::Duration};

// Pinch detection: distance between thumb-tip (joint 5) and index-tip (joint 10).
const PINCH_DISTANCE: f32 = 0.03;
const TRIGGER_THRESHOLD: f32 = 0.8;

// todo: add interaction?
pub struct Lobby {
    xr_session: xr::Session<xr::OpenGlEs>,
    interaction_ctx: Arc<RwLock<InteractionContext>>,
    platform: Platform,
    reference_space: xr::Space,
    // Persistent STAGE space for the anchor chain. STAGE (room/guardian origin)
    // does NOT recenter on headset re-illumination — unlike LOCAL_FLOOR — so the
    // anchor stays physically fixed across take-off/wake without re-scanning. It
    // is also the frame ALVR streams in, matching what UE receives.
    stage_space: xr::Space,
    swapchains: [xr::Swapchain<xr::OpenGlEs>; 2],
    view_resolution: UVec2,
    reference_space_type: xr::ReferenceSpaceType,
    renderer: LobbyRenderer,
    anchor_ui: AnchorUi,
    // Phase A = pre-connection anchor check (HUD fully driven by anchor_ui).
    // Phase B = after resume() (HUD driven by ALVR connection messages).
    anchor_phase_a: bool,
    last_qr_update: std::time::Instant,
    // Rising-edge tracker so each lock logs a single COMMIT line (not one per tick).
    qr_was_stable: bool,
    // Recent QR world-pose candidates for the stability gate. We only commit a
    // QR transform once consecutive readings agree (i.e. the head is still),
    // which filters out the large transient swings seen during fast motion
    // (camera capture-to-decode latency vs. render-time head pose).
    qr_stable_buf: std::collections::VecDeque<(u32, Pose, std::time::Instant)>,
    // One-shot guard so the established origin is pushed to the anchor service
    // once (not every render frame). Reset to false whenever the origin clears
    // (e.g. during reconfigure) so a freshly established origin re-publishes.
    origin_pushed: bool,
    // Decode instant of the last camera reading we consumed, to avoid pushing the
    // same detection twice (detection is slower than the 300ms tick while streaming).
    last_cam_instant: Option<std::time::Instant>,
}

/// Result of one tick of the marker coordinate chain (`process_marker`).
#[cfg(target_os = "android")]
enum AnchorTick {
    /// Throttled — not enough time since the last tick; nothing computed.
    Throttled,
    /// Ran, but no fresh/valid marker in view (buffers cleared).
    Lost,
    /// A marker reading in STAGE (with the stability-gate `stable` flag).
    Marker(QrStatus),
}

impl Lobby {
    pub fn new(
        xr_session: xr::Session<xr::OpenGlEs>,
        gfx_ctx: Rc<GraphicsContext>,
        interaction_ctx: Arc<RwLock<InteractionContext>>,
        platform: Platform,
        view_resolution: UVec2,
        initial_hud_message: &str,
    ) -> Self {
        let reference_space_type = if xr_session.instance().exts().ext_local_floor.is_some() {
            xr::ReferenceSpaceType::LOCAL_FLOOR_EXT
        } else {
            // The Quest 1 doesn't support LOCAL_FLOOR_EXT, recentering is required for AppLab, but
            // the Quest 1 is excluded from AppLab anyway.
            xr::ReferenceSpaceType::STAGE
        };

        let reference_space = interaction::get_reference_space(&xr_session, reference_space_type);
        let stage_space =
            interaction::get_reference_space(&xr_session, xr::ReferenceSpaceType::STAGE);

        let swapchains = [
            graphics::create_swapchain(&xr_session, &gfx_ctx, view_resolution, SDR_FORMAT_GL, None),
            graphics::create_swapchain(&xr_session, &gfx_ctx, view_resolution, SDR_FORMAT_GL, None),
        ];

        let renderer = LobbyRenderer::new(
            gfx_ctx,
            view_resolution,
            [
                swapchains[0]
                    .enumerate_images()
                    .unwrap()
                    .iter()
                    .map(|i| *i as _)
                    .collect(),
                swapchains[1]
                    .enumerate_images()
                    .unwrap()
                    .iter()
                    .map(|i| *i as _)
                    .collect(),
            ],
            initial_hud_message,
        );

        Self {
            xr_session,
            interaction_ctx,
            platform,
            reference_space,
            stage_space,
            swapchains,
            view_resolution,
            reference_space_type,
            renderer,
            anchor_ui: AnchorUi::new(),
            anchor_phase_a: true,
            last_qr_update: std::time::Instant::now(),
            qr_was_stable: false,
            qr_stable_buf: std::collections::VecDeque::new(),
            origin_pushed: false,
            last_cam_instant: None,
        }
    }

    /// True once the anchor check phase has located/created an anchor.
    pub fn is_anchor_ready(&self) -> bool {
        self.anchor_ui.is_ready()
    }

    /// Called by the main loop after resume() to hand HUD control back to ALVR.
    pub fn end_anchor_phase_a(&mut self) {
        self.anchor_phase_a = false;
    }

    /// T3.3: re-enter marker scanning while streaming to recompute the origin
    /// (the volume +−+−+− gesture). Keeps the saved config; matching any known
    /// marker re-establishes the origin. The connection is NOT dropped.
    pub fn begin_realign(&mut self) {
        self.anchor_ui.begin_realign();
        self.anchor_phase_a = true;
        self.origin_pushed = false;
        self.qr_stable_buf.clear();
        self.qr_was_stable = false;
    }

    /// Run the throttled coordinate chain + stability gate for one tick (the lobby
    /// render path). Returns the per-tick marker status; mutates the stability
    /// buffer / lock state.
    #[cfg(target_os = "android")]
    fn process_marker(&mut self, xr_vsync_time: xr::Time) -> AnchorTick {
        if self.last_qr_update.elapsed() <= Duration::from_millis(300) {
            return AnchorTick::Throttled;
        }
        self.last_qr_update = std::time::Instant::now();

        // Age-prune the stability window. We do NOT clear the buffer on a missed
        // frame (only drop readings older than STABLE_WINDOW), so the sparse
        // detection seen while streaming can still accumulate a lock over a longer
        // span. The per-reading tightness (same id, <1cm, 4 samples) is unchanged.
        const STABLE_WINDOW: Duration = Duration::from_secs(5);
        let now_i = std::time::Instant::now();
        while self
            .qr_stable_buf
            .front()
            .map(|(_, _, t)| now_i.duration_since(*t) > STABLE_WINDOW)
            .unwrap_or(false)
        {
            self.qr_stable_buf.pop_front();
        }

        // A reading is "live" only if the detection thread decoded it recently;
        // otherwise the marker has left the camera view.
        let latest = (*crate::camera::LATEST_QR_IN_CAM.lock())
            .filter(|(_, _, _, t)| t.elapsed() < Duration::from_millis(700));

        // Locate the head in STAGE (persistent across re-illumination), not the
        // lobby's LOCAL_FLOOR space, so the anchor lives in the frame ALVR streams
        // in and survives take-off/wake.
        let stage_views = self
            .xr_session
            .locate_views(
                xr::ViewConfigurationType::PRIMARY_STEREO,
                xr_vsync_time,
                &self.stage_space,
            )
            .ok()
            .filter(|(f, _)| f.contains(xr::ViewStateFlags::ORIENTATION_VALID))
            .map(|(_, v)| v);

        let (Some((id, size_m, qr_in_cam, cam_t)), Some(sv)) = (latest, stage_views) else {
            // Missed frame: keep the buffer (it ages out via STABLE_WINDOW) so a
            // brief occlusion doesn't reset progress toward a lock.
            return AnchorTick::Lost;
        };

        // Skip if it's the same camera frame we already consumed: while streaming
        // the detection rate is below the 300ms tick rate, so the same reading
        // lingers for several ticks — counting it repeatedly would fake a 4-sample
        // lock from a single measurement.
        if self.last_cam_instant == Some(cam_t) {
            return AnchorTick::Throttled;
        }
        self.last_cam_instant = Some(cam_t);

        // HMD center ≈ midpoint of the two eye poses, in STAGE space.
        let head = Pose {
            position: (crate::from_xr_vec3(sv[0].pose.position)
                + crate::from_xr_vec3(sv[1].pose.position))
                * 0.5,
            orientation: crate::from_xr_quat(sv[0].pose.orientation),
        };
        // Camera relative to HMD, in OpenXR head frame.
        //
        // The raw Camera2 LENS_POSE_ROTATION maps the device frame to the OpenCV
        // optical frame (≈180° about X). But `qr_in_cam` was already converted to
        // the OpenXR camera frame in camera.rs (its own 180°-X flip), so the raw
        // lens rotation flips a second time. Cancelling the gross 180° leaves the
        // camera's real downward tilt (~11° about X); we need its INVERSE here
        // (camera->head un-tilts), else the QR floats ~25cm high.
        let lens_rot =
            alvr_common::glam::Quat::from_xyzw(-0.995406, 0.001095, -0.004451, 0.095631);
        let flip_x = alvr_common::glam::Quat::from_rotation_x(std::f32::consts::PI);
        let cam_in_head = Pose {
            position: Vec3::new(-0.031597, -0.018107, -0.063111),
            orientation: (flip_x * lens_rot).inverse().normalize(),
        };
        let qr_in_stage = head * cam_in_head * qr_in_cam;

        // Gravity constraint: STAGE is gravity-aligned (Y = up), so we replace the
        // ill-conditioned out-of-plane PnP rotation with a clean frame derived from
        // the known up vector (wall: keep facing yaw, force up = +Y; floor: keep
        // heading, normal = ±Y). Marker must be mounted plumb/level. Position kept.
        let qr_in_stage = Pose {
            position: qr_in_stage.position,
            orientation: gravity_align(qr_in_stage.orientation),
        };

        // Stability gate: a short sliding window (4 frames @ 300ms ≈ 1.2s still).
        // Commit only when all share the same marker id and the positions cluster
        // tightly (< 1cm). Rejects the transient swings from camera latency.
        const STABLE_FRAMES: usize = 4;
        const STABLE_SPREAD_M: f32 = 0.01;

        self.qr_stable_buf.push_back((id, qr_in_stage, now_i));
        while self.qr_stable_buf.len() > STABLE_FRAMES {
            self.qr_stable_buf.pop_front();
        }

        let stable = self.qr_stable_buf.len() == STABLE_FRAMES
            && self.qr_stable_buf.iter().all(|(i, _, _)| *i == id)
            && self
                .qr_stable_buf
                .iter()
                .all(|(_, p, _)| (p.position - qr_in_stage.position).length() < STABLE_SPREAD_M);

        // When stable, commit the AVERAGE of the window (halves position noise and
        // cancels the symmetric ±pitch flips of the planar-PnP ambiguity).
        let commit_pose = if stable {
            average_poses(&self.qr_stable_buf)
        } else {
            qr_in_stage
        };

        if stable && !self.qr_was_stable {
            // Log once per lock (rising edge) for repeatability comparison.
            alvr_common::info!(
                "lobby: [chain] marker id={id} LOCK world-pos=({:.3},{:.3},{:.3})m quat=({:.3},{:.3},{:.3},{:.3})",
                commit_pose.position.x,
                commit_pose.position.y,
                commit_pose.position.z,
                commit_pose.orientation.x,
                commit_pose.orientation.y,
                commit_pose.orientation.z,
                commit_pose.orientation.w
            );
        }
        self.qr_was_stable = stable;

        AnchorTick::Marker(QrStatus {
            id,
            size_m,
            pose: commit_pose,
            stable,
        })
    }

    /// Suffix appended to ALVR connection HUD messages in phase B.
    pub fn anchor_status_suffix(&self) -> String {
        self.anchor_ui.status_suffix()
    }

    pub fn update_reference_space(&mut self) {
        self.reference_space =
            interaction::get_reference_space(&self.xr_session, self.reference_space_type);
    }


    pub fn update_hud_message(&self, message: &str) {
        self.renderer.update_hud_message(message);
    }

    pub fn render(&mut self, vsync_time: Duration) -> ProjectionLayerBuilder<'_> {
        let xr_vsync_time = crate::to_xr_time(vsync_time);

        let (flags, maybe_views) = self
            .xr_session
            .locate_views(
                xr::ViewConfigurationType::PRIMARY_STEREO,
                xr_vsync_time,
                &self.reference_space,
            )
            .unwrap();

        let views = if flags.contains(xr::ViewStateFlags::ORIENTATION_VALID) {
            maybe_views
        } else {
            vec![crate::default_view(), crate::default_view()]
        };

        // Marker anchor coordinate chain → world (STAGE): feed the per-tick result
        // to the anchor UI.
        #[cfg(target_os = "android")]
        match self.process_marker(xr_vsync_time) {
            AnchorTick::Throttled => {}
            AnchorTick::Lost => self.anchor_ui.set_qr(None),
            AnchorTick::Marker(status) => self.anchor_ui.set_qr(Some(status)),
        }

        self.xr_session
            .sync_actions(&[(&self.interaction_ctx.read().action_set).into()])
            .ok();

        // future_time doesn't have to be any particular value, just something after vsync_time
        let future_time = vsync_time + Duration::from_millis(80);
        let left_hand_data = interaction::get_hand_data(
            &self.xr_session,
            self.platform,
            &self.reference_space,
            vsync_time,
            future_time,
            &self.interaction_ctx.read().hands_interaction[0],
            &mut Pose::default(),
            &mut Pose::default(),
        );
        let right_hand_data = interaction::get_hand_data(
            &self.xr_session,
            self.platform,
            &self.reference_space,
            vsync_time,
            future_time,
            &self.interaction_ctx.read().hands_interaction[1],
            &mut Pose::default(),
            &mut Pose::default(),
        );

        let mut additional_motions = vec![];
        if let Some(source) = &self.interaction_ctx.read().body_source {
            additional_motions.extend(
                interaction::get_bd_motion_trackers(source, vsync_time)
                    .iter()
                    .map(|(_, motion)| *motion),
            )
        }

        let body_skeleton = self
            .interaction_ctx
            .read()
            .body_source
            .as_ref()
            .and_then(|source| {
                interaction::get_body_skeleton(source, &self.reference_space, vsync_time)
            });

        // Anchor UI: collect pointer rays + select state (trigger / pinch).
        // Controllers use the aim pose; hand tracking falls back to the wrist joint
        // (the controller aim action is not active in hands-only mode).
        let mut pointers = Vec::new();
        {
            let ctx = self.interaction_ctx.read();
            for (i, hand_data) in [&left_hand_data, &right_hand_data].iter().enumerate() {
                let aim = ctx.hands_interaction[i]
                    .aim_space
                    .locate(&self.reference_space, xr_vsync_time)
                    .ok()
                    .filter(|loc| {
                        loc.location_flags.contains(
                            xr::SpaceLocationFlags::ORIENTATION_VALID
                                | xr::SpaceLocationFlags::POSITION_VALID,
                        )
                    });

                let ray = if let Some(loc) = aim {
                    // Controller (or runtime-provided hand aim)
                    let pose = crate::from_xr_pose(loc.pose);
                    Some((pose.position, (pose.orientation * Vec3::NEG_Z).normalize()))
                } else if let Some(j) = hand_data.skeleton_joints.as_ref() {
                    // Hand-tracking fallback: ray from the wrist along the distal (-Z) axis
                    let wrist = j[1];
                    Some((wrist.position, (wrist.orientation * Vec3::NEG_Z).normalize()))
                } else {
                    None
                };

                let Some((origin, direction)) = ray else {
                    continue;
                };

                let trigger_id = if i == 0 {
                    *LEFT_TRIGGER_VALUE_ID
                } else {
                    *RIGHT_TRIGGER_VALUE_ID
                };
                let trigger_val = match ctx.button_actions.get(&trigger_id) {
                    Some(ButtonAction::Scalar(a)) => a
                        .state(&self.xr_session, xr::Path::NULL)
                        .map(|s| s.current_state)
                        .unwrap_or(0.0),
                    _ => 0.0,
                };

                let pinch = hand_data
                    .skeleton_joints
                    .as_ref()
                    .map(|j| j[5].position.distance(j[10].position) < PINCH_DISTANCE)
                    .unwrap_or(false);

                pointers.push(PointerInput {
                    origin,
                    direction,
                    select: trigger_val > TRIGGER_THRESHOLD || pinch,
                });
            }
        }

        // The anchor poses are stored in STAGE; the lobby renders in LOCAL_FLOOR.
        // Locate the STAGE origin in the render space to map the gizmo each frame
        // (this is what keeps the gizmo on the physical marker after re-centering).
        let stage_to_render = self
            .stage_space
            .locate(&self.reference_space, xr_vsync_time)
            .ok()
            .filter(|loc| {
                loc.location_flags.contains(
                    xr::SpaceLocationFlags::ORIENTATION_VALID
                        | xr::SpaceLocationFlags::POSITION_VALID,
                )
            })
            .map(|loc| crate::from_xr_pose(loc.pose))
            .unwrap_or(Pose::IDENTITY);

        // Head pose in the render space (for the head-locked button panel).
        let head_pose_render = Pose {
            position: (crate::from_xr_vec3(views[0].pose.position)
                + crate::from_xr_vec3(views[1].pose.position))
                * 0.5,
            orientation: crate::from_xr_quat(views[0].pose.orientation),
        };

        let anchor_lines = self
            .anchor_ui
            .update(&pointers, head_pose_render, stage_to_render);

        // Publish the established game origin to the anchor responder (once).
        match self.anchor_ui.current_origin() {
            Some(origin) if !self.origin_pushed => {
                alvr_client_core::anchor_service::get().update("origin".to_owned(), origin);
                self.origin_pushed = true;
            }
            None => self.origin_pushed = false,
            _ => {}
        }

        if self.anchor_phase_a {
            self.renderer.update_hud_message(&self.anchor_ui.hud_text());
        }

        let left_swapchain_idx = self.swapchains[0].acquire_image().unwrap();
        let right_swapchain_idx = self.swapchains[1].acquire_image().unwrap();

        self.swapchains[0]
            .wait_image(xr::Duration::INFINITE)
            .unwrap();
        self.swapchains[1]
            .wait_image(xr::Duration::INFINITE)
            .unwrap();

        self.renderer.render(
            [
                LobbyViewParams {
                    view_params: ViewParams {
                        pose: crate::from_xr_pose(views[0].pose),
                        fov: crate::from_xr_fov(views[0].fov),
                    },
                    swapchain_index: left_swapchain_idx,
                },
                LobbyViewParams {
                    view_params: ViewParams {
                        pose: crate::from_xr_pose(views[1].pose),
                        fov: crate::from_xr_fov(views[1].fov),
                    },
                    swapchain_index: right_swapchain_idx,
                },
            ],
            [left_hand_data, right_hand_data],
            body_skeleton,
            additional_motions,
            false,
            cfg!(debug_assertions),
            &anchor_lines,
        );

        self.swapchains[0].release_image().unwrap();
        self.swapchains[1].release_image().unwrap();

        let rect = xr::Rect2Di {
            offset: xr::Offset2Di { x: 0, y: 0 },
            extent: xr::Extent2Di {
                width: self.view_resolution.x as _,
                height: self.view_resolution.y as _,
            },
        };

        ProjectionLayerBuilder::new(
            &self.reference_space,
            [
                xr::CompositionLayerProjectionView::new()
                    .pose(views[0].pose)
                    .fov(views[0].fov)
                    .sub_image(
                        xr::SwapchainSubImage::new()
                            .swapchain(&self.swapchains[0])
                            .image_array_index(0)
                            .image_rect(rect),
                    ),
                xr::CompositionLayerProjectionView::new()
                    .pose(views[1].pose)
                    .fov(views[1].fov)
                    .sub_image(
                        xr::SwapchainSubImage::new()
                            .swapchain(&self.swapchains[1])
                            .image_array_index(0)
                            .image_rect(rect),
                    ),
            ],
            Some(ProjectionLayerAlphaConfig {
                premultiplied: true,
            }),
            None,
        )
    }
}

/// Snap a measured marker orientation to a gravity-consistent frame (STAGE is
/// Y-up / gravity-aligned). Removes the noisy out-of-plane pitch/roll of planar
/// PnP, keeping only the well-conditioned heading. Assumes the marker is mounted
/// plumb (wall) or level (floor).
#[cfg(target_os = "android")]
fn gravity_align(rot: alvr_common::glam::Quat) -> alvr_common::glam::Quat {
    use alvr_common::glam::{Mat3, Quat, Vec3};

    let normal = (rot * Vec3::Z).normalize_or_zero();
    if normal.length_squared() < 1e-6 {
        return rot;
    }

    // |normal.y| large => marker plane is horizontal (floor); else vertical (wall).
    const FLOOR_NORMAL_Y: f32 = 0.7;
    let (x, y, z) = if normal.y.abs() < FLOOR_NORMAL_Y {
        // WALL: up = world +Y, facing = horizontal component of the normal.
        let z = Vec3::new(normal.x, 0.0, normal.z).normalize_or_zero();
        if z.length_squared() < 1e-6 {
            return rot;
        }
        let y = Vec3::Y;
        let x = y.cross(z).normalize();
        (x, y, x.cross(y))
    } else {
        // FLOOR: normal = ±world Y, heading from the marker's up edge.
        let m_up = rot * Vec3::Y;
        let heading = Vec3::new(m_up.x, 0.0, m_up.z).normalize_or_zero();
        if heading.length_squared() < 1e-6 {
            return rot;
        }
        let z = Vec3::new(0.0, normal.y.signum(), 0.0);
        let y = heading;
        let x = y.cross(z).normalize();
        (x, y, x.cross(y))
    };

    Quat::from_mat3(&Mat3::from_cols(x, y, z)).normalize()
}

/// Average a window of QR world poses: arithmetic mean of positions and a
/// sign-aligned normalised-linear mean of orientations (valid for the small
/// spreads seen within a stable window). Reduces per-frame PnP noise.
#[cfg(target_os = "android")]
fn average_poses(buf: &std::collections::VecDeque<(u32, Pose, std::time::Instant)>) -> Pose {
    use alvr_common::glam::{Quat, Vec3, Vec4};

    if buf.is_empty() {
        return Pose::IDENTITY;
    }
    let n = buf.len() as f32;
    let ref_q = buf.front().unwrap().1.orientation;

    let mut pos = Vec3::ZERO;
    let mut q_acc = Vec4::ZERO;
    for (_, p, _) in buf {
        pos += p.position;
        let q = p.orientation;
        // Flip antipodal quaternions so the linear mean doesn't cancel.
        let q = if q.dot(ref_q) < 0.0 { -q } else { q };
        q_acc += Vec4::new(q.x, q.y, q.z, q.w);
    }

    Pose {
        position: pos / n,
        orientation: Quat::from_xyzw(q_acc.x, q_acc.y, q_acc.z, q_acc.w).normalize(),
    }
}
