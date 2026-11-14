//! Writing compressed meshes: [`build_compressed_mesh`] turns triangles into the arrays of an `hknpCompressedMeshShapeData`
//! mesh tree, and [`replace_compressed_mesh`] puts them into the shape of an editable [`TagfileImage`].
//!
//! The format ([`compressed_mesh`](crate::compressed_mesh) reads it): the triangles are split into *sections* of at most
//! 127 primitives and 255 distinct vertices; a vertex only one section uses is stored in that section as an 11/11/10-bit
//! code over the box of the section's own such vertices (its *packed* vertices), a vertex two or more sections use is
//! stored once as a 21/21/22-bit code over the box of the whole mesh (a *shared* vertex) and named in each section by a
//! 16-bit entry of the shared-vertex index; a primitive is four vertex indices (`[a, b, c, c]` for a triangle); each
//! section has a 4-byte-node tree over its primitives, the mesh has a 5-byte-node tree over its sections, and the mesh a
//! four-lane simd tree over all primitives. A primitive's shape key is `section << 8 | primitive << 1`.
//!
//! **What the stock files show, and what is written here** (the census over every compressed mesh of Sekiro and Elden
//! Ring, 3,344 and 22,899 meshes):
//!
//! - A section's codec offset is the minimum of its packed vertices and its scale the extent over the code range (2047,
//!   2047, 1023): its packed codes span 0 to the top code on every axis in 99% of the sections. The section `domain` is
//!   the box of its triangle vertices grown by the shape's convex radius, the tree `domain` the same for the whole mesh.
//! - A vertex is shared exactly when two or more sections use it in Sekiro's `h` and object meshes; Elden Ring's meshes
//!   and Sekiro's `l` meshes also store some single-section vertices as shared. Written here: shared exactly when two or
//!   more sections use it. The shared array lists a section's new vertices after the earlier sections' ones, in the order
//!   the sections first use them.
//! - A section's 16-bit entries address `page * 65,536 + entry` of the shared array. A section whose entries would not
//!   fit one page of the array gets copies of the vertices it needs on the page it appends to (the array is padded to
//!   the next page boundary when a section would straddle one).
//! - The key bits are `bitlen(maxKeyValue + 1)` (`bitsPerKey` and the shape's `numShapeKeyBits`), `numPrimitiveKeys`
//!   counts the primitives, and one data run `{0xFFFF, 0, count}` covers a section's primitives (identical runs are
//!   stored once).
//! - The simd tree's leaf boxes are exactly the primitives' vertex boxes; its lane boxes exactly the unions below.
//!
//! By default sections are cut by the surface-area heuristic over the primitives' boxes ([`crate::bvtree_write`] has what the
//! stock trees show of this) and the trees over their primitives and over the sections are [`sah_shape`]s.
//! [`Layout::Stock`] instead builds one [`StockTree`] over all the primitives and cuts it the way the stock tool does: a
//! section's tree is its piece of it and the mesh tree the part above the sections; the primitives of a section are stored
//! in the stock order ([`crate::section_numbering`]).
//!
//! The interior flags of the shape (`triangleIsInterior`) are computed from the written triangles ([`crate::interior_keys`]
//! has the rule, which reproduces the stock flags exactly).
//!
//! Quads: by default every primitive written is a triangle. With [`Primitives::PairedQuads`] the triangle pairs the stock
//! tool joins ([`pair_quads`] has the rule; about a quarter of the stock triangles are quad halves, 28% in Sekiro and 23%
//! in Elden Ring) become quad primitives `[a, b, c, d]`, the first half `a b c` at the primitive's key and the second half
//! `a c d` at the key with the half bit set; the quads are never flat (`primitiveStoresIsFlatConvex` stays 0).
//!
//! What is not reproduced: the exact stock trees (the stock tool built them from vertices before they were quantised,
//! see [`StockTree`]), custom and extern primitives, and the connectivity. A tree of another topology is read the same way.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use crate::bvtree::NodeWidth;
use crate::bvtree_write::{Aabb, Shape, encode_tree, sah_shape, sah_split};
use crate::degenerate::is_degenerate_triangle;
use crate::interior::{MeshView, SectionView, interior_keys, primitives_of};
use crate::quads::{QuadCandidate, pair_quads};
use crate::schema::{Schema, put_f32s, put_uint};
use crate::simd_write::{SimdEncoding, SimdLeaf, build_simd_tree, node_bytes};
use crate::simdtree::SimdNode;
use crate::stock_tree::{Box3, StockTree, section_numbering};
use crate::{ImageItem, ItemKind, Result, TagfileImage, Triangle, invalid};
use primase_foundations::vertex::{packed_position32, packed_position64};

/// The most primitives of a section (the stock sections have 127 at most).
pub const MAX_PRIMITIVES: usize = 127;
/// The most vertex slots (packed vertices and shared-vertex index entries) of a section: a vertex index is a byte.
pub const MAX_SLOTS: usize = 255;

/// The code range of a packed vertex's three axes.
const PACKED_CODES: [f64; 3] = [2047.0, 2047.0, 1023.0];
/// The code range of a shared vertex's three axes.
const SHARED_CODES: [f64; 3] = [2_097_151.0, 2_097_151.0, 4_194_303.0];
/// A shared-vertex page: the entries are 16 bits.
const PAGE: usize = 1 << 16;
/// The data-run value of primitives that name no material.
const NO_MATERIAL: u16 = 0xffff;

/// The triangles to build a mesh from.
#[derive(Debug, Clone, Copy)]
pub struct MeshSource<'a> {
    /// The vertices, shape-local (finite).
    pub vertices: &'a [[f32; 3]],
    /// The triangles as indices of `vertices`, in the winding the mesh keeps.
    pub triangles: &'a [[u32; 3]],
    /// The shape's convex radius (`convexRadius`; 0.01 in the Sekiro and Elden Ring map meshes): the section and tree
    /// boxes are grown by it.
    pub convex_radius: f32,
}

/// One section of a built mesh, in the terms both SDK layouts share.
#[derive(Debug, Clone, PartialEq)]
pub struct BuiltSection {
    /// The packed vertices' offset (xyz) and scale (xyz).
    pub codec: [f32; 6],
    /// The `domain` box, minimum then maximum.
    pub domain: [[f32; 3]; 2],
    /// The section tree's nodes, 4 bytes each.
    pub nodes: Vec<u8>,
    /// Index of the section's first packed vertex in [`BuiltMesh::packed_vertices`].
    pub first_packed_vertex: usize,
    /// How many packed vertices it has.
    pub packed_vertices: usize,
    /// Index of its first entry in [`BuiltMesh::shared_vertex_index`].
    pub first_shared_index: usize,
    /// How many shared-vertex index entries it has.
    pub shared_indices: usize,
    /// Index of its first primitive in [`BuiltMesh::primitives`].
    pub first_primitive: usize,
    /// How many primitives it has.
    pub primitives: usize,
    /// Its shared-vertex page.
    pub page: usize,
    /// The first of its data runs in [`BuiltMesh::data_runs`], and how many it has.
    pub first_data_run: usize,
    /// How many data runs the section has.
    pub data_runs: usize,
    /// The node of the section's leaf in the mesh tree.
    pub leaf_index: usize,
}

/// The arrays of a built compressed mesh tree.
#[derive(Debug, Clone, PartialEq)]
pub struct BuiltMesh {
    /// The tree's `domain` (the shared vertices' box), minimum then maximum.
    pub domain: [[f32; 3]; 2],
    /// The mesh tree's nodes, 5 bytes each.
    pub mesh_nodes: Vec<u8>,
    /// The sections.
    pub sections: Vec<BuiltSection>,
    /// All primitives' vertex indices, by section (`[a, b, c, c]` for a triangle, `[a, b, c, d]` for a quad).
    pub primitives: Vec<[u8; 4]>,
    /// The shared-vertex index: 16-bit entries, by section.
    pub shared_vertex_index: Vec<u16>,
    /// The packed vertices (11/11/10-bit codes), by section.
    pub packed_vertices: Vec<u32>,
    /// The shared vertices (21/21/22-bit codes).
    pub shared_vertices: Vec<u64>,
    /// The data runs `(value, index, count)`.
    pub data_runs: Vec<(u16, u8, u8)>,
    /// `numPrimitiveKeys`.
    pub num_primitive_keys: u32,
    /// `maxKeyValue`.
    pub max_key_value: u32,
    /// `bitsPerKey`, and the shape's `numShapeKeyBits`.
    pub bits_per_key: u32,
    /// The convex radius the boxes were grown by.
    pub convex_radius: f32,
    /// For each source triangle, the shape key it got (the second half of a quad has the half bit set); `None` for one left
    /// out (a triangle with a repeated vertex, or two vertices at one position, or a degenerate one if
    /// [`BuildOptions::drop_degenerate`] says so).
    pub triangle_keys: Vec<Option<u32>>,
    /// The simd tree's leaves: each primitive's key (its first half) and exact vertex box.
    pub simd_leaves: Vec<SimdLeaf>,
}

/// A value narrowed to `f32`.
#[allow(clippy::cast_possible_truncation, reason = "the stored boxes are f32")]
fn narrow(x: f64) -> f32 {
    x as f32
}

/// The largest `f32` not above `x`.
fn down(x: f64) -> f32 {
    let f = narrow(x);
    if f64::from(f) > x { f.next_down() } else { f }
}

/// The smallest `f32` not below `x`.
fn up(x: f64) -> f32 {
    let f = narrow(x);
    if f64::from(f) < x { f.next_up() } else { f }
}

/// The code of `x` over a range of `max_code + 1` codes, rounded and clamped.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to the code range first"
)]
fn code_of(x: f64, max_code: f64) -> u64 {
    if x.is_nan() {
        return 0;
    }
    x.round().clamp(0.0, max_code) as u64
}

/// The shared-vertex array under construction, a page at a time.
#[derive(Debug)]
struct SharedArray {
    /// How many entries a section's 16-bit index entries can name from one page.
    page_size: usize,
    /// The codes stored so far.
    codes: Vec<u64>,
    /// Where each vertex is stored on each page it has been placed on: (vertex, page) to index in `codes`.
    at: std::collections::BTreeMap<(u32, usize), usize>,
}

impl SharedArray {
    fn new(page_size: usize) -> Self {
        Self {
            page_size,
            codes: Vec::new(),
            at: std::collections::BTreeMap::new(),
        }
    }

    /// Place the (distinct) vertices `ids` of one section so that all of them are on one page: the page the array is
    /// appending to, with a copy of each vertex that is only stored on another page. When the new entries would run
    /// past the end of that page the array is padded to the next page and the section starts it. Returns the page and the
    /// index in the array of each vertex, in the order of `ids`; `code` gives the code of a vertex that has to be stored.
    fn place(&mut self, ids: &[u32], code: &dyn Fn(u32) -> Result<u64>) -> Result<(usize, Vec<usize>)> {
        if ids.len() > self.page_size {
            return Err(invalid(
                0,
                "a section names more shared vertices than a page holds",
            ));
        }
        let mut page = self.codes.len().checked_div(self.page_size).unwrap_or(0);
        let missing = ids.iter().filter(|&&v| !self.at.contains_key(&(v, page))).count();
        let used = self.codes.len().checked_rem(self.page_size).unwrap_or(0);
        if used.saturating_add(missing) > self.page_size {
            page = page.saturating_add(1);
            self.codes.resize(page.saturating_mul(self.page_size), 0);
        }
        let mut indexes = Vec::with_capacity(ids.len());
        for &v in ids {
            let index = match self.at.get(&(v, page)).copied() {
                Some(index) => index,
                None => {
                    let index = self.codes.len();
                    self.codes.push(code(v)?);
                    self.at.insert((v, page), index);
                    index
                }
            };
            indexes.push(index);
        }
        Ok((page, indexes))
    }
}

/// A primitive kept for the build: a triangle, or a quad made of two triangles.
#[derive(Debug, Clone, Copy)]
struct Kept {
    /// The source triangle that is the primitive's first half.
    source: usize,
    /// The source triangle that is its second half, for a quad.
    partner: Option<usize>,
    /// The primitive's four vertex indices (a triangle repeats its third).
    corners: [u32; 4],
}

impl Kept {
    /// The distinct corners the primitive names.
    fn used(&self) -> &[u32] {
        let count = if self.partner.is_some() { 4 } else { 3 };
        self.corners.get(..count).unwrap_or(&[])
    }
}

fn vertex(vertices: &[[f32; 3]], index: u32) -> Result<[f32; 3]> {
    usize::try_from(index)
        .ok()
        .and_then(|i| vertices.get(i))
        .copied()
        .ok_or_else(|| {
            invalid(
                0,
                format!("a triangle names vertex {index} of {}", vertices.len()),
            )
        })
}

/// The primitives of `kept` (all triangles) with the pairs [`pair_quads`] finds joined into quads: a quad keeps its first
/// half's corners and takes the second half's third corner as its fourth. The primitives come out in the order of their
/// first halves.
fn join_quads(kept: &[Kept], vertices: &[[f32; 3]]) -> Result<Vec<Kept>> {
    let mut candidates = Vec::with_capacity(kept.len());
    for k in kept {
        let [a, b, c, _] = k.corners;
        candidates.push(QuadCandidate {
            vertices: [vertex(vertices, a)?, vertex(vertices, b)?, vertex(vertices, c)?],
            data: 0,
        });
    }
    let mut second_of: Vec<Option<usize>> = vec![None; kept.len()];
    let mut is_second = vec![false; kept.len()];
    for [first, second] in pair_quads(&candidates) {
        if let (Some(slot), Some(flag)) = (second_of.get_mut(first), is_second.get_mut(second)) {
            *slot = Some(second);
            *flag = true;
        }
    }
    let mut out = Vec::with_capacity(kept.len());
    for (i, k) in kept.iter().enumerate() {
        if is_second.get(i).copied().unwrap_or(false) {
            continue;
        }
        match second_of.get(i).copied().flatten().and_then(|s| kept.get(s)) {
            Some(second) => {
                let [a, b, c, _] = k.corners;
                let [_, _, d, _] = second.corners;
                out.push(Kept {
                    source: k.source,
                    partner: Some(second.source),
                    corners: [a, b, c, d],
                });
            }
            None => out.push(*k),
        }
    }
    Ok(out)
}

/// The boxes of the primitives for a [`StockTree`]: their source vertices' box grown by `radius`, in single precision.
fn primitive_boxes(kept: &[Kept], vertices: &[[f32; 3]], radius: f32) -> Result<Vec<Box3>> {
    let mut boxes = Vec::with_capacity(kept.len());
    for k in kept {
        let mut low = [f32::MAX; 3];
        let mut high = [f32::MIN; 3];
        for &v in k.used() {
            let p = vertex(vertices, v)?;
            for (axis, &x) in p.iter().enumerate() {
                if let (Some(l), Some(h)) = (low.get_mut(axis), high.get_mut(axis)) {
                    *l = l.min(x);
                    *h = h.max(x);
                }
            }
        }
        boxes.push([low.map(|x| x - radius), high.map(|x| x + radius)]);
    }
    Ok(boxes)
}

/// How many distinct vertices the triangles `group` use, up to `limit + 1`; `stamp` and `mark` keep the count O(group).
fn distinct_vertices(
    kept: &[Kept],
    group: &[usize],
    stamp: &mut [usize],
    mark: usize,
    limit: usize,
) -> usize {
    let mut count = 0usize;
    for &t in group {
        let Some(k) = kept.get(t) else { continue };
        for &v in k.used() {
            if let Some(slot) = usize::try_from(v).ok().and_then(|v| stamp.get_mut(v))
                && *slot != mark
            {
                *slot = mark;
                count = count.saturating_add(1);
                if count > limit {
                    return count;
                }
            }
        }
    }
    count
}

/// Split the triangles into sections: split a group by the surface-area heuristic over the boxes of its triangles (grown
/// by `radius`, [`sah_split`]) until it has at most [`MAX_PRIMITIVES`] triangles using at most [`MAX_SLOTS`] vertices.
/// Sections come out in the order of the splits, the low side first.
fn partition(kept: &[Kept], vertices: &[[f32; 3]], radius: f64) -> Result<Vec<Vec<usize>>> {
    let mut boxes: Vec<Aabb> = Vec::with_capacity(kept.len());
    for k in kept {
        let points = k
            .used()
            .iter()
            .map(|&v| vertex(vertices, v))
            .collect::<Result<Vec<_>>>()?;
        boxes.push(grown_box(&points, radius));
    }
    let mut stamp = vec![0usize; vertices.len()];
    let mut mark = 0usize;
    let mut out: Vec<Vec<usize>> = Vec::new();
    let mut stack: Vec<Vec<usize>> = vec![(0..kept.len()).collect()];
    while let Some(mut group) = stack.pop() {
        mark = mark.saturating_add(1);
        let fits = group.len() <= MAX_PRIMITIVES
            && distinct_vertices(kept, &group, &mut stamp, mark, MAX_SLOTS) <= MAX_SLOTS;
        if fits || group.len() < 2 {
            out.push(group);
            continue;
        }
        let low = sah_split(&boxes, &mut group);
        let second = group.split_off(low);
        // the first half is taken first
        stack.push(second);
        stack.push(group);
    }
    Ok(out)
}

/// The box of the vertices of `corners` grown by `radius`, in `f64`.
fn grown_box(points: &[[f32; 3]], radius: f64) -> Aabb {
    let mut low = [f64::MAX; 3];
    let mut high = [f64::MIN; 3];
    for p in points {
        for axis in 0..3 {
            let x = f64::from(p.get(axis).copied().unwrap_or(0.0));
            if let (Some(l), Some(h)) = (low.get_mut(axis), high.get_mut(axis)) {
                *l = l.min(x - radius);
                *h = h.max(x + radius);
            }
        }
    }
    [low, high]
}

/// A box as `f32` bounds that hold it.
fn outward(b: &Aabb) -> [[f32; 3]; 2] {
    [b[0].map(down), b[1].map(up)]
}

/// The packed code of `p` over a codec.
fn pack32(p: [f32; 3], offset: [f32; 3], scale: [f32; 3]) -> u32 {
    let mut code = 0u32;
    for axis in 0..3 {
        let s = f64::from(scale.get(axis).copied().unwrap_or(0.0));
        let o = f64::from(offset.get(axis).copied().unwrap_or(0.0));
        let x = f64::from(p.get(axis).copied().unwrap_or(0.0));
        let max_code = PACKED_CODES.get(axis).copied().unwrap_or(0.0);
        let c = if s > 0.0 {
            code_of((x - o) / s, max_code)
        } else {
            0
        };
        let shift = match axis {
            0 => 0u32,
            1 => 11,
            _ => 22,
        };
        code |= u32::try_from(c).unwrap_or(0).checked_shl(shift).unwrap_or(0);
    }
    code
}

/// The shared code of `p` over the box `min..max`.
fn pack64(p: [f32; 3], min: [f32; 3], max: [f32; 3]) -> u64 {
    let mut code = 0u64;
    for axis in 0..3 {
        let lo = f64::from(min.get(axis).copied().unwrap_or(0.0));
        let hi = f64::from(max.get(axis).copied().unwrap_or(0.0));
        let x = f64::from(p.get(axis).copied().unwrap_or(0.0));
        let range = SHARED_CODES.get(axis).copied().unwrap_or(0.0);
        let c = if hi > lo {
            code_of((x - lo) / (hi - lo) * range, range)
        } else {
            0
        };
        let shift = match axis {
            0 => 0u32,
            1 => 21,
            _ => 42,
        };
        code |= c.checked_shl(shift).unwrap_or(0);
    }
    code
}

/// Build the mesh tree arrays of `source`.
///
/// The triangles are split into sections ([`MAX_PRIMITIVES`], [`MAX_SLOTS`]), their vertices quantised, and trees built
/// over primitives and sections; the arrays come back as the file stores them, for [`replace_compressed_mesh`]. Decoding
/// the result gives every triangle back within the quantisation of its section's codec (half a code step: a few
/// millimetres for a 5 metre section), in the same winding and vertex order. A triangle with a repeated vertex index or
/// two vertices at one position is left out ([`BuiltMesh::triangle_keys`] says which).
///
/// Refused: no triangle left; a vertex that is not finite; an index past the vertices; a convex radius that is negative or
/// not finite; more sections or shared vertices than the file's fields number.
pub fn build_compressed_mesh(source: &MeshSource<'_>) -> Result<BuiltMesh> {
    build_compressed_mesh_with(source, &BuildOptions::default())
}

/// What the primitives of a built mesh are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Primitives {
    /// One primitive per triangle.
    Triangles,
    /// Triangles that [`pair_quads`] pairs are joined into quad primitives (the first half's key has the half bit clear,
    /// the second half's set), as the stock tool does for about a quarter of its triangles. The pairs are looked for over
    /// the whole mesh before the sections are cut, as the stock tool does.
    PairedQuads,
}

/// How the sections and their trees are laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// Sections are cut by the surface-area heuristic over the primitives' boxes, and each tree is built by
    /// [`sah_shape`] over its own leaves.
    Sah,
    /// One [`StockTree`] is built over all the primitives and cut into sections ([`StockTree::cut`]); a section's tree is
    /// its piece of it, and the mesh tree the part above the sections. The sections are numbered in queue order, and
    /// a section's primitives in the stock order ([`section_numbering`]).
    Stock,
}

/// The choices of [`build_compressed_mesh_with`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BuildOptions {
    /// Triangles, or pairs of them joined into quads.
    pub primitives: Primitives,
    /// How sections and trees are laid out.
    pub layout: Layout,
    /// Leave out the triangles that [`is_degenerate_triangle`] calls degenerate with this threshold (the stock tool's own
    /// is [`DEFAULT_DEGENERATE_THRESHOLD`](crate::DEFAULT_DEGENERATE_THRESHOLD)), on top of those with a repeated vertex. `None` keeps them.
    pub drop_degenerate: Option<f32>,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            primitives: Primitives::Triangles,
            layout: Layout::Sah,
            drop_degenerate: None,
        }
    }
}

/// A section tree of the stock layout with its primitives numbered as the stock tool does ([`section_numbering`]): the
/// primitive stored at index `p` is the leaf of pre-order rank `ranks[p]`, and a leaf's payload is the index it is stored at.
fn stock_numbered(shape: &[Shape], order: &[usize]) -> (Vec<Shape>, Vec<usize>) {
    let ranks = section_numbering(order.len());
    let mut stored_at = vec![0u32; order.len()];
    for (p, &rank) in ranks.iter().enumerate() {
        if let Some(slot) = stored_at.get_mut(rank) {
            *slot = u32::try_from(p).unwrap_or(u32::MAX);
        }
    }
    let shape = shape
        .iter()
        .map(|node| match *node {
            Shape::Leaf { payload } => Shape::Leaf {
                payload: usize::try_from(payload)
                    .ok()
                    .and_then(|rank| stored_at.get(rank).copied())
                    .unwrap_or(payload),
            },
            other => other,
        })
        .collect();
    let order = ranks
        .iter()
        .filter_map(|&rank| order.get(rank).copied())
        .collect();
    (shape, order)
}

/// [`build_compressed_mesh`] with the choices of [`BuildOptions`].
pub fn build_compressed_mesh_with(source: &MeshSource<'_>, options: &BuildOptions) -> Result<BuiltMesh> {
    let verts = source.vertices;
    let radius = source.convex_radius;
    if !radius.is_finite() || radius < 0.0 {
        return Err(invalid(0, format!("a convex radius of {radius}")));
    }
    if verts.iter().flatten().any(|c| !c.is_finite()) {
        return Err(invalid(0, "a vertex that is not finite"));
    }
    let r = f64::from(radius);
    // the triangles that can be stored
    let mut kept: Vec<Kept> = Vec::with_capacity(source.triangles.len());
    for (i, &corners) in source.triangles.iter().enumerate() {
        let [a, b, c] = corners;
        let (pa, pb, pc) = (vertex(verts, a)?, vertex(verts, b)?, vertex(verts, c)?);
        if a == b || b == c || a == c || pa == pb || pb == pc || pa == pc {
            continue;
        }
        if options
            .drop_degenerate
            .is_some_and(|threshold| is_degenerate_triangle(pa, pb, pc, threshold))
        {
            continue;
        }
        kept.push(Kept {
            source: i,
            partner: None,
            corners: [a, b, c, c],
        });
    }
    if kept.is_empty() {
        return Err(invalid(0, "no triangle to build a mesh from"));
    }
    if options.primitives == Primitives::PairedQuads {
        kept = join_quads(&kept, verts)?;
    }
    let mut triangle_keys: Vec<Option<u32>> = vec![None; source.triangles.len()];

    // sections, in the order of a tree over their (unquantised) boxes, so that a section's number is its leaf's payload
    let mut stock_shapes: Option<Vec<(Vec<Shape>, Vec<usize>)>> = None;
    let (groups, mesh_shape, section_order) = match options.layout {
        Layout::Sah => {
            let groups = partition(&kept, verts, r)?;
            let rough: Vec<Aabb> = groups
                .iter()
                .map(|g| {
                    let points: Vec<[f32; 3]> = g
                        .iter()
                        .filter_map(|&t| kept.get(t))
                        .flat_map(|k| k.used().iter().copied())
                        .filter_map(|v| vertex(verts, v).ok())
                        .collect();
                    grown_box(&points, r)
                })
                .collect();
            let (mesh_shape, section_order) = sah_shape(&rough)?;
            (groups, mesh_shape, section_order)
        }
        Layout::Stock => {
            let boxes = primitive_boxes(&kept, verts, radius)?;
            let tree = StockTree::build(&boxes);
            let items = tree.cut(MAX_PRIMITIVES, &|leaves: &[usize]| {
                let mut ids: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
                for &t in leaves {
                    if let Some(k) = kept.get(t) {
                        ids.extend(k.used().iter().copied());
                    }
                }
                ids.len() <= MAX_SLOTS
            });
            let groups: Vec<Vec<usize>> = items.iter().map(|&n| tree.leaves_under(n)).collect();
            let shapes: Vec<(Vec<Shape>, Vec<usize>)> = items
                .iter()
                .zip(&groups)
                .map(|(&n, g)| (tree.shape_under(n).0, (0..g.len()).collect()))
                .collect();
            stock_shapes = Some(shapes);
            let section_order: Vec<usize> = (0..groups.len()).collect();
            (groups, tree.shape_above(&items), section_order)
        }
    };
    let sections_tris: Vec<&Vec<usize>> = section_order.iter().filter_map(|&g| groups.get(g)).collect();
    let section_count = sections_tris.len();
    if section_count > 0x00ff_ffff {
        return Err(invalid(0, "more sections than a shape key holds"));
    }

    // which vertices more than one section uses
    let mut first_user: Vec<Option<usize>> = vec![None; verts.len()];
    let mut shared: Vec<bool> = vec![false; verts.len()];
    for (s, group) in sections_tris.iter().enumerate() {
        for &t in group.iter() {
            let Some(k) = kept.get(t) else { continue };
            for &v in k.used() {
                let Ok(i) = usize::try_from(v) else {
                    continue;
                };
                match first_user.get(i).copied().flatten() {
                    None => {
                        if let Some(slot) = first_user.get_mut(i) {
                            *slot = Some(s);
                        }
                    }
                    Some(first) if first != s => {
                        if let Some(flag) = shared.get_mut(i) {
                            *flag = true;
                        }
                    }
                    Some(_) => {}
                }
            }
        }
    }

    // the shared vertices' box: the mesh's vertices grown by the radius
    let used: Vec<[f32; 3]> = kept
        .iter()
        .flat_map(|k| k.used().iter().copied())
        .filter_map(|v| vertex(verts, v).ok())
        .collect();
    let domain = outward(&grown_box(&used, r));
    let (dmin, dmax) = (domain[0], domain[1]);

    let mut packed_vertices: Vec<u32> = Vec::new();
    let mut shared_array = SharedArray::new(PAGE);
    let mut shared_vertex_index: Vec<u16> = Vec::new();
    let mut primitives: Vec<[u8; 4]> = Vec::new();
    let mut sections: Vec<BuiltSection> = Vec::with_capacity(section_count);
    let mut section_boxes: Vec<Aabb> = Vec::with_capacity(section_count);
    let mut simd_leaves: Vec<SimdLeaf> = Vec::new();
    let mut data_runs: Vec<(u16, u8, u8)> = Vec::new();
    let mut slot_stamp = vec![usize::MAX; verts.len()];
    let mut slot_of = vec![0usize; verts.len()];
    let mut max_key = 0u32;

    for (s, group) in sections_tris.iter().enumerate() {
        // the section's vertices in the order its triangles first use them: packed ones, then shared ones
        let mut packed_ids: Vec<u32> = Vec::new();
        let mut shared_ids: Vec<u32> = Vec::new();
        let mut seen: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
        for &t in group.iter() {
            let Some(k) = kept.get(t) else { continue };
            for &v in k.used() {
                if !seen.insert(v) {
                    continue;
                }
                let is_shared = usize::try_from(v)
                    .ok()
                    .and_then(|i| shared.get(i))
                    .copied()
                    .unwrap_or(false);
                if is_shared {
                    shared_ids.push(v);
                } else {
                    packed_ids.push(v);
                }
            }
        }
        if packed_ids.len().saturating_add(shared_ids.len()) > MAX_SLOTS {
            return Err(invalid(
                0,
                format!("section {s} needs more than {MAX_SLOTS} vertex slots"),
            ));
        }
        // the packed codec: the box of the packed vertices
        let packed_points: Vec<[f32; 3]> = packed_ids.iter().filter_map(|&v| vertex(verts, v).ok()).collect();
        let (offset, scale) = if packed_points.is_empty() {
            (dmin, [0.0f32; 3])
        } else {
            let mut low = [f32::MAX; 3];
            let mut high = [f32::MIN; 3];
            for p in &packed_points {
                for axis in 0..3 {
                    let x = p.get(axis).copied().unwrap_or(0.0);
                    if let (Some(l), Some(h)) = (low.get_mut(axis), high.get_mut(axis)) {
                        *l = l.min(x);
                        *h = h.max(x);
                    }
                }
            }
            let mut scale = [0.0f32; 3];
            for axis in 0..3 {
                let codes = PACKED_CODES.get(axis).copied().unwrap_or(1.0);
                let extent = f64::from(high.get(axis).copied().unwrap_or(0.0))
                    - f64::from(low.get(axis).copied().unwrap_or(0.0));
                if let Some(sc) = scale.get_mut(axis) {
                    *sc = narrow(extent / codes);
                }
            }
            (low, scale)
        };
        // slot numbers and decoded positions
        let mut positions: Vec<[f32; 3]> =
            Vec::with_capacity(packed_ids.len().saturating_add(shared_ids.len()));
        let first_packed_vertex = packed_vertices.len();
        for (n, &v) in packed_ids.iter().enumerate() {
            let p = vertex(verts, v)?;
            let code = pack32(p, offset, scale);
            packed_vertices.push(code);
            positions.push(packed_position32(code, scale, offset).map_err(|e| invalid(0, e.to_string()))?);
            if let Ok(i) = usize::try_from(v)
                && let (Some(stamp), Some(slot)) = (slot_stamp.get_mut(i), slot_of.get_mut(i))
            {
                *stamp = s;
                *slot = n;
            }
        }
        // the shared vertices: on the page the shared array is appending to
        let (page, indexes) =
            shared_array.place(&shared_ids, &|v| Ok(pack64(vertex(verts, v)?, dmin, dmax)))?;
        if page > 255 {
            return Err(invalid(
                0,
                "more shared-vertex pages than a section's page byte holds",
            ));
        }
        let first_shared_index = shared_vertex_index.len();
        for (n, (&v, &index)) in shared_ids.iter().zip(&indexes).enumerate() {
            let code = shared_array.codes.get(index).copied().unwrap_or(0);
            positions.push(packed_position64(code, dmin, dmax).map_err(|e| invalid(0, e.to_string()))?);
            let entry = index.saturating_sub(page.saturating_mul(PAGE));
            shared_vertex_index
                .push(u16::try_from(entry).map_err(|_| invalid(0, "a shared entry past its page"))?);
            if let Ok(i) = usize::try_from(v)
                && let (Some(stamp), Some(slot)) = (slot_stamp.get_mut(i), slot_of.get_mut(i))
            {
                *stamp = s;
                *slot = packed_ids.len().saturating_add(n);
            }
        }
        // the primitives: the leaves of the section's tree in order
        let slot = |v: u32| -> Result<usize> {
            let i = usize::try_from(v).map_err(|_| invalid(0, "a vertex index past usize"))?;
            if slot_stamp.get(i).copied() != Some(s) {
                return Err(invalid(0, "a triangle vertex without a slot"));
            }
            slot_of
                .get(i)
                .copied()
                .ok_or_else(|| invalid(0, "a vertex without a slot"))
        };
        let mut leaf_boxes: Vec<Aabb> = Vec::with_capacity(group.len());
        let mut tri_slots: Vec<[usize; 4]> = Vec::with_capacity(group.len());
        for &t in group.iter() {
            let k = kept
                .get(t)
                .copied()
                .ok_or_else(|| invalid(0, "a triangle past the kept ones"))?;
            let [a, b, c, d] = k.corners;
            let slots = [slot(a)?, slot(b)?, slot(c)?, slot(d)?];
            let points: Vec<[f32; 3]> = slots.iter().filter_map(|&i| positions.get(i).copied()).collect();
            leaf_boxes.push(grown_box(&points, r));
            tri_slots.push(slots);
        }
        let (shape, order) = match stock_shapes.as_ref().and_then(|v| v.get(s)) {
            Some((shape, order)) => stock_numbered(shape, order),
            None => sah_shape(&leaf_boxes)?,
        };
        let section_domain = outward(&grown_box(&positions, r));
        let tree = encode_tree(NodeWidth::Four, section_domain, &shape, &|payload| {
            order
                .get(usize::try_from(payload).ok()?)
                .and_then(|&i| leaf_boxes.get(i))
                .copied()
        })?;
        let first_primitive = primitives.len();
        for (k, &i) in order.iter().enumerate() {
            let slots = tri_slots
                .get(i)
                .copied()
                .ok_or_else(|| invalid(0, "a leaf past the triangles"))?;
            let bytes = slots.map(|x| u8::try_from(x).map_err(|_| invalid(0, "a vertex slot past a byte")));
            let [a, b, c, d] = bytes;
            primitives.push([a?, b?, c?, d?]);
            let key = u32::try_from(s)
                .ok()
                .and_then(|s| s.checked_mul(256))
                .zip(u32::try_from(k).ok().and_then(|k| k.checked_mul(2)))
                .map(|(s, k)| s | k)
                .ok_or_else(|| invalid(0, "a shape key past 32 bits"))?;
            let primitive = group
                .get(i)
                .and_then(|&t| kept.get(t))
                .ok_or_else(|| invalid(0, "a leaf past the triangles"))?;
            if let Some(slot) = triangle_keys.get_mut(primitive.source) {
                *slot = Some(key);
            }
            // a quad's second half has the half bit set, and the largest key counts it
            max_key = max_key.max(key);
            if let Some(second) = primitive.partner {
                if let Some(slot) = triangle_keys.get_mut(second) {
                    *slot = Some(key | 1);
                }
                max_key = max_key.max(key | 1);
            }
            let points: Vec<[f32; 3]> = slots.iter().filter_map(|&x| positions.get(x).copied()).collect();
            let b = grown_box(&points, 0.0);
            simd_leaves.push(SimdLeaf {
                key,
                aabb: [b[0].map(narrow), b[1].map(narrow)],
            });
        }
        let run = (
            NO_MATERIAL,
            0u8,
            u8::try_from(group.len()).map_err(|_| invalid(0, "a section of more than 255 primitives"))?,
        );
        let first_data_run = match data_runs.iter().position(|x| *x == run) {
            Some(i) => i,
            None => {
                data_runs.push(run);
                data_runs.len().saturating_sub(1)
            }
        };
        section_boxes.push(section_domain.map(|c| c.map(f64::from)));
        sections.push(BuiltSection {
            codec: [offset[0], offset[1], offset[2], scale[0], scale[1], scale[2]],
            domain: section_domain,
            nodes: tree.bytes,
            first_packed_vertex,
            packed_vertices: packed_ids.len(),
            first_shared_index,
            shared_indices: shared_ids.len(),
            first_primitive,
            primitives: group.len(),
            page,
            first_data_run,
            data_runs: 1,
            leaf_index: 0,
        });
    }

    // the mesh tree over the sections' domains
    let mesh = encode_tree(NodeWidth::Five, domain, &mesh_shape, &|payload| {
        section_boxes.get(usize::try_from(payload).ok()?).copied()
    })?;
    for (payload, node) in &mesh.leaves {
        if let Some(section) = usize::try_from(*payload).ok().and_then(|p| sections.get_mut(p)) {
            section.leaf_index = *node;
        }
    }
    let num_primitive_keys =
        u32::try_from(primitives.len()).map_err(|_| invalid(0, "more primitives than 2^32"))?;
    let bits_per_key = 32u32.saturating_sub(max_key.saturating_add(1).leading_zeros());
    Ok(BuiltMesh {
        domain,
        mesh_nodes: mesh.bytes,
        sections,
        primitives,
        shared_vertex_index,
        packed_vertices,
        shared_vertices: shared_array.codes,
        data_runs,
        num_primitive_keys,
        max_key_value: max_key,
        bits_per_key,
        convex_radius: radius,
        triangle_keys,
        simd_leaves,
    })
}

impl BuiltMesh {
    /// The shape keys whose interior flag is set: the pass of [`interior_keys`] over the mesh's triangles, with the
    /// vertices named as they are stored (a shared vertex by its page and entry) and decoded from the codes.
    pub fn interior_keys(&self) -> Result<Vec<u32>> {
        let sections: Vec<SectionView> = self
            .sections
            .iter()
            .map(|s| SectionView {
                codec: s.codec,
                first_packed_vertex: s.first_packed_vertex,
                packed_vertices: s.packed_vertices,
                first_shared_index: s.first_shared_index,
                page: s.page,
                first_primitive: s.first_primitive,
                primitives: s.primitives,
            })
            .collect();
        let view = MeshView {
            domain: self.domain,
            sections: &sections,
            primitives: &self.primitives,
            packed_vertices: &self.packed_vertices,
            shared_vertices: &self.shared_vertices,
            shared_index: &self.shared_vertex_index,
            flat_mask: 0,
        };
        Ok(interior_keys(&primitives_of(&view, true)?))
    }

    /// The simd tree's nodes in `encoding`.
    pub fn simd_tree(&self, encoding: SimdEncoding) -> Result<Vec<SimdNode>> {
        build_simd_tree(&self.simd_leaves, encoding)
    }

    /// The triangles the arrays decode to, the way [`compressed_mesh`](crate::compressed_mesh) reads a file: in key
    /// order, each with its key and vertices (a quad gives its two halves).
    pub fn triangles(&self) -> Result<Vec<Triangle>> {
        let mut out = Vec::with_capacity(self.primitives.len());
        for (s, section) in self.sections.iter().enumerate() {
            let offset = [section.codec[0], section.codec[1], section.codec[2]];
            let scale = [section.codec[3], section.codec[4], section.codec[5]];
            let vertex = |slot: u8| -> Result<[f32; 3]> {
                let slot = usize::from(slot);
                if slot < section.packed_vertices {
                    let code = self
                        .packed_vertices
                        .get(section.first_packed_vertex.saturating_add(slot))
                        .copied()
                        .ok_or_else(|| invalid(0, "a packed vertex past the array"))?;
                    packed_position32(code, scale, offset).map_err(|e| invalid(0, e.to_string()))
                } else {
                    let entry = slot.saturating_sub(section.packed_vertices);
                    let index = self
                        .shared_vertex_index
                        .get(section.first_shared_index.saturating_add(entry))
                        .copied()
                        .ok_or_else(|| invalid(0, "a shared entry past the array"))?;
                    let code = self
                        .shared_vertices
                        .get(
                            section
                                .page
                                .saturating_mul(PAGE)
                                .saturating_add(usize::from(index)),
                        )
                        .copied()
                        .ok_or_else(|| invalid(0, "a shared vertex past the array"))?;
                    packed_position64(code, self.domain[0], self.domain[1])
                        .map_err(|e| invalid(0, e.to_string()))
                }
            };
            for k in 0..section.primitives {
                let p = self
                    .primitives
                    .get(section.first_primitive.saturating_add(k))
                    .ok_or_else(|| invalid(0, "a primitive past the array"))?;
                let key = u32::try_from(s)
                    .ok()
                    .and_then(|s| s.checked_mul(256))
                    .zip(u32::try_from(k).ok().and_then(|k| k.checked_mul(2)))
                    .map(|(s, k)| s | k)
                    .ok_or_else(|| invalid(0, "a shape key past 32 bits"))?;
                out.push(Triangle {
                    key,
                    vertices: [vertex(p[0])?, vertex(p[1])?, vertex(p[2])?],
                });
                // a quad's second half is `a c d`
                if p[3] != p[2] {
                    out.push(Triangle {
                        key: key | 1,
                        vertices: [vertex(p[0])?, vertex(p[2])?, vertex(p[3])?],
                    });
                }
            }
        }
        Ok(out)
    }
}

/// The object items of `image` that are compressed mesh shapes (`hknpCompressedMeshShape` or the
/// `fsnpCustomParamCompressedMeshShape` of Sekiro and Elden Ring).
pub fn compressed_mesh_shapes(image: &TagfileImage) -> Vec<usize> {
    let mut items = image.objects_named("fsnpCustomParamCompressedMeshShape");
    items.extend(image.objects_named("hknpCompressedMeshShape"));
    items.sort_unstable();
    items
}

/// The `Section` element's field writer for either SDK layout.
struct SectionWriter<'s, 't> {
    schema: &'s Schema<'t>,
    class: usize,
    size: usize,
    /// The shape's convex radius, which the `w` lanes of a domain keep as `1 -/+ radius`.
    radius: f32,
    /// 2018.1's separate fields, against 2016.2's packed words.
    separate: bool,
}

impl SectionWriter<'_, '_> {
    /// Put the unsigned `value` into the member `path` of element `index` of `buf`.
    fn put(&self, buf: &mut [u8], index: usize, path: &[&str], value: u64) -> Result<()> {
        let member = self.schema.path(self.class, path)?;
        let width = self.schema.int_width(member.type_index)?;
        let at = index
            .checked_mul(self.size)
            .and_then(|b| b.checked_add(member.offset))
            .ok_or_else(|| invalid(0, "section offset overflow"))?;
        put_uint(buf, at, width, value)
    }

    fn write(&self, buf: &mut [u8], index: usize, s: &BuiltSection) -> Result<()> {
        let big = |x: usize| u64::try_from(x).map_err(|_| invalid(0, "a section field past 64 bits"));
        let member = self.schema.field(self.class, "codecParms")?;
        let at = index
            .checked_mul(self.size)
            .and_then(|b| b.checked_add(member.offset))
            .ok_or_else(|| invalid(0, "section offset overflow"))?;
        put_f32s(buf, at, &s.codec)?;
        for (name, corner, w) in [
            ("min", &s.domain[0], 1.0 - self.radius),
            ("max", &s.domain[1], 1.0 + self.radius),
        ] {
            let m = self.schema.path(self.class, &["domain", name])?;
            let at = index
                .checked_mul(self.size)
                .and_then(|b| b.checked_add(m.offset))
                .ok_or_else(|| invalid(0, "section offset overflow"))?;
            put_f32s(buf, at, &[corner[0], corner[1], corner[2], w])?;
        }
        self.put(buf, index, &["numPackedVertices"], big(s.packed_vertices)?)?;
        self.put(buf, index, &["leafIndex"], big(s.leaf_index)?)?;
        self.put(buf, index, &["page"], big(s.page)?)?;
        if self.separate {
            self.put(
                buf,
                index,
                &["firstPackedVertexIndex"],
                big(s.first_packed_vertex)?,
            )?;
            self.put(
                buf,
                index,
                &["firstSharedVertexIndex"],
                big(s.first_shared_index)?,
            )?;
            self.put(buf, index, &["firstPrimitiveIndex"], big(s.first_primitive)?)?;
            self.put(buf, index, &["numPrimitives"], big(s.primitives)?)?;
            self.put(buf, index, &["firstDataRunIndex"], big(s.first_data_run)?)?;
            self.put(buf, index, &["numDataRuns"], big(s.data_runs)?)?;
        } else {
            self.put(buf, index, &["firstPackedVertex"], big(s.first_packed_vertex)?)?;
            self.put(buf, index, &["numSharedIndices"], big(s.shared_indices)?)?;
            let packed = |first: usize, count: usize| -> Result<u64> {
                let word = first
                    .checked_mul(256)
                    .and_then(|f| f.checked_add(count))
                    .ok_or_else(|| invalid(0, "a packed section word overflows"))?;
                u64::try_from(word)
                    .ok()
                    .filter(|w| *w <= u64::from(u32::MAX))
                    .ok_or_else(|| invalid(0, "a packed section word past 32 bits"))
            };
            self.put(
                buf,
                index,
                &["sharedVertices", "data"],
                packed(s.first_shared_index, s.packed_vertices)?,
            )?;
            self.put(
                buf,
                index,
                &["primitives", "data"],
                packed(s.first_primitive, s.primitives)?,
            )?;
            self.put(
                buf,
                index,
                &["dataRuns", "data"],
                packed(s.first_data_run, s.data_runs)?,
            )?;
        }
        Ok(())
    }
}

/// The element type of the array member at `path` of `class`, checked to be `bytes` wide.
fn element_of(schema: &Schema<'_>, class: usize, path: &[&str], bytes: usize) -> Result<(usize, usize)> {
    let member = schema.path(class, path)?;
    let element = schema.subtype_of(member.type_index)?;
    let size = schema.size_of(element)?;
    if size != bytes {
        return Err(invalid(
            0,
            format!(
                "{} has elements of {size} bytes, not the {bytes} this writer stores",
                path.join(".")
            ),
        ));
    }
    Ok((member.offset, element))
}

/// Replace the mesh of the compressed mesh shape in item `shape_item` of `image` with `built`: the mesh tree, the simd tree
/// and the shape's key bits, interior flags ([`BuiltMesh::interior_keys`]) and (where the shape has one, as Sekiro's and Elden Ring's collision does)
/// `triangleIndexToShapeKey`, rewritten with one entry per source triangle in the form the files store (the key left
/// aligned in 32 bits and the rest ones; `0xFFFFFFFF` for a triangle left out). The connectivity, the extern shapes, and
/// anything the shape holds of a game's own layer on top (Sekiro's `pParam`, a per-triangle table indexed by source
/// triangle) are not touched.
///
/// The layout comes from the image's type table: either SDK version's sections, and either simd node encoding. Refused:
/// an item that is not a compressed mesh shape or has no data, a type table that lacks a member the write needs, an
/// array whose elements are not the width this writer stores. Nothing is written until every check has passed.
pub fn replace_compressed_mesh(image: &mut TagfileImage, shape_item: usize, built: &BuiltMesh) -> Result<()> {
    let types = image.types.clone();
    let schema = Schema { types: &types };
    let shape_type = image
        .items
        .get(shape_item)
        .map(|i| i.type_index)
        .ok_or_else(|| invalid(0, format!("no item {shape_item}")))?;
    if !matches!(
        schema.name(shape_type),
        "fsnpCustomParamCompressedMeshShape" | "hknpCompressedMeshShape"
    ) {
        return Err(invalid(
            0,
            format!(
                "item {shape_item} is a {}, not a compressed mesh shape",
                schema.name(shape_type)
            ),
        ));
    }
    let data_item = image.reference_at(shape_item, schema.field(shape_type, "data")?.offset)?;
    if data_item == 0 {
        return Err(invalid(0, "the shape has no data"));
    }
    let data_type = image
        .items
        .get(data_item)
        .map(|i| i.type_index)
        .ok_or_else(|| invalid(0, format!("no item {data_item}")))?;

    // checks first: the element types
    let (nodes_at, node_type) = element_of(&schema, data_type, &["meshTree", "nodes"], 5)?;
    let (primitives_at, primitive_type) = element_of(&schema, data_type, &["meshTree", "primitives"], 4)?;
    let (index_at, index_type) = element_of(&schema, data_type, &["meshTree", "sharedVerticesIndex"], 2)?;
    let (packed_at, packed_type) = element_of(&schema, data_type, &["meshTree", "packedVertices"], 4)?;
    let (shared_at, shared_type) = element_of(&schema, data_type, &["meshTree", "sharedVertices"], 8)?;
    let sections_member = schema.path(data_type, &["meshTree", "sections"])?;
    let section_type = schema.subtype_of(sections_member.type_index)?;
    let section_size = schema.size_of(section_type)?;
    let section_nodes = schema.field(section_type, "nodes")?;
    let section_node_type = schema.subtype_of(section_nodes.type_index)?;
    if schema.size_of(section_node_type)? != 4 {
        return Err(invalid(0, "section tree nodes are not 4 bytes"));
    }
    let separate = schema.has_field(section_type, "firstPackedVertexIndex");
    let runs_member = schema.path(data_type, &["meshTree", "primitiveDataRuns"])?;
    let run_type = schema.subtype_of(runs_member.type_index)?;
    let run_size = schema.size_of(run_type)?;
    let simd_member = schema.path(data_type, &["simdTree", "nodes"])?;
    let simd_node_type = schema.subtype_of(simd_member.type_index)?;
    let encoding = SimdEncoding::of_node(&schema, simd_node_type);
    let simd = built.simd_tree(encoding)?;
    let simd_bytes = node_bytes(&schema, simd_node_type, &simd, encoding)?;

    // the section elements
    let writer = SectionWriter {
        schema: &schema,
        class: section_type,
        size: section_size,
        radius: built.convex_radius,
        separate,
    };
    let mut section_bytes = schema.zeroed(section_type, built.sections.len())?;
    for (i, s) in built.sections.iter().enumerate() {
        writer.write(&mut section_bytes, i, s)?;
    }
    // the data runs
    let mut run_bytes = schema.zeroed(run_type, built.data_runs.len())?;
    for (i, (value, index, count)) in built.data_runs.iter().enumerate() {
        let base = i
            .checked_mul(run_size)
            .ok_or_else(|| invalid(0, "run offset overflow"))?;
        let value_member = schema.field(run_type, "value")?;
        let value_at = base
            .checked_add(value_member.offset)
            .ok_or_else(|| invalid(0, "run offset overflow"))?;
        // 2016.2 wraps the 16-bit value in a one-member class
        let (value_at, value_width) = if schema.int_width(value_member.type_index).is_ok() {
            (value_at, schema.int_width(value_member.type_index)?)
        } else {
            let inner = schema.path(value_member.type_index, &["data"])?;
            (
                value_at
                    .checked_add(inner.offset)
                    .ok_or_else(|| invalid(0, "run offset overflow"))?,
                schema.int_width(inner.type_index)?,
            )
        };
        put_uint(&mut run_bytes, value_at, value_width, u64::from(*value))?;
        let index_at_run = base
            .checked_add(schema.field(run_type, "index")?.offset)
            .ok_or_else(|| invalid(0, "run offset overflow"))?;
        put_uint(&mut run_bytes, index_at_run, 1, u64::from(*index))?;
        let count_at = base
            .checked_add(schema.field(run_type, "count")?.offset)
            .ok_or_else(|| invalid(0, "run offset overflow"))?;
        put_uint(&mut run_bytes, count_at, 1, u64::from(*count))?;
    }
    // the little-endian element arrays
    let mut primitive_bytes = Vec::with_capacity(built.primitives.len().saturating_mul(4));
    for p in &built.primitives {
        primitive_bytes.extend_from_slice(p);
    }
    let mut index_bytes = Vec::new();
    for e in &built.shared_vertex_index {
        index_bytes.extend_from_slice(&e.to_le_bytes());
    }
    let mut packed_bytes = Vec::new();
    for c in &built.packed_vertices {
        packed_bytes.extend_from_slice(&c.to_le_bytes());
    }
    let mut shared_bytes = Vec::new();
    for c in &built.shared_vertices {
        shared_bytes.extend_from_slice(&c.to_le_bytes());
    }
    // the scalars of the tree and the shape
    let tree_scalar = |name: &str| -> Result<(usize, usize)> {
        let m = schema.path(data_type, &["meshTree", name])?;
        Ok((m.offset, schema.int_width(m.type_index)?))
    };
    let w = |x: u32| u64::from(x);
    let scalars = [
        ("numPrimitiveKeys", w(built.num_primitive_keys)),
        ("bitsPerKey", w(built.bits_per_key)),
        ("maxKeyValue", w(built.max_key_value)),
        ("primitiveStoresIsFlatConvex", 0),
    ];
    let mut scalar_writes: Vec<(usize, Vec<u8>)> = Vec::new();
    for (name, value) in scalars {
        let (at, width) = tree_scalar(name)?;
        let mut bytes = vec![0u8; width];
        put_uint(&mut bytes, 0, width, value)?;
        scalar_writes.push((at, bytes));
    }
    // the tree's domain
    for (name, corner, w_lane) in [
        ("min", built.domain[0], 1.0 - built.convex_radius),
        ("max", built.domain[1], 1.0 + built.convex_radius),
    ] {
        let m = schema.path(data_type, &["meshTree", "domain", name])?;
        let mut bytes = vec![0u8; 16];
        put_f32s(&mut bytes, 0, &[corner[0], corner[1], corner[2], w_lane])?;
        scalar_writes.push((m.offset, bytes));
    }
    // flags of the simd tree
    if let Ok(m) = schema.path(data_type, &["simdTree", "isCompact"]) {
        let width = schema.int_width(m.type_index)?;
        let mut bytes = vec![0u8; width];
        put_uint(&mut bytes, 0, width, 1)?;
        scalar_writes.push((m.offset, bytes));
    }
    if let Ok(m) = schema.field(data_type, "hasSimdTree") {
        let width = schema.int_width(m.type_index)?;
        let mut bytes = vec![0u8; width];
        put_uint(&mut bytes, 0, width, u64::from(!built.simd_leaves.is_empty()))?;
        scalar_writes.push((m.offset, bytes));
    }
    // the shape: key bits, interior flags and the triangle map
    let mut shape_writes: Vec<(usize, Vec<u8>)> = Vec::new();
    let bits_member = schema.field(shape_type, "numShapeKeyBits")?;
    let bits_width = schema.int_width(bits_member.type_index)?;
    let mut bytes = vec![0u8; bits_width];
    put_uint(&mut bytes, 0, bits_width, u64::from(built.bits_per_key))?;
    shape_writes.push((bits_member.offset, bytes));
    let num_bits = built.max_key_value.saturating_add(1);
    let interior_bits = schema.path(shape_type, &["triangleIsInterior", "storage", "numBits"])?;
    let bits_width = schema.int_width(interior_bits.type_index)?;
    let mut bytes = vec![0u8; bits_width];
    put_uint(&mut bytes, 0, bits_width, u64::from(num_bits))?;
    shape_writes.push((interior_bits.offset, bytes));
    let words_member = schema.path(shape_type, &["triangleIsInterior", "storage", "words"])?;
    let words_type = schema.subtype_of(words_member.type_index)?;
    if schema.size_of(words_type)? != 4 {
        return Err(invalid(0, "the interior flag words are not 4 bytes"));
    }
    let word_count =
        usize::try_from(num_bits.div_ceil(32)).map_err(|_| invalid(0, "too many interior flag words"))?;
    // the interior flags: one bit per shape key
    let mut interior_words = vec![0u32; word_count];
    for key in built.interior_keys()? {
        if let Some(word) = usize::try_from(key.wrapping_shr(5))
            .ok()
            .and_then(|w| interior_words.get_mut(w))
        {
            *word |= 1u32.checked_shl(key & 31).unwrap_or(0);
        }
    }
    let interior_bytes: Vec<u8> = interior_words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let map_member = schema.field(shape_type, "triangleIndexToShapeKey").ok();
    let mut map_bytes = Vec::new();
    if let Some(m) = map_member {
        if schema.size_of(schema.subtype_of(m.type_index)?)? != 4 {
            return Err(invalid(0, "triangleIndexToShapeKey does not hold 32-bit words"));
        }
        let fill = 32u32.saturating_sub(built.bits_per_key);
        for key in &built.triangle_keys {
            let word = match key {
                Some(k) => {
                    let low = 1u32.checked_shl(fill).unwrap_or(0).saturating_sub(1);
                    k.checked_shl(fill).unwrap_or(0) | low
                }
                None => u32::MAX,
            };
            map_bytes.extend_from_slice(&word.to_le_bytes());
        }
    }

    // everything checked: write
    for (at, bytes) in scalar_writes {
        image.patch(data_item, at, &bytes)?;
    }
    for (at, bytes) in shape_writes {
        image.patch(shape_item, at, &bytes)?;
    }
    let item_base = data_item;
    image.set_array(
        item_base,
        nodes_at,
        node_type,
        built.mesh_nodes.len().checked_div(5).unwrap_or(0),
        built.mesh_nodes.clone(),
    )?;
    image.set_array(
        item_base,
        primitives_at,
        primitive_type,
        built.primitives.len(),
        primitive_bytes,
    )?;
    image.set_array(
        item_base,
        index_at,
        index_type,
        built.shared_vertex_index.len(),
        index_bytes,
    )?;
    image.set_array(
        item_base,
        packed_at,
        packed_type,
        built.packed_vertices.len(),
        packed_bytes,
    )?;
    image.set_array(
        item_base,
        shared_at,
        shared_type,
        built.shared_vertices.len(),
        shared_bytes,
    )?;
    image.set_array(
        item_base,
        runs_member.offset,
        run_type,
        built.data_runs.len(),
        run_bytes,
    )?;
    image.set_array(
        item_base,
        simd_member.offset,
        simd_node_type,
        simd.len(),
        simd_bytes,
    )?;
    let sections_item = image.set_array(
        item_base,
        sections_member.offset,
        section_type,
        built.sections.len(),
        section_bytes,
    )?;
    for (i, s) in built.sections.iter().enumerate() {
        let node_item = image.push_item(ImageItem {
            type_index: section_node_type,
            kind: ItemKind::Run,
            count: s.nodes.len().checked_div(4).unwrap_or(0),
            data: s.nodes.clone(),
        });
        let at = i
            .checked_mul(section_size)
            .and_then(|b| b.checked_add(section_nodes.offset))
            .ok_or_else(|| invalid(0, "section offset overflow"))?;
        image.set_reference(sections_item, at, node_item)?;
    }
    // the interior flags and the triangle map
    image.set_array(
        shape_item,
        words_member.offset,
        words_type,
        word_count,
        interior_bytes,
    )?;
    if let Some(m) = map_member {
        let element = schema.subtype_of(m.type_index)?;
        image.set_array(
            shape_item,
            m.offset,
            element,
            built.triangle_keys.len(),
            map_bytes,
        )?;
    }
    Ok(())
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

    fn code(v: u32) -> Result<u64> {
        Ok(u64::from(v) + 1000)
    }

    #[test]
    fn a_section_keeps_to_one_page_by_padding_and_copying() {
        // pages of four entries
        let mut array = SharedArray::new(4);
        // section 0: vertices 10, 11, 12 on page 0
        let (page, at) = array.place(&[10, 11, 12], &code).unwrap();
        assert_eq!((page, at), (0, vec![0, 1, 2]));
        // section 1 shares 11 and 12 and has a new 13: all on page 0
        let (page, at) = array.place(&[11, 12, 13], &code).unwrap();
        assert_eq!((page, at), (0, vec![1, 2, 3]));
        assert_eq!(array.codes.len(), 4);
        // section 2 has two new vertices and the page is full: the next page, and 12 is copied onto it
        let (page, at) = array.place(&[12, 20, 21], &code).unwrap();
        assert_eq!((page, at), (1, vec![4, 5, 6]));
        assert_eq!(array.codes[4], 1012, "the copy of vertex 12");
        // section 3 shares 20 with section 2 and has one new vertex: still page 1 (one entry left)
        let (page, at) = array.place(&[20, 30], &code).unwrap();
        assert_eq!((page, at), (1, vec![5, 7]));
        // section 4 has two new vertices and the array is full: it starts page 2
        let (page, at) = array.place(&[40, 41], &code).unwrap();
        assert_eq!((page, at), (2, vec![8, 9]));
        // a section of more than a page is refused
        assert!(array.place(&[1, 2, 3, 4, 5], &code).is_err());
    }

    #[test]
    fn a_section_that_would_straddle_a_page_starts_the_next_one() {
        let mut array = SharedArray::new(4);
        array.place(&[1, 2, 3], &code).unwrap();
        // two new vertices after three used entries of a four-entry page: the array is padded with one filler entry
        let (page, at) = array.place(&[4, 5], &code).unwrap();
        assert_eq!((page, at), (1, vec![4, 5]));
        assert_eq!(array.codes.len(), 6);
        assert_eq!(array.codes[3], 0, "the filler");
    }

    /// A unit square of two triangles, worked out by hand: one section, four packed vertices in the order their
    /// triangles first use them, the codes of a codec over the vertex box, keys 0 and 2.
    #[test]
    fn a_unit_square_builds_as_worked_out_by_hand() {
        let vertices = [
            [0.0f32, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 1.0, 0.0],
        ];
        let triangles = [[0u32, 1, 2], [1, 3, 2]];
        let built = build_compressed_mesh(&MeshSource {
            vertices: &vertices,
            triangles: &triangles,
            convex_radius: 0.01,
        })
        .unwrap();
        assert_eq!(built.sections.len(), 1);
        let section = &built.sections[0];
        assert_eq!(
            (
                section.packed_vertices,
                section.shared_indices,
                section.primitives
            ),
            (4, 0, 2)
        );
        assert_eq!(
            (section.page, section.leaf_index, section.first_data_run),
            (0, 0, 0)
        );
        assert!(built.shared_vertices.is_empty() && built.shared_vertex_index.is_empty());
        // both triangles have the box of the whole square, so their surfaces and leaf counts tie and the input order stands; each triangle
        // names its vertices by their slots of first use
        assert_eq!(built.primitives, [[0, 1, 2, 2], [1, 3, 2, 2]]);
        // the codec spans the unit box on x and y and is flat on z: x in the low 11 bits, y in the next 11, z in the top 10
        assert_eq!(built.packed_vertices, [0, 2047, 2047 << 11, 2047 | 2047 << 11]);
        assert_eq!(section.codec[..3], [0.0, 0.0, 0.0]);
        assert!(
            (section.codec[3] - 1.0 / 2047.0).abs() < 1e-9 && (section.codec[4] - 1.0 / 2047.0).abs() < 1e-9
        );
        assert_eq!(section.codec[5], 0.0);
        // keys 0 and 2, so the largest key is 2 and the key width is the bit length of 3
        assert_eq!(built.triangle_keys, [Some(0), Some(2)]);
        assert_eq!(
            (built.num_primitive_keys, built.max_key_value, built.bits_per_key),
            (2, 2, 2)
        );
        // one run covers both primitives
        assert_eq!(built.data_runs, [(0xffff, 0, 2)]);
        // a section tree of three nodes (an inner node and two leaves), a mesh tree of one leaf
        assert_eq!(section.nodes.len(), 3 * 4);
        assert_eq!(built.mesh_nodes.len(), 5);
        // the domain is the vertex box grown by the radius, rounded outward
        assert!(built.domain[0].iter().all(|&x| x <= -0.01 && x > -0.0101));
        assert!(built.domain[1][0] >= 1.01 && built.domain[1][0] < 1.0101);
        // the simd tree: both primitives as leaves of one node, exact boxes
        assert_eq!(built.simd_leaves.len(), 2);
        assert_eq!(built.simd_leaves[0].key, 0);
        assert_eq!(built.simd_leaves[1].key, 2);
        let decoded = built.triangles().unwrap();
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].key, 0);
        assert_eq!(decoded[0].vertices[0], [0.0, 0.0, 0.0]);
        assert_eq!(decoded[1].key, 2);
    }

    /// The unit square again, with the triangles listed as `a b c` and `a c d` so that the diagonal is the third edge of the
    /// first and the first edge of the second: they become one quad primitive, the second half at the key with the half
    /// bit set. (The square of the test above lists them with the diagonal as the second edge of one and the third of the other: no quad.)
    #[test]
    fn a_square_listed_as_abc_acd_builds_as_one_quad() {
        let vertices = [
            [0.0f32, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 1.0, 0.0],
        ];
        let triangles = [[0u32, 1, 3], [0, 3, 2]];
        let source = MeshSource {
            vertices: &vertices,
            triangles: &triangles,
            convex_radius: 0.01,
        };
        let plain = build_compressed_mesh(&source).unwrap();
        assert_eq!(plain.primitives.len(), 2);
        let built = build_compressed_mesh_with(
            &source,
            &BuildOptions {
                primitives: Primitives::PairedQuads,
                layout: Layout::Sah,
                drop_degenerate: None,
            },
        )
        .unwrap();
        // slots by first use: vertex 0, 1, 3, then 2
        assert_eq!(built.primitives, [[0, 1, 2, 3]]);
        assert_eq!(built.sections[0].primitives, 1);
        assert_eq!(built.triangle_keys, [Some(0), Some(1)]);
        assert_eq!(
            (built.num_primitive_keys, built.max_key_value, built.bits_per_key),
            (1, 1, 2)
        );
        assert_eq!(built.data_runs, [(0xffff, 0, 1)]);
        assert_eq!(built.simd_leaves.len(), 1);
        assert_eq!(built.simd_leaves[0].key, 0);
        let decoded = built.triangles().unwrap();
        assert_eq!(decoded.len(), 2);
        assert_eq!((decoded[0].key, decoded[1].key), (0, 1));
        let near = |p: [f32; 3], q: [f32; 3]| p.iter().zip(&q).all(|(a, b)| (a - b).abs() < 1e-5);
        // a b c = vertices 0 1 3, and a c d = vertices 0 3 2
        assert!(near(decoded[0].vertices[1], vertices[1]) && near(decoded[0].vertices[2], vertices[3]));
        assert!(near(decoded[1].vertices[1], vertices[3]) && near(decoded[1].vertices[2], vertices[2]));
        // a lone square has an open boundary on every edge: no interior flag
        assert!(built.interior_keys().unwrap().is_empty());
    }

    #[test]
    fn a_vertex_is_stored_once_while_it_stays_on_the_page() {
        let mut array = SharedArray::new(100);
        array.place(&[1, 2, 3], &code).unwrap();
        let (page, at) = array.place(&[3, 2, 1], &code).unwrap();
        assert_eq!((page, at), (0, vec![2, 1, 0]));
        assert_eq!(array.codes.len(), 3);
    }

    #[test]
    fn narrowing_rounds_outward() {
        let x = 0.1f64;
        assert!(f64::from(down(x)) <= x && f64::from(up(x)) >= x);
        assert!(down(x) < up(x));
        // a value that is exactly an f32 stays
        assert_eq!(down(0.5), 0.5);
        assert_eq!(up(0.5), 0.5);
    }

    #[test]
    fn codes_round_and_clamp() {
        assert_eq!(code_of(0.4, 2047.0), 0);
        assert_eq!(code_of(0.6, 2047.0), 1);
        assert_eq!(code_of(-5.0, 2047.0), 0);
        assert_eq!(code_of(1e9, 2047.0), 2047);
        assert_eq!(code_of(f64::NAN, 2047.0), 0);
        // the packed codes of the corners of a codec box are the first and last codes
        let offset = [1.0f32, 2.0, 3.0];
        let scale = [0.5f32 / 2047.0, 0.5 / 2047.0, 0.5 / 1023.0];
        assert_eq!(pack32(offset, offset, scale), 0);
        assert_eq!(
            pack32([1.5, 2.5, 3.5], offset, scale),
            2047 | (2047 << 11) | (1023 << 22)
        );
        // a flat axis has scale 0 and code 0
        assert_eq!(
            pack32([9.0, 2.0, 3.0], offset, [0.0, 0.5 / 2047.0, 0.5 / 1023.0]),
            0
        );
        let min = [0.0f32; 3];
        let max = [2_097_151.0f32, 2_097_151.0, 4_194_303.0];
        assert_eq!(pack64([5.0, 6.0, 7.0], min, max), 5 | (6 << 21) | (7 << 42));
    }
}
