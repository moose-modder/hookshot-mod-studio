//! Writing the static bounding-volume trees of compressed and extern meshes: a tree's shape, and the bytes of its nodes.
//!
//! A tree is a pre-order array of nodes ([`crate::bvtree`] has the layout). A node's box is not stored as numbers but as
//! three nibble pairs that shrink the parent's box: on each axis the high nibble `h` takes `extent × h² / 226` from the
//! minimum side and the low nibble `l` takes `extent × l² / 226` from the maximum side. Encoding a node is choosing the
//! nibbles whose box is the smallest that still holds what lies under the node. [`encode_tree`] does that for a given
//! [`Shape`] and boxes of the leaves; [`sah_shape`] and [`balanced_shape`] make a shape for boxes.
//!
//! **What the stock files show** (a census over every section tree of the Sekiro and Elden Ring map, object and asset
//! collision, 200,418 inner and 205,670 leaf nodes of the Sekiro objects alone): with the leaf boxes taken from the
//! decoded vertices grown by the shape's convex radius and the nodes' boxes taken from their stored parents, this rule
//! gives the stored nibbles of 93% of the inner nodes and 86% of the leaves, and every root. The nodes that differ differ
//! by one nibble in all but 1.4%, and the stored side of such a node lies within half a codec step of the boundary
//! between the two nibbles: the files' trees were built from the vertices before they were quantised, and the rule is
//! applied here to the vertices after.
//!
//! **The topology** is partly a rule (a census over every section and mesh tree of the Sekiro and Elden Ring map, object
//! and asset collision, leaf boxes the primitives' vertices grown by the convex radius):
//!
//! - A mesh's trees are the pieces of one tree over all its primitives: a section is a subtree cut where it has 127
//!   primitives or fewer (99,158 of the 99,164 pairs of sibling sections in Sekiro hold more than 127 together, 274,861
//!   of 274,943 in Elden Ring; in the others the pair holds 127 or fewer).
//! - A node of 16 leaves or fewer is built from the bottom, always joining the two nodes whose union has the least
//!   surface area: of the 2,043,084 maximal such subtrees of Sekiro, 1,591,211 (78%) are what that gives, and 94.6% of
//!   the 19,692,840 inner nodes inside them are nodes it makes; in Elden Ring 75% of 5,454,605 and 93.4% of 52,922,096.
//!   A subtree of 16 leaves is that in 67.5% (Sekiro) and 66.2% (Elden Ring), one of 17 leaves in 8.9% and 8.6%: above
//!   16 the tree is not built that way.
//! - The nodes above 16 leaves split within a few percent of the best surface-area split along an axis of the centres
//!   (in one Sekiro map, half of the 7,597 top-level splits cost 2% or less over it), without being it. What the stock
//!   split is, and how far this crate follows it, is in [`crate::StockTree`]. The splits of 8 to 64 bins over the centres
//!   or the boxes, with two costs, were the stock split in 0.5% to 7% of the first 25 meshes' splits; the rule that
//!   [`StockTree`](crate::StockTree) follows (32 bins by the place of the centre in the list's box, rounded; the first
//!   best bin of three axes by the summed surfaces from both ends; the bin itself sorted by its centres) differs from
//!   them in the bin placement and the cost, and in the order the leaves reach it (that of an insertion tree).
//! - A node's first child is the one with the larger box: 20,184,136 against 417,276 nodes of Sekiro (913,005 have
//!   children of one surface, 834,964 of them two leaves; of the others that differ in their leaves, 37,033 have the
//!   child with more leaves first and 7,311 the one with fewer), 53,597,280 against 1,699,537 of Elden Ring.
//!
//! [`sah_shape`] follows that: it splits by the surface-area heuristic down to 16 leaves, merges those from the
//! bottom, and puts the larger child first. It does not reproduce the stock trees ([`crate::StockTree`] is the closer
//! rule), but they cost more to walk, by the total surface area of the inner nodes: 0.924 of the stock's in Sekiro and 0.915 in Elden Ring (the median split of
//! [`balanced_shape`], which this crate wrote before, makes 1.34 and 1.32).
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use crate::bvtree::{BvNodeKind, BvTree, NodeWidth};
use crate::{Result, invalid};

/// A box, minimum then maximum (xyz).
pub type Aabb = [[f64; 3]; 2];

/// The divisor of the nibble squares.
const NIBBLE_DIVISOR: f64 = 226.0;

/// One node of a tree's shape: the tree without its boxes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// An internal node; its first child is the next node, its second is at this index.
    Internal {
        /// The index of the second child.
        second_child: usize,
    },
    /// A leaf for a primitive (section trees, extern aabb trees) or a section (mesh trees).
    Leaf {
        /// The primitive or section index.
        payload: u32,
    },
}

/// The shape of a decoded tree.
pub fn shape_of(tree: &BvTree) -> Vec<Shape> {
    tree.nodes()
        .iter()
        .map(|n| match n.kind {
            BvNodeKind::Internal { second_child } => Shape::Internal { second_child },
            BvNodeKind::Leaf { payload } => Shape::Leaf { payload },
        })
        .collect()
}

/// An encoded tree.
#[derive(Debug, Clone, PartialEq)]
pub struct EncodedTree {
    /// The nodes, `width` bytes each.
    pub bytes: Vec<u8>,
    /// The box each node decodes to (the box [`BvTree::decode`] computes from the same bytes and domain).
    pub boxes: Vec<Aabb>,
    /// Each leaf's payload and node index, in node order.
    pub leaves: Vec<(u32, usize)>,
}

/// The union of two boxes.
pub fn union(a: &Aabb, b: &Aabb) -> Aabb {
    let mut out = *a;
    for axis in 0..3 {
        if let (Some(lo), Some(blo)) = (out[0].get_mut(axis), b[0].get(axis))
            && *blo < *lo
        {
            *lo = *blo;
        }
        if let (Some(hi), Some(bhi)) = (out[1].get_mut(axis), b[1].get(axis))
            && *bhi > *hi
        {
            *hi = *bhi;
        }
    }
    out
}

/// A nibble guess: the value clamped to 0 to 15.
#[allow(clippy::cast_possible_truncation, reason = "clamped to 0..=15 first")]
fn small(x: f64) -> u8 {
    x.clamp(0.0, 15.0) as u8
}

/// The largest nibble (0 to 15) whose side lies no further in than `target`: `parent_edge + extent × n² / 226` (the
/// minimum side, `from_min`) or `parent_edge - extent × n² / 226` (the maximum side) tested with the expression the
/// decoder evaluates.
fn nibble(parent_edge: f64, extent: f64, target: f64, from_min: bool) -> u8 {
    let gap = if from_min {
        target - parent_edge
    } else {
        parent_edge - target
    };
    if extent.is_nan() || gap.is_nan() || extent <= 0.0 || gap <= 0.0 {
        return 0;
    }
    let mut n = small((NIBBLE_DIVISOR * gap / extent).sqrt().floor());
    // the decoder's own arithmetic decides: step down until the side is not past the target
    while n > 0 {
        let step = extent * f64::from(n) * f64::from(n) / NIBBLE_DIVISOR;
        let side = if from_min {
            parent_edge + step
        } else {
            parent_edge - step
        };
        let inside = if from_min { side <= target } else { side >= target };
        if inside {
            break;
        }
        n = n.saturating_sub(1);
    }
    n
}

/// The three axis bytes of a node whose subtree needs `tight` under a parent box `parent`, and the box they decode to.
fn axes_for(parent: &Aabb, tight: &Aabb) -> ([u8; 3], Aabb) {
    let mut bytes = [0u8; 3];
    let mut decoded = *parent;
    for axis in 0..3 {
        let (Some(plo), Some(phi), Some(tlo), Some(thi)) = (
            parent[0].get(axis).copied(),
            parent[1].get(axis).copied(),
            tight[0].get(axis).copied(),
            tight[1].get(axis).copied(),
        ) else {
            continue;
        };
        let extent = phi - plo;
        let h = nibble(plo, extent, tlo, true);
        let l = nibble(phi, extent, thi, false);
        if let Some(b) = bytes.get_mut(axis) {
            *b = h << 4 | l;
        }
        // exactly the decoder's expression
        if let Some(lo) = decoded[0].get_mut(axis) {
            *lo = plo + extent * f64::from(h) * f64::from(h) / NIBBLE_DIVISOR;
        }
        if let Some(hi) = decoded[1].get_mut(axis) {
            *hi = phi - extent * f64::from(l) * f64::from(l) / NIBBLE_DIVISOR;
        }
    }
    (bytes, decoded)
}

/// The data bytes of a node: an internal node's second child `offset` nodes on, or a leaf's `payload`. Refused when the
/// value does not fit the width.
fn data_bytes(width: NodeWidth, shape: Shape, index: usize) -> Result<Vec<u8>> {
    let too_big = |what: &str| {
        invalid(
            index,
            format!("a tree node's {what} does not fit its {} bytes", width.bytes()),
        )
    };
    match (width, shape) {
        (NodeWidth::Four, Shape::Internal { second_child }) => {
            let offset = second_child
                .checked_sub(index)
                .filter(|o| o & 1 == 0)
                .ok_or_else(|| too_big("second child offset"))?;
            let byte = u8::try_from(offset)
                .ok()
                .filter(|b| b & 1 == 0)
                .ok_or_else(|| too_big("second child offset"))?;
            Ok(vec![byte | 1])
        }
        (NodeWidth::Four, Shape::Leaf { payload }) => {
            let byte = u8::try_from(payload)
                .ok()
                .filter(|p| *p < 128)
                .ok_or_else(|| too_big("primitive index"))?;
            Ok(vec![byte << 1])
        }
        (NodeWidth::Five, Shape::Internal { second_child }) => {
            let offset = second_child
                .checked_sub(index)
                .filter(|o| o & 1 == 0)
                .ok_or_else(|| too_big("second child offset"))?;
            let half = u16::try_from(offset >> 1)
                .ok()
                .filter(|h| *h < 0x8000)
                .ok_or_else(|| too_big("second child offset"))?;
            Ok((half | 0x8000).to_be_bytes().to_vec())
        }
        (NodeWidth::Five, Shape::Leaf { payload }) => {
            let value = u16::try_from(payload)
                .ok()
                .filter(|p| *p < 0x8000)
                .ok_or_else(|| too_big("section index"))?;
            Ok(value.to_be_bytes().to_vec())
        }
        (NodeWidth::Six, Shape::Internal { second_child }) => {
            let offset = second_child
                .checked_sub(index)
                .filter(|o| o & 1 == 0)
                .ok_or_else(|| too_big("second child offset"))?;
            let half = u32::try_from(offset >> 1)
                .ok()
                .filter(|h| *h < 0x80_0000)
                .ok_or_else(|| too_big("second child offset"))?;
            six(0x80, half)
        }
        (NodeWidth::Six, Shape::Leaf { payload }) => {
            if payload >= 0x80_0000 {
                return Err(too_big("primitive index"));
            }
            six(0, payload)
        }
    }
}

/// `hiData` and the little-endian `loData` of a 23-bit `value` with the high flag byte `flag`.
fn six(flag: u8, value: u32) -> Result<Vec<u8>> {
    let hi = u8::try_from(value >> 16).map_err(|_| invalid(0, "a 23-bit value that is wider"))?;
    let [low, high, ..] = (value & 0xffff).to_le_bytes();
    Ok(vec![flag | hi, low, high])
}

/// Encode a tree of `shape` with the box `leaf_box(payload)` under each leaf, rooted at `domain`.
///
/// Each node's nibbles are the largest that keep its box inside what lies below it; the box a node decodes
/// to is then at least the union of its children's boxes grown back to the nodes' own, so every leaf's decoded box holds
/// the box it was given (to the rounding of the decoder's `f64` arithmetic).
///
/// Refused: a shape that is not a pre-order binary tree (a second child outside the node's subtree, a leaf that does not
/// end its subtree), a leaf without a box, a value too large for the node width, a domain that is not finite and ordered.
pub fn encode_tree(
    width: NodeWidth,
    domain: [[f32; 3]; 2],
    shape: &[Shape],
    leaf_box: &dyn Fn(u32) -> Option<Aabb>,
) -> Result<EncodedTree> {
    let count = shape.len();
    let root: Aabb = domain.map(|corner| corner.map(f64::from));
    if root.iter().flatten().any(|c| !c.is_finite()) || root[0].iter().zip(&root[1]).any(|(lo, hi)| lo > hi) {
        return Err(invalid(
            0,
            format!("a tree cannot be rooted at the domain {domain:?}"),
        ));
    }
    // the tight box under each node: children come after their parent, so fill from the back
    let mut tight: Vec<Aabb> = vec![root; count];
    for i in (0..count).rev() {
        let node = match shape.get(i).copied() {
            Some(Shape::Leaf { payload }) => leaf_box(payload)
                .ok_or_else(|| invalid(i, format!("no box for the leaf of payload {payload}")))?,
            Some(Shape::Internal { second_child }) => {
                let first = tight.get(i.saturating_add(1)).copied();
                let second = tight.get(second_child).copied();
                match (first, second) {
                    (Some(a), Some(b)) if second_child > i.saturating_add(1) => union(&a, &b),
                    _ => {
                        return Err(invalid(
                            i,
                            format!("node {i}: its second child {second_child} is not a later node"),
                        ));
                    }
                }
            }
            None => return Err(invalid(i, "a node past the shape")),
        };
        if let Some(slot) = tight.get_mut(i) {
            *slot = node;
        }
    }
    let node_bytes = width.bytes();
    let mut bytes = vec![0u8; count.saturating_mul(node_bytes)];
    let mut boxes: Vec<Aabb> = vec![root; count];
    let mut leaves: Vec<(u32, usize)> = Vec::new();
    // (node, end of its subtree, the parent's decoded box); the second child is pushed first so the first pops first
    let mut stack: Vec<(usize, usize, Aabb)> = Vec::new();
    if count > 0 {
        stack.push((0, count, root));
    }
    let mut reached = 0usize;
    while let Some((i, end, parent)) = stack.pop() {
        let this = shape.get(i).copied().filter(|_| i < end);
        let Some(this) = this else {
            return Err(invalid(
                i,
                format!("node {i} lies outside its subtree, which ends at {end}"),
            ));
        };
        let tight_here = tight.get(i).copied().unwrap_or(root);
        let (axes, decoded) = axes_for(&parent, &tight_here);
        let data = data_bytes(width, this, i)?;
        let at = i.saturating_mul(node_bytes);
        let [a, b, c] = axes;
        let mut node = vec![a, b, c];
        node.extend(data);
        if node.len() != node_bytes {
            return Err(invalid(i, "a node of the wrong width"));
        }
        bytes
            .get_mut(at..at.saturating_add(node_bytes))
            .ok_or_else(|| invalid(i, "a node past the bytes"))?
            .copy_from_slice(&node);
        if let Some(slot) = boxes.get_mut(i) {
            *slot = decoded;
        }
        reached = reached.saturating_add(1);
        match this {
            Shape::Internal { second_child } => {
                if second_child <= i.saturating_add(1) || second_child >= end {
                    return Err(invalid(
                        i,
                        format!(
                            "node {i}: its second child at {second_child} is not inside its subtree, which ends at {end}"
                        ),
                    ));
                }
                stack.push((second_child, end, decoded));
                stack.push((i.saturating_add(1), second_child, decoded));
            }
            Shape::Leaf { payload } => {
                if i.saturating_add(1) != end {
                    return Err(invalid(
                        i,
                        format!("leaf {i} does not end its subtree, which ends at {end}"),
                    ));
                }
                leaves.push((payload, i));
            }
        }
    }
    if reached != count {
        return Err(invalid(0, format!("{reached} of {count} nodes reached")));
    }
    leaves.sort_unstable();
    if let Some(pair) = leaves
        .windows(2)
        .find(|w| w.first().map(|a| a.0) == w.get(1).map(|b| b.0))
    {
        let payload = pair.first().map_or(0, |a| a.0);
        return Err(invalid(0, format!("two leaves name {payload}")));
    }
    // back in node order
    leaves.sort_unstable_by_key(|&(_, node)| node);
    Ok(EncodedTree { bytes, boxes, leaves })
}

/// A shape for the boxes `boxes`: split the leaves in two at the median along the longest axis of their centres, and
/// each half again. Leaf payloads count up in pre-order, so the leaf of payload `k` is the `k`-th leaf. Returns the shape
/// and, for each payload, the index of the box it stands for.
///
/// No boxes make an empty shape. Refused: a box with a non-finite side.
pub fn balanced_shape(boxes: &[Aabb]) -> Result<(Vec<Shape>, Vec<usize>)> {
    if boxes.iter().flatten().flatten().any(|c| !c.is_finite()) {
        return Err(invalid(0, "a box with a side that is not finite"));
    }
    let centre = |i: usize, axis: usize| -> f64 {
        boxes.get(i).map_or(0.0, |b| {
            (b[0].get(axis).copied().unwrap_or(0.0) + b[1].get(axis).copied().unwrap_or(0.0)) * 0.5
        })
    };
    let mut order: Vec<usize> = (0..boxes.len()).collect();
    let mut shape: Vec<Shape> = Vec::with_capacity(boxes.len().saturating_mul(2));
    let mut leaves: Vec<usize> = Vec::with_capacity(boxes.len());
    enum Frame {
        Range { start: usize, end: usize },
        Second { node: usize, start: usize, end: usize },
    }
    let mut stack = vec![Frame::Range {
        start: 0,
        end: boxes.len(),
    }];
    while let Some(frame) = stack.pop() {
        let (start, end) = match frame {
            Frame::Range { start, end } => (start, end),
            Frame::Second { node, start, end } => {
                let next = shape.len();
                if let Some(Shape::Internal { second_child }) = shape.get_mut(node) {
                    *second_child = next;
                }
                (start, end)
            }
        };
        let len = end.saturating_sub(start);
        if len == 0 {
            continue;
        }
        if len == 1 {
            let payload = u32::try_from(leaves.len()).map_err(|_| invalid(0, "more than 2^32 leaves"))?;
            shape.push(Shape::Leaf { payload });
            if let Some(&item) = order.get(start) {
                leaves.push(item);
            }
            continue;
        }
        // the longest axis of the centres of this range
        let mut low = [f64::MAX; 3];
        let mut high = [f64::MIN; 3];
        for &item in order.get(start..end).unwrap_or(&[]) {
            for axis in 0..3 {
                let c = centre(item, axis);
                if let (Some(l), Some(h)) = (low.get_mut(axis), high.get_mut(axis)) {
                    *l = l.min(c);
                    *h = h.max(c);
                }
            }
        }
        let axis = (0..3)
            .max_by(|&a, &b| {
                let ea = high.get(a).copied().unwrap_or(0.0) - low.get(a).copied().unwrap_or(0.0);
                let eb = high.get(b).copied().unwrap_or(0.0) - low.get(b).copied().unwrap_or(0.0);
                ea.partial_cmp(&eb).unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or(0);
        if let Some(range) = order.get_mut(start..end) {
            range.sort_unstable_by(|&a, &b| {
                centre(a, axis)
                    .partial_cmp(&centre(b, axis))
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(a.cmp(&b))
            });
        }
        let mid = start.saturating_add(len >> 1);
        let node = shape.len();
        shape.push(Shape::Internal { second_child: 0 });
        stack.push(Frame::Second {
            node,
            start: mid,
            end,
        });
        stack.push(Frame::Range { start, end: mid });
    }
    Ok((shape, leaves))
}

/// The number of leaves up to which [`sah_shape`] merges a node's leaves pairwise from the bottom instead of splitting it.
pub const MERGE_LEAVES: usize = 16;

/// Half the surface area of a box: the sum of the products of its extents (an extent below zero counts as zero).
fn surface(b: &Aabb) -> f64 {
    let extent = |axis: usize| -> f64 {
        let lo = b[0].get(axis).copied().unwrap_or(0.0);
        let hi = b[1].get(axis).copied().unwrap_or(0.0);
        (hi - lo).max(0.0)
    };
    let (x, y, z) = (extent(0), extent(1), extent(2));
    x * y + y * z + x * z
}

/// The centre of a box along an axis.
fn centre_of(b: &Aabb, axis: usize) -> f64 {
    (b[0].get(axis).copied().unwrap_or(0.0) + b[1].get(axis).copied().unwrap_or(0.0)) * 0.5
}

/// A count as a float.
#[allow(clippy::cast_precision_loss, reason = "counts of leaves are far below 2^53")]
fn float_of(n: usize) -> f64 {
    n as f64
}

/// Order two items of `boxes` by the centres of their boxes along `axis`, the lower index first on a tie.
fn by_centre(boxes: &[Aabb], a: usize, b: usize, axis: usize) -> std::cmp::Ordering {
    let ca = boxes.get(a).map_or(0.0, |x| centre_of(x, axis));
    let cb = boxes.get(b).map_or(0.0, |x| centre_of(x, axis));
    ca.partial_cmp(&cb)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then(a.cmp(&b))
}

/// Split `items` (indices into `boxes`; at least two) into a low and a high side, and return how many items the low side
/// has (one or more, and fewer than all). `items` comes back ordered by the centres along the axis of the split, the low
/// side first.
///
/// The split is the one, over the three axes and every position in the order of the items' centres along the axis, with
/// the least `surface(low) * count(low) + surface(high) * count(high)`, where a side's surface is that of the union of
/// its boxes. Costs within a relative `1e-12` of each other tie and the more even split of the two wins (identical boxes
/// would otherwise peel one item at a time); of equal splits the lower axis and position win.
pub(crate) fn sah_split(boxes: &[Aabb], items: &mut [usize]) -> usize {
    let len = items.len();
    // (cost, imbalance, axis, low count)
    let mut best: Option<(f64, usize, usize, usize)> = None;
    let mut high_surface = vec![0.0f64; len];
    for axis in 0..3 {
        items.sort_by(|&a, &b| by_centre(boxes, a, b, axis));
        // the surface of the union of items[k..]
        let mut joined: Option<Aabb> = None;
        for k in (0..len).rev() {
            if let Some(b) = items.get(k).and_then(|&i| boxes.get(i)) {
                let next = joined.map_or(*b, |j| union(&j, b));
                joined = Some(next);
                if let Some(slot) = high_surface.get_mut(k) {
                    *slot = surface(&next);
                }
            }
        }
        let mut joined: Option<Aabb> = None;
        for k in 1..len {
            let Some(b) = items.get(k.saturating_sub(1)).and_then(|&i| boxes.get(i)) else {
                continue;
            };
            let next = joined.map_or(*b, |j| union(&j, b));
            joined = Some(next);
            let high = len.saturating_sub(k);
            let cost =
                surface(&next) * float_of(k) + high_surface.get(k).copied().unwrap_or(0.0) * float_of(high);
            let imbalance = k.abs_diff(high);
            let better = match best {
                None => true,
                Some((c, imb, _, _)) => {
                    let slack = c.abs() * 1e-12;
                    cost < c - slack || (cost <= c + slack && imbalance < imb)
                }
            };
            if better {
                best = Some((cost, imbalance, axis, k));
            }
        }
    }
    let (axis, low) = best.map_or((0, len >> 1), |(_, _, axis, k)| (axis, k));
    items.sort_by(|&a, &b| by_centre(boxes, a, b, axis));
    low
}

/// A node of the hierarchy [`sah_shape`] builds before it numbers anything.
#[derive(Debug, Clone, Copy)]
struct Joined {
    /// The union of the boxes below.
    bounds: Aabb,
    /// How many leaves are below.
    leaves: usize,
    /// A leaf (its box index), or two children (node indices).
    kind: JoinedKind,
}

#[derive(Debug, Clone, Copy)]
enum JoinedKind {
    Leaf(usize),
    Join(usize, usize),
}

/// Where the root of a piece of the hierarchy goes once it is built.
#[derive(Debug, Clone, Copy)]
enum Slot {
    Root,
    Child { parent: usize, second: bool },
}

/// Merge the leaves `items` from the bottom: the two nodes whose union has the least surface join, until one is left.
/// The first pair of the least cost in the order of the nodes still free wins a tie, and the join takes its place at the
/// end. Pushes the nodes onto `hier` and returns the index of the root.
fn merge_leaves(boxes: &[Aabb], items: &[usize], hier: &mut Vec<Joined>) -> usize {
    let mut free: Vec<usize> = Vec::with_capacity(items.len());
    for &item in items {
        if let Some(b) = boxes.get(item) {
            free.push(hier.len());
            hier.push(Joined {
                bounds: *b,
                leaves: 1,
                kind: JoinedKind::Leaf(item),
            });
        }
    }
    while free.len() > 1 {
        let mut best: Option<(f64, usize, usize)> = None;
        for i in 0..free.len() {
            for j in i.saturating_add(1)..free.len() {
                let pair = free
                    .get(i)
                    .and_then(|&a| hier.get(a))
                    .zip(free.get(j).and_then(|&b| hier.get(b)));
                let Some((a, b)) = pair else { continue };
                let cost = surface(&union(&a.bounds, &b.bounds));
                if best.is_none_or(|(c, _, _)| cost < c) {
                    best = Some((cost, i, j));
                }
            }
        }
        let Some((_, i, j)) = best else { break };
        // j is after i: take it out first
        let (b, a) = (free.remove(j), free.remove(i));
        let (bounds, leaves) = match (hier.get(a), hier.get(b)) {
            (Some(x), Some(y)) => (union(&x.bounds, &y.bounds), x.leaves.saturating_add(y.leaves)),
            _ => break,
        };
        free.push(hier.len());
        hier.push(Joined {
            bounds,
            leaves,
            kind: JoinedKind::Join(a, b),
        });
    }
    free.first().copied().unwrap_or(0)
}

/// A shape for the boxes `boxes` that is cheap to walk: a node of more than [`MERGE_LEAVES`] leaves is split by
/// the surface-area heuristic over the three axes, a node of that many or fewer has its leaves merged
/// from the bottom, always the two nodes whose union has the least surface, and in every node the child with the larger
/// box is the first (of two boxes of one surface, the child with more leaves). Leaf payloads count up in pre-order, so the leaf of payload `k` is the `k`-th leaf. Returns the shape
/// and, for each payload, the index of the box it stands for.
///
/// No boxes make an empty shape. Refused: a box with a non-finite side.
pub fn sah_shape(boxes: &[Aabb]) -> Result<(Vec<Shape>, Vec<usize>)> {
    sah_shape_with(boxes, MERGE_LEAVES)
}

/// [`sah_shape`] with the number of leaves up to which a node is merged from the bottom given: `0` or `1` splits every
/// node of two leaves or more, `usize::MAX` merges the whole tree from the bottom (cubic in the number of boxes).
pub fn sah_shape_with(boxes: &[Aabb], merge_up_to: usize) -> Result<(Vec<Shape>, Vec<usize>)> {
    if boxes.iter().flatten().flatten().any(|c| !c.is_finite()) {
        return Err(invalid(0, "a box with a side that is not finite"));
    }
    if boxes.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    // the hierarchy: split the big pieces, merge the small ones
    let mut hier: Vec<Joined> = Vec::with_capacity(boxes.len().saturating_mul(2));
    let mut root = 0usize;
    let mut work: Vec<(Vec<usize>, Slot)> = vec![((0..boxes.len()).collect(), Slot::Root)];
    while let Some((mut items, slot)) = work.pop() {
        let id = if items.len() < 2 || items.len() <= merge_up_to {
            merge_leaves(boxes, &items, &mut hier)
        } else {
            let bounds = items
                .iter()
                .filter_map(|&i| boxes.get(i))
                .fold(None, |acc: Option<Aabb>, b| {
                    Some(acc.map_or(*b, |a| union(&a, b)))
                })
                .unwrap_or([[0.0; 3]; 2]);
            let leaves = items.len();
            let low = sah_split(boxes, &mut items);
            let high = items.split_off(low);
            let id = hier.len();
            hier.push(Joined {
                bounds,
                leaves,
                kind: JoinedKind::Join(usize::MAX, usize::MAX),
            });
            // the low side is built first
            work.push((
                high,
                Slot::Child {
                    parent: id,
                    second: true,
                },
            ));
            work.push((
                items,
                Slot::Child {
                    parent: id,
                    second: false,
                },
            ));
            id
        };
        match slot {
            Slot::Root => root = id,
            Slot::Child { parent, second } => {
                if let Some(Joined {
                    kind: JoinedKind::Join(first_child, second_child),
                    ..
                }) = hier.get_mut(parent)
                {
                    *(if second { second_child } else { first_child }) = id;
                }
            }
        }
    }
    // numbering: pre-order, the child with the larger box first, of equal boxes the one with more leaves (else the earlier)
    enum Emit {
        Node(usize),
        Second { at: usize, id: usize },
    }
    let mut shape: Vec<Shape> = Vec::with_capacity(boxes.len().saturating_mul(2));
    let mut leaves: Vec<usize> = Vec::with_capacity(boxes.len());
    let mut stack = vec![Emit::Node(root)];
    while let Some(frame) = stack.pop() {
        let id = match frame {
            Emit::Node(id) => id,
            Emit::Second { at, id } => {
                let next = shape.len();
                if let Some(Shape::Internal { second_child }) = shape.get_mut(at) {
                    *second_child = next;
                }
                id
            }
        };
        match hier.get(id).map(|h| h.kind) {
            Some(JoinedKind::Leaf(item)) => {
                let payload = u32::try_from(leaves.len()).map_err(|_| invalid(0, "more than 2^32 leaves"))?;
                shape.push(Shape::Leaf { payload });
                leaves.push(item);
            }
            Some(JoinedKind::Join(a, b)) => {
                let rank = |n: usize| hier.get(n).map_or((0.0, 0), |h| (surface(&h.bounds), h.leaves));
                let (ra, rb) = (rank(a), rank(b));
                let b_first = rb.0 > ra.0 || (rb.0 == ra.0 && rb.1 > ra.1);
                let (first, second) = if b_first { (b, a) } else { (a, b) };
                let at = shape.len();
                shape.push(Shape::Internal { second_child: 0 });
                stack.push(Emit::Second { at, id: second });
                stack.push(Emit::Node(first));
            }
            None => {
                return Err(invalid(0, "a hierarchy node that was not built"));
            }
        }
    }
    Ok((shape, leaves))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    reason = "a test reads what it just built"
)]
mod tests {
    use super::*;

    /// A box whose extent is 226 on every axis, so a nibble `n` moves a side by exactly `n * n`.
    const DOMAIN: [[f32; 3]; 2] = [[0.0; 3], [226.0; 3]];

    fn boxed(lo: [f64; 3], hi: [f64; 3]) -> Aabb {
        [lo, hi]
    }

    /// A tree of `leaves` leaves whose every internal node has an internal first child (but the last), so the root's
    /// second child is `2 * (leaves - 1)` nodes on: the internal nodes first, then the leaves.
    fn left_comb(leaves: u32) -> Vec<Shape> {
        let internal = (leaves - 1) as usize;
        let mut shape = Vec::new();
        for k in 0..internal {
            shape.push(Shape::Internal {
                second_child: 2 * (internal - k) + k,
            });
        }
        for payload in 0..leaves {
            shape.push(Shape::Leaf { payload });
        }
        shape
    }

    #[test]
    fn a_nibble_is_the_largest_that_stays_inside() {
        // extent 226: the minimum side moves n * n
        assert_eq!(nibble(0.0, 226.0, 0.0, true), 0);
        assert_eq!(nibble(0.0, 226.0, 3.9, true), 1);
        assert_eq!(nibble(0.0, 226.0, 4.0, true), 2);
        assert_eq!(nibble(0.0, 226.0, 80.0, true), 8);
        assert_eq!(nibble(0.0, 226.0, 225.0, true), 15);
        assert_eq!(nibble(0.0, 226.0, 1e9, true), 15);
        // the maximum side moves in from 226
        assert_eq!(nibble(226.0, 226.0, 226.0, false), 0);
        assert_eq!(nibble(226.0, 226.0, 222.0, false), 2);
        assert_eq!(nibble(226.0, 226.0, 222.5, false), 1);
        // a flat parent, or a target outside it, takes nothing
        assert_eq!(nibble(5.0, 0.0, 5.0, true), 0);
        assert_eq!(nibble(0.0, 226.0, -10.0, true), 0);
    }

    #[test]
    fn a_two_leaf_tree_encodes_and_decodes_to_boxes_that_hold_its_leaves() {
        let boxes = [
            boxed([1.0, 1.0, 1.0], [10.0, 10.0, 10.0]),
            boxed([100.0, 4.0, 0.0], [200.0, 225.0, 226.0]),
        ];
        let shape = [
            Shape::Internal { second_child: 2 },
            Shape::Leaf { payload: 0 },
            Shape::Leaf { payload: 1 },
        ];
        let leaf = |p: u32| boxes.get(p as usize).copied();
        for width in [NodeWidth::Four, NodeWidth::Five, NodeWidth::Six] {
            let tree = encode_tree(width, DOMAIN, &shape, &leaf).unwrap();
            assert_eq!(tree.bytes.len(), 3 * width.bytes());
            let decoded = BvTree::decode(&tree.bytes, width, DOMAIN, 0).unwrap();
            assert_eq!(shape_of(&decoded), shape);
            assert_eq!(tree.leaves, [(0, 1), (1, 2)]);
            for (i, node) in decoded.nodes().iter().enumerate() {
                // the encoder's boxes are the decoder's, bit for bit
                assert_eq!(node.aabb, tree.boxes[i], "{width:?} node {i}");
            }
            // each leaf's box holds the box it was given
            for (p, want) in boxes.iter().enumerate() {
                let got = &decoded.nodes()[tree.leaves[p].1].aabb;
                for axis in 0..3 {
                    assert!(
                        got[0][axis] <= want[0][axis] && got[1][axis] >= want[1][axis],
                        "{width:?} leaf {p}"
                    );
                }
            }
        }
    }

    #[test]
    fn stock_nibbles_are_chosen_by_the_floor_rule() {
        // a root over the domain whose first child needs [4, 4, 0]..[225, 226, 226]: the minimum side moves 4 = 2 * 2
        let boxes = [boxed([4.0, 4.0, 0.0], [225.0, 226.0, 226.0])];
        let shape = [Shape::Leaf { payload: 0 }];
        let tree = encode_tree(NodeWidth::Four, DOMAIN, &shape, &|p| {
            boxes.get(p as usize).copied()
        })
        .unwrap();
        // x: min 4 -> 2, max 225 -> 226 - 1 -> l = 1; y: 2 and 0; z: 0 and 0
        assert_eq!(tree.bytes[..3], [0x21, 0x20, 0x00]);
        assert_eq!(tree.bytes[3], 0, "the lone leaf names payload 0");
    }

    #[test]
    fn values_too_large_for_the_width_are_refused() {
        let boxes = [boxed([0.0; 3], [1.0; 3])];
        let leaf = |_: u32| Some(boxes[0]);
        // a leaf payload of 128 does not fit a 4-byte node's seven bits
        let e = encode_tree(NodeWidth::Four, DOMAIN, &[Shape::Leaf { payload: 128 }], &leaf)
            .unwrap_err()
            .to_string();
        assert!(e.contains("primitive index"), "{e}");
        // a second child 254 nodes on is the furthest a 4-byte node reaches, 256 is too far
        assert!(encode_tree(NodeWidth::Four, DOMAIN, &left_comb(128), &leaf).is_ok());
        let e = encode_tree(NodeWidth::Four, DOMAIN, &left_comb(129), &leaf)
            .unwrap_err()
            .to_string();
        assert!(e.contains("second child offset"), "{e}");
        // a shape whose second child is not inside the tree
        let e = encode_tree(
            NodeWidth::Five,
            DOMAIN,
            &[Shape::Internal { second_child: 5 }],
            &leaf,
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("not a later node"), "{e}");
        // a leaf without a box
        let e = encode_tree(NodeWidth::Five, DOMAIN, &[Shape::Leaf { payload: 0 }], &|_| None)
            .unwrap_err()
            .to_string();
        assert!(e.contains("no box for the leaf"), "{e}");
    }

    #[test]
    fn a_balanced_shape_visits_every_box_once_and_numbers_leaves_in_pre_order() {
        let boxes: Vec<Aabb> = (0..37)
            .map(|i| {
                let x = f64::from((i * 7919) % 101);
                boxed([x, 0.0, 0.0], [x + 1.0, 1.0, 1.0])
            })
            .collect();
        let (shape, leaves) = balanced_shape(&boxes).unwrap();
        assert_eq!(shape.len(), 2 * 37 - 1);
        let mut seen = leaves.clone();
        seen.sort_unstable();
        assert_eq!(seen, (0..37).collect::<Vec<_>>());
        let payloads: Vec<u32> = shape
            .iter()
            .filter_map(|s| {
                if let Shape::Leaf { payload } = s {
                    Some(*payload)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(payloads, (0..37).collect::<Vec<_>>());
        // the shape is a valid tree: encoding it succeeds, and it decodes back to itself
        let leaf = |p: u32| leaves.get(p as usize).and_then(|&i| boxes.get(i).copied());
        let domain = [[0.0f32, 0.0, 0.0], [110.0, 1.0, 1.0]];
        let tree = encode_tree(NodeWidth::Four, domain, &shape, &leaf).unwrap();
        let decoded = BvTree::decode(&tree.bytes, NodeWidth::Four, domain, 0).unwrap();
        assert_eq!(shape_of(&decoded), shape);
        // a single box, and none
        assert_eq!(
            balanced_shape(&boxes[..1]).unwrap().0,
            [Shape::Leaf { payload: 0 }]
        );
        assert!(balanced_shape(&[]).unwrap().0.is_empty());
        // a non-finite box
        assert!(balanced_shape(&[boxed([f64::NAN; 3], [0.0; 3])]).is_err());
    }

    /// A unit cube whose minimum corner is at `x` on the first axis.
    fn cube(x: f64) -> Aabb {
        boxed([x, 0.0, 0.0], [x + 1.0, 1.0, 1.0])
    }

    #[test]
    fn the_two_nearest_boxes_join_first_and_the_larger_child_comes_first() {
        // two cubes side by side make a 2 x 1 x 1 box (surface 2 + 1 + 2 = 5), a cube alone has 3, and the third cube is
        // far off: the joins are (0, 1) with 5, (1, 2) with 21 and (0, 2) with 23. The pair is the larger child of the
        // root (5 against 3) so it is first; its own two cubes tie and stay in order.
        let (shape, leaves) = sah_shape(&[cube(0.0), cube(1.0), cube(10.0)]).unwrap();
        assert_eq!(
            shape,
            [
                Shape::Internal { second_child: 4 },
                Shape::Internal { second_child: 3 },
                Shape::Leaf { payload: 0 },
                Shape::Leaf { payload: 1 },
                Shape::Leaf { payload: 2 },
            ]
        );
        assert_eq!(leaves, [0, 1, 2]);
        // a far box that is larger than the pair (a 6 x 6 x 6 box has surface 108) goes first, and is numbered first
        let big = boxed([10.0, 0.0, 0.0], [16.0, 6.0, 6.0]);
        let (shape, leaves) = sah_shape(&[cube(0.0), cube(1.0), big]).unwrap();
        assert_eq!(
            shape,
            [
                Shape::Internal { second_child: 2 },
                Shape::Leaf { payload: 0 },
                Shape::Internal { second_child: 4 },
                Shape::Leaf { payload: 1 },
                Shape::Leaf { payload: 2 },
            ]
        );
        assert_eq!(leaves, [2, 0, 1]);
    }

    #[test]
    fn children_of_one_surface_come_with_the_one_of_more_leaves_first() {
        // a box [0, 2] x [0, 1] x [0, 1] (surface 5) and two cubes that fill the same box between them: all three joins cost
        // 5, the first (the two cubes) wins, and the root has a child of 5 with two leaves and one of 5 with one
        let wide = boxed([0.0, 0.0, 0.0], [2.0, 1.0, 1.0]);
        let (shape, leaves) = sah_shape(&[cube(0.0), cube(1.0), wide]).unwrap();
        assert_eq!(
            shape,
            [
                Shape::Internal { second_child: 4 },
                Shape::Internal { second_child: 3 },
                Shape::Leaf { payload: 0 },
                Shape::Leaf { payload: 1 },
                Shape::Leaf { payload: 2 },
            ]
        );
        assert_eq!(leaves, [0, 1, 2]);
    }

    #[test]
    fn a_split_separates_a_far_group_where_the_median_would_not() {
        // eight cubes in a row at x = 0 to 7 and two at x = 100 and 101. The split after eight costs 17 * 8 + 5 * 2 = 146
        // (the row is 8 x 1 x 1: surface 8 + 1 + 8; the pair is 2 x 1 x 1: surface 5); after seven it costs
        // 15 * 7 + 191 * 3 = 678, after five (the median) 11 * 5 + 195 * 5 = 1030.
        let mut boxes: Vec<Aabb> = (0..8).map(|i| cube(f64::from(i))).collect();
        boxes.push(cube(100.0));
        boxes.push(cube(101.0));
        let mut items = vec![9, 3, 0, 7, 5, 1, 8, 2, 6, 4];
        let low = sah_split(&boxes, &mut items);
        assert_eq!(low, 8);
        let mut row = items[..8].to_vec();
        row.sort_unstable();
        assert_eq!(row, (0..8).collect::<Vec<_>>());
        let mut far = items[8..].to_vec();
        far.sort_unstable();
        assert_eq!(far, [8, 9]);
        // identical boxes cost the same wherever they are split: the even split wins
        let same = vec![cube(0.0); 40];
        let mut items: Vec<usize> = (0..40).collect();
        assert_eq!(sah_split(&same, &mut items), 20);
    }

    #[test]
    fn a_sah_shape_visits_every_box_once_and_fits_a_section() {
        // 127 boxes, the most a section holds: more than the 16 that are merged from the bottom, so the split runs too
        let boxes: Vec<Aabb> = (0..127)
            .map(|i| {
                let x = f64::from((i * 7919) % 101);
                boxed([x, 0.0, f64::from(i % 5)], [x + 1.0, 1.0, f64::from(i % 5) + 1.0])
            })
            .collect();
        let (shape, leaves) = sah_shape(&boxes).unwrap();
        assert_eq!(shape.len(), 2 * 127 - 1);
        let mut seen = leaves.clone();
        seen.sort_unstable();
        assert_eq!(seen, (0..127).collect::<Vec<_>>());
        let payloads: Vec<u32> = shape
            .iter()
            .filter_map(|s| {
                if let Shape::Leaf { payload } = s {
                    Some(*payload)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(payloads, (0..127).collect::<Vec<_>>());
        // a valid pre-order tree that a section's 4-byte nodes hold, and decodes to itself
        let leaf = |p: u32| leaves.get(p as usize).and_then(|&i| boxes.get(i).copied());
        let domain = [[0.0f32, 0.0, 0.0], [110.0, 1.0, 6.0]];
        let tree = encode_tree(NodeWidth::Four, domain, &shape, &leaf).unwrap();
        let decoded = BvTree::decode(&tree.bytes, NodeWidth::Four, domain, 0).unwrap();
        assert_eq!(shape_of(&decoded), shape);
        // every node's first child has at least the surface of its second (the larger child first)
        let tight = |i: usize| -> f64 {
            let mut lo = [f64::MAX; 3];
            let mut hi = [f64::MIN; 3];
            let mut stack = vec![i];
            // the leaves under node i
            while let Some(n) = stack.pop() {
                match shape[n] {
                    Shape::Leaf { payload } => {
                        let b = boxes[leaves[payload as usize]];
                        for a in 0..3 {
                            lo[a] = lo[a].min(b[0][a]);
                            hi[a] = hi[a].max(b[1][a]);
                        }
                    }
                    Shape::Internal { second_child } => {
                        stack.push(second_child);
                        stack.push(n + 1);
                    }
                }
            }
            surface(&[lo, hi])
        };
        for (i, node) in shape.iter().enumerate() {
            if let Shape::Internal { second_child } = node {
                assert!(tight(i + 1) >= tight(*second_child), "node {i}");
            }
        }
        // a single box, and none
        assert_eq!(sah_shape(&boxes[..1]).unwrap().0, [Shape::Leaf { payload: 0 }]);
        assert!(sah_shape(&[]).unwrap().0.is_empty());
        assert!(sah_shape(&[boxed([f64::NAN; 3], [0.0; 3])]).is_err());
    }
}
