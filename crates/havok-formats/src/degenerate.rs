//! The test the stock tool applies to a triangle before it keeps it. A triangle is degenerate when the sum of the squared
//! components of its cross product, worked out in either of two ways (from the first corner and from the second), is below
//! a threshold, or when its three points are exactly collinear. On the stock meshes it passes every kept triangle and
//! fails a large part of the triangles the files leave out (see the engine wiki's page on Havok collision for the census).
//!
//! The arithmetic is single precision, in the order written below: for corners `p`, `q`, `r` the first sum is over the
//! components of `(p - r) x (p - q)` and the second over those of `(q - r) x (q - p)`, each product and sum taken left to
//! right; collinear is `|r - q|^2 * |p - q|^2 - ((r - q) . (p - q))^2 == 0`.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

/// The threshold the stock tool's builder starts from.
pub const DEFAULT_DEGENERATE_THRESHOLD: f32 = 1.0e-7;

/// Whether the triangle `p`, `q`, `r` is degenerate: either sum of squared cross-product components is below `threshold`,
/// or the points are exactly collinear. A negative threshold passes every triangle.
pub fn is_degenerate_triangle(p: [f32; 3], q: [f32; 3], r: [f32; 3], threshold: f32) -> bool {
    if threshold < 0.0 {
        return false;
    }
    let [x1, y1, z1] = p;
    let [x2, y2, z2] = q;
    let [x3, y3, z3] = r;
    let (d15, d16, d17) = (x1 - x2, y1 - y2, z1 - z2);
    let c12 = (y1 - y3) * d15 - d16 * (x1 - x3);
    let c13 = (z1 - z3) * d16 - d17 * (y1 - y3);
    let c14 = (x1 - x3) * d17 - d15 * (z1 - z3);
    let first = (c14 * c14 + c13 * c13) + c12 * c12;
    let c9 = (y2 - y3) * (x2 - x1) - (y2 - y1) * (x2 - x3);
    let c10 = (z2 - z3) * (y2 - y1) - (z2 - z1) * (y2 - y3);
    let c11 = (x2 - x3) * (z2 - z1) - (x2 - x1) * (z2 - z3);
    let second = (c11 * c11 + c10 * c10) + c9 * c9;
    if first < threshold || second < threshold {
        return true;
    }
    let (a, b, c) = (x3 - x2, y3 - y2, z3 - z2);
    let dot = (b * d16 + a * d15) + c * d17;
    let along = ((b * b + a * a) + c * c) * ((d16 * d16 + d15 * d15) + d17 * d17);
    along - dot * dot == 0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_unit_right_triangle_is_not_degenerate() {
        assert!(!is_degenerate_triangle(
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            DEFAULT_DEGENERATE_THRESHOLD
        ));
    }

    #[test]
    fn collinear_points_and_repeated_points_are_degenerate() {
        let t = DEFAULT_DEGENERATE_THRESHOLD;
        assert!(is_degenerate_triangle(
            [0.0; 3],
            [1.0, 1.0, 1.0],
            [2.0, 2.0, 2.0],
            t
        ));
        assert!(is_degenerate_triangle([1.0; 3], [1.0; 3], [3.0, 0.0, 5.0], t));
        // collinear even with no threshold
        assert!(is_degenerate_triangle(
            [0.0; 3],
            [1.0, 0.0, 0.0],
            [3.0, 0.0, 0.0],
            0.0
        ));
    }

    #[test]
    fn a_small_triangle_is_degenerate_by_the_threshold_only() {
        // legs of 0.01: the cross product has length 1e-4, its square 1e-8
        let (p, q, r) = ([0.0, 0.0, 0.0], [0.01, 0.0, 0.0], [0.0, 0.01, 0.0]);
        assert!(is_degenerate_triangle(p, q, r, DEFAULT_DEGENERATE_THRESHOLD));
        assert!(!is_degenerate_triangle(p, q, r, 1.0e-9));
    }

    #[test]
    fn a_negative_threshold_passes_everything() {
        assert!(!is_degenerate_triangle([0.0; 3], [1.0; 3], [2.0; 3], -1.0));
    }
}
