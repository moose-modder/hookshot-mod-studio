//! The bounding-volume trees of a compressed mesh: the *mesh tree* (5-byte nodes; its leaves are the sections) and each
//! section's *section tree* (4-byte nodes; its leaves are the section's primitives), decoded and checked.
//!
//! A tree is stored as a pre-order array of nodes, so every subtree is a contiguous run of nodes: the first child of an
//! internal node is the next node, its second child starts after the first child's subtree, and a leaf is the last node
//! of its subtree. A node holds a nibble pair per axis (the high nibble shrinks the box from its minimum side, the low
//! nibble from its maximum side, each by `extent × nibble² / 226` of the parent's extent) and a data word that names
//! either the second child or the leaf's payload. The root's box is the tree's `domain` shrunk by its own nibbles, which
//! are zero in all but a few percent of the Sekiro and Elden Ring trees.
//!
//! What the decoders refuse, and what they count instead, is documented on [`BvTree::decode`], [`mesh_trees`] and
//! [`BoundsCheck`].
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use crate::mesh::{MeshSection, aabb, mesh_sections};
use crate::{Result, Value, invalid};

/// Divisor of the nibble squares: a nibble `n` takes `n² / 226` of the parent's extent from its side.
const NIBBLE_DIVISOR: f64 = 226.0;

/// How a tree stores its nodes, which is what its leaves name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeWidth {
    /// Four bytes: `x y z` nibble pairs and one data byte. Bit 0 set is an internal node whose second child is
    /// `data & 0xFE` nodes on; clear is a leaf for primitive `data >> 1`. The section trees.
    Four,
    /// Five bytes: `x y z` nibble pairs, then `hiData` and `loData` (`value = hiData << 8 | loData`). Bit 15 set is an
    /// internal node whose second child is `2 × (value & 0x7FFF)` nodes on; clear is a leaf for section `value`. The
    /// mesh trees.
    Five,
    /// Six bytes: `x y z` nibble pairs, `hiData` and a 16-bit `loData` (`value = (hiData & 0x7F) << 16 | loData`). Bit 7
    /// of `hiData` set is an internal node whose second child is `2 × value` nodes on; clear is a leaf for primitive
    /// `value`. The aabb trees of extern meshes (`hknpExternMeshShapeData.aabbTree`, `Codec3Axis6`).
    Six,
}

impl NodeWidth {
    /// The bytes of one node.
    pub const fn bytes(self) -> usize {
        match self {
            Self::Four => 4,
            Self::Five => 5,
            Self::Six => 6,
        }
    }
}

/// What a node is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BvNodeKind {
    /// The first child is the next node; the second is at this index.
    Internal {
        /// Index of the second child.
        second_child: usize,
    },
    /// A leaf: a primitive of the section (section trees) or a section of the mesh (mesh trees).
    Leaf {
        /// The primitive or section index.
        payload: u32,
    },
}

/// One node of a decoded tree.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BvNode {
    /// The node's three stored axis bytes: per axis the high nibble (which shrinks the box from its minimum side) and the
    /// low nibble (from its maximum side).
    pub axes: [u8; 3],
    /// The node's box, minimum then maximum (xyz), computed in double precision from the `f32` domain.
    pub aabb: [[f64; 3]; 2],
    /// Internal (with its second child) or leaf (with its payload).
    pub kind: BvNodeKind,
}

/// A decoded bounding-volume tree: its nodes in file (pre-order) order.
#[derive(Debug, Clone, PartialEq)]
pub struct BvTree {
    nodes: Vec<BvNode>,
    /// The box the root decodes from, in double precision.
    domain: [[f64; 3]; 2],
    /// File offset of the node array, for refusals.
    at: usize,
}

impl BvTree {
    /// Decode a tree stored as `bytes` (whole nodes of `width`) under the root box `domain` (xyz minimum then maximum;
    /// `at` is where the nodes sit in the file, for refusals). No nodes is an empty tree.
    ///
    /// Refused: a domain that is not finite and ordered; bytes that are not a whole number of nodes; a second child
    /// that is not a later node inside its parent's subtree (an offset below 2, or at or past the end of the subtree);
    /// a leaf that does not end its subtree, which would leave the nodes after it unreached (or reached twice). With
    /// those rules the subtrees partition the array, so every node is reached exactly once.
    pub fn decode(bytes: &[u8], width: NodeWidth, domain: [[f32; 3]; 2], at: usize) -> Result<Self> {
        let stride = width.bytes();
        let node_at = |i: usize| at.saturating_add(i.saturating_mul(stride));
        if bytes.len().checked_rem(stride) != Some(0) {
            return Err(invalid(
                at,
                format!("{} bytes of {stride}-byte nodes", bytes.len()),
            ));
        }
        let count = bytes.len().checked_div(stride).unwrap_or(0);
        if count == 0 {
            return Ok(Self::empty(at));
        }
        let root = domain.map(|corner| corner.map(f64::from));
        if root.iter().flatten().any(|c| !c.is_finite())
            || root[0].iter().zip(&root[1]).any(|(low, high)| low > high)
        {
            return Err(invalid(
                at,
                format!("the root box cannot decode from the domain {domain:?}"),
            ));
        }
        let placeholder = BvNode {
            axes: [0; 3],
            aabb: root,
            kind: BvNodeKind::Leaf { payload: 0 },
        };
        let mut nodes = vec![placeholder; count];
        // (node, end of its subtree, the parent's box); the second child is pushed first so the first pops first
        let mut stack = vec![(0usize, count, root)];
        let mut reached = 0usize;
        while let Some((i, end, parent)) = stack.pop() {
            let raw = bytes
                .get(i.saturating_mul(stride)..i.saturating_add(1).saturating_mul(stride))
                .filter(|_| i < end)
                .ok_or_else(|| {
                    invalid(
                        node_at(i),
                        format!("node {i} lies outside its subtree, which ends at {end}"),
                    )
                })?;
            let (nibbles, data) = raw.split_at(3);
            let axes = [nibbles.first(), nibbles.get(1), nibbles.get(2)].map(|b| b.copied().unwrap_or(0));
            let [mut min, mut max] = parent;
            for ((&pair, low), high) in nibbles.iter().zip(&mut min).zip(&mut max) {
                let extent = *high - *low;
                let (hi, lo) = (f64::from(pair >> 4), f64::from(pair & 15));
                *low += extent * hi * hi / NIBBLE_DIVISOR;
                *high -= extent * lo * lo / NIBBLE_DIVISOR;
            }
            let aabb = [min, max];
            let (internal, offset, payload) = match (width, data) {
                (NodeWidth::Four, &[d]) => (d & 1 == 1, usize::from(d & 0xfe), u32::from(d >> 1)),
                (NodeWidth::Five, &[hi, lo]) => {
                    let value = u16::from(hi) << 8 | u16::from(lo);
                    (
                        value & 0x8000 != 0,
                        usize::from(value & 0x7fff).saturating_mul(2),
                        u32::from(value & 0x7fff),
                    )
                }
                (NodeWidth::Six, &[hi, low, high]) => {
                    // `loData` is a 16-bit little-endian word
                    let value = u32::from(hi & 0x7f) << 16 | u32::from(high) << 8 | u32::from(low);
                    (
                        hi & 0x80 != 0,
                        usize::try_from(value).unwrap_or(usize::MAX).saturating_mul(2),
                        value,
                    )
                }
                _ => return Err(invalid(node_at(i), "a node of the wrong width")),
            };
            reached = reached.saturating_add(1);
            let kind = if internal {
                let second = i.saturating_add(offset);
                if offset < 2 || second >= end {
                    return Err(invalid(
                        node_at(i),
                        format!(
                            "node {i}: its second child at {second} is not inside its subtree, which ends at {end}"
                        ),
                    ));
                }
                stack.push((second, end, aabb));
                stack.push((i.saturating_add(1), second, aabb));
                BvNodeKind::Internal { second_child: second }
            } else {
                if i.saturating_add(1) != end {
                    return Err(invalid(
                        node_at(i),
                        format!(
                            "leaf {i} does not end its subtree, which ends at {end}: the nodes after it are not reached once"
                        ),
                    ));
                }
                BvNodeKind::Leaf { payload }
            };
            if let Some(slot) = nodes.get_mut(i) {
                *slot = BvNode { axes, aabb, kind };
            }
        }
        if reached != count {
            return Err(invalid(at, format!("{reached} of {count} nodes reached")));
        }
        Ok(Self {
            nodes,
            domain: root,
            at,
        })
    }

    fn empty(at: usize) -> Self {
        Self {
            nodes: Vec::new(),
            domain: [[0.0; 3]; 2],
            at,
        }
    }

    /// How far `point` lies outside the box of node `node`, in `f32` units in the last place; zero for a point inside,
    /// `None` for a node the tree lacks. On each axis the distance beyond the box is over the ULP of the largest
    /// magnitude among the coordinate, the box's two bounds and the tree's domain on that axis, and the worst axis
    /// counts. The domain is in the magnitude because the vertices a leaf holds were computed from it (a section's
    /// packed vertex is its domain's minimum plus a code times a scale, in `f32`), so a vertex near zero on an axis whose
    /// domain reaches further carries the rounding of the domain's magnitude, not of its own.
    pub fn excess_ulps(&self, node: usize, point: [f32; 3]) -> Option<f64> {
        Some(excess_ulps_in(&self.nodes.get(node)?.aabb, &self.domain, point))
    }

    /// The nodes, in file order: node `i + 1` follows node `i` as its first child when `i` is internal.
    pub fn nodes(&self) -> &[BvNode] {
        &self.nodes
    }

    /// The leaves in file order: each one's node index and payload.
    pub fn leaves(&self) -> impl Iterator<Item = (usize, u32)> + '_ {
        self.nodes.iter().enumerate().filter_map(|(i, n)| match n.kind {
            BvNodeKind::Leaf { payload } => Some((i, payload)),
            BvNodeKind::Internal { .. } => None,
        })
    }

    /// The node index of the leaf for each payload `0..count`, requiring the leaves to cover them exactly once: a
    /// payload from `count` on, a payload two leaves name, or a payload no leaf names is refused.
    pub fn leaf_nodes(&self, count: usize) -> Result<Vec<usize>> {
        let mut found: Vec<Option<usize>> = vec![None; count];
        for (node, payload) in self.leaves() {
            let at = self.at.saturating_add(node);
            let slot = usize::try_from(payload)
                .ok()
                .and_then(|p| found.get_mut(p))
                .ok_or_else(|| {
                    invalid(
                        at,
                        format!("leaf {node} names {payload}, outside the {count} it must cover"),
                    )
                })?;
            if let Some(first) = slot.replace(node) {
                return Err(invalid(
                    at,
                    format!("leaves {first} and {node} both name {payload}"),
                ));
            }
        }
        found
            .into_iter()
            .enumerate()
            .map(|(payload, node)| {
                node.ok_or_else(|| invalid(self.at, format!("no leaf names {payload} of {count}")))
            })
            .collect()
    }
}

/// How far `point` lies outside `aabb`, in `f32` ULPs at the largest magnitude among the coordinate, the box's bounds and
/// the `domain`'s on each axis; the worst axis. Zero for a point inside.
pub(crate) fn excess_ulps_in(aabb: &[[f64; 3]; 2], domain: &[[f64; 3]; 2], point: [f32; 3]) -> f64 {
    let axes = point
        .into_iter()
        .zip(aabb[0])
        .zip(aabb[1])
        .zip(domain[0])
        .zip(domain[1]);
    let mut worst = 0.0f64;
    for ((((p, lo), hi), domain_lo), domain_hi) in axes {
        let x = f64::from(p);
        let excess = (lo - x).max(x - hi).max(0.0);
        if excess > 0.0 {
            let magnitude = [x, lo, hi, domain_lo, domain_hi]
                .into_iter()
                .fold(1e-30f64, |m, v| m.max(v.abs()));
            // the power of two at or below the magnitude: its exponent field alone
            let power = f64::from_bits(magnitude.to_bits() & (0x7ff << 52));
            worst = worst.max(excess / (f64::from(f32::EPSILON) * power));
        }
    }
    worst
}

/// A section's decoded tree and where its primitives' leaves are.
#[derive(Debug, Clone, PartialEq)]
pub struct SectionTree {
    /// The tree, rooted at the section's `domain`.
    pub tree: BvTree,
    /// The node index of each primitive's leaf, indexed by the primitive's index within the section.
    pub leaves: Vec<usize>,
}

impl SectionTree {
    /// The node of primitive `primitive` of the section (its leaf), whose box contains the primitive's vertices.
    pub fn leaf(&self, primitive: usize) -> Option<&BvNode> {
        self.leaves.get(primitive).and_then(|&n| self.tree.nodes.get(n))
    }

    /// How far `point` lies outside the box of primitive `primitive`'s leaf ([`BvTree::excess_ulps`]).
    pub fn excess_ulps(&self, primitive: usize, point: [f32; 3]) -> Option<f64> {
        self.tree.excess_ulps(*self.leaves.get(primitive)?, point)
    }
}

/// The decoded trees of a compressed mesh.
#[derive(Debug, Clone, PartialEq)]
pub struct MeshTrees {
    /// The mesh tree, rooted at the tree's `domain`.
    pub mesh: BvTree,
    /// The node index of each section's leaf in [`mesh`](Self::mesh), indexed by section; each equals the section's
    /// stored `leafIndex`.
    pub mesh_leaves: Vec<usize>,
    /// One tree per section.
    pub sections: Vec<SectionTree>,
}

/// What decoding and testing the trees of compressed meshes found. Everything but the vertex test is a refusal; the
/// vertex test is counted, because it compares float vertices with float boxes and a few stored triangles sit a hair
/// outside theirs (see [`BoundsCheck::outside`]).
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct BoundsCheck {
    /// Mesh trees decoded.
    pub mesh_trees: usize,
    /// Section trees decoded.
    pub section_trees: usize,
    /// Nodes of all of them.
    pub nodes: usize,
    /// Leaves of the mesh trees: one per section.
    pub mesh_leaves: usize,
    /// Leaves of the section trees: one per primitive, dead and custom ones included.
    pub section_leaves: usize,
    /// Nodes whose box is inverted on an axis (minimum above maximum): not a refusal. In the collision of both
    /// installs the nibbles of such a node take more than the whole extent of a box that is already thin, by under a
    /// tenth of a millimetre (73 micrometres at most).
    pub inverted_boxes: usize,
    /// Triangle and quad primitives whose vertices were compared with the box of their leaf.
    pub primitives: usize,
    /// Of those, with every vertex inside the box.
    pub inside: usize,
    /// Of those, with a vertex outside by at most [`BOUNDS_TOLERANCE_ULPS`].
    pub rounding: usize,
    /// Of those, with a vertex outside by more than [`BOUNDS_TOLERANCE_ULPS`]: a finding, not a refusal (a tree that
    /// leaves a vertex out is wrong for a query, but the triangles read the same). None in the collision of either
    /// install.
    pub outside: usize,
    /// The largest [`BvTree::excess_ulps`] over those primitives.
    pub worst_ulps: f64,
}

/// A vertex this many `f32` ULPs outside its leaf's box, or fewer, is float rounding: [`BoundsCheck::rounding`].
pub const BOUNDS_TOLERANCE_ULPS: f64 = 16.0;

impl BoundsCheck {
    /// Count one primitive's worst vertex excess ([`BvTree::excess_ulps`]).
    pub(crate) fn record(&mut self, excess_ulps: f64) {
        self.primitives = self.primitives.saturating_add(1);
        let slot = if excess_ulps == 0.0 {
            &mut self.inside
        } else if excess_ulps <= BOUNDS_TOLERANCE_ULPS {
            &mut self.rounding
        } else {
            &mut self.outside
        };
        *slot = slot.saturating_add(1);
        self.worst_ulps = self.worst_ulps.max(excess_ulps);
    }

    /// Add `other`'s counts to these.
    pub fn merge(&mut self, other: &Self) {
        for (total, add) in [
            (&mut self.mesh_trees, other.mesh_trees),
            (&mut self.section_trees, other.section_trees),
            (&mut self.nodes, other.nodes),
            (&mut self.mesh_leaves, other.mesh_leaves),
            (&mut self.section_leaves, other.section_leaves),
            (&mut self.inverted_boxes, other.inverted_boxes),
            (&mut self.primitives, other.primitives),
            (&mut self.inside, other.inside),
            (&mut self.rounding, other.rounding),
            (&mut self.outside, other.outside),
        ] {
            *total = total.saturating_add(add);
        }
        self.worst_ulps = self.worst_ulps.max(other.worst_ulps);
    }

    /// The counts of the trees alone, from the trees themselves (the vertex test fills the rest).
    pub(crate) fn of_trees(trees: &MeshTrees) -> Self {
        let section_nodes: usize = trees.sections.iter().map(|s| s.tree.nodes.len()).sum();
        let inverted = trees
            .sections
            .iter()
            .map(|s| &s.tree)
            .chain([&trees.mesh])
            .flat_map(|t| &t.nodes)
            .filter(|n| n.aabb[0].iter().zip(&n.aabb[1]).any(|(low, high)| low > high))
            .count();
        Self {
            inverted_boxes: inverted,
            mesh_trees: 1,
            section_trees: trees.sections.len(),
            nodes: trees.mesh.nodes.len().saturating_add(section_nodes),
            mesh_leaves: trees.mesh_leaves.len(),
            section_leaves: trees.sections.iter().map(|s| s.leaves.len()).sum(),
            ..Self::default()
        }
    }
}

/// The tree object `tree` (a mesh tree, or one of its sections: each is `nodes` and a `domain`) at `width`.
fn tree_of(tree: Value<'_, '_>, width: NodeWidth) -> Result<BvTree> {
    let nodes = tree.field("nodes")?;
    let stored = nodes.element_size()?;
    if stored != width.bytes() {
        return Err(invalid(
            nodes.at(),
            format!(
                "the tree's nodes are {stored} bytes wide, not the {} this reader decodes here",
                width.bytes()
            ),
        ));
    }
    let bytes = nodes.array()?.element_bytes()?;
    BvTree::decode(bytes, width, aabb(tree.field("domain")?)?, nodes.at())
}

/// The mesh tree and every section tree of an hknpCompressedMeshShapeData, given its [`mesh_sections`].
pub(crate) fn decode_trees(data: Value<'_, '_>, ranges: &[MeshSection]) -> Result<MeshTrees> {
    let tree = data.field("meshTree")?;
    let mesh = tree_of(tree, NodeWidth::Five)?;
    let mesh_leaves = mesh.leaf_nodes(ranges.len())?;
    let sections = tree.field("sections")?.array()?;
    let mut trees = Vec::with_capacity(ranges.len());
    for (i, (range, &leaf)) in ranges.iter().zip(&mesh_leaves).enumerate() {
        let section = sections.get(i)?;
        if range.leaf_index != leaf {
            return Err(invalid(
                section.at(),
                format!(
                    "section {i}: its leafIndex is {}, its leaf in the mesh tree is node {leaf}",
                    range.leaf_index
                ),
            ));
        }
        let tree = tree_of(section, NodeWidth::Four)?;
        let leaves = tree
            .leaf_nodes(range.primitives)
            .map_err(|e| invalid(e.offset, format!("section {i}: {}", e.detail)))?;
        trees.push(SectionTree { tree, leaves });
    }
    Ok(MeshTrees {
        mesh,
        mesh_leaves,
        sections: trees,
    })
}

/// Decode and check the bounding-volume trees of an hknpCompressedMeshShapeData: its mesh tree (5-byte nodes, rooted at
/// the tree's `domain`) and each section's tree (4-byte nodes, rooted at the section's `domain`).
///
/// On top of [`BvTree::decode`]'s refusals: a node width other than 5 (mesh) or 4 (section); mesh-tree leaves that do not
/// name each section once; a section whose `leafIndex` is not its leaf's node in the mesh tree; section-tree leaves that
/// do not name each primitive of the section once. All hold for the 3,344 + 22,897 mesh trees and 273,228 + 722,403
/// section trees of the Sekiro and Elden Ring collision. [`compressed_mesh`](crate::compressed_mesh) runs this for
/// every mesh it reads and tests each triangle and quad against its leaf's box ([`BoundsCheck`]).
pub fn mesh_trees(data: Value<'_, '_>) -> Result<MeshTrees> {
    decode_trees(data, &mesh_sections(data)?)
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

    /// A box whose extent is 226 on every axis, so a nibble `n` moves a side by exactly `n²`.
    const DOMAIN: [[f32; 3]; 2] = [[0.0; 3], [226.0; 3]];

    fn four(nodes: &[([u8; 3], u8)]) -> Vec<u8> {
        nodes.iter().flat_map(|&([x, y, z], d)| [x, y, z, d]).collect()
    }

    fn five(nodes: &[([u8; 3], u16)]) -> Vec<u8> {
        nodes
            .iter()
            .flat_map(|&([x, y, z], v)| {
                let [hi, lo] = v.to_be_bytes();
                [x, y, z, hi, lo]
            })
            .collect()
    }

    fn refusal(bytes: &[u8], width: NodeWidth, domain: [[f32; 3]; 2]) -> String {
        BvTree::decode(bytes, width, domain, 0x100).unwrap_err().detail
    }

    #[test]
    fn a_four_byte_tree_decodes_its_boxes_and_payloads() {
        // root: x nibbles 2 (high) and 1 (low); first child a leaf for primitive 0 with y nibbles 3 and 0; second a
        // leaf for primitive 1 with z nibbles 0 and 4
        let bytes = four(&[([0x21, 0, 0], 0b11), ([0, 0x30, 0], 0), ([0, 0, 0x04], 1 << 1)]);
        let tree = BvTree::decode(&bytes, NodeWidth::Four, DOMAIN, 0x100).unwrap();
        let [root, first, second] = tree.nodes() else {
            panic!("three nodes");
        };
        assert_eq!(root.kind, BvNodeKind::Internal { second_child: 2 });
        assert_eq!(root.axes, [0x21, 0, 0]);
        assert_eq!(root.aabb, [[4.0, 0.0, 0.0], [225.0, 226.0, 226.0]]);
        assert_eq!(first.kind, BvNodeKind::Leaf { payload: 0 });
        // the root's box shrunk by the first child's own nibbles: y by 3² on the minimum side
        assert_eq!(first.aabb, [[4.0, 9.0, 0.0], [225.0, 226.0, 226.0]]);
        assert_eq!(second.kind, BvNodeKind::Leaf { payload: 1 });
        assert_eq!(second.aabb, [[4.0, 0.0, 0.0], [225.0, 226.0, 226.0 - 16.0]]);
        assert_eq!(tree.leaf_nodes(2).unwrap(), [1, 2]);
        assert_eq!(tree.leaves().collect::<Vec<_>>(), [(1, 0), (2, 1)]);
    }

    #[test]
    fn a_five_byte_tree_puts_the_second_child_twice_the_value_on() {
        // root (second child at 0 + 2 × 2 = 4), whose first subtree is nodes 1 to 3: an internal node (second child
        // at 1 + 2 × 1 = 3) over leaves 2 and 3; then leaf 4. A reader that took the offset as `i + 1 + value` would
        // put the root's second child at 3.
        let bytes = five(&[
            ([0; 3], 0x8002),
            ([0; 3], 0x8001),
            ([0; 3], 0),
            ([0; 3], 1),
            ([0; 3], 2),
        ]);
        let tree = BvTree::decode(&bytes, NodeWidth::Five, DOMAIN, 0).unwrap();
        assert_eq!(tree.nodes()[0].kind, BvNodeKind::Internal { second_child: 4 });
        assert_eq!(tree.nodes()[1].kind, BvNodeKind::Internal { second_child: 3 });
        assert_eq!(tree.leaf_nodes(3).unwrap(), [2, 3, 4]);
    }

    #[test]
    fn a_single_leaf_and_an_empty_tree_are_trees() {
        let one = BvTree::decode(&four(&[([0; 3], 0)]), NodeWidth::Four, DOMAIN, 0).unwrap();
        assert_eq!(one.nodes().len(), 1);
        assert_eq!(one.nodes()[0].aabb, [[0.0; 3], [226.0; 3]]);
        assert_eq!(one.leaf_nodes(1).unwrap(), [0]);
        let empty = BvTree::decode(&[], NodeWidth::Five, DOMAIN, 0).unwrap();
        assert!(empty.nodes().is_empty());
        assert!(empty.leaf_nodes(0).unwrap().is_empty());
        assert!(empty.leaf_nodes(1).is_err());
    }

    #[test]
    fn a_tree_that_is_not_a_pre_order_partition_is_refused() {
        let leaf = ([0u8; 3], 0u8);
        // not a whole number of nodes
        assert!(refusal(&[0; 6], NodeWidth::Four, DOMAIN).contains("6 bytes of 4-byte nodes"));
        // a second child at offset 0 (the node itself) and at an offset past the subtree
        let e = refusal(&four(&[([0; 3], 1), leaf, leaf]), NodeWidth::Four, DOMAIN);
        assert!(e.contains("not inside its subtree"), "{e}");
        let e = refusal(&four(&[([0; 3], 7), leaf, leaf]), NodeWidth::Four, DOMAIN);
        assert!(e.contains("node 0") && e.contains("ends at 3"), "{e}");
        // the same in the 5-byte layout
        let e = refusal(
            &five(&[([0; 3], 0x8002), ([0; 3], 0), ([0; 3], 1)]),
            NodeWidth::Five,
            DOMAIN,
        );
        assert!(e.contains("not inside its subtree"), "{e}");
        // a leaf with nodes after it that nothing reaches
        let e = refusal(&four(&[leaf, leaf, leaf]), NodeWidth::Four, DOMAIN);
        assert!(e.contains("does not end its subtree"), "{e}");
        let e = refusal(&four(&[([0; 3], 3), leaf, leaf, leaf]), NodeWidth::Four, DOMAIN);
        assert!(e.contains("leaf 2 does not end its subtree"), "{e}");
        // a first child that is a leaf but whose subtree runs on to the second child's start
        let e = refusal(
            &four(&[([0; 3], 5), leaf, leaf, leaf, leaf]),
            NodeWidth::Four,
            DOMAIN,
        );
        assert!(e.contains("does not end its subtree"), "{e}");
    }

    #[test]
    fn a_domain_the_root_cannot_decode_from_is_refused() {
        let bytes = four(&[([0; 3], 0)]);
        for domain in [
            [[f32::NAN, 0.0, 0.0], [1.0; 3]],
            [[0.0; 3], [f32::INFINITY, 1.0, 1.0]],
            [[0.0, 2.0, 0.0], [1.0, 1.0, 1.0]],
        ] {
            let e = refusal(&bytes, NodeWidth::Four, domain);
            assert!(e.contains("cannot decode from the domain"), "{e}");
        }
        // a box with no extent on an axis is a domain
        assert!(BvTree::decode(&bytes, NodeWidth::Four, [[0.0; 3], [0.0, 1.0, 1.0]], 0).is_ok());
    }

    #[test]
    fn leaves_must_cover_the_payloads_exactly_once() {
        let tree = |payloads: &[u8]| {
            // a chain: internal nodes whose first child is a leaf, then the last leaf
            let mut nodes = Vec::new();
            for (k, &p) in payloads.iter().enumerate() {
                if k + 1 < payloads.len() {
                    nodes.push(([0u8; 3], 3));
                }
                nodes.push(([0u8; 3], p << 1));
            }
            BvTree::decode(&four(&nodes), NodeWidth::Four, DOMAIN, 0x40).unwrap()
        };
        assert_eq!(tree(&[2, 0, 1]).leaf_nodes(3).unwrap(), [3, 4, 1]);
        let e = tree(&[0, 0]).leaf_nodes(2).unwrap_err().detail;
        assert!(e.contains("both name 0"), "{e}");
        let e = tree(&[0, 2]).leaf_nodes(2).unwrap_err().detail;
        assert!(e.contains("names 2, outside the 2"), "{e}");
        let e = tree(&[0, 1]).leaf_nodes(3).unwrap_err().detail;
        assert!(e.contains("no leaf names 2 of 3"), "{e}");
    }

    #[test]
    fn nibbles_that_shrink_a_box_past_nothing_invert_it_without_a_refusal() {
        // both nibbles 15: 225/226 of the extent comes off each side
        let bytes = four(&[([0xff, 0, 0], 0)]);
        let tree = BvTree::decode(&bytes, NodeWidth::Four, DOMAIN, 0).unwrap();
        let [low, high] = tree.nodes()[0].aabb;
        assert!(low[0] > high[0]);
        assert_eq!((low[1], high[1]), (0.0, 226.0));
    }

    #[test]
    fn excess_is_counted_in_f32_ulps_at_the_largest_magnitude() {
        // one node: its box is the domain
        let domain = [[1.0, 0.0, -4.0], [2.0, 8.0, 4.0]];
        let tree = BvTree::decode(&four(&[([0; 3], 0)]), NodeWidth::Four, domain, 0).unwrap();
        let ulps = |p: [f32; 3]| tree.excess_ulps(0, p).unwrap();
        assert_eq!(ulps([2.0, 8.0, -4.0]), 0.0);
        // one f32 ULP above 2.0 is 2^-22
        assert_eq!(ulps([2.0 + f32::EPSILON * 2.0, 1.0, 0.0]), 1.0);
        // one ULP at 8 on the axis whose box reaches 8
        assert_eq!(ulps([1.5, 8.0 + f32::EPSILON * 8.0, 0.0]), 1.0);
        // the worst axis counts
        assert_eq!(
            ulps([2.0 + f32::EPSILON * 4.0, 8.0 + f32::EPSILON * 8.0, 0.0]),
            2.0
        );
        // below the minimum counts too, in ULPs at the box's largest bound on the axis (2.0): a quarter of one
        assert_eq!(ulps([1.0 - f32::EPSILON / 2.0, 1.0, 0.0]), 0.25);
        assert_eq!(tree.excess_ulps(1, [0.0; 3]), None);
    }

    #[test]
    fn the_domain_sets_the_magnitude_of_a_box_near_zero() {
        // a leaf whose z box ends at 2^-20 inside a domain z of [-1, 2^-20]: a vertex 2^-26 above the box is 2^-26 over
        // the ULP of 2^-20 (2^-43) at the box, but only 2^-26 / 2^-23 = 1/8 of one at the domain's magnitude of 1
        let z = 2f32.powi(-20);
        let domain = [[0.0, 0.0, -1.0], [1.0, 1.0, z]];
        // the first leaf's z nibbles are high 15, low 0: its maximum stays the domain's
        let bytes = four(&[([0; 3], 3), ([0, 0, 0xf0], 0), ([0; 3], 2)]);
        let tree = BvTree::decode(&bytes, NodeWidth::Four, domain, 0).unwrap();
        let leaf = tree.nodes()[1].aabb;
        assert_eq!(leaf[1][2], f64::from(z));
        let point = [0.5, 0.5, z + 2f32.powi(-26)];
        assert_eq!(tree.excess_ulps(1, point), Some(0.125));
    }
}
