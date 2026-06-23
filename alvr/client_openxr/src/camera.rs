//! Passthrough Camera Access bridge.
//!
//! Camera2 is driven from a Java helper (CameraHelper.java) because Meta exposes
//! the passthrough vendor tags and the async StateCallback only in Java. The
//! helper is compiled to a dex (scripts/build_camera_helper.ps1), embedded via
//! include_bytes!, loaded with InMemoryDexClassLoader, and called over JNI.
//!
//! A dedicated background thread opens the passthrough camera and continuously
//! decodes QR codes with rqrr. Runs off the render thread so startup isn't
//! blocked. (Next: PnP + coordinate transform to derive the QR world pose.)

#[cfg(target_os = "android")]
const CAMERA_HELPER_DEX: &[u8] = include_bytes!("../assets/camera_helper.dex");

/// Latest detected marker pose in the OpenXR camera frame, as
/// `(id, size_m, pose, decoded_at)`:
///   - `id`      — numeric ArUco id (the real identity; the display `letter` is
///                 just `id % 26` and is NOT unique, so we carry the id itself);
///   - `size_m`  — marker physical edge length in metres (from its id range);
///   - `pose`    — marker-local → camera (OpenXR convention);
///   - `decoded_at` — instant of decode, used to tell whether tracking is fresh.
/// Written by the detection thread, read by the render thread to build the world
/// (STAGE) pose and match against the saved `AnchorConfig`.
#[cfg(target_os = "android")]
pub static LATEST_QR_IN_CAM: alvr_common::parking_lot::Mutex<
    Option<(u32, f32, alvr_common::Pose, std::time::Instant)>,
> = alvr_common::parking_lot::Mutex::new(None);

// Anchor markers are OpenCV DICT_4X4 (4×4 inner grid + 1-cell border = 6×6).
// We feed aruco-rs the DICT_4X4 codebook (see `aruco_dict_4x4.rs`) — its detector
// is dictionary-agnostic. Only the first 250 codes are used (= DICT_4X4_250,
// larger inter-marker distance than _1000, covers all our printed ids 0..249).
#[cfg(target_os = "android")]
const DICT_LEN: usize = 250;
#[cfg(target_os = "android")]
const MARKER_INNER: usize = 4; // 4×4 data cells
#[cfg(target_os = "android")]
const MARKER_TOTAL: f32 = 6.0; // 4 data + 2 border cells

/// DICT_4X4_250 config for aruco-rs (first 250 codes of the embedded table).
#[cfg(target_os = "android")]
static DICT_4X4_CFG: aruco_rs::core::dictionary::DictionaryConfig =
    aruco_rs::core::dictionary::DictionaryConfig {
        n_bits: MARKER_INNER * MARKER_INNER,
        tau: 3,
        code_list: crate::aruco_dict_4x4::DICT_4X4_250_CODES,
    };

/// Physical side length (metres) of the printed marker, by id range. The anchor
/// set (`alvr/Aruco/`) groups marker ids by paper size, so the id itself encodes
/// the marker's physical size (the full black-bordered square). Unknown ids are
/// rejected (returns None) so stray detections don't get a wrong scale.
#[cfg(target_os = "android")]
fn marker_size_m(id: i32) -> Option<f32> {
    match id {
        0..=19 => Some(0.160),   // A4 sheet
        50..=69 => Some(0.240),  // A3
        100..=119 => Some(0.340), // A2
        150..=169 => Some(0.500), // A1
        200..=219 => Some(0.720), // A0
        _ => None,
    }
}

/// aruco-rs 0.1.0 canonicalises the 4 corners inconsistently under in-plane
/// rotation: for ~half of viewing angles it labels them 180° rotated (diagonal
/// swap). The PnP is exact given the labels, so this flips the marker's yaw as
/// the head turns. We fix it physically using the marker's own code: sample the
/// inner cells with the given corner order, and if the read bits match the
/// 180°-rotated code better than the upright code, swap to the correct labels.
///
/// `corners`: TL,TR,BR,BL as returned by aruco-rs (pixels). `expected` is the
/// marker's code from the dictionary. Returns the corrected corner order.
#[cfg(target_os = "android")]
fn disambiguate_corners(
    gray: &[u8],
    w: usize,
    h: usize,
    corners: [alvr_common::glam::Vec2; 4],
    expected: u64,
) -> [alvr_common::glam::Vec2; 4] {
    let [tl, tr, br, bl] = corners;
    // Bilinear map of grid coords (gx,gy)∈[0,1]² across the marker quad.
    let sample = |gx: f32, gy: f32| -> f32 {
        let top = tl + (tr - tl) * gx;
        let bot = bl + (br - bl) * gx;
        let p = top + (bot - top) * gy;
        let x = (p.x.round() as isize).clamp(0, w as isize - 1) as usize;
        let y = (p.y.round() as isize).clamp(0, h as isize - 1) as usize;
        gray[y * w + x] as f32
    };
    // Read the inner cells (centres at (c+1.5, r+1.5)/total). Threshold at the
    // mean of the samples (DICT_4X4 markers are roughly bit-balanced).
    let n = MARKER_INNER * MARKER_INNER;
    let mut vals = [0.0f32; 36]; // max inner cells (6×6); we use n of them
    let mut mean = 0.0;
    for r in 0..MARKER_INNER {
        for c in 0..MARKER_INNER {
            let v = sample(
                (c as f32 + 1.5) / MARKER_TOTAL,
                (r as f32 + 1.5) / MARKER_TOTAL,
            );
            vals[r * MARKER_INNER + c] = v;
            mean += v;
        }
    }
    mean /= n as f32;
    // code: MSB = inner TL (k=0), row-major, bit 1 = white (above mean).
    let mut code: u64 = 0;
    for k in 0..n {
        code = (code << 1) | ((vals[k] > mean) as u64);
    }
    // 180°-rotated reading = the n bits reversed (cell k <-> n-1-k).
    let mut code_180: u64 = 0;
    for k in 0..n {
        code_180 = (code_180 << 1) | ((code >> k) & 1);
    }
    let ham = |a: u64, b: u64| (a ^ b).count_ones();
    if ham(code_180, expected) < ham(code, expected) {
        // Corners are labelled 180° off — swap to the diagonal order.
        [br, bl, tl, tr]
    } else {
        corners
    }
}

/// Spawn the passthrough-camera QR detection thread. Non-blocking.
pub fn start_qr_detection() {
    #[cfg(target_os = "android")]
    {
        std::thread::spawn(run_detection);
    }
}

#[cfg(target_os = "android")]
fn run_detection() {
    use alvr_common::{error, info};
    use jni::{
        JavaVM,
        errors::Result as JniResult,
        jni_sig, jni_str,
        objects::{JByteArray, JClass, JObject, JString},
        refs::Reference,
        sys::jobject,
    };

    // Passthrough camera needs CAMERA / HEADSET_CAMERA (requested on first run).
    alvr_system_info::try_get_permission("horizonos.permission.HEADSET_CAMERA");
    alvr_system_info::try_get_permission("android.permission.CAMERA");

    let vm = unsafe { JavaVM::from_raw(ndk_context::android_context().vm().cast()) };
    let context: jobject = ndk_context::android_context().context().cast();

    let res: JniResult<()> = vm.attach_current_thread(|env| {
        // ---- Load the helper dex ----
        let dex_buf = unsafe {
            env.new_direct_byte_buffer(
                CAMERA_HELPER_DEX.as_ptr() as *mut u8,
                CAMERA_HELPER_DEX.len(),
            )?
        };
        let parent_loader = env
            .call_method(
                unsafe { JObject::global_kind_from_raw(context) },
                jni_str!("getClassLoader"),
                jni_sig!("()Ljava/lang/ClassLoader;"),
                &[],
            )?
            .l()?;
        let loader = env.new_object(
            jni_str!("dalvik/system/InMemoryDexClassLoader"),
            jni_sig!("(Ljava/nio/ByteBuffer;Ljava/lang/ClassLoader;)V"),
            &[(&dex_buf).into(), (&parent_loader).into()],
        )?;
        let class_name = env.new_string("alvr.client.camera.CameraHelper")?;
        let helper_class = env
            .call_method(
                &loader,
                jni_str!("loadClass"),
                jni_sig!("(Ljava/lang/String;)Ljava/lang/Class;"),
                &[(&class_name).into()],
            )?
            .l()?;
        // Global ref so the class stays valid across local frames in the loop.
        let helper = env.new_global_ref(env.cast_local::<JClass>(helper_class)?)?;

        // ---- Start the passthrough stream ----
        let ctx_obj = unsafe { JObject::global_kind_from_raw(context) };
        let start_msg = env
            .call_static_method(
                <&JClass>::from(&helper),
                jni_str!("startPassthrough"),
                jni_sig!("(Landroid/content/Context;)Ljava/lang/String;"),
                &[(&ctx_obj).into()],
            )?
            .l()?;
        info!(
            "camera: passthrough QR detection started: {}",
            env.cast_local::<JString>(start_msg)?.to_string()
        );

        // Stage 2c: read intrinsics + extrinsics once (logged for verification).
        let calib = env
            .call_static_method(
                <&JClass>::from(&helper),
                jni_str!("getCalibration"),
                jni_sig!("()Ljava/lang/String;"),
                &[],
            )?
            .l()?;
        info!(
            "camera: [STAGE2c] calibration: {}",
            env.cast_local::<JString>(calib)?.to_string()
        );

        // ---- ArUco detector: OpenCV DICT_4X4 (first 250 = DICT_4X4_250),
        // scalar CV for aarch64. aruco-rs's detector is dictionary-agnostic. ----
        use aruco_rs::{ImageBuffer, core::detector::Detector, cv::scalar::ScalarCV};
        let dict = aruco_rs::core::dictionary::Dictionary::new(&DICT_4X4_CFG);
        let detector = Detector::new(&dict, ScalarCV);
        // Reused RGBA scratch: aruco-rs `detect` expects an RGBA buffer (it runs
        // its own grayscale pass), so we expand the Y plane into R=G=B=Y, A=255.
        let mut rgba: Vec<u8> = Vec::new();

        // ---- Detection loop (each iteration in its own local frame) ----
        loop {
            std::thread::sleep(std::time::Duration::from_millis(150));

            env.with_local_frame(16, |env| -> JniResult<()> {
                let helper = <&JClass>::from(&helper);

                let w = env
                    .call_static_method(helper, jni_str!("getFrameWidth"), jni_sig!("()I"), &[])?
                    .i()?;
                let h = env
                    .call_static_method(helper, jni_str!("getFrameHeight"), jni_sig!("()I"), &[])?
                    .i()?;
                if w <= 0 || h <= 0 {
                    return JniResult::Ok(());
                }

                let gray_obj = env
                    .call_static_method(helper, jni_str!("getLatestGray"), jni_sig!("()[B"), &[])?
                    .l()?;
                if gray_obj.is_null() {
                    return JniResult::Ok(());
                }
                let gray_arr = env.cast_local::<JByteArray>(gray_obj)?;
                let gray = env.convert_byte_array(&gray_arr)?;

                let (w, h) = (w as usize, h as usize);
                if gray.len() != w * h {
                    return JniResult::Ok(());
                }

                // Expand grayscale -> RGBA for the detector.
                rgba.resize(w * h * 4, 255);
                for (i, &v) in gray.iter().enumerate() {
                    rgba[i * 4] = v;
                    rgba[i * 4 + 1] = v;
                    rgba[i * 4 + 2] = v;
                    rgba[i * 4 + 3] = 255;
                }
                let image = ImageBuffer {
                    data: &rgba,
                    width: w as u32,
                    height: h as u32,
                };

                for m in detector.detect(&image) {
                    use alvr_common::glam::Vec2;
                    // Accept only anchor-set ids (their range also gives the size).
                    let Some(size_m) = marker_size_m(m.id) else {
                        continue;
                    };
                    let expected = crate::aruco_dict_4x4::DICT_4X4_250_CODES[m.id as usize];
                    // aruco-rs returns the 4 corners clockwise from the marker's
                    // canonical top-left (TL, TR, BR, BL) — but its canonicalisation
                    // flips 180° with viewing angle, so re-resolve via the code.
                    let c = m.corners;
                    let corners = disambiguate_corners(
                        &gray,
                        w,
                        h,
                        [
                            Vec2::new(c[0].x, c[0].y),
                            Vec2::new(c[1].x, c[1].y),
                            Vec2::new(c[2].x, c[2].y),
                            Vec2::new(c[3].x, c[3].y),
                        ],
                        expected,
                    );
                    // Exact intrinsics from LENS_INTRINSIC_CALIBRATION at the 1280²
                    // active array; we stream the full 1280² so they apply directly.
                    let scale = w as f32 / 1280.0;
                    let (fx, fy) = (866.1479 * scale, 866.1479 * scale);
                    let (cx, cy) = (643.3569 * scale, 641.3317 * scale);
                    let half = size_m / 2.0;
                    if let Some((rot, t)) =
                        crate::qr_pose::solve_qr_pose(corners, fx, fy, cx, cy, half)
                    {
                        use alvr_common::{Pose, glam::Quat, glam::Vec3};
                        // OpenCV cam (+X right, +Y down, +Z fwd) -> OpenXR cam
                        // (+X right, +Y up, -Z fwd): 180° about X flips Y and Z.
                        let flip = Quat::from_rotation_x(std::f32::consts::PI);
                        let qr_in_cam = Pose {
                            position: Vec3::new(t.x, -t.y, -t.z),
                            orientation: (flip * rot).normalize(),
                        };
                        *LATEST_QR_IN_CAM.lock() =
                            Some((m.id as u32, size_m, qr_in_cam, std::time::Instant::now()));
                        info!(
                            "camera: [aruco] id={} ({:.0}cm) cam-pos=({:.3},{:.3},{:.3}) dist={:.3}m",
                            m.id,
                            size_m * 100.0,
                            qr_in_cam.position.x,
                            qr_in_cam.position.y,
                            qr_in_cam.position.z,
                            t.length()
                        );
                    }
                }
                JniResult::Ok(())
            })?;
        }

        #[allow(unreachable_code)]
        JniResult::Ok(())
    });

    if let Err(e) = res {
        error!("camera: detection thread error: {e}");
    }
}

#[cfg(not(target_os = "android"))]
pub fn start_qr_detection() {}
