//! The little linear algebra bone transforms need: column-major 4x4 matrices (as in glTF) and quaternions.

pub type Mat4 = [f32; 16];
/// (x, y, z, w)
pub type Quat = [f32; 4];

pub const IDENTITY: Mat4 = [
    1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
];

pub fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [0f32; 16];
    for col in 0..4 {
        for row in 0..4 {
            out[col * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[col * 4 + k]).sum();
        }
    }
    out
}

/// Translation * rotation * scale.
pub fn from_trs(t: [f32; 3], q: Quat, s: [f32; 3]) -> Mat4 {
    let [x, y, z, w] = q;
    let (xx, yy, zz) = (x * x, y * y, z * z);
    let (xy, xz, yz) = (x * y, x * z, y * z);
    let (wx, wy, wz) = (w * x, w * y, w * z);
    [
        (1. - 2. * (yy + zz)) * s[0],
        (2. * (xy + wz)) * s[0],
        (2. * (xz - wy)) * s[0],
        0.,
        (2. * (xy - wz)) * s[1],
        (1. - 2. * (xx + zz)) * s[1],
        (2. * (yz + wx)) * s[1],
        0.,
        (2. * (xz + wy)) * s[2],
        (2. * (yz - wx)) * s[2],
        (1. - 2. * (xx + yy)) * s[2],
        0.,
        t[0],
        t[1],
        t[2],
        1.,
    ]
}

/// Hamilton product `a * b` (rotate by `b`, then by `a`).
pub fn quat_mul(a: Quat, b: Quat) -> Quat {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

/// Euler angles in radians, applied X, then Y, then Z (`qz * qy * qx`), as BFRES stores them.
pub fn quat_from_euler_xyz(r: [f32; 3]) -> Quat {
    let axis = |i: usize, angle: f32| {
        let mut q = [0., 0., 0., (angle / 2.).cos()];
        q[i] = (angle / 2.).sin();
        q
    };
    quat_mul(axis(2, r[2]), quat_mul(axis(1, r[1]), axis(0, r[0])))
}

pub fn normalize_quat(q: Quat) -> Quat {
    let len = q.iter().map(|v| v * v).sum::<f32>().sqrt();
    if len > 0. {
        q.map(|v| v / len)
    } else {
        [0., 0., 0., 1.]
    }
}

/// Inverse of an affine matrix (rotation, scale, translation; no projection).
pub fn inverse_affine(m: &Mat4) -> Option<Mat4> {
    // The 3x3 part as rows: a[r][c] = m[c * 4 + r].
    let a = |r: usize, c: usize| m[c * 4 + r];
    let det = a(0, 0) * (a(1, 1) * a(2, 2) - a(1, 2) * a(2, 1))
        - a(0, 1) * (a(1, 0) * a(2, 2) - a(1, 2) * a(2, 0))
        + a(0, 2) * (a(1, 0) * a(2, 1) - a(1, 1) * a(2, 0));
    if det.abs() < 1e-20 {
        return None;
    }
    let inv_det = 1. / det;
    let mut inv = [[0f32; 3]; 3];
    for (r, row) in inv.iter_mut().enumerate() {
        for (c, v) in row.iter_mut().enumerate() {
            // Cofactor of (c, r), transposed.
            let (r0, r1) = match c {
                0 => (1, 2),
                1 => (0, 2),
                _ => (0, 1),
            };
            let (c0, c1) = match r {
                0 => (1, 2),
                1 => (0, 2),
                _ => (0, 1),
            };
            let minor = a(r0, c0) * a(r1, c1) - a(r0, c1) * a(r1, c0);
            let sign = if (r + c) % 2 == 0 { 1. } else { -1. };
            *v = sign * minor * inv_det;
        }
    }
    let t = [m[12], m[13], m[14]];
    let mut out = IDENTITY;
    for r in 0..3 {
        for c in 0..3 {
            out[c * 4 + r] = inv[r][c];
        }
        out[12 + r] = -(inv[r][0] * t[0] + inv[r][1] * t[1] + inv[r][2] * t[2]);
    }
    Some(out)
}

pub fn transform_point(m: &Mat4, p: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|r| m[r] * p[0] + m[4 + r] * p[1] + m[8 + r] * p[2] + m[12 + r])
}

/// Transforms a direction by the inverse transpose (correct for normals under non-uniform scale), normalised.
pub fn transform_normal(m: &Mat4, n: [f32; 3]) -> [f32; 3] {
    let inv = inverse_affine(m).unwrap_or(IDENTITY);
    // (inverse)^T applied to n: row r of the transpose is column r of the inverse.
    let v = [0, 1, 2].map(|r| inv[r * 4] * n[0] + inv[r * 4 + 1] * n[1] + inv[r * 4 + 2] * n[2]);
    normalize3(v)
}

/// The sRGB transfer function's inverse (a stored sRGB value to linear).
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear to sRGB.
pub fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

pub fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 0. { v.map(|c| c / len) } else { v }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: &[f32], b: &[f32]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5)
    }

    #[test]
    fn trs_and_inverse() {
        let q = normalize_quat([0.2, -0.4, 0.1, 0.9]);
        let m = from_trs([1., 2., 3.], q, [2., 0.5, 1.5]);
        let inv = inverse_affine(&m).unwrap();
        assert!(close(&mul(&m, &inv), &IDENTITY));
        assert!(close(&mul(&inv, &m), &IDENTITY));
        let p = transform_point(&m, [0., 0., 0.]);
        assert!(close(&p, &[1., 2., 3.]));
    }

    #[test]
    fn euler_order() {
        // 90 degrees about X then 90 about Z: +Y goes to +Z (X turn), +Z stays +Z (Z turn).
        let h = std::f32::consts::FRAC_PI_2;
        let m = from_trs([0.; 3], quat_from_euler_xyz([h, 0., h]), [1.; 3]);
        assert!(close(&transform_point(&m, [0., 1., 0.]), &[0., 0., 1.]));
        // +X: unchanged by X, then Z turns it to +Y.
        assert!(close(&transform_point(&m, [1., 0., 0.]), &[0., 1., 0.]));
    }

    #[test]
    fn srgb_round_trips_every_byte() {
        for b in 0..=255u8 {
            let c = f32::from(b) / 255.0;
            let back = (linear_to_srgb(srgb_to_linear(c)) * 255.0).round() as u8;
            assert_eq!(back, b);
        }
    }

    #[test]
    fn normals_under_scale() {
        let m = from_trs([5., 0., 0.], [0., 0., 0., 1.], [2., 1., 1.]);
        // A surface tilted 45 degrees in XY, stretched along X, tilts towards Y.
        let n = transform_normal(&m, normalize3([1., 1., 0.]));
        assert!(close(&n, &normalize3([0.5, 1., 0.])));
    }
}
