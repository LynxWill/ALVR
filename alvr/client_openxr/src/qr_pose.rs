//! Planar 4-point PnP: QR pose relative to the passthrough camera.
//!
//! The QR is a flat square of known size. We solve a homography from its 4
//! image corners to its local plane, then decompose it into rotation +
//! translation (classic planar pose estimation).

use alvr_common::glam::{Mat3, Quat, Vec2, Vec3};

/// Solve the QR pose in the camera frame (OpenCV convention: +X right, +Y down,
/// +Z forward into the scene).
///
/// `corners`: 4 QR corners in image pixels, ordered TL, TR, BR, BL.
/// `half`: half the QR side length in metres.
///
/// Returns (rotation, translation) of the QR's local frame
/// (+X right, +Y up, +Z out of the code) in the camera frame.
pub fn solve_qr_pose(
    corners: [Vec2; 4],
    fx: f32,
    fy: f32,
    cx: f32,
    cy: f32,
    half: f32,
) -> Option<(Quat, Vec3)> {
    // QR local corners on its Z=0 plane (+X right, +Y up).
    let obj = [
        Vec2::new(-half, half),  // TL
        Vec2::new(half, half),   // TR
        Vec2::new(half, -half),  // BR
        Vec2::new(-half, -half), // BL
    ];
    // Normalize image points by intrinsics (so the camera matrix becomes identity).
    let mut img = [Vec2::ZERO; 4];
    for i in 0..4 {
        img[i] = Vec2::new((corners[i].x - cx) / fx, (corners[i].y - cy) / fy);
    }

    let h = homography_4pt(&obj, &img)?;
    let (rot, t) = decompose_homography(&h)?;

    // Planar PnP has a 180°-about-normal in-plane ambiguity: the homography
    // decomposition can land on either branch, and which one it picks flips
    // deterministically with the viewing side (e.g. crossing a floor marker's
    // mid-line). Both branches share the same normal (r3) but mirror the
    // in-plane axes. Only the correct branch reprojects the object corners onto
    // the detected (canonical) image corners, so pick by reprojection error.
    let flip = Quat::from_rotation_z(std::f32::consts::PI);
    let rot_flipped = (rot * flip).normalize();

    let reproj_err = |r: Quat| -> f32 {
        let mut e = 0.0;
        for i in 0..4 {
            let cam = r * Vec3::new(obj[i].x, obj[i].y, 0.0) + t;
            if cam.z.abs() < 1e-6 {
                return f32::INFINITY;
            }
            let du = cam.x / cam.z - img[i].x;
            let dv = cam.y / cam.z - img[i].y;
            e += du * du + dv * dv;
        }
        e
    };

    let rot = if reproj_err(rot_flipped) < reproj_err(rot) {
        rot_flipped
    } else {
        rot
    };
    Some((rot, t))
}

/// Solve the 8 homography parameters (h33 = 1) from 4 correspondences.
fn homography_4pt(obj: &[Vec2; 4], img: &[Vec2; 4]) -> Option<[f64; 9]> {
    let mut a = [[0.0f64; 8]; 8];
    let mut b = [0.0f64; 8];
    for i in 0..4 {
        let (xo, yo) = (obj[i].x as f64, obj[i].y as f64);
        let (xi, yi) = (img[i].x as f64, img[i].y as f64);
        a[2 * i] = [xo, yo, 1.0, 0.0, 0.0, 0.0, -xi * xo, -xi * yo];
        b[2 * i] = xi;
        a[2 * i + 1] = [0.0, 0.0, 0.0, xo, yo, 1.0, -yi * xo, -yi * yo];
        b[2 * i + 1] = yi;
    }
    let x = solve_linear8(a, b)?;
    Some([x[0], x[1], x[2], x[3], x[4], x[5], x[6], x[7], 1.0])
}

/// Gaussian elimination with partial pivoting for an 8x8 system.
fn solve_linear8(mut a: [[f64; 8]; 8], mut b: [f64; 8]) -> Option<[f64; 8]> {
    for col in 0..8 {
        let mut piv = col;
        for r in (col + 1)..8 {
            if a[r][col].abs() > a[piv][col].abs() {
                piv = r;
            }
        }
        if a[piv][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        let d = a[col][col];
        for r in 0..8 {
            if r == col {
                continue;
            }
            let f = a[r][col] / d;
            for c in col..8 {
                a[r][c] -= f * a[col][c];
            }
            b[r] -= f * b[col];
        }
    }
    let mut x = [0.0; 8];
    for i in 0..8 {
        x[i] = b[i] / a[i][i];
    }
    Some(x)
}

/// Decompose normalized homography H = [r1 r2 t] into rotation + translation.
fn decompose_homography(h: &[f64; 9]) -> Option<(Quat, Vec3)> {
    let h1 = Vec3::new(h[0] as f32, h[3] as f32, h[6] as f32);
    let h2 = Vec3::new(h[1] as f32, h[4] as f32, h[7] as f32);
    let h3 = Vec3::new(h[2] as f32, h[5] as f32, h[8] as f32);

    let n = h1.length();
    if n < 1e-6 {
        return None;
    }
    let mut lambda = 1.0 / n;

    let mut t = h3 * lambda;
    // QR must be in front of the camera (+Z forward).
    if t.z < 0.0 {
        lambda = -lambda;
        t = h3 * lambda;
    }

    // Gram-Schmidt orthonormalize the first two columns, third = cross.
    let r1 = (h1 * lambda).normalize();
    let r2 = {
        let r2_raw = h2 * lambda;
        (r2_raw - r1 * r1.dot(r2_raw)).normalize()
    };
    let r3 = r1.cross(r2);

    let rot = Mat3::from_cols(r1, r2, r3);
    Some((Quat::from_mat3(&rot).normalize(), t))
}
