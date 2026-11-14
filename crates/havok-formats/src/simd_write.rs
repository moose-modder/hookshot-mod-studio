//! Writing the four-lane bounding-volume tree (`hkcdSimdTree`) of a mesh: [`build_simd_tree`] groups the primitives'
//! boxes four to a node and numbers the nodes the way the games' files do, and the node bytes are written from the file's
//! own type table.
//!
//! **What the stock files show** (every simd tree of the Sekiro and Elden Ring map, object and asset collision,
//! 3,344 + 22,899 compressed meshes and 12,688 + 264,858 extern meshes; the numbering below was tested on the 959 Sekiro and
//! 278,433 Elden Ring trees of 300 nodes or fewer, and holds for all of them):
//!
//! - Node 0 is a sentinel with four empty lanes, node 1 is the root, a child's index is above its parent's. A lane's
//!   box is exactly the union of the boxes of the lanes of the node it names, and a leaf lane's box is exactly the box of
//!   its primitive's vertices.
//! - Elden Ring's nodes carry an `isLeaf` flag: a node has either four child lanes or four leaf lanes (a lane without
//!   either is empty), never both. Its children are numbered when their parent is taken from a stack, in lane order: the
//!   root's children are 2 to 5, then the last of them is taken, and its children get the next numbers, and so on.
//! - Sekiro's nodes have no flag: each lane's word is tagged, and a node may hold leaves and children together. The
//!   nodes are numbered in depth-first pre-order, visiting a node's lanes from the last.
//! - What decides which primitives share a node is not a rule this crate reproduces: in the extern meshes of up to 32
//!   triangles the leaves are four consecutive keys to a node, in the rest the groups follow the geometry in ways that
//!   were not decoded. [`build_simd_tree`] splits at the median along the longest axis of the centres.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use crate::schema::{Schema, put_f32s, put_uint};
use crate::simdtree::{LaneBox, SimdLane, SimdNode};
use crate::{Result, invalid};

/// How a file encodes the lanes of its simd nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimdEncoding {
    /// Sekiro's (SDK 2016.2): tagged data words, mixed nodes, depth-first numbering.
    Tagged,
    /// Elden Ring's (SDK 2018.1): an `isLeaf` flag per node, breadth-first-from-a-stack numbering.
    Flagged,
}

impl SimdEncoding {
    /// The encoding a simd node class uses: flagged when it has an `isLeaf` member.
    pub(crate) fn of_node(schema: &Schema<'_>, node: usize) -> Self {
        if schema.has_field(node, "isLeaf") {
            Self::Flagged
        } else {
            Self::Tagged
        }
    }
}

/// One primitive of a simd tree: its shape key and the box of its vertices.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SimdLeaf {
    /// The key a leaf lane names: the shape key of a compressed mesh's primitive (its first half), or the triangle index
    /// of an extern mesh.
    pub key: u32,
    /// The box of the primitive's vertices.
    pub aabb: LaneBox,
}

/// The box of an empty lane: its minimum above its maximum.
pub const EMPTY_BOX: LaneBox = [[f32::MAX; 3], [-f32::MAX; 3]];

/// What a lane of a node under construction holds.
#[derive(Debug, Clone, Copy)]
enum Slot {
    Empty,
    /// The leaf with this index.
    Leaf(usize),
    /// The node with this arena index.
    Child(usize),
}

#[derive(Debug, Clone, Copy)]
struct Draft {
    lanes: [Slot; 4],
    /// The union of the lanes' boxes.
    aabb: LaneBox,
}

fn union_box(a: &LaneBox, b: &LaneBox) -> LaneBox {
    let mut out = *a;
    for axis in 0..3 {
        if let (Some(lo), Some(blo)) = (out[0].get_mut(axis), b[0].get(axis)) {
            *lo = lo.min(*blo);
        }
        if let (Some(hi), Some(bhi)) = (out[1].get_mut(axis), b[1].get(axis)) {
            *hi = hi.max(*bhi);
        }
    }
    out
}

fn centre(leaf: &SimdLeaf, axis: usize) -> f32 {
    (leaf.aabb[0].get(axis).copied().unwrap_or(0.0) + leaf.aabb[1].get(axis).copied().unwrap_or(0.0)) * 0.5
}

/// Sort `items` along the axis where the centres of `leaves[item]` spread furthest, and return the middle index.
fn halve(leaves: &[SimdLeaf], items: &mut [usize]) -> usize {
    let mut low = [f32::MAX; 3];
    let mut high = [f32::MIN; 3];
    for &i in items.iter() {
        if let Some(leaf) = leaves.get(i) {
            for axis in 0..3 {
                let c = centre(leaf, axis);
                if let (Some(l), Some(h)) = (low.get_mut(axis), high.get_mut(axis)) {
                    *l = l.min(c);
                    *h = h.max(c);
                }
            }
        }
    }
    let spread = |axis: usize| high.get(axis).copied().unwrap_or(0.0) - low.get(axis).copied().unwrap_or(0.0);
    let axis = (0..3)
        .max_by(|&a, &b| {
            spread(a)
                .partial_cmp(&spread(b))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(0);
    items.sort_unstable_by(|&a, &b| {
        let (ca, cb) = (
            leaves.get(a).map_or(0.0, |l| centre(l, axis)),
            leaves.get(b).map_or(0.0, |l| centre(l, axis)),
        );
        ca.partial_cmp(&cb)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.cmp(&b))
    });
    items.len() >> 1
}

/// Build the node for `items`: up to four of them make a node of leaves, more are split in four and each part is a child.
fn draft_node(
    leaves: &[SimdLeaf],
    items: &mut [usize],
    drafts: &mut Vec<Draft>,
    encoding: SimdEncoding,
) -> usize {
    let mut lanes = [Slot::Empty; 4];
    if items.len() <= 4 {
        for (lane, &i) in lanes.iter_mut().zip(items.iter()) {
            *lane = Slot::Leaf(i);
        }
    } else {
        let mid = halve(leaves, items);
        let (left, right) = items.split_at_mut(mid);
        let mut parts: Vec<&mut [usize]> = Vec::with_capacity(4);
        for half in [left, right] {
            let cut = halve(leaves, half);
            let (a, b) = half.split_at_mut(cut);
            parts.push(a);
            parts.push(b);
        }
        for (lane, part) in lanes.iter_mut().zip(parts) {
            *lane = match (encoding, part.first().copied()) {
                // Sekiro's lanes hold a lone leaf directly
                (SimdEncoding::Tagged, Some(only)) if part.len() == 1 => Slot::Leaf(only),
                _ => Slot::Child(draft_node(leaves, part, drafts, encoding)),
            };
        }
    }
    let mut aabb = EMPTY_BOX;
    for lane in &lanes {
        let b = match *lane {
            Slot::Empty => continue,
            Slot::Leaf(i) => match leaves.get(i) {
                Some(leaf) => leaf.aabb,
                None => continue,
            },
            Slot::Child(c) => match drafts.get(c) {
                Some(d) => d.aabb,
                None => continue,
            },
        };
        aabb = union_box(&aabb, &b);
    }
    drafts.push(Draft { lanes, aabb });
    drafts.len().saturating_sub(1)
}

/// Build the simd tree of `leaves`: the nodes in file order, node 0 the empty sentinel and node 1 the root. No leaf gives
/// the two-node tree of all empty lanes that a mesh without primitives stores.
///
/// Refused: a leaf box that is not finite or is inverted, a key the encoding cannot store (Sekiro's tagged words keep 31
/// bits, Elden Ring's `u32::MAX` means empty).
pub fn build_simd_tree(leaves: &[SimdLeaf], encoding: SimdEncoding) -> Result<Vec<SimdNode>> {
    let empty = SimdNode {
        lanes: [SimdLane::Empty; 4],
    };
    if leaves.is_empty() {
        return Ok(vec![empty, empty]);
    }
    for leaf in leaves {
        let ok = leaf.aabb.iter().flatten().all(|c| c.is_finite())
            && leaf.aabb[0].iter().zip(&leaf.aabb[1]).all(|(lo, hi)| lo <= hi);
        if !ok {
            return Err(invalid(
                0,
                format!("the box of simd leaf {} is not a finite, ordered box", leaf.key),
            ));
        }
        let fits = match encoding {
            SimdEncoding::Tagged => leaf.key < 0x8000_0000,
            SimdEncoding::Flagged => leaf.key != u32::MAX,
        };
        if !fits {
            return Err(invalid(
                0,
                format!("the key {} cannot be stored in a simd lane", leaf.key),
            ));
        }
    }
    let mut items: Vec<usize> = (0..leaves.len()).collect();
    let mut drafts: Vec<Draft> = Vec::new();
    let root = draft_node(leaves, &mut items, &mut drafts, encoding);
    // the numbers: Elden Ring's children take the next numbers in lane order as their parent is taken from a stack;
    // Sekiro's nodes are numbered as a depth-first walk reaches them, last lane first
    let mut number = vec![0usize; drafts.len()];
    if let Some(slot) = number.get_mut(root) {
        *slot = 1;
    }
    let mut next = 2usize;
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        let Some(draft) = drafts.get(n) else { continue };
        for lane in &draft.lanes {
            let Slot::Child(c) = *lane else { continue };
            if encoding == SimdEncoding::Flagged {
                if let Some(slot) = number.get_mut(c) {
                    *slot = next;
                }
                next = next.saturating_add(1);
            }
            stack.push(c);
        }
        if encoding == SimdEncoding::Tagged {
            // pre-order numbers are given as nodes are visited, which is when they are popped
            if n != root {
                if let Some(slot) = number.get_mut(n) {
                    *slot = next;
                }
                next = next.saturating_add(1);
            }
        }
    }
    let mut out = vec![empty; drafts.len().saturating_add(1)];
    for (n, draft) in drafts.iter().enumerate() {
        let at = number.get(n).copied().unwrap_or(0);
        let mut lanes = [SimdLane::Empty; 4];
        for (lane, slot) in lanes.iter_mut().zip(&draft.lanes) {
            *lane = match *slot {
                Slot::Empty => SimdLane::Empty,
                Slot::Leaf(i) => match leaves.get(i) {
                    Some(leaf) => SimdLane::Leaf {
                        key: leaf.key,
                        aabb: leaf.aabb,
                    },
                    None => SimdLane::Empty,
                },
                Slot::Child(c) => SimdLane::Child {
                    node: number.get(c).copied().unwrap_or(0),
                    aabb: drafts.get(c).map_or(EMPTY_BOX, |d| d.aabb),
                },
            };
        }
        if let Some(node) = out.get_mut(at) {
            *node = SimdNode { lanes };
        }
    }
    Ok(out)
}

/// The bytes of `nodes` as elements of the simd node class `node_type` of the type table `schema`: the boxes of the four
/// lanes as `lx hx ly hy lz hz`, the four data words, and Elden Ring's `isLeaf` flag (`isActive` stays 0).
///
/// Refused: a node class that lacks one of those members, a node with leaf and child lanes together in an encoding
/// that flags its nodes, a key or node index the data word cannot hold.
pub(crate) fn node_bytes(
    schema: &Schema<'_>,
    node_type: usize,
    nodes: &[SimdNode],
    encoding: SimdEncoding,
) -> Result<Vec<u8>> {
    let size = schema.size_of(node_type)?;
    let mut boxes = [0usize; 6];
    for (slot, name) in boxes.iter_mut().zip(["lx", "hx", "ly", "hy", "lz", "hz"]) {
        *slot = schema.field(node_type, name)?.offset;
    }
    let data = schema.field(node_type, "data")?.offset;
    let flag = if encoding == SimdEncoding::Flagged {
        Some(schema.field(node_type, "isLeaf")?)
    } else {
        None
    };
    let mut out = schema.zeroed(node_type, nodes.len())?;
    for (index, node) in nodes.iter().enumerate() {
        let base = index
            .checked_mul(size)
            .ok_or_else(|| invalid(0, "simd node offset overflow"))?;
        let leaf_node = node.lanes.iter().any(|l| matches!(l, SimdLane::Leaf { .. }));
        let child_node = node.lanes.iter().any(|l| matches!(l, SimdLane::Child { .. }));
        if encoding == SimdEncoding::Flagged && leaf_node && child_node {
            return Err(invalid(
                index,
                format!("simd node {index} holds leaves and children together"),
            ));
        }
        for (lane, l) in node.lanes.iter().enumerate() {
            let (aabb, word) = match *l {
                SimdLane::Empty => (
                    EMPTY_BOX,
                    if encoding == SimdEncoding::Flagged && leaf_node {
                        u32::MAX
                    } else {
                        0
                    },
                ),
                SimdLane::Leaf { key, aabb } => (
                    aabb,
                    match encoding {
                        SimdEncoding::Tagged => {
                            key.checked_shl(1).filter(|_| key < 0x8000_0000).map(|w| w | 1)
                        }
                        SimdEncoding::Flagged => Some(key).filter(|k| *k != u32::MAX),
                    }
                    .ok_or_else(|| {
                        invalid(index, format!("the key {key} cannot be stored in a simd lane"))
                    })?,
                ),
                SimdLane::Child { node: child, aabb } => (
                    aabb,
                    match encoding {
                        SimdEncoding::Tagged => u32::try_from(child)
                            .ok()
                            .filter(|c| *c < 0x8000_0000)
                            .map(|c| c << 1),
                        SimdEncoding::Flagged => u32::try_from(child).ok(),
                    }
                    .ok_or_else(|| {
                        invalid(
                            index,
                            format!("the node index {child} cannot be stored in a simd lane"),
                        )
                    })?,
                ),
            };
            let values = [
                aabb[0][0], aabb[1][0], aabb[0][1], aabb[1][1], aabb[0][2], aabb[1][2],
            ];
            for (offset, value) in boxes.iter().zip(values) {
                let at = base
                    .checked_add(*offset)
                    .and_then(|o| o.checked_add(lane.checked_mul(4)?))
                    .ok_or_else(|| invalid(0, "simd node offset overflow"))?;
                put_f32s(&mut out, at, &[value])?;
            }
            let at = base
                .checked_add(data)
                .and_then(|o| o.checked_add(lane.checked_mul(4)?))
                .ok_or_else(|| invalid(0, "simd node offset overflow"))?;
            put_uint(&mut out, at, 4, u64::from(word))?;
        }
        if let Some(flag) = flag {
            let at = base
                .checked_add(flag.offset)
                .ok_or_else(|| invalid(0, "simd node offset overflow"))?;
            put_uint(&mut out, at, 1, u64::from(leaf_node))?;
        }
    }
    Ok(out)
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

    fn leaf(key: u32, x: f32, y: f32) -> SimdLeaf {
        SimdLeaf {
            key,
            aabb: [[x, y, 0.0], [x + 1.0, y + 1.0, 1.0]],
        }
    }

    /// A grid of leaves with keys `base + 2 * k`.
    fn grid(n: u32) -> Vec<SimdLeaf> {
        (0..n)
            .map(|k| leaf(k * 2, (k % 7) as f32 * 3.0, (k / 7) as f32 * 3.0))
            .collect()
    }

    /// What a valid tree is, in either encoding: the sentinel, every other node the child of one lane, children above
    /// their parents, every leaf once, lane boxes the unions of what lies below.
    fn check(nodes: &[SimdNode], leaves: &[SimdLeaf], encoding: SimdEncoding) {
        assert_eq!(nodes[0].lanes, [SimdLane::Empty; 4]);
        let mut parents = vec![0usize; nodes.len()];
        let mut keys = Vec::new();
        for (i, node) in nodes.iter().enumerate() {
            let leaf_lanes = node
                .lanes
                .iter()
                .filter(|l| matches!(l, SimdLane::Leaf { .. }))
                .count();
            let child_lanes = node
                .lanes
                .iter()
                .filter(|l| matches!(l, SimdLane::Child { .. }))
                .count();
            if encoding == SimdEncoding::Flagged {
                assert!(
                    leaf_lanes == 0 || child_lanes == 0,
                    "node {i} mixes leaves and children"
                );
            }
            for lane in &node.lanes {
                match *lane {
                    SimdLane::Child { node: c, aabb } => {
                        assert!(c > i && c < nodes.len(), "child {c} of node {i}");
                        parents[c] += 1;
                        // the box is the union of the lanes of the child
                        let mut union = EMPTY_BOX;
                        for inner in &nodes[c].lanes {
                            if let SimdLane::Child { aabb: b, .. } | SimdLane::Leaf { aabb: b, .. } = inner {
                                union = union_box(&union, b);
                            }
                        }
                        assert_eq!(aabb, union, "node {i} lane box");
                    }
                    SimdLane::Leaf { key, aabb } => {
                        keys.push(key);
                        let want = leaves.iter().find(|l| l.key == key).unwrap();
                        assert_eq!(aabb, want.aabb);
                    }
                    SimdLane::Empty => {}
                }
            }
        }
        for (i, p) in parents.iter().enumerate().skip(1) {
            assert_eq!(*p, usize::from(i != 1), "node {i} is the child of {p} lanes");
        }
        keys.sort_unstable();
        let mut want: Vec<u32> = leaves.iter().map(|l| l.key).collect();
        want.sort_unstable();
        assert_eq!(keys, want);
    }

    /// The numbering of Elden Ring's files: children numbered in lane order as their parent is taken from a stack.
    fn flagged_numbering_holds(nodes: &[SimdNode]) -> bool {
        let mut next = 2;
        let mut stack = vec![1usize];
        while let Some(n) = stack.pop() {
            let mut kids = Vec::new();
            for lane in &nodes[n].lanes {
                if let SimdLane::Child { node, .. } = lane {
                    if *node != next {
                        return false;
                    }
                    kids.push(*node);
                    next += 1;
                }
            }
            stack.extend(kids);
        }
        next == nodes.len()
    }

    /// The numbering of Sekiro's files: a depth-first walk, last lane first.
    fn tagged_numbering_holds(nodes: &[SimdNode]) -> bool {
        let mut next = 1;
        let mut stack = vec![1usize];
        while let Some(n) = stack.pop() {
            if n != next {
                return false;
            }
            next += 1;
            for lane in &nodes[n].lanes {
                if let SimdLane::Child { node, .. } = lane {
                    stack.push(*node);
                }
            }
        }
        next == nodes.len()
    }

    #[test]
    fn no_leaf_is_the_two_empty_nodes() {
        for encoding in [SimdEncoding::Tagged, SimdEncoding::Flagged] {
            let nodes = build_simd_tree(&[], encoding).unwrap();
            assert_eq!(nodes.len(), 2);
            assert!(nodes.iter().all(|n| n.lanes == [SimdLane::Empty; 4]));
        }
    }

    #[test]
    fn up_to_four_leaves_are_one_node_below_the_root_in_the_flagged_encoding() {
        let leaves = grid(3);
        let nodes = build_simd_tree(&leaves, SimdEncoding::Flagged).unwrap();
        // the sentinel and one node of leaves; the root is that node
        assert_eq!(nodes.len(), 2);
        check(&nodes, &leaves, SimdEncoding::Flagged);
        assert!(matches!(nodes[1].lanes[0], SimdLane::Leaf { .. }));
        assert_eq!(nodes[1].lanes[3], SimdLane::Empty);
    }

    #[test]
    fn trees_are_valid_and_numbered_as_the_files_are() {
        for n in [1u32, 4, 5, 8, 18, 33, 100, 257, 1000] {
            let leaves = grid(n);
            let flagged = build_simd_tree(&leaves, SimdEncoding::Flagged).unwrap();
            check(&flagged, &leaves, SimdEncoding::Flagged);
            assert!(flagged_numbering_holds(&flagged), "flagged, {n} leaves");
            let tagged = build_simd_tree(&leaves, SimdEncoding::Tagged).unwrap();
            check(&tagged, &leaves, SimdEncoding::Tagged);
            assert!(tagged_numbering_holds(&tagged), "tagged, {n} leaves");
        }
    }

    #[test]
    fn keys_a_word_cannot_hold_and_bad_boxes_are_refused() {
        let mut one = grid(1);
        one[0].key = u32::MAX;
        assert!(build_simd_tree(&one, SimdEncoding::Flagged).is_err());
        one[0].key = 0x8000_0000;
        assert!(build_simd_tree(&one, SimdEncoding::Tagged).is_err());
        assert!(build_simd_tree(&one, SimdEncoding::Flagged).is_ok());
        let mut bad = grid(1);
        bad[0].aabb[0][0] = f32::NAN;
        assert!(build_simd_tree(&bad, SimdEncoding::Flagged).is_err());
        let mut inverted = grid(1);
        inverted[0].aabb = [[1.0; 3], [0.0; 3]];
        assert!(build_simd_tree(&inverted, SimdEncoding::Flagged).is_err());
    }
}
