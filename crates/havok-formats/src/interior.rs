//! The interior flags of a compressed mesh: [`interior_keys`] says which triangles a shape's `triangleIsInterior` field marks.
//!
//! What the field marks, and how a triangle gets there. This is the rule that reproduces the flags of every compressed mesh
//! of the Sekiro and Elden Ring installs that has any flag set (25,383,271 shape keys of 2,982 meshes and 65,846,233 of
//! 17,122) with no key wrong, from the decoded vertices alone (the tests of `fromsoft-formats` run it):
//!
//! 1. *Neighbours.* Every directed edge of a triangle is paired with the same two vertices in the opposite direction, if a
//!    triangle has them: by vertex identity (a packed vertex is named by its section and slot, a shared one by its entry
//!    in the shared-vertex index), the primitives taken in key order and a triangle's edges in vertex order (`a b`, `b c`,
//!    `c a`). A reversed edge that was already waiting is the one paired, the latest of them; an edge nobody pairs waits.
//!    The diagonal of a quad is the edge between its two halves, linked directly. A *flat* quad (see
//!    [`InteriorPrimitive::flat`]) is one polygon `a b c d` with four edges, the outer ones, and is flagged under its first
//!    half's key alone. A triangle or polygon with a missing neighbour on any edge is never flagged.
//! 2. *Convex edges.* An edge of a triangle, with `e` the unit direction of the edge (the next corner minus this one) and
//!    `m` the unit normal of the triangle across it, is *convex* when `e . (n x m)` is more than the single-precision
//!    epsilon, `n` being the triangle's own normal (the raw cross product of two edges in the ordering pass, the unit
//!    normal in the marking pass). Normals are `(b - a) x (c - a)`; a polygon's normal is that of its first three corners.
//! 3. *Order.* The triangles with all neighbours are put in ascending order of `|n| + 10 * (convex edges)` (`|n|` being
//!    twice the area) by the quicksort [`game_sort`] spells out: it is not stable, so the order of triangles of an equal
//!    value is part of the result.
//! 4. *Marking.* In that order a triangle is flagged unless one of its convex edges meets a triangle that is already
//!    flagged, or whose normal is more than 70 degrees from its own.
//!
//! All of it is single-precision arithmetic in the order the stock tool does it, with an estimate of the reciprocal
//! square root ([`rsqrt_estimate`], refined by one Newton step) in the unit vectors and the lengths. Whether an edge is
//! convex is often a difference of a few units in the last place, so the estimate matters: the table in
//! [`rsqrt_estimate`] is an approximation fitted from these flags (no other estimate tried reproduces them), not a copy of
//! a processor's table.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use std::collections::BTreeMap;

use crate::gamesort::game_sort_by;
use crate::{Result, invalid};

type V3 = [f32; 3];

/// No neighbour.
const NONE: u32 = u32::MAX;
/// An edge is convex when its triple product is more than this.
const CONVEX_EPSILON: f32 = f32::EPSILON;
/// The cosine of 70 degrees: neighbours whose normals are further apart than that do not make a triangle interior.
const COS_LIMIT: f32 = 0.342_020_14;
/// What a convex edge adds to the sort key.
const CONVEX_WEIGHT: f32 = 10.0;
/// The edges of a triangle as pairs of vertex positions.
const EDGES: [(usize, usize); 3] = [(0, 1), (1, 2), (2, 0)];

/// A triangle or a quad of a mesh, as the pass reads it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InteriorPrimitive {
    /// The shape key of the primitive's first half: `section << 8 | index << 1`.
    pub key: u32,
    /// A quad (two halves, `a b c` and `a c d`, sharing the diagonal `c a`) rather than a triangle (`a b c`, then `c` again).
    pub quad: bool,
    /// A quad that is flat: the pass takes it as one polygon `a b c d` with the outer four edges, and flags it by its
    /// first half's key only. A quad is flat when the shape's `primitiveStoresIsFlatConvex` mask and its slots say so
    /// (`d < mask & b`).
    pub flat: bool,
    /// The identity of each of the four vertex slots: two slots that name one vertex have one identity.
    pub ids: [u32; 4],
    /// The positions of the four slots as the file's codes decode.
    pub positions: [V3; 4],
}

/// The reciprocal square root of `x` as an estimate with a 12-bit table: the values of `x` that share an exponent
/// parity and the top ten mantissa bits (a cell of 8,192 units in the last place) have the reciprocal square root of the
/// cell's middle, rounded to the nearest multiple of 2,048 units in the last place of the result.
///
/// The table is an approximation fitted from the flags of the stock meshes, over several formulas for the cell, the point
/// of the cell sampled and the rounding (the sampling at the middle with rounding to the nearest is the one that gives every
/// flag); it is not known to be the table of any processor. With it the pass below reproduces the flags (see the module
/// documentation); an exact reciprocal square root, or the table of the processor of the machine reading this, gets some
/// hundreds of flags per million wrong.
pub fn rsqrt_estimate(x: f32) -> f32 {
    let bits = x.to_bits();
    let cell: u32 = 1 << 13;
    let low_bits = bits & !(cell.wrapping_sub(1));
    let low = f64::from(f32::from_bits(low_bits));
    let high = f64::from(f32::from_bits(low_bits.wrapping_add(cell)));
    let middle = low + (high - low) * 0.5;
    let exact = 1.0 / middle.sqrt();
    let grid: u32 = 1 << 11;
    let base = narrow(exact).to_bits() & !(grid.wrapping_sub(1));
    let below = f64::from(f32::from_bits(base));
    let above = f64::from(f32::from_bits(base.wrapping_add(grid)));
    let t = (exact - below) / (above - below);
    f32::from_bits(if t >= 0.5 { base.wrapping_add(grid) } else { base })
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "the estimate is a single-precision value"
)]
fn narrow(x: f64) -> f32 {
    x as f32
}

/// `(3 - (r s) r) (r / 2)` for the estimate `r` of the reciprocal square root of `s`: the refined factor that scales
/// a vector of squared length `s` to unit length.
fn refined(s: f32) -> f32 {
    let r = rsqrt_estimate(s);
    let t = (r * s) * r;
    (3.0 - t) * (r * 0.5)
}

/// The squared length, summed as `(y y + x x) + z z`.
fn squared(v: V3) -> f32 {
    let [x, y, z] = v;
    (y * y + x * x) + z * z
}

fn sub(a: V3, b: V3) -> V3 {
    let [ax, ay, az] = a;
    let [bx, by, bz] = b;
    [ax - bx, ay - by, az - bz]
}

fn scale(f: f32, v: V3) -> V3 {
    let [x, y, z] = v;
    [f * x, f * y, f * z]
}

/// The raw normal of the triangle `a b c`: the cross product of `b - a` and `c - a`.
fn normal(a: V3, b: V3, c: V3) -> V3 {
    let [ux, uy, uz] = sub(b, a);
    let [vx, vy, vz] = sub(c, a);
    [vz * uy - uz * vy, vx * uz - ux * vz, vy * ux - uy * vx]
}

/// The cross product `n x m`, each component the difference of two products.
fn cross(n: V3, m: V3) -> V3 {
    let [nx, ny, nz] = n;
    let [mx, my, mz] = m;
    [mz * ny - nz * my, mx * nz - nx * mz, my * nx - ny * mx]
}

/// The vector scaled to unit length (zero for a zero vector).
fn unit(v: V3) -> V3 {
    let s = squared(v);
    if s <= 0.0 { [0.0; 3] } else { scale(refined(s), v) }
}

/// The triple product of the unit direction of the edge `from -> to` with `n x m`, summed `(y + x) + z`.
fn edge_triple(from: V3, to: V3, n: V3, m: V3) -> f32 {
    let e = sub(to, from);
    let unit_e = scale(refined(squared(e)), e);
    let [cx, cy, cz] = cross(n, m);
    let [ex, ey, ez] = unit_e;
    ((ey * cy) + (ex * cx)) + (ez * cz)
}

/// A half-triangle of a primitive: its vertex identities and positions. A triangle has half 0 only.
fn half(p: &InteriorPrimitive, half: usize) -> ([u32; 3], [V3; 3]) {
    let [ia, ib, ic, id] = p.ids;
    let [pa, pb, pc, pd] = p.positions;
    if half == 0 {
        ([ia, ib, ic], [pa, pb, pc])
    } else {
        ([ia, ic, id], [pa, pc, pd])
    }
}

fn halves(p: &InteriorPrimitive) -> usize {
    if p.quad { 2 } else { 1 }
}

/// The neighbour link of every half-triangle: for the half with key `k`, `links[k][edge]` is the key of the half across
/// the edge shifted two bits up with that half's edge number in the low two, or [`NONE`].
fn build_links(prims: &[InteriorPrimitive]) -> BTreeMap<u32, [u32; 3]> {
    let mut links: BTreeMap<u32, [u32; 3]> = BTreeMap::new();
    let mut waiting: Vec<Vec<[u32; 3]>> = vec![Vec::new(); 64];
    for p in prims {
        if p.quad {
            let k0 = p.key;
            if let Some(l) = links.entry(k0).or_insert([NONE; 3]).get_mut(2) {
                *l = (k0 | 1).wrapping_shl(2);
            }
            if let Some(l) = links.entry(k0 | 1).or_insert([NONE; 3]).get_mut(0) {
                *l = k0.wrapping_shl(2) | 2;
            }
        }
        for h in 0..halves(p) {
            let (ids, _) = half(p, h);
            let key = p.key | u32::from(h == 1);
            for (edge, &(from, to)) in EDGES.iter().enumerate() {
                if p.quad && ((h == 0 && edge == 2) || (h == 1 && edge == 0)) {
                    continue;
                }
                let (Some(&a), Some(&b)) = (ids.get(from), ids.get(to)) else {
                    continue;
                };
                let edge_bits = u32::try_from(edge).unwrap_or(0);
                let bucket = usize::try_from((a ^ b).wrapping_mul(0u32.wrapping_sub(27)) & 0x3f).unwrap_or(0);
                let Some(list) = waiting.get_mut(bucket) else {
                    continue;
                };
                // the latest waiting edge that is this one reversed
                let found = list.iter().rposition(|w| w[0] == b && w[1] == a);
                match found {
                    None => list.push([a, b, key | edge_bits.wrapping_shl(30)]),
                    Some(i) => {
                        let stored = list.get(i).map_or(0, |w| w[2]);
                        let other_key = stored & 0x3fff_ffff;
                        let other_edge = stored.wrapping_shr(30);
                        if let Some(l) = links.entry(key).or_insert([NONE; 3]).get_mut(edge) {
                            *l = other_key.wrapping_shl(2) | other_edge;
                        }
                        if let Some(l) = links
                            .entry(other_key)
                            .or_insert([NONE; 3])
                            .get_mut(usize::try_from(other_edge).unwrap_or(0))
                        {
                            *l = key.wrapping_shl(2) | edge_bits;
                        }
                        // the last waiting edge takes the place of the paired one
                        list.swap_remove(i);
                    }
                }
            }
        }
    }
    links
}

/// The quicksort the stock tool sorts with, on `(key, value)` pairs by value ([`game_sort_by`]): the pivot is the value in
/// the middle of the range, a pair moves when it is on the wrong side of the pivot strictly, and pairs that are neither stay
/// where the partition leaves them. It is not stable; equal values come out in the order this procedure gives them.
pub fn game_sort(list: &mut [(u32, f32)]) {
    game_sort_by(list, &|a, b| a.1 < b.1);
}

/// The polygon a key stands for in the pass: its corners and the links of its edges. A flat quad is one polygon (by its
/// first half's key) whose edges are the outer four; the second half of a flat quad stands for nothing.
struct Polygon {
    corners: Vec<V3>,
    links: Vec<u32>,
}

fn point(corners: &[V3], i: usize) -> V3 {
    corners.get(i).copied().unwrap_or([0.0; 3])
}

/// The raw normal of the first three corners.
fn polygon_normal(corners: &[V3]) -> V3 {
    normal(point(corners, 0), point(corners, 1), point(corners, 2))
}

/// The keys of the half-triangles whose interior flag is set, ascending, for the primitives `prims`.
pub fn interior_keys(prims: &[InteriorPrimitive]) -> Vec<u32> {
    let links = build_links(prims);
    // key -> (primitive, half)
    let mut by_key: BTreeMap<u32, (usize, usize)> = BTreeMap::new();
    for (pi, p) in prims.iter().enumerate() {
        for h in 0..halves(p) {
            by_key.insert(p.key | u32::from(h == 1), (pi, h));
        }
    }
    let polygon = |key: u32| -> Option<Polygon> {
        let &(pi, h) = by_key.get(&key)?;
        let p = prims.get(pi)?;
        let own = links.get(&key).copied().unwrap_or([NONE; 3]);
        if p.flat {
            if h == 1 {
                return None;
            }
            let other = links.get(&(key | 1)).copied().unwrap_or([NONE; 3]);
            let [first, second, _] = own;
            let [_, third, fourth] = other;
            Some(Polygon {
                corners: p.positions.to_vec(),
                links: vec![first, second, third, fourth],
            })
        } else {
            Some(Polygon {
                corners: half(p, h).1.to_vec(),
                links: own.to_vec(),
            })
        }
    };
    // the unit normal of the triangle (half) across a link, and the key its flag is kept under
    let across = |link: u32| -> (V3, u32) {
        let key = link.wrapping_shr(2);
        let (normal_of, flag_key) = by_key
            .get(&key)
            .and_then(|&(pi, h)| {
                prims
                    .get(pi)
                    .map(|p| (half(p, h).1, if p.flat { key & !1 } else { key }))
            })
            .unwrap_or(([[0.0; 3]; 3], key));
        let [a, b, c] = normal_of;
        (unit(normal(a, b, c)), flag_key)
    };
    // the triangles with a neighbour on every edge, with the weight they sort by
    let mut list: Vec<(u32, f32)> = Vec::new();
    for &key in by_key.keys() {
        let Some(poly) = polygon(key) else { continue };
        if poly.links.contains(&NONE) {
            continue;
        }
        let n = polygon_normal(&poly.corners);
        let mut convex = 0u32;
        for (edge, &link) in poly.links.iter().enumerate() {
            let (m, _) = across(link);
            let next = edge
                .saturating_add(1)
                .checked_rem(poly.corners.len())
                .unwrap_or(0);
            if edge_triple(point(&poly.corners, edge), point(&poly.corners, next), n, m) > CONVEX_EPSILON {
                convex = convex.saturating_add(1);
            }
        }
        let s = squared(n);
        let length = if s <= 0.0 { 0.0 } else { refined(s) * s };
        list.push((
            key,
            length + (f32::from(u16::try_from(convex).unwrap_or(u16::MAX))) * CONVEX_WEIGHT,
        ));
    }
    game_sort(&mut list);
    let mut marked: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
    for &(key, _) in &list {
        let Some(poly) = polygon(key) else { continue };
        let n = unit(polygon_normal(&poly.corners));
        let mut blocked = false;
        for (edge, &link) in poly.links.iter().enumerate() {
            let (m, flag_key) = across(link);
            let next = edge
                .saturating_add(1)
                .checked_rem(poly.corners.len())
                .unwrap_or(0);
            if edge_triple(point(&poly.corners, edge), point(&poly.corners, next), n, m) > CONVEX_EPSILON {
                let [nx, ny, nz] = n;
                let [mx, my, mz] = m;
                let dot = ((my * ny) + (mx * nx)) + (mz * nz);
                if marked.contains(&flag_key) || COS_LIMIT > dot {
                    blocked = true;
                    break;
                }
            }
        }
        if !blocked {
            marked.insert(key);
        }
    }
    marked.into_iter().collect()
}

/// A mesh's arrays as the pass reads them; [`primitives_of`] turns them into primitives.
#[derive(Debug, Clone, Copy)]
pub struct MeshView<'a> {
    /// The tree's `domain` box, which the shared vertices' codes span.
    pub domain: [V3; 2],
    /// The sections, in order.
    pub sections: &'a [SectionView],
    /// The primitives' four vertex slots, section after section.
    pub primitives: &'a [[u8; 4]],
    /// The packed vertex codes (11, 11 and 10 bits).
    pub packed_vertices: &'a [u32],
    /// The shared vertex codes (21, 21 and 22 bits).
    pub shared_vertices: &'a [u64],
    /// The shared-vertex index entries (16 bits).
    pub shared_index: &'a [u16],
    /// The tree's `primitiveStoresIsFlatConvex` byte: with it, a quad whose last slot is below `mask & second slot` is flat.
    pub flat_mask: u8,
}

/// One section of a [`MeshView`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SectionView {
    /// The packed vertices' offset (xyz) and scale (xyz).
    pub codec: [f32; 6],
    /// The section's first packed vertex.
    pub first_packed_vertex: usize,
    /// How many packed vertices the section has.
    pub packed_vertices: usize,
    /// The section's first shared-vertex index entry.
    pub first_shared_index: usize,
    /// The page of the shared array its entries are on.
    pub page: usize,
    /// The section's first primitive.
    pub first_primitive: usize,
    /// How many primitives the section has.
    pub primitives: usize,
}

/// A code lane (at most 22 bits) as a single-precision number.
fn lane(x: u64) -> f32 {
    u32::try_from(x).map_or(0.0, |v| v as f32)
}

/// Whether a primitive's four slots are a quad (`Some(true)`) or a triangle (`Some(false)`); `None` for a custom or a
/// removed primitive, which has no keys.
fn is_triangle_or_quad(slots: &[u8; 4]) -> Option<bool> {
    let [_, b, c, d] = *slots;
    if b == d {
        return None;
    }
    Some(c != d)
}

/// The primitives of `view` that have keys (triangles and quads), each with its vertices' identities and positions.
/// A packed vertex is `section * 65,536 + slot`; a shared one is `sections * 65,536 + entry`, where `entry` is the
/// 16-bit entry of the shared-vertex index and, when `page_aware`, `page * 65,536` is added to it (the stock tool names
/// a shared vertex by its entry alone). Positions are `code * scale + offset` in single precision; a shared vertex's scale is
/// the box's extent times the single-precision value of `1 / 2,097,151` (`1 / 4,194,303` for z).
pub fn primitives_of(view: &MeshView<'_>, page_aware: bool) -> Result<Vec<InteriorPrimitive>> {
    let sections = view.sections.len();
    let [low, high] = view.domain;
    let unit_xy = f32::from_bits(0x3500_0004);
    let unit_z = f32::from_bits(0x3480_0002);
    let shared_scale = [
        (high[0] - low[0]) * unit_xy,
        (high[1] - low[1]) * unit_xy,
        (high[2] - low[2]) * unit_z,
    ];
    let page_size = 65_536usize;
    let mut out = Vec::new();
    for (s, section) in view.sections.iter().enumerate() {
        let offset = [section.codec[0], section.codec[1], section.codec[2]];
        let step = [section.codec[3], section.codec[4], section.codec[5]];
        let vertex = |slot: usize| -> Result<(u32, V3)> {
            if slot < section.packed_vertices {
                let code = *view
                    .packed_vertices
                    .get(section.first_packed_vertex.saturating_add(slot))
                    .ok_or_else(|| invalid(0, "a packed vertex past the array"))?;
                let q = [
                    f32::from(u16::try_from(code & 0x7ff).unwrap_or(0)),
                    f32::from(u16::try_from((code >> 11) & 0x7ff).unwrap_or(0)),
                    f32::from(u16::try_from(code >> 22).unwrap_or(0)),
                ];
                let position = [
                    q[0] * step[0] + offset[0],
                    q[1] * step[1] + offset[1],
                    q[2] * step[2] + offset[2],
                ];
                let id = s.saturating_mul(page_size).saturating_add(slot);
                Ok((u32::try_from(id).unwrap_or(u32::MAX), position))
            } else {
                let at = section
                    .first_shared_index
                    .saturating_add(slot.saturating_sub(section.packed_vertices));
                let entry = usize::from(
                    *view
                        .shared_index
                        .get(at)
                        .ok_or_else(|| invalid(0, "a shared entry past the array"))?,
                );
                let code = *view
                    .shared_vertices
                    .get(section.page.saturating_mul(page_size).saturating_add(entry))
                    .ok_or_else(|| invalid(0, "a shared vertex past the array"))?;
                let q = [
                    lane(code & 0x1f_ffff),
                    lane((code >> 21) & 0x1f_ffff),
                    lane(code >> 42),
                ];
                let position = [
                    q[0] * shared_scale[0] + low[0],
                    q[1] * shared_scale[1] + low[1],
                    q[2] * shared_scale[2] + low[2],
                ];
                let named = if page_aware {
                    entry.saturating_add(section.page.saturating_mul(page_size))
                } else {
                    entry
                };
                let id = sections.saturating_mul(page_size).saturating_add(named);
                Ok((u32::try_from(id).unwrap_or(u32::MAX), position))
            }
        };
        for i in 0..section.primitives {
            let slots = view
                .primitives
                .get(section.first_primitive.saturating_add(i))
                .ok_or_else(|| invalid(0, "a primitive past the array"))?;
            let Some(quad) = is_triangle_or_quad(slots) else {
                continue;
            };
            let mut ids = [0u32; 4];
            let mut positions = [[0.0f32; 3]; 4];
            for ((id, position), &slot) in ids.iter_mut().zip(positions.iter_mut()).zip(slots.iter()) {
                let (a, b) = vertex(usize::from(slot))?;
                *id = a;
                *position = b;
            }
            let key = u32::try_from(s)
                .ok()
                .and_then(|s| s.checked_mul(256))
                .zip(u32::try_from(i).ok().and_then(|i| i.checked_mul(2)))
                .map(|(s, i)| s | i)
                .ok_or_else(|| invalid(0, "a shape key past 32 bits"))?;
            let [_, second, _, last] = *slots;
            let flat = quad && last < (view.flat_mask & second);
            out.push(InteriorPrimitive {
                key,
                quad,
                flat,
                ids,
                positions,
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "a test reads what it just built"
)]
mod tests {
    use super::*;

    fn triangle(key: u32, ids: [u32; 3], p: [V3; 3]) -> InteriorPrimitive {
        InteriorPrimitive {
            key,
            quad: false,
            flat: false,
            ids: [ids[0], ids[1], ids[2], ids[2]],
            positions: [p[0], p[1], p[2], p[2]],
        }
    }

    #[test]
    fn the_estimate_is_within_the_tables_error() {
        for i in 1..2000u32 {
            let x = f32::from_bits(0x3f80_0000 + i * 4999);
            let exact = 1.0 / f64::from(x).sqrt();
            let got = f64::from(rsqrt_estimate(x));
            assert!(
                ((got - exact) / exact).abs() < 2.0 / 4096.0,
                "x = {x}: {got} against {exact}"
            );
        }
    }

    #[test]
    fn the_sort_orders_by_value_and_keeps_every_pair() {
        let mut list: Vec<(u32, f32)> = (0..50u32).map(|k| (k, ((k * 37) % 11) as f32)).collect();
        game_sort(&mut list);
        assert!(list.windows(2).all(|w| w[0].1 <= w[1].1));
        let mut keys: Vec<u32> = list.iter().map(|p| p.0).collect();
        keys.sort_unstable();
        assert_eq!(keys, (0..50).collect::<Vec<u32>>());
    }

    #[test]
    fn a_triangle_without_neighbours_is_not_interior() {
        let t = triangle(0, [0, 1, 2], [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
        assert!(interior_keys(&[t]).is_empty());
    }

    /// A tetrahedron: four triangles, each edge shared by two with opposite directions. With outward normals every edge is
    /// convex and the faces are 109 degrees apart (more than 70): no face is flagged.
    #[test]
    fn the_faces_of_a_tetrahedron_are_not_interior() {
        let v = [
            [1.0f32, 1.0, 1.0],
            [1.0, -1.0, -1.0],
            [-1.0, 1.0, -1.0],
            [-1.0, -1.0, 1.0],
        ];
        // outward windings (the normals point away from the centre)
        let outward = [[0usize, 1, 2], [0, 3, 1], [0, 2, 3], [1, 3, 2]];
        let build = |faces: &[[usize; 3]]| -> Vec<InteriorPrimitive> {
            faces
                .iter()
                .enumerate()
                .map(|(i, f)| {
                    triangle(
                        u32::try_from(i).unwrap() * 2,
                        f.map(|c| u32::try_from(c).unwrap()),
                        [v[f[0]], v[f[1]], v[f[2]]],
                    )
                })
                .collect()
        };
        let prims = build(&outward);
        let links = build_links(&prims);
        assert!(
            links.values().all(|l| !l.contains(&NONE)),
            "every edge has a neighbour: {links:?}"
        );
        assert!(interior_keys(&prims).is_empty());
        // the same faces wound the other way round (the normals point inwards) have no convex edge: all are flagged
        let inward: Vec<[usize; 3]> = outward.iter().map(|f| [f[0], f[2], f[1]]).collect();
        assert_eq!(interior_keys(&build(&inward)), vec![0, 2, 4, 6]);
    }

    /// Two coplanar triangles sharing a diagonal: the shared edge of one is paired with the shared edge of the other,
    /// the other four edges have no neighbour, so neither triangle is flagged.
    #[test]
    fn two_coplanar_triangles_have_missing_neighbours() {
        let a = triangle(0, [0, 1, 2], [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0]]);
        let b = triangle(2, [0, 2, 3], [[0.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]]);
        let links = build_links(&[a, b]);
        // a's edge 2 (2 -> 0) pairs with b's edge 0 (0 -> 2)
        assert_eq!(links[&0][2], 8);
        assert_eq!(links[&2][0], 2);
        assert!(interior_keys(&[a, b]).is_empty());
    }
}
