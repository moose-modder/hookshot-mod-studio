//! Which triangles the stock tool joins into quads. A quad primitive stores four vertices `[a, b, c, d]` and stands for the
//! triangles `a b c` and `a c d`; the two halves share the diagonal `c a`, which is the third edge of the first and the
//! first edge of the second. The rule below was read off the stock meshes (see the engine wiki's page on Havok collision
//! for the census): run on all the source triangles of a mesh at once, it makes 99.3% of the stock quads of Sekiro and
//! 99.1% of Elden Ring's (a pair counts when it is the same two source triangles), and for every stock quad it makes it
//! puts the same triangle first.
//!
//! 1. The triangles give each distinct vertex position a number, in the order the triangles first use it. Every triangle
//!    contributes its three edges as records `(lower number, higher number, triangle, edge)`, the edge `n` running from
//!    corner `n` to the next one.
//! 2. The records are sorted by the two numbers with the stock tool's quicksort ([`game_sort_by`]); that sort is not stable,
//!    so which of several triangles on one edge comes first follows from its procedure.
//! 3. For each record in turn, the first later record on the same two vertices that fits makes a candidate: the other
//!    triangle runs the edge the other way, has another vertex opposite it, has the same primitive data, and the two edges
//!    are a third edge and a first edge (so the first half of the quad is the triangle whose third edge is shared).
//! 4. A candidate costs the increase of box surface the pair brings: for the two triangles' boxes `A` and `B` and the box
//!    `U` of both, `(U - B) + (U - A)` with a box's surface taken as `ez*ey + ey*ex + ex*ez` of its extents, all in `f32`.
//!    The candidates are sorted by cost with the same quicksort and taken in that order, skipping one whose triangle is
//!    taken already.
//!
//! Where the stock tool differs: among edges that three or more triangles share, the stock pairs are not always the ones
//! the order of the records gives; the Sekiro object meshes, which have most of those, come out at 86.0%, and three of the
//! 370 (which repeat their triangles up to 23 times, so that candidates cost exactly the same) hold 97% of the misses; the
//! other 367 come out at 99.1%. The stock tool numbers vertices by the index of a welded source vertex, which the stock
//! files do not keep (the indices of the source rows are local to parts of the source mesh in some meshes); positions stand
//! for that here, in the order the triangles first use them.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use std::collections::BTreeMap;

use crate::gamesort::game_sort_by;

type V3 = [f32; 3];

/// A triangle offered to [`pair_quads`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuadCandidate {
    /// The three corners.
    pub vertices: [V3; 3],
    /// What two triangles must have in common to share a quad: the primitive data (material) they use; `0` for all when the
    /// source names none.
    pub data: u64,
}

/// The corner after `n` in a triangle.
fn next(n: usize) -> usize {
    match n {
        0 => 1,
        1 => 2,
        _ => 0,
    }
}

fn corner(row: &[usize; 3], n: usize) -> usize {
    row.get(n).copied().unwrap_or(usize::MAX)
}

/// The box of a triangle, minimum then maximum.
fn bounds(t: &[V3; 3]) -> [V3; 2] {
    let mut lo = t[0];
    let mut hi = t[0];
    for q in t {
        for (axis, &x) in q.iter().enumerate() {
            if let (Some(l), Some(h)) = (lo.get_mut(axis), hi.get_mut(axis)) {
                *l = l.min(x);
                *h = h.max(x);
            }
        }
    }
    [lo, hi]
}

/// The surface measure of a box, in the order the stock tool adds it.
fn surface(lo: V3, hi: V3) -> f32 {
    let ex = hi[0] - lo[0];
    let ey = hi[1] - lo[1];
    let ez = hi[2] - lo[2];
    (ez * ey + ey * ex) + ex * ez
}

/// What merging two triangles into one box costs.
fn merge_cost(a: &[V3; 3], b: &[V3; 3]) -> f32 {
    let (ba, bb) = (bounds(a), bounds(b));
    let mut lo = ba[0];
    let mut hi = ba[1];
    for axis in 0..3 {
        if let (Some(l), Some(h), Some(bl), Some(bh)) = (
            lo.get_mut(axis),
            hi.get_mut(axis),
            bb[0].get(axis),
            bb[1].get(axis),
        ) {
            *l = l.min(*bl);
            *h = h.max(*bh);
        }
    }
    let all = surface(lo, hi);
    (all - surface(bb[0], bb[1])) + (all - surface(ba[0], ba[1]))
}

/// The pairs of `triangles` the stock tool would join into quads, each as `[first half, second half]` by index, in the
/// order they were taken. A triangle is in at most one pair. Offer the triangles of a whole mesh at once: the stock tool
/// pairs before it cuts sections.
pub fn pair_quads(triangles: &[QuadCandidate]) -> Vec<[usize; 2]> {
    // vertex numbers, by position bits, in order of first use
    let mut by_position: BTreeMap<[u32; 3], usize> = BTreeMap::new();
    let mut ids: Vec<[usize; 3]> = Vec::with_capacity(triangles.len());
    for t in triangles {
        let mut row = [0usize; 3];
        for (slot, p) in row.iter_mut().zip(&t.vertices) {
            let count = by_position.len();
            *slot = *by_position.entry(p.map(f32::to_bits)).or_insert(count);
        }
        ids.push(row);
    }
    // edge records: (lower number, higher number, triangle * 4 + edge)
    let mut records: Vec<(usize, usize, usize)> = Vec::with_capacity(ids.len().saturating_mul(3));
    for (t, row) in ids.iter().enumerate() {
        for edge in 0..3usize {
            let (x, y) = (corner(row, edge), corner(row, next(edge)));
            records.push((x.min(y), x.max(y), t.saturating_mul(4).saturating_add(edge)));
        }
    }
    game_sort_by(&mut records, &|p, q| (p.0, p.1) < (q.0, q.1));
    // candidates: (cost, first half, second half)
    let mut candidates: Vec<(f32, usize, usize)> = Vec::new();
    for (n, ra) in records.iter().enumerate() {
        let (ta, ea) = (ra.2 >> 2, ra.2 & 3);
        for rb in records.iter().skip(n.saturating_add(1)) {
            if (rb.0, rb.1) != (ra.0, ra.1) {
                break;
            }
            let (tb, eb) = (rb.2 >> 2, rb.2 & 3);
            let (Some(ia), Some(ib), Some(da), Some(db)) =
                (ids.get(ta), ids.get(tb), triangles.get(ta), triangles.get(tb))
            else {
                continue;
            };
            if da.data != db.data {
                continue;
            }
            let reversed = corner(ia, ea) == corner(ib, next(eb)) && corner(ib, eb) == corner(ia, next(ea));
            if !reversed || corner(ia, next(next(ea))) == corner(ib, next(next(eb))) {
                continue;
            }
            let (first, second) = match (ea, eb) {
                (2, 0) => (ta, tb),
                (0, 2) => (tb, ta),
                _ => continue,
            };
            candidates.push((merge_cost(&da.vertices, &db.vertices), first, second));
            break;
        }
    }
    game_sort_by(&mut candidates, &|p, q| p.0 < q.0);
    let mut taken = vec![false; triangles.len()];
    let mut pairs = Vec::new();
    for (_, first, second) in candidates {
        if taken.get(first).copied().unwrap_or(true) || taken.get(second).copied().unwrap_or(true) {
            continue;
        }
        for t in [first, second] {
            if let Some(flag) = taken.get_mut(t) {
                *flag = true;
            }
        }
        pairs.push([first, second]);
    }
    pairs
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "a test reads what it just built"
)]
mod tests {
    use super::*;

    fn tri(a: V3, b: V3, c: V3, data: u64) -> QuadCandidate {
        QuadCandidate {
            vertices: [a, b, c],
            data,
        }
    }

    const P0: V3 = [0.0, 0.0, 0.0];
    const P1: V3 = [1.0, 0.0, 0.0];
    const P2: V3 = [1.0, 1.0, 0.0];
    const P3: V3 = [0.0, 1.0, 0.0];

    #[test]
    fn a_square_pairs_with_the_third_edge_triangle_first() {
        // a b c and a c d: the first has the diagonal as its third edge, the second as its first edge
        let square = [tri(P0, P1, P2, 0), tri(P0, P2, P3, 0)];
        assert_eq!(pair_quads(&square), [[0, 1]]);
        let swapped = [square[1], square[0]];
        assert_eq!(pair_quads(&swapped), [[1, 0]]);
    }

    #[test]
    fn different_data_do_not_pair() {
        let square = [tri(P0, P1, P2, 1), tri(P0, P2, P3, 2)];
        assert!(pair_quads(&square).is_empty());
    }

    #[test]
    fn only_a_third_edge_and_a_first_edge_pair() {
        // the shared edge is the first edge of both (run in opposite directions)
        let pair = [tri(P0, P1, P2, 0), tri(P1, P0, P3, 0)];
        assert!(pair_quads(&pair).is_empty());
    }

    #[test]
    fn a_shared_apex_or_a_same_way_edge_does_not_pair() {
        // the same triangle twice: the edge runs the same way
        let twin = [tri(P0, P1, P2, 0), tri(P0, P1, P2, 0)];
        assert!(pair_quads(&twin).is_empty());
        // a folded pair with the same apex on both sides of the diagonal
        let folded = [tri(P0, P1, P2, 0), tri(P0, P2, P1, 0)];
        assert!(pair_quads(&folded).is_empty());
    }

    #[test]
    fn a_triangle_joins_one_quad_at_most() {
        // the third triangle shares the second one's second edge, which is not a third edge meeting a first edge
        let p4: V3 = [2.0, 1.0, 0.0];
        let strip = [tri(P0, P1, P2, 0), tri(P0, P2, P3, 0), tri(P3, P2, p4, 0)];
        assert_eq!(pair_quads(&strip), [[0, 1]]);
    }
}
