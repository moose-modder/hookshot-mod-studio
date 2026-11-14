//! The four-lane bounding-volume tree (`hkcdSimdTree`) of a compressed mesh: `hknpCompressedMeshShapeData.simdTree`.
//!
//! A node holds four child boxes (`lx hx ly hy lz hz`, one lane each) and four data words. Node 0 is a sentinel whose
//! lanes are all empty (an empty lane's box has its minimum above its maximum: `+FLT_MAX` and `-FLT_MAX`), the root is
//! node 1, and a child always has a higher index than its parent. A lane is empty, holds a child node, or is a leaf
//! naming one primitive by the shape key of its first half (a triangle's, or the first triangle of a quad). Sekiro's
//! nodes have no flags and tag each lane's data word (`0` empty, odd `key << 1 | 1` a leaf, even `node << 1` a child);
//! Elden Ring's nodes carry an `isLeaf` flag for all four lanes, and a lane's data is the key or the node index as
//! it is (`u32::MAX` empty in a leaf node, `0` empty in the others).
//!
//! A tree can have no leaf: a mesh that has no primitive to name, or one the tree was not built for (two nodes, all lanes
//! empty; in Elden Ring the meshes whose `hasSimdTree` is false). Every other tree has one leaf for each triangle, quad
//! and custom primitive of the mesh and none for a dead one.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use crate::bvtree::excess_ulps_in;
use crate::{Result, Value, invalid};

/// A box, minimum then maximum (xyz), as the lanes store it.
pub type LaneBox = [[f32; 3]; 2];

/// What one lane of a node holds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SimdLane {
    /// No child: the lane's box is empty.
    Empty,
    /// A child node (always one with a higher index than this node's), whose lanes' boxes this box encloses.
    Child {
        /// The child's node index.
        node: usize,
        /// The box of everything below the child.
        aabb: LaneBox,
    },
    /// A primitive.
    Leaf {
        /// The shape key of the primitive's first half: section, primitive and a zero quad bit, as
        /// [`Triangle::key`](crate::Triangle::key) has it.
        key: u32,
        /// The box of the primitive's vertices.
        aabb: LaneBox,
    },
}

/// One node: four lanes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SimdNode {
    /// The lanes, in the order the file stores them.
    pub lanes: [SimdLane; 4],
}

/// A decoded simd tree.
#[derive(Debug, Clone, PartialEq)]
pub struct SimdTree {
    /// The nodes in file order; node 0 is the empty sentinel and node 1 the root. Empty when the file stores none.
    pub nodes: Vec<SimdNode>,
    /// Elden Ring's `hasSimdTree` (`None` in Sekiro's meshes, which have no such field).
    pub has_simd_tree: Option<bool>,
}

impl SimdTree {
    /// The leaves in file order: each one's shape key and box.
    pub fn leaves(&self) -> impl Iterator<Item = (u32, LaneBox)> + '_ {
        self.nodes.iter().flat_map(|n| &n.lanes).filter_map(|l| match *l {
            SimdLane::Leaf { key, aabb } => Some((key, aabb)),
            _ => None,
        })
    }
}

/// Whether a lane's box is empty: its minimum is above its maximum on an axis.
fn empty_box(b: &LaneBox) -> bool {
    b[0].iter().zip(&b[1]).any(|(low, high)| low > high)
}

/// Decode and check the simd tree of an hknpCompressedMeshShapeData.
///
/// Refused: a node array whose first node is not the empty sentinel, or that has no node past it; a lane whose box is
/// empty exactly when its data says it holds nothing; a child lane that names a node that is not above its own node
/// or past the nodes; a node that is not the child of exactly one lane (the nodes from 1 on form one tree rooted at 1).
/// All hold for the simd trees of the 3,344 + 22,899 compressed meshes of the Sekiro and Elden Ring collision.
pub fn simd_tree(data: Value<'_, '_>) -> Result<SimdTree> {
    let nodes = data.field("simdTree")?.field("nodes")?.array()?;
    let has_simd_tree = if data.has_field("hasSimdTree") {
        Some(data.field("hasSimdTree")?.unsigned()? != 0)
    } else {
        None
    };
    let mut out = Vec::with_capacity(nodes.len());
    for i in 0..nodes.len() {
        let node = nodes.get(i)?;
        let vector = |name: &str| node.field(name)?.vector::<4>();
        let (lx, hx, ly, hy, lz, hz) = (
            vector("lx")?,
            vector("hx")?,
            vector("ly")?,
            vector("hy")?,
            vector("lz")?,
            vector("hz")?,
        );
        let words = node.field("data")?.array()?;
        let flagged = node.has_field("isLeaf");
        let is_leaf = if flagged {
            Some(node.field("isLeaf")?.unsigned()? != 0)
        } else {
            None
        };
        let mut lanes = [SimdLane::Empty; 4];
        for (lane, slot) in lanes.iter_mut().enumerate() {
            let word = u32::try_from(words.get(lane)?.unsigned()?)
                .map_err(|_| invalid(node.at(), "a simd tree data word wider than 32 bits"))?;
            let pick = |v: [f32; 4]| v.get(lane).copied().unwrap_or(f32::NAN);
            let aabb: LaneBox = [[pick(lx), pick(ly), pick(lz)], [pick(hx), pick(hy), pick(hz)]];
            // (a child's node index, a leaf's key), or neither for an empty lane
            let (child, leaf) = match is_leaf {
                Some(true) => (None, (word != u32::MAX).then_some(word)),
                Some(false) => ((word != 0).then_some(word), None),
                None if word == 0 => (None, None),
                None if word & 1 == 1 => (None, Some(word >> 1)),
                None => (Some(word >> 1), None),
            };
            let empty_data = child.is_none() && leaf.is_none();
            if empty_box(&aabb) != empty_data {
                return Err(invalid(
                    node.at(),
                    format!(
                        "simd node {i} lane {lane}: its box is {} but its data word {word:#x} says {}",
                        if empty_data { "not empty" } else { "empty" },
                        if empty_data { "empty" } else { "a child or leaf" },
                    ),
                ));
            }
            *slot = match (child, leaf) {
                (Some(c), _) => {
                    let node_index = usize::try_from(c)
                        .ok()
                        .filter(|&c| c > i && c < nodes.len())
                        .ok_or_else(|| {
                            invalid(
                                node.at(),
                                format!(
                                    "simd node {i} lane {lane}: its child {c} is not a later node of the {}",
                                    nodes.len()
                                ),
                            )
                        })?;
                    SimdLane::Child {
                        node: node_index,
                        aabb,
                    }
                }
                (None, Some(key)) => SimdLane::Leaf { key, aabb },
                (None, None) => SimdLane::Empty,
            };
        }
        out.push(SimdNode { lanes });
    }
    let tree = SimdTree {
        nodes: out,
        has_simd_tree,
    };
    check_shape(&tree, data)?;
    Ok(tree)
}

/// The sentinel and the tree: node 0 empty, nodes 1 on each the child of exactly one lane.
fn check_shape(tree: &SimdTree, data: Value<'_, '_>) -> Result<()> {
    let [sentinel, _root, ..] = tree.nodes.as_slice() else {
        return if tree.nodes.is_empty() {
            Ok(())
        } else {
            Err(invalid(data.at(), "a simd tree of the sentinel and no root"))
        };
    };
    if sentinel.lanes.iter().any(|l| *l != SimdLane::Empty) {
        return Err(invalid(data.at(), "simd node 0 is not the empty sentinel"));
    }
    // a child index is above its node's, so counting each node's parents finds the nodes reached twice or not at all
    let mut parents = vec![0usize; tree.nodes.len()];
    for node in &tree.nodes {
        for lane in &node.lanes {
            if let SimdLane::Child { node, .. } = lane
                && let Some(count) = parents.get_mut(*node)
            {
                *count = count.saturating_add(1);
            }
        }
    }
    for (i, count) in parents.iter().enumerate().skip(1) {
        // the root, node 1, is no lane's child
        let want = usize::from(i != 1);
        if *count != want {
            return Err(invalid(
                data.at(),
                format!("simd node {i} is the child of {count} lanes, not {want}"),
            ));
        }
    }
    Ok(())
}

/// What testing a simd tree against the primitives of its mesh found. The coverage is a refusal; the boxes are counted.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct SimdCheck {
    /// Simd trees decoded.
    pub trees: usize,
    /// Of those, trees with no leaf: meshes with no primitive to name, and meshes whose tree was not built (Elden Ring's
    /// with `hasSimdTree` false, and Sekiro's equivalent: two nodes, all lanes empty).
    pub leafless: usize,
    /// Nodes of all the trees, sentinels included.
    pub nodes: usize,
    /// Leaves: one per triangle, quad and custom primitive of each tree that has leaves.
    pub leaves: usize,
    /// Leaf boxes that are exactly the box of the primitive's vertices.
    pub leaves_exact: usize,
    /// Leaf boxes that contain the primitive's vertices and are larger than their box.
    pub leaves_looser: usize,
    /// Leaf boxes that leave a vertex out by at most [`BOUNDS_TOLERANCE_ULPS`](crate::BOUNDS_TOLERANCE_ULPS).
    pub leaves_rounding: usize,
    /// Leaf boxes that leave a vertex out by more: a finding, not a refusal.
    pub leaves_outside: usize,
    /// Child lanes whose box is exactly the union of the boxes of the child's lanes.
    pub children_exact: usize,
    /// Child lanes whose box is larger than that union.
    pub children_looser: usize,
    /// Child lanes whose box leaves part of that union out.
    pub children_short: usize,
}

impl SimdCheck {
    /// Add `other`'s counts to these.
    pub fn merge(&mut self, other: &Self) {
        for (total, add) in [
            (&mut self.trees, other.trees),
            (&mut self.leafless, other.leafless),
            (&mut self.nodes, other.nodes),
            (&mut self.leaves, other.leaves),
            (&mut self.leaves_exact, other.leaves_exact),
            (&mut self.leaves_looser, other.leaves_looser),
            (&mut self.leaves_rounding, other.leaves_rounding),
            (&mut self.leaves_outside, other.leaves_outside),
            (&mut self.children_exact, other.children_exact),
            (&mut self.children_looser, other.children_looser),
            (&mut self.children_short, other.children_short),
        ] {
            *total = total.saturating_add(add);
        }
    }
}

/// A primitive of the mesh a simd tree must name: the shape key of its first half and, for a triangle or quad, the box
/// of its vertices with the domain of its section.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LivePrimitive {
    pub key: u32,
    /// A custom primitive has no vertices to compare.
    pub vertices: Option<(LaneBox, [[f64; 3]; 2])>,
}

/// Test `tree` against the mesh's live primitives: unless it has no leaf its leaves name each of them once and no
/// other; the box counts go to the returned [`SimdCheck`].
pub(crate) fn check_against(
    tree: &SimdTree,
    live: &mut [LivePrimitive],
    data: Value<'_, '_>,
) -> Result<SimdCheck> {
    let mut check = SimdCheck {
        trees: 1,
        nodes: tree.nodes.len(),
        ..SimdCheck::default()
    };
    let mut keys: Vec<(u32, LaneBox)> = tree.leaves().collect();
    if keys.is_empty() {
        // ... in Elden Ring, for a mesh that has primitives, exactly those whose flag says there is none
        if tree.has_simd_tree == Some(true) && !live.is_empty() {
            return Err(invalid(
                data.at(),
                "hasSimdTree is set, the mesh has primitives and the simd tree has no leaf",
            ));
        }
        check.leafless = 1;
        return Ok(check);
    }
    if tree.has_simd_tree == Some(false) {
        return Err(invalid(
            data.at(),
            "hasSimdTree is clear and the simd tree has leaves",
        ));
    }
    keys.sort_by_key(|(key, _)| *key);
    live.sort_by_key(|p| p.key);
    if keys.len() != live.len() {
        return Err(invalid(
            data.at(),
            format!(
                "the simd tree has {} leaves for the mesh's {} triangles, quads and custom primitives",
                keys.len(),
                live.len()
            ),
        ));
    }
    for ((key, lane), primitive) in keys.iter().zip(live.iter()) {
        if *key != primitive.key {
            return Err(invalid(
                data.at(),
                format!(
                    "the simd tree names shape key {key} where the mesh's next primitive is {}",
                    primitive.key
                ),
            ));
        }
        check.leaves = check.leaves.saturating_add(1);
        let Some((vertices, domain)) = primitive.vertices else {
            continue;
        };
        let as_f64 = [lane[0].map(f64::from), lane[1].map(f64::from)];
        // the primitive's vertex box has a vertex on each of its two corners' axes: its corners decide
        let worst = vertices
            .iter()
            .map(|corner| excess_ulps_in(&as_f64, &domain, *corner))
            .fold(0.0f64, f64::max);
        let slot = if worst > crate::BOUNDS_TOLERANCE_ULPS {
            &mut check.leaves_outside
        } else if worst > 0.0 {
            &mut check.leaves_rounding
        } else if vertices == *lane {
            &mut check.leaves_exact
        } else {
            &mut check.leaves_looser
        };
        *slot = slot.saturating_add(1);
    }
    // each child lane against the union of the lanes below it
    for node in &tree.nodes {
        for lane in &node.lanes {
            let SimdLane::Child { node: child, aabb } = lane else {
                continue;
            };
            let Some(below) = tree.nodes.get(*child) else {
                continue;
            };
            let mut union: Option<LaneBox> = None;
            for inner in &below.lanes {
                let (SimdLane::Child { aabb: b, .. } | SimdLane::Leaf { aabb: b, .. }) = inner else {
                    continue;
                };
                union = Some(match union {
                    None => *b,
                    Some(u) => [
                        [u[0][0].min(b[0][0]), u[0][1].min(b[0][1]), u[0][2].min(b[0][2])],
                        [u[1][0].max(b[1][0]), u[1][1].max(b[1][1]), u[1][2].max(b[1][2])],
                    ],
                });
            }
            let Some(union) = union else {
                continue;
            };
            let slot = if *aabb == union {
                &mut check.children_exact
            } else if (0..3).all(|a| aabb[0].get(a) <= union[0].get(a) && aabb[1].get(a) >= union[1].get(a)) {
                &mut check.children_looser
            } else {
                &mut check.children_short
            };
            *slot = slot.saturating_add(1);
        }
    }
    Ok(check)
}
