//! T3.4/T3.5 — Anchor setup wizard + startup screen (pre-connection phase).
//!
//! Drives the lobby HUD and an in-world gizmo through a small state machine:
//!
//!   Startup ──[Reconfigure]──▶ ConfirmReset ──[Confirm]──▶ Wizard(ScanPrimary)
//!      │                                          │
//!   primary found                         ScanPrimary → PlaceOrigin → PlaceForward
//!      ▼                                          → AskAux ⇄ ScanAux ──[Finish]─┐
//!    Ready ◀───────────────────────────────────────────────────────────────────┘
//!
//! - **Startup**: a saved primary marker exists → search for it; once it's seen
//!   and stable, apply the saved origin offset and go Ready (resume streaming).
//!   A [Reconfigure] button wipes the saved config and enters the wizard.
//! - **Wizard**: scan + confirm a primary marker, then two-point placement of the
//!   game origin (point on floor + a forward point), then optionally add auxiliary
//!   markers (each storing its offset to the primary). Finishing saves the config.
//!
//! Rendering constraint: the lobby renderer only draws line segments + one HUD
//! text block — no per-button text. So buttons are colour-coded line-boxes on a
//! head-locked panel (GREEN = primary action, RED = secondary, BLUE = tertiary),
//! and the HUD text says what each colour does in the current step.
//!
//! Frames: marker poses, the origin and placed points live in STAGE. Pointer rays
//! and the head pose arrive in the render (LOCAL_FLOOR) space; `stage_to_render`
//! maps STAGE → render for drawing, and its inverse maps rays → STAGE for the
//! floor raycast used by two-point placement.

use alvr_client_core::anchor_config::{self, AnchorConfig, AuxiliaryMarker, MarkerRef};
use alvr_common::{
    Pose,
    glam::{Mat3, Quat, Vec3},
};

const RAY_MAX_LEN: f32 = 5.0;
const LIVE_AXIS_LEN: f32 = 0.12; // current detection (jittery preview)
const MARKER_AXIS_LEN: f32 = 0.18; // captured / saved marker
const ORIGIN_AXIS_LEN: f32 = 0.30; // game origin
const POINT_CROSS: f32 = 0.05; // placed-point crosshair half-size
const THICK: f32 = 0.004;

const RED: [u8; 4] = [255, 60, 60, 255];
const GREEN: [u8; 4] = [0, 230, 90, 255];
const BLUE: [u8; 4] = [60, 140, 255, 255];
const GREY: [u8; 4] = [120, 120, 120, 255];
const WHITE: [u8; 4] = [235, 235, 235, 255];
const YELLOW: [u8; 4] = [255, 210, 40, 255];

// Head-locked button panel geometry (render space, metres).
const PANEL_DIST: f32 = 1.1; // forward distance from head
const PANEL_DROP: f32 = 0.30; // below eye line
const BTN_HW: f32 = 0.11; // half width
const BTN_HH: f32 = 0.07; // half height
const BTN_SPACING: f32 = 0.30; // centre-to-centre

/// One pointer (controller or hand) for the current frame.
pub struct PointerInput {
    pub origin: Vec3,
    /// Normalised ray direction in render space.
    pub direction: Vec3,
    /// True while trigger is pressed past threshold, or fingers are pinched.
    pub select: bool,
}

/// (from, to, rgba) — fed to `LobbyRenderer::render`'s `extra_lines`.
pub type Line = (Vec3, Vec3, [u8; 4]);

/// Latest stable marker detection passed in from the coordinate chain (in STAGE).
#[derive(Clone, Copy)]
pub struct QrStatus {
    pub id: u32,
    pub size_m: f32,
    pub pose: Pose,
    pub stable: bool,
}

/// Non-unique display alias for a marker id (id % 26 → 'A'..'Z'). HUD only.
fn id_letter(id: u32) -> char {
    (b'A' + (id % 26) as u8) as char
}

/// HUD control prefix understood by the lobby renderer: a line beginning with it
/// is drawn at a smaller size. Used for the English line of each bilingual block.
const EN_SMALL: &str = "\u{1}";

/// One bilingual HUD block: a full-size Chinese line, then a smaller English line
/// (stacked, so neither overflows the HUD width).
fn bz(zh: &str, en: &str) -> String {
    format!("{zh}\n{EN_SMALL}{en}")
}

/// Which colour-coded button was clicked this frame.
#[derive(Clone, Copy, PartialEq)]
enum Btn {
    Green,
    Red,
    Blue,
}

/// Wizard steps (only meaningful inside `Phase::Wizard`).
#[derive(Clone, Copy, PartialEq)]
enum Step {
    ScanPrimary,
    PlaceOrigin,
    PlaceForward,
    AskAux,
    ScanAux,
}

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Startup,
    ConfirmReset,
    Wizard(Step),
    Ready,
}

pub struct AnchorUi {
    phase: Phase,
    /// Loaded saved config (None = unconfigured / just wiped).
    config: Option<AnchorConfig>,
    /// Latest stable detection from the chain this tick (None if marker lost).
    latest: Option<QrStatus>,

    // --- wizard accumulators (STAGE frame) ---
    /// A scanned marker awaiting [Confirm]/[Rescan] in a scan step.
    captured: Option<(u32, f32, Pose)>,
    primary: Option<MarkerRef>,
    primary_pose: Option<Pose>,
    origin_pos: Option<Vec3>,
    forward_pos: Option<Vec3>,
    auxiliary: Vec<AuxiliaryMarker>,

    /// Established game origin in STAGE (Some once Ready). Pushed to anchor_service.
    origin_pose: Option<Pose>,

    /// Per-pointer previous select state, for click (rising-edge) detection.
    prev_select: [bool; 2],
}

impl AnchorUi {
    pub fn new() -> Self {
        let config = anchor_config::get();
        let phase = if config.is_some() {
            Phase::Startup
        } else {
            Phase::Wizard(Step::ScanPrimary)
        };
        Self {
            phase,
            config,
            latest: None,
            captured: None,
            primary: None,
            primary_pose: None,
            origin_pos: None,
            forward_pos: None,
            auxiliary: Vec::new(),
            origin_pose: None,
            prev_select: [false; 2],
        }
    }

    pub fn is_ready(&self) -> bool {
        self.phase == Phase::Ready
    }

    /// Established game origin in STAGE, once ready. Lobby pushes this to the
    /// anchor responder service for UE.
    pub fn current_origin(&self) -> Option<Pose> {
        self.origin_pose
    }

    /// Feed the latest stable marker detection (called every chain tick).
    pub fn set_qr(&mut self, qr: Option<QrStatus>) {
        self.latest = qr;
    }

    /// T3.3: re-enter marker search to recompute the origin while streaming.
    /// Keeps the saved config; clears the current origin so the freshly matched
    /// one is what gets re-published. No-op if not configured.
    pub fn begin_realign(&mut self) {
        if self.config.is_some() {
            self.origin_pose = None;
            self.phase = Phase::Startup;
        }
    }

    /// Per-frame update. Processes pointer clicks against the colour-coded button
    /// panel and the floor (two-point placement), advances the state machine, and
    /// returns the line segments to draw (rays + gizmo + buttons).
    pub fn update(
        &mut self,
        pointers: &[PointerInput],
        head_pose: Pose,
        stage_to_render: Pose,
    ) -> Vec<Line> {
        // Auto-capture / startup matching from the latest stable reading.
        self.consume_latest();

        // Lay out the buttons for the current state, in render space.
        let buttons = self.layout_buttons(head_pose);

        // Resolve hover (any pointer) and click (rising edge per pointer).
        let mut hovered: Option<Btn> = None;
        let mut clicked: Option<Btn> = None;
        let mut floor_click_ray: Option<(Vec3, Vec3)> = None;
        for (i, p) in pointers.iter().take(2).enumerate() {
            let hit = buttons
                .iter()
                .find(|b| ray_hits_button(p.origin, p.direction, b))
                .map(|b| b.btn);
            if hit.is_some() && hovered.is_none() {
                hovered = hit;
            }
            let rising = p.select && !self.prev_select[i];
            if rising {
                if let Some(b) = hit {
                    clicked = Some(b);
                } else {
                    floor_click_ray = Some((p.origin, p.direction));
                }
            }
            self.prev_select[i] = p.select;
        }
        // Clear stale prev_select for absent pointers.
        for i in pointers.len()..2 {
            self.prev_select[i] = false;
        }

        if let Some(btn) = clicked {
            self.on_button(btn);
        } else if let Some((o, d)) = floor_click_ray {
            self.on_floor_click(o, d, stage_to_render);
        }

        // Build the frame's line set.
        let mut lines = Vec::new();
        for p in pointers {
            let end = p.origin + p.direction * RAY_MAX_LEN;
            push_thick_line(&mut lines, p.origin, end, GREY);
        }
        self.draw_gizmo(&mut lines, stage_to_render);
        for b in &buttons {
            let bright = hovered == Some(b.btn);
            push_button(&mut lines, b, bright);
        }
        lines
    }

    /// Auto-capture a stable marker in scan steps, and match the saved primary
    /// during startup.
    fn consume_latest(&mut self) {
        let Some(s) = self.latest else { return };
        if !s.stable {
            return;
        }
        match self.phase {
            Phase::Startup => {
                if let Some(cfg) = &self.config {
                    // Any known marker (primary OR auxiliary) re-establishes the
                    // origin — at startup and during T3.3 re-align.
                    if cfg.knows_marker(s.id) {
                        if let Some(origin) = cfg.origin_in_stage(s.id, s.pose) {
                            alvr_common::info!(
                                "AnchorUi: matched marker id={} — origin established",
                                s.id
                            );
                            self.origin_pose = Some(origin);
                            self.phase = Phase::Ready;
                        }
                    }
                }
            }
            Phase::Wizard(Step::ScanPrimary) | Phase::Wizard(Step::ScanAux) => {
                if self.captured.is_none() {
                    self.captured = Some((s.id, s.size_m, s.pose));
                }
            }
            _ => {}
        }
    }

    fn on_button(&mut self, btn: Btn) {
        match self.phase {
            Phase::Startup => {
                if btn == Btn::Blue {
                    self.phase = Phase::ConfirmReset;
                }
            }
            Phase::ConfirmReset => match btn {
                Btn::Green => {
                    anchor_config::clear();
                    self.config = None;
                    self.reset_wizard();
                    self.phase = Phase::Wizard(Step::ScanPrimary);
                }
                Btn::Red => self.phase = Phase::Startup,
                _ => {}
            },
            Phase::Wizard(Step::ScanPrimary) => {
                if let Some((id, size_m, pose)) = self.captured {
                    match btn {
                        Btn::Green => {
                            self.primary = Some(MarkerRef { id, size_m });
                            self.primary_pose = Some(pose);
                            self.captured = None;
                            self.phase = Phase::Wizard(Step::PlaceOrigin);
                        }
                        Btn::Red => self.captured = None, // rescan
                        _ => {}
                    }
                }
            }
            Phase::Wizard(Step::PlaceOrigin) => {
                if btn == Btn::Green && self.origin_pos.is_some() {
                    self.phase = Phase::Wizard(Step::PlaceForward);
                }
            }
            Phase::Wizard(Step::PlaceForward) => {
                if btn == Btn::Green && self.forward_pos.is_some() {
                    self.phase = Phase::Wizard(Step::AskAux);
                }
            }
            Phase::Wizard(Step::AskAux) => match btn {
                Btn::Green => {
                    self.captured = None;
                    self.phase = Phase::Wizard(Step::ScanAux);
                }
                Btn::Red => self.finish(),
                _ => {}
            },
            Phase::Wizard(Step::ScanAux) => {
                if let Some((id, size_m, pose)) = self.captured {
                    if self.is_known(id) {
                        // Duplicate: GREEN = rescan, RED = cancel (back to step 4).
                        match btn {
                            Btn::Green => self.captured = None,
                            Btn::Red => {
                                self.captured = None;
                                self.phase = Phase::Wizard(Step::AskAux);
                            }
                            _ => {}
                        }
                    } else {
                        // New marker: GREEN = confirm, RED = rescan.
                        match btn {
                            Btn::Green => {
                                // offset_to_primary: primary = aux * offset → offset = aux⁻¹ · primary
                                if let Some(pp) = self.primary_pose {
                                    self.auxiliary.push(AuxiliaryMarker {
                                        marker: MarkerRef { id, size_m },
                                        offset_to_primary: pose.inverse() * pp,
                                    });
                                }
                                self.captured = None;
                                self.phase = Phase::Wizard(Step::AskAux);
                            }
                            Btn::Red => self.captured = None, // rescan
                            _ => {}
                        }
                    }
                }
            }
            Phase::Ready => {}
        }
    }

    /// Floor raycast (STAGE y=0) for two-point placement clicks.
    fn on_floor_click(&mut self, ray_o: Vec3, ray_d: Vec3, stage_to_render: Pose) {
        let Some(hit) = floor_hit_stage(ray_o, ray_d, stage_to_render) else {
            return;
        };
        match self.phase {
            Phase::Wizard(Step::PlaceOrigin) => self.origin_pos = Some(hit),
            Phase::Wizard(Step::PlaceForward) => self.forward_pos = Some(hit),
            _ => {}
        }
    }

    /// Finalize the wizard: compute the primary→origin offset, save the config,
    /// publish the origin, go Ready.
    fn finish(&mut self) {
        let (Some(primary), Some(pp), Some(origin)) =
            (self.primary, self.primary_pose, self.origin_pose_from_points())
        else {
            alvr_common::warn!("AnchorUi: finish() with incomplete data — staying in wizard");
            return;
        };
        let cfg = AnchorConfig {
            primary,
            origin_offset: pp.inverse() * origin, // primary⁻¹ · origin
            auxiliary: std::mem::take(&mut self.auxiliary),
        };
        anchor_config::save(cfg.clone());
        self.config = Some(cfg);
        self.origin_pose = Some(origin);
        self.phase = Phase::Ready;
    }

    /// Origin pose in STAGE built from the two placed floor points (position =
    /// point1, +Z forward = point1→point2 horizontal, +Y up).
    fn origin_pose_from_points(&self) -> Option<Pose> {
        let (o, f) = (self.origin_pos?, self.forward_pos?);
        let mut fwd = f - o;
        fwd.y = 0.0;
        let fwd = fwd.normalize_or_zero();
        if fwd.length_squared() < 1e-6 {
            return None;
        }
        // OpenXR convention: local -Z is forward, +X right, +Y up. Building the
        // frame this way maps cleanly to UE (forward→+X, right→+Y, up→+Z); using
        // +Z=forward injects a 180° yaw that shows up in UE as flipped X/Y axes.
        let up = Vec3::Y;
        let z_axis = -fwd; // local +Z points backward
        let x_axis = up.cross(z_axis).normalize(); // local +X = right
        let orientation = Quat::from_mat3(&Mat3::from_cols(x_axis, up, z_axis)).normalize();
        Some(Pose {
            position: o,
            orientation,
        })
    }

    fn is_known(&self, id: u32) -> bool {
        self.primary.map(|m| m.id == id).unwrap_or(false)
            || self.auxiliary.iter().any(|a| a.marker.id == id)
    }

    fn reset_wizard(&mut self) {
        self.captured = None;
        self.primary = None;
        self.primary_pose = None;
        self.origin_pos = None;
        self.forward_pos = None;
        self.auxiliary.clear();
        self.origin_pose = None;
    }

    // ----------------------------------------------------------------------
    // Button layout
    // ----------------------------------------------------------------------

    fn layout_buttons(&self, head_pose: Pose) -> Vec<Button> {
        // Which colours are active in the current state.
        let spec: &[Btn] = match self.phase {
            Phase::Startup => &[Btn::Blue], // Reconfigure
            Phase::ConfirmReset => &[Btn::Green, Btn::Red], // Confirm / Cancel
            Phase::Wizard(Step::ScanPrimary) | Phase::Wizard(Step::ScanAux) => {
                if self.captured.is_some() {
                    &[Btn::Green, Btn::Red] // Confirm / Rescan
                } else {
                    &[]
                }
            }
            Phase::Wizard(Step::PlaceOrigin) => {
                if self.origin_pos.is_some() {
                    &[Btn::Green] // Next
                } else {
                    &[]
                }
            }
            Phase::Wizard(Step::PlaceForward) => {
                if self.forward_pos.is_some() {
                    &[Btn::Green] // Next
                } else {
                    &[]
                }
            }
            Phase::Wizard(Step::AskAux) => &[Btn::Green, Btn::Red], // Add / Finish
            Phase::Ready => &[],
        };
        if spec.is_empty() {
            return Vec::new();
        }

        // Head-locked panel basis.
        let hf = head_pose.orientation * Vec3::NEG_Z;
        let fwd = {
            let h = Vec3::new(hf.x, 0.0, hf.z);
            h.normalize_or_zero()
        };
        let fwd = if fwd.length_squared() < 1e-6 {
            Vec3::NEG_Z
        } else {
            fwd
        };
        let right = fwd.cross(Vec3::Y).normalize_or_zero();
        let up = Vec3::Y;
        let center = head_pose.position + fwd * PANEL_DIST - up * PANEL_DROP;

        let n = spec.len();
        spec.iter()
            .enumerate()
            .map(|(i, &btn)| {
                let off = (i as f32 - (n as f32 - 1.0) / 2.0) * BTN_SPACING;
                Button {
                    btn,
                    center: center + right * off,
                    right,
                    up,
                    normal: fwd,
                    color: match btn {
                        Btn::Green => GREEN,
                        Btn::Red => RED,
                        Btn::Blue => BLUE,
                    },
                }
            })
            .collect()
    }

    // ----------------------------------------------------------------------
    // Gizmo
    // ----------------------------------------------------------------------

    fn draw_gizmo(&self, lines: &mut Vec<Line>, s2r: Pose) {
        // Live detection preview (short, jittery).
        if let Some(l) = &self.latest {
            push_axes(lines, s2r * l.pose, LIVE_AXIS_LEN);
        }
        // Captured marker awaiting confirm.
        if let Some((_, _, pose)) = self.captured {
            push_axes(lines, s2r * pose, MARKER_AXIS_LEN);
        }
        // Placed origin point + forward point + connecting line (STAGE → render).
        if let Some(o) = self.origin_pos {
            push_cross(lines, xf_point(s2r, o), YELLOW);
        }
        if let Some(f) = self.forward_pos {
            push_cross(lines, xf_point(s2r, f), WHITE);
            if let Some(o) = self.origin_pos {
                push_thick_line(lines, xf_point(s2r, o), xf_point(s2r, f), WHITE);
            }
        }
        // Established / previewed origin frame (long axes).
        let origin = self.origin_pose.or_else(|| self.origin_pose_from_points());
        if let Some(p) = origin {
            push_axes(lines, s2r * p, ORIGIN_AXIS_LEN);
        }
    }

    // ----------------------------------------------------------------------
    // HUD text
    // ----------------------------------------------------------------------

    pub fn hud_text(&self) -> String {
        // Each block is a full-size Chinese line + a smaller English line; blocks
        // are separated by a blank line. Single-language per line avoids the
        // horizontal clipping that the old "中文 / English" lines caused.
        let mut blocks: Vec<String> = vec![bz("ALVR Lynx · 锚点", "ALVR Lynx · Anchor")];
        match self.phase {
            Phase::Startup => {
                if let Some(c) = &self.config {
                    let (l, id, cm, n) = (
                        id_letter(c.primary.id),
                        c.primary.id,
                        c.primary.size_m * 100.0,
                        c.auxiliary.len(),
                    );
                    blocks.push(bz(
                        &format!("已保存主码 '{l}' (id {id}, {cm:.0}cm)，辅助 {n} 个"),
                        &format!("Saved primary '{l}' (id {id}, {cm:.0}cm), {n} aux"),
                    ));
                }
                blocks.push(bz(
                    "正在查找主码 — 用头显对准它",
                    "Looking for primary marker — point at it",
                ));
                blocks.push(bz("[蓝] 重新配置", "[BLUE] Reconfigure"));
            }
            Phase::ConfirmReset => {
                blocks.push(bz(
                    "重新配置会清除已保存的锚点设置",
                    "Reconfigure will ERASE the saved setup",
                ));
                blocks.push(bz("[绿] 确认    [红] 取消", "[GREEN] Confirm    [RED] Cancel"));
            }
            Phase::Wizard(Step::ScanPrimary) => {
                blocks.push(bz("步骤 1 — 主码", "Step 1 — Primary marker"));
                if let Some((id, _, _)) = self.captured {
                    blocks.push(bz(
                        &format!("检测到 '{}' (id {id})", id_letter(id)),
                        &format!("Detected '{}' (id {id})", id_letter(id)),
                    ));
                    blocks.push(bz("[绿] 确认    [红] 重扫", "[GREEN] Confirm    [RED] Rescan"));
                } else {
                    blocks.push(bz(
                        "对准主码并保持静止…",
                        "Point at the primary marker, hold still…",
                    ));
                }
            }
            Phase::Wizard(Step::PlaceOrigin) => {
                blocks.push(bz("步骤 2 — 原点", "Step 2 — Origin point"));
                if self.origin_pos.is_some() {
                    blocks.push(bz("已放置；扣扳机点地面可移动", "Placed; trigger on floor to move"));
                    blocks.push(bz("[绿] 下一步", "[GREEN] Next"));
                } else {
                    blocks.push(bz(
                        "瞄准地面扣扳机放置游戏原点",
                        "Aim at floor, pull trigger to place origin",
                    ));
                }
            }
            Phase::Wizard(Step::PlaceForward) => {
                blocks.push(bz("步骤 3 — 正方向", "Step 3 — Forward direction"));
                if self.forward_pos.is_some() {
                    blocks.push(bz("已放置；扣扳机点地面可移动", "Placed; trigger on floor to move"));
                    blocks.push(bz("[绿] 下一步", "[GREEN] Next"));
                } else {
                    blocks.push(bz(
                        "瞄准原点【前方】一点扣扳机",
                        "Aim IN FRONT of origin, pull trigger",
                    ));
                }
            }
            Phase::Wizard(Step::AskAux) => {
                let n = self.auxiliary.len();
                blocks.push(bz(
                    &format!("步骤 4 — 辅助码（已加 {n}）"),
                    &format!("Step 4 — Auxiliary markers ({n} added)"),
                ));
                blocks.push(bz(
                    "[绿] 继续添加    [红] 完成配置",
                    "[GREEN] Add another    [RED] Finish",
                ));
            }
            Phase::Wizard(Step::ScanAux) => {
                blocks.push(bz("步骤 5 — 辅助码", "Step 5 — Auxiliary marker"));
                if let Some((id, _, _)) = self.captured {
                    if self.is_known(id) {
                        blocks.push(bz(
                            &format!("'{}' (id {id}) 已保存", id_letter(id)),
                            &format!("'{}' (id {id}) already saved", id_letter(id)),
                        ));
                        blocks.push(bz(
                            "[绿] 重扫    [红] 取消添加",
                            "[GREEN] Rescan    [RED] Cancel",
                        ));
                    } else {
                        blocks.push(bz(
                            &format!("检测到新码 '{}' (id {id})", id_letter(id)),
                            &format!("Detected new '{}' (id {id})", id_letter(id)),
                        ));
                        blocks.push(bz("[绿] 确认    [红] 重扫", "[GREEN] Confirm    [RED] Rescan"));
                    }
                } else {
                    blocks.push(bz(
                        "对准一个新码并保持静止…",
                        "Point at a new marker, hold still…",
                    ));
                }
            }
            Phase::Ready => {
                let p = self.origin_pose.unwrap_or(Pose::IDENTITY).position;
                blocks.push(bz("[OK] 原点已建立", "[OK] Origin established"));
                blocks.push(format!(
                    "({:.1}, {:.1}, {:.1}) cm",
                    p.x * 100.0,
                    p.y * 100.0,
                    p.z * 100.0
                ));
                blocks.push(bz("连接中…", "connecting…"));
            }
        }
        blocks.join("\n\n")
    }

    /// Short suffix appended to the ALVR connection HUD in the streaming phase.
    pub fn status_suffix(&self) -> String {
        if let Some(p) = self.origin_pose {
            format!(
                "\n\nAnchor [OK] origin ({:.1}, {:.1}, {:.1}) cm",
                p.position.x * 100.0,
                p.position.y * 100.0,
                p.position.z * 100.0
            )
        } else {
            "\n\nAnchor [X] none".to_owned()
        }
    }
}

// --------------------------------------------------------------------------
// Geometry helpers
// --------------------------------------------------------------------------

/// A colour-coded button quad in render space.
struct Button {
    btn: Btn,
    center: Vec3,
    right: Vec3,
    up: Vec3,
    normal: Vec3,
    color: [u8; 4],
}

/// Apply a pose to a point: `pose.position + pose.orientation * v`.
fn xf_point(pose: Pose, v: Vec3) -> Vec3 {
    pose.position + pose.orientation * v
}

/// Ray (render space) vs the floor plane (STAGE y=0), returning the hit in STAGE.
/// `stage_to_render` maps STAGE → render, so its inverse maps the ray → STAGE.
fn floor_hit_stage(ray_o: Vec3, ray_d: Vec3, stage_to_render: Pose) -> Option<Vec3> {
    let r2s = stage_to_render.inverse();
    let o = xf_point(r2s, ray_o);
    let d = (r2s.orientation * ray_d).normalize_or_zero();
    if d.y > -1e-4 {
        return None; // not pointing down at the floor
    }
    let t = -o.y / d.y;
    if t <= 0.0 {
        return None;
    }
    Some(o + d * t)
}

/// Ray (render space) vs a button quad.
fn ray_hits_button(ray_o: Vec3, ray_d: Vec3, b: &Button) -> bool {
    let denom = ray_d.dot(b.normal);
    if denom.abs() < 1e-5 {
        return false;
    }
    let t = (b.center - ray_o).dot(b.normal) / denom;
    if t <= 0.0 {
        return false;
    }
    let hit = ray_o + ray_d * t;
    let local = hit - b.center;
    local.dot(b.right).abs() <= BTN_HW && local.dot(b.up).abs() <= BTN_HH
}

/// Draw the three local axes of `pose` at its origin (X red, Y green, Z blue).
fn push_axes(lines: &mut Vec<Line>, pose: Pose, len: f32) {
    let o = pose.position;
    let r = pose.orientation;
    push_thick_line(lines, o, o + (r * Vec3::X) * len, RED);
    push_thick_line(lines, o, o + (r * Vec3::Y) * len, GREEN);
    push_thick_line(lines, o, o + (r * Vec3::Z) * len, BLUE);
}

/// Draw a small 3-axis-aligned cross at a world point.
fn push_cross(lines: &mut Vec<Line>, p: Vec3, color: [u8; 4]) {
    push_thick_line(lines, p - Vec3::X * POINT_CROSS, p + Vec3::X * POINT_CROSS, color);
    push_thick_line(lines, p - Vec3::Y * POINT_CROSS, p + Vec3::Y * POINT_CROSS, color);
    push_thick_line(lines, p - Vec3::Z * POINT_CROSS, p + Vec3::Z * POINT_CROSS, color);
}

/// Draw a button as a (thick) rectangle outline. When hovered (a pointer ray is
/// on it) it clearly "lights up": colour blends strongly toward white and an
/// inner frame + fill lines are added, so controller AND hand-ray pointing give
/// obvious feedback.
fn push_button(lines: &mut Vec<Line>, b: &Button, bright: bool) {
    let c = if bright {
        let mix = |x: u8| (x as u16 + (255 - x as u16) * 7 / 10) as u8;
        [mix(b.color[0]), mix(b.color[1]), mix(b.color[2]), 255]
    } else {
        b.color
    };
    let rw = b.right * BTN_HW;
    let uh = b.up * BTN_HH;
    let frame = |lines: &mut Vec<Line>, sw: f32, sh: f32| {
        let rw = b.right * (BTN_HW * sw);
        let uh = b.up * (BTN_HH * sh);
        let tl = b.center - rw + uh;
        let tr = b.center + rw + uh;
        let br = b.center + rw - uh;
        let bl = b.center - rw - uh;
        push_thick_line(lines, tl, tr, c);
        push_thick_line(lines, tr, br, c);
        push_thick_line(lines, br, bl, c);
        push_thick_line(lines, bl, tl, c);
    };
    frame(lines, 1.0, 1.0);
    // X through the box so it reads as a target even far away.
    push_thick_line(lines, b.center - rw + uh, b.center + rw - uh, c);
    push_thick_line(lines, b.center + rw + uh, b.center - rw - uh, c);
    if bright {
        // Inner frame + horizontal fill lines make the highlight pop.
        frame(lines, 0.6, 0.6);
        for k in [-0.4_f32, 0.0, 0.4] {
            let off = b.up * (BTN_HH * k);
            push_thick_line(lines, b.center - rw + off, b.center + rw + off, c);
        }
    }
}

/// Fatten a single segment into a small bundle of parallel lines so it is
/// visible in VR (the line pipeline only draws 1px-wide segments).
fn push_thick_line(lines: &mut Vec<Line>, from: Vec3, to: Vec3, color: [u8; 4]) {
    let dir = to - from;
    let mut n1 = dir.cross(Vec3::Y);
    if n1.length_squared() < 1e-8 {
        n1 = dir.cross(Vec3::X);
    }
    let n1 = n1.normalize_or_zero() * THICK;
    let n2 = dir.cross(n1).normalize_or_zero() * THICK;

    for off in [Vec3::ZERO, n1, -n1, n2, -n2] {
        lines.push((from + off, to + off, color));
    }
}
