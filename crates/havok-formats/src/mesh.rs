use crate::bvtree::{BoundsCheck, decode_trees};
use crate::interior::{InteriorPrimitive, MeshView, SectionView, primitives_of};
use crate::simdtree::{LivePrimitive, SimdCheck, check_against, simd_tree};
use crate::{Result, Value, invalid};

/// One triangle of a compressed mesh.
#[derive(Debug, Clone, Copy)]
pub struct Triangle {
    /// Right-aligned shape key: section, primitive, and quad half.
    pub key: u32,
    /// Shape-local coordinates, in the coordinate frame the file stores.
    pub vertices: [[f32; 3]; 3],
}
/// The triangles of a compressed mesh tree, and the primitives that are not triangles.
#[derive(Debug, Default)]
pub struct CompressedMesh {
    /// Every triangle; a quad is two.
    pub triangles: Vec<Triangle>,
    /// How many primitives are the `DEAD` sentinel (`[0xde, 0xad, 0xde, 0xad]`), which hold nothing.
    pub dead_primitives: usize,
    /// Primitives that are not triangles or quads, in key order.
    pub custom: Vec<CustomPrimitive>,
    /// What decoding the mesh's bounding-volume trees found, and how its triangles and quads sit in their leaves' boxes.
    pub bounds: BoundsCheck,
    /// What testing the mesh's simd tree against its primitives found ([`simd_tree`]).
    pub simd: SimdCheck,
}

/// A custom primitive of a mesh tree: one whose second and fourth indices are equal (`[k, x, n, x]`; no triangle or
/// quad in the Sekiro or Elden Ring collision read has them equal). Instead of vertices, `k` names where its data starts in the
/// shared-vertex index (counting the section's packed vertices first, as a vertex index does).
#[derive(Debug, Clone, PartialEq)]
pub struct CustomPrimitive {
    /// Right-aligned shape key: section and primitive, quad half zero.
    pub key: u32,
    /// The primitive's four stored indices.
    pub indices: [usize; 4],
    /// What the data is.
    pub kind: CustomKind,
    /// The decoded points of a primitive whose first word has the low byte 0x52 ([`CustomKind::Unread`]); `None` for
    /// the others.
    pub points: Option<PointSet>,
}

/// The point set of a custom primitive whose record starts `[header, slot, tag]` with the low byte of `header` 0x52: the
/// header's high byte is the number of points, `slot` the first shared vertex that holds them, `tag` a word whose meaning
/// is not known. The points are 11/11/10-bit codes, two to a shared vertex (the even point in its high 32 bits), over the
/// box of the primitive's leaf in the section's tree. They are the corners of a convex solid (cylinders, boxes and
/// prisms in the Sekiro and Elden Ring objects and assets that have them); the stored data does not say it is a hull.
#[derive(Debug, Clone, PartialEq)]
pub struct PointSet {
    /// The points, in the order stored (shape-local coordinates, like a triangle's).
    pub vertices: Vec<[f32; 3]>,
    /// The record's third word: `0x3C24` in every Elden Ring record, `0x3D4D`, `0x3C9D`, `0x3CC7` or `0x3C8B` in Sekiro's.
    pub tag: u32,
}

/// What a custom primitive's data is, by its first word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CustomKind {
    /// `[146, w, index low, index high]` with indices `[k, k, 4, k]`: the extern shape at `index` of the shape's
    /// `externShapes` (the second word is not read). Elden Ring's asset collision.
    Extern(usize),
    /// Records whose first word's low byte is 0x52 (`[0x2252, 8, 0x3d4d]`: the point count in the high byte, the first
    /// shared vertex of the points and a tag; in both games): a convex point set, decoded into
    /// [`CustomPrimitive::points`]. The variant keeps the name it had when the records were not read, so that readers
    /// that tell them from extern shape references by it still do: no reader acts on the points yet.
    Unread,
}

/// One extern shape instance of a compressed mesh shape (`externShapes`): a shape the mesh holds whole, placed by its
/// own transform (columns: rotation x, y and z, then the translation) in the mesh's space. Its scale is one: a scaled
/// instance is refused.
#[derive(Debug, Clone, Copy)]
pub struct ShapeInstance<'t, 'a> {
    /// Four columns of three floats: rotation x, y and z, then translation.
    pub transform: [[f32; 3]; 4],
    /// The placed shape.
    pub shape: Value<'t, 'a>,
}

/// How many shared vertices a page of a section's 16-bit shared-vertex index entries addresses.
const PAGE_SIZE: usize = 1 << 16;

/// The first word of a custom primitive that references an extern shape.
const EXTERN_SHAPE_TAG: u32 = 146;

/// The low byte of the first word of a point-set record: the record type 2 in the low nibble, 32-bit point codes in
/// bits 4 to 5 and one tag word in bits 6 to 7.
const POINT_SET_HEADER: u32 = 0x52;

/// The 11/11/10-bit code range of a point on each axis.
const POINT_CODES: [f64; 3] = [2047.0, 2047.0, 1023.0];

/// A value narrowed to the `f32` the file's codecs compute in.
#[allow(
    clippy::cast_possible_truncation,
    reason = "the codecs decode in f32, so the box is rounded to it"
)]
fn narrow(x: f64) -> f32 {
    x as f32
}

/// Decode the point set of the record `[header, slot, tag]` of a custom primitive in a section of page `page`, from the
/// shared vertices `shared`, under the box `leaf` of the primitive's leaf in the section's tree. Refused: a record with
/// no points, or whose points run past the shared vertices.
fn point_set(
    at: usize,
    header: u32,
    slot: u32,
    tag: u32,
    page: usize,
    shared: crate::Array<'_, '_>,
    leaf: &[[f64; 3]; 2],
) -> Result<PointSet> {
    let count =
        usize::try_from((header >> 8) & 0xff).map_err(|_| invalid(at, "point count exceeds usize"))?;
    if count == 0 {
        return Err(invalid(at, "a point set without points"));
    }
    let first = usize::try_from(slot)
        .ok()
        .and_then(|slot| {
            page.checked_mul(PAGE_SIZE)
                .and_then(|base| base.checked_add(slot))
        })
        .ok_or_else(|| invalid(at, "point set slot overflow"))?;
    let [low, high] = *leaf;
    let mut scale = [0.0f32; 3];
    let mut offset = [0.0f32; 3];
    for axis in 0..3 {
        let (min, max, codes) = (
            low.get(axis).copied().unwrap_or(0.0),
            high.get(axis).copied().unwrap_or(0.0),
            POINT_CODES.get(axis).copied().unwrap_or(1.0),
        );
        if let (Some(s), Some(o)) = (scale.get_mut(axis), offset.get_mut(axis)) {
            *s = narrow((max - min) / codes);
            *o = narrow(min);
        }
    }
    let mut vertices = Vec::with_capacity(count);
    for point in 0..count {
        let entry = first
            .checked_add(point >> 1)
            .ok_or_else(|| invalid(at, "point set slot overflow"))?;
        let stored = shared.get(entry)?.unsigned()?;
        // two codes to a shared vertex, the even point in the high half
        let code = if point & 1 == 0 {
            stored >> 32
        } else {
            stored & 0xffff_ffff
        };
        let code = u32::try_from(code).map_err(|_| invalid(at, "a point code wider than 32 bits"))?;
        vertices.push(
            primase_foundations::vertex::packed_position32(code, scale, offset)
                .map_err(|e| invalid(at, e.to_string()))?,
        );
    }
    Ok(PointSet { vertices, tag })
}

/// The extern shape instances of an hknpCompressedMeshShape (`externShapes`), each with its transform; each one is
/// referenced by exactly one of its mesh's extern custom primitives, and they reference no other. A class without the
/// field (Havok 2016's) holds none.
pub fn extern_shapes<'t, 'a>(
    shape: Value<'t, 'a>,
    mesh: &CompressedMesh,
) -> Result<Vec<ShapeInstance<'t, 'a>>> {
    if !shape.has_field("externShapes") {
        if mesh
            .custom
            .iter()
            .any(|c| matches!(c.kind, CustomKind::Extern(_)))
        {
            return Err(invalid(
                shape.at(),
                "extern shape references in a shape without extern shapes",
            ));
        }
        return Ok(Vec::new());
    }
    let stored = shape.field("externShapes")?.array()?;
    let mut instances = Vec::with_capacity(stored.len());
    for i in 0..stored.len() {
        let instance = stored.get(i)?;
        let t = instance.field("transform")?.vector::<16>()?;
        let scale = instance.field("scale")?.vector::<4>()?;
        if scale[..3] != [1., 1., 1.] {
            return Err(invalid(instance.at(), "a scaled extern shape instance"));
        }
        // four columns of `x y z` and a padding lane
        let mut transform = [[0.0f32; 3]; 4];
        for (column, stored) in transform.iter_mut().zip(t.as_chunks::<4>().0) {
            let [x, y, z, _] = *stored;
            *column = [x, y, z];
        }
        if transform.iter().flatten().any(|x| !x.is_finite()) {
            return Err(invalid(instance.at(), "nonfinite extern shape transform"));
        }
        instances.push(ShapeInstance {
            transform,
            shape: instance.field("shape")?.dereference()?,
        });
    }
    let mut referenced = vec![false; instances.len()];
    for custom in &mesh.custom {
        let CustomKind::Extern(index) = custom.kind else {
            continue;
        };
        match referenced.get_mut(index) {
            Some(seen @ false) => *seen = true,
            Some(true) => {
                return Err(invalid(
                    shape.at(),
                    format!("extern shape {index} referenced twice"),
                ));
            }
            None => {
                return Err(invalid(
                    shape.at(),
                    format!(
                        "custom primitive references extern shape {index} of {}",
                        instances.len()
                    ),
                ));
            }
        }
    }
    if referenced.contains(&false) {
        return Err(invalid(
            shape.at(),
            "an extern shape no custom primitive references",
        ));
    }
    Ok(instances)
}

/// How a mesh-tree section names its vertex and primitive ranges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SectionLayout {
    /// 2016.2: `firstPackedVertex`, `numPackedVertices` and `numSharedIndices`, and `sharedVertices.data` /
    /// `primitives.data` each packing `(first << 8) | count`; the count in `sharedVertices.data` is the packed-vertex
    /// count again.
    Packed,
    /// 2018.1: separate `firstPackedVertexIndex`, `numPackedVertices`, `firstSharedVertexIndex`, `firstPrimitiveIndex`
    /// and `numPrimitives` fields; no shared-index count (a section's run ends where the next one's starts).
    Fields,
}
/// The section layout of each supported SDK version.
const SECTION_LAYOUTS: [([u8; 8], SectionLayout); 2] = [
    (*b"20160200", SectionLayout::Packed),
    (*b"20180100", SectionLayout::Fields),
];
/// A mesh-tree section's `flags` byte, a bit set. Both installs store only 0 and 1 (Sekiro: 273,399 sections 0 and 10
/// sections 1; Elden Ring: 705,641 and 157, counted over every distinct tagfile), and the bit is set on exactly the
/// sections that hold a custom primitive ([`CustomPrimitive`], extern shape references and point sets alike): all 167
/// flagged sections hold one, and none of the 979,040 others does.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct SectionFlags(pub u8);

impl SectionFlags {
    /// Bit 0 (value 1): the section needs its bounding-volume tree to be read. The name is that of
    /// `hkcdStaticMeshTree::Section::Flags::SF_REQUIRE_TREE` in HKX2Library's (MIT) reflected classes; the data agrees with
    /// it where a custom primitive is decoded from the box of its own leaf in the section's tree.
    pub const REQUIRE_TREE: u8 = 1;
    /// The union of the bits any section of either install stores.
    pub const SEEN: u8 = 1;

    /// Whether every bit of `mask` is set.
    pub fn contains(self, mask: u8) -> bool {
        self.0 & mask == mask
    }
    /// The bits no section of either install stores, as stored.
    pub fn unseen(self) -> u8 {
        self.0 & !Self::SEEN
    }
}

/// One section of a compressed mesh tree as stored, whichever SDK version stored it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshSection {
    /// The dequantisation offset (xyz) and scale (xyz) of the section's packed vertices.
    pub codec: [f32; 6],
    /// Index of the section's first packed vertex in the tree's packed-vertex array.
    pub first_packed_vertex: usize,
    /// How many packed vertices the section has.
    pub packed_vertices: usize,
    /// The section's first entry of the shared-vertex index.
    pub first_shared_vertex: usize,
    /// The section's page: its shared-vertex index entries are 16 bits, and the shared vertex one names is
    /// `sharedVertices[page * 65536 + entry]`. Zero in every section but some of the `l` collision sets' (pages 1 to
    /// 4, where a tree has more than 65,536 shared vertices).
    pub page: usize,
    /// How many entries of the shared-vertex index the section has: vertex indices from `packed_vertices` on reach
    /// them.
    pub shared_indices: usize,
    /// Index of the section's first primitive in the tree's primitive array.
    pub first_primitive: usize,
    /// How many primitives the section has.
    pub primitives: usize,
    /// The section's `domain` box, minimum then maximum (xyz): the box of its triangle vertices grown by the shape's
    /// convex radius in the Sekiro and Elden Ring map meshes.
    pub domain: [[f32; 3]; 2],
    /// The node of the section's leaf in the mesh tree (`leafIndex`): the node index, not the leaf's ordinal among the
    /// leaves. [`mesh_trees`](crate::mesh_trees) checks it against the decoded mesh tree.
    pub leaf_index: usize,
    /// The section's `flags` byte. A section with bits outside [`SectionFlags::SEEN`] is refused.
    pub flags: SectionFlags,
    /// The section's `layerData` byte: 0 in all 979,207 sections of Sekiro (273,409) and Elden Ring (705,798) counted over
    /// every distinct tagfile, and a section that stores anything else is refused.
    pub layer_data: u8,
}
/// An unsigned stored value as an index; a refusal at `at` when it does not fit the platform's `usize`.
fn index(value: Value<'_, '_>, at: usize) -> Result<usize> {
    let stored = value.unsigned()?;
    usize::try_from(stored)
        .map_err(|_| invalid(at, format!("expected an index that fits usize, found {stored}")))
}

/// The `min` and `max` (xyz) of an `hkAabb` value.
pub(crate) fn aabb(v: Value<'_, '_>) -> Result<[[f32; 3]; 2]> {
    let min = v.field("min")?.vector::<4>()?;
    let max = v.field("max")?.vector::<4>()?;
    Ok([[min[0], min[1], min[2]], [max[0], max[1], max[2]]])
}

/// The element array `name` of a mesh tree, refused unless its elements are `bytes` wide. The decoders read an element
/// as the one integer width the Sekiro and Elden Ring trees store (`packedVertices` 4 bytes, `sharedVertices` 8,
/// `sharedVerticesIndex` 2, which the section page arithmetic relies on); a tree with another width (a different
/// `hkcdStaticMeshTree` configuration) would decode to wrong vertices without a sign.
fn array_of_width<'t, 'a>(tree: Value<'t, 'a>, name: &str, bytes: usize) -> Result<crate::Array<'t, 'a>> {
    let field = tree.field(name)?;
    let width = field.element_size()?;
    if width != bytes {
        return Err(invalid(
            field.at(),
            format!("{name} has elements of {width} bytes, not the {bytes} this reader decodes"),
        ));
    }
    field.array()
}

/// The sections of an hknpCompressedMeshShapeData mesh tree, as its SDK version names their ranges.
///
/// The sections' ranges tile the tree's arrays: each section's packed vertices, shared-vertex index entries and
/// primitives start where the previous section's end (the first at 0), and the last ends at the array's end. In 2016.2
/// the count packed into `sharedVertices.data` equals `numPackedVertices`. Both hold in every compressed mesh of the
/// Sekiro and Elden Ring installs' collision (273,228 and 722,403 sections); a tree that breaks either is refused.
pub fn mesh_sections(data: Value<'_, '_>) -> Result<Vec<MeshSection>> {
    let layout = SECTION_LAYOUTS
        .iter()
        .find(|(version, _)| *version == data.tag.version)
        .map(|(_, layout)| *layout)
        .ok_or_else(|| invalid(data.at(), "no mesh section layout for this SDK version"))?;
    let tree = data.field("meshTree")?;
    let sections = tree.field("sections")?.array()?;
    let shared_index_len = tree.field("sharedVerticesIndex")?.array()?.len();
    let mut out: Vec<MeshSection> = Vec::with_capacity(sections.len());
    // where the next section's ranges must start
    let (mut packed_at, mut shared_at, mut primitive_at) = (0usize, 0usize, 0usize);
    for i in 0..sections.len() {
        let section = sections.get(i)?;
        let field = |name: &str| index(section.field(name)?, section.at());
        let (
            first_packed_vertex,
            packed_vertices,
            first_shared_vertex,
            shared_indices,
            first_primitive,
            primitives,
        ) = match layout {
            SectionLayout::Packed => {
                let shared = index(section.field("sharedVertices")?.field("data")?, section.at())?;
                let range = index(section.field("primitives")?.field("data")?, section.at())?;
                let packed_vertices = field("numPackedVertices")?;
                if shared & 255 != packed_vertices {
                    return Err(invalid(
                        section.at(),
                        format!(
                            "section {i}: the count in sharedVertices.data is {}, numPackedVertices {packed_vertices}",
                            shared & 255
                        ),
                    ));
                }
                (
                    field("firstPackedVertex")?,
                    packed_vertices,
                    shared >> 8,
                    field("numSharedIndices")?,
                    range >> 8,
                    range & 255,
                )
            }
            SectionLayout::Fields => {
                let first_shared = field("firstSharedVertexIndex")?;
                let end = if i.saturating_add(1) < sections.len() {
                    let next = sections.get(i.saturating_add(1))?;
                    index(next.field("firstSharedVertexIndex")?, next.at())?
                } else {
                    shared_index_len
                };
                let shared_indices = end.checked_sub(first_shared).ok_or_else(|| {
                        invalid(
                            section.at(),
                            format!("section {i}: its shared-vertex index entries start at {first_shared}, past the next section's {end}"),
                        )
                    })?;
                (
                    field("firstPackedVertexIndex")?,
                    field("numPackedVertices")?,
                    first_shared,
                    shared_indices,
                    field("firstPrimitiveIndex")?,
                    field("numPrimitives")?,
                )
            }
        };
        for (what, first, want) in [
            ("packed vertex", first_packed_vertex, packed_at),
            ("shared-vertex index entry", first_shared_vertex, shared_at),
            ("primitive", first_primitive, primitive_at),
        ] {
            if first != want {
                return Err(invalid(
                    section.at(),
                    format!(
                        "section {i}: its first {what} is {first}, not {want} where the earlier sections end"
                    ),
                ));
            }
        }
        let overflow = || invalid(section.at(), "section range overflow");
        packed_at = packed_at.checked_add(packed_vertices).ok_or_else(overflow)?;
        shared_at = shared_at.checked_add(shared_indices).ok_or_else(overflow)?;
        primitive_at = primitive_at.checked_add(primitives).ok_or_else(overflow)?;
        let flags = u8::try_from(field("flags")?)
            .ok()
            .map(SectionFlags)
            .filter(|f| f.unseen() == 0)
            .ok_or_else(|| {
                invalid(
                    section.at(),
                    format!(
                        "section {i}: flags are not a bit set within {:#x}, the bits the sections of both installs use",
                        SectionFlags::SEEN
                    ),
                )
            })?;
        let layer_data = u8::try_from(field("layerData")?)
            .ok()
            .filter(|&b| b == 0)
            .ok_or_else(|| {
                invalid(
                    section.at(),
                    format!("section {i}: layerData is not 0, as in every section of both installs"),
                )
            })?;
        out.push(MeshSection {
            codec: section.field("codecParms")?.vector::<6>()?,
            first_packed_vertex,
            packed_vertices,
            first_shared_vertex,
            page: field("page")?,
            shared_indices,
            first_primitive,
            primitives,
            domain: aabb(section.field("domain")?)?,
            leaf_index: field("leafIndex")?,
            flags,
            layer_data,
        });
    }
    for (what, end, len) in [
        (
            "packedVertices",
            packed_at,
            tree.field("packedVertices")?.array()?.len(),
        ),
        ("sharedVerticesIndex", shared_at, shared_index_len),
        (
            "primitives",
            primitive_at,
            tree.field("primitives")?.array()?.len(),
        ),
    ] {
        if end != len {
            return Err(invalid(
                sections.get(0).map_or(data.at(), |s| s.at()),
                format!("the sections cover {end} entries of {what}, which has {len}"),
            ));
        }
    }
    Ok(out)
}

/// Decode an hknpCompressedMeshShapeData mesh tree. Materials and rigid-body
/// transforms belong to the caller. Packed section vertices are 11/11/10 bits;
/// shared vertices are 21/21/22 bits over the declared domain, and a section's shared-vertex index entries address
/// them from its [`MeshSection::page`] times 65,536 on. Custom primitives
/// are listed ([`CustomPrimitive`]); the point set of one is decoded ([`PointSet`]), an extern shape reference is not
/// followed (see [`extern_shapes`]).
///
/// The tree must store its vertex arrays in the one configuration the Sekiro and Elden Ring collision uses (4-byte
/// `packedVertices`, 8-byte `sharedVertices`, 16-bit `sharedVerticesIndex`): another width is refused.
pub fn compressed_mesh(data: Value<'_, '_>) -> Result<CompressedMesh> {
    let tree = data.field("meshTree")?;
    let [min, max] = aabb(tree.field("domain")?)?;
    let primitives = tree.field("primitives")?.array()?;
    let packed = array_of_width(tree, "packedVertices", 4)?;
    let shared = array_of_width(tree, "sharedVertices", 8)?;
    let shared_indices = array_of_width(tree, "sharedVerticesIndex", 2)?;
    let sections = tree.field("sections")?.array()?;
    let ranges = mesh_sections(data)?;
    let trees = decode_trees(data, &ranges)?;
    let mut result = CompressedMesh {
        bounds: BoundsCheck::of_trees(&trees),
        ..CompressedMesh::default()
    };
    // the triangles, quads and custom primitives the simd tree must name
    let mut live: Vec<LivePrimitive> = Vec::new();
    for (section_index, range) in ranges.iter().enumerate() {
        let section = sections.get(section_index)?;
        let &MeshSection {
            codec,
            first_packed_vertex: first,
            packed_vertices: local_count,
            first_shared_vertex: shared_base,
            page,
            shared_indices: shared_count,
            first_primitive: start,
            primitives: count,
            domain: _,
            leaf_index: _,
            flags: _,
            layer_data: _,
        } = range;
        let offset = [codec[0], codec[1], codec[2]];
        let scale = [codec[3], codec[4], codec[5]];
        // The primitive occupies seven key bits; its quad half occupies one.
        if count > 128 || section_index > 0x00ff_ffff {
            return Err(invalid(section.at(), "shape key range overflow"));
        }
        let vertex = |index: usize| -> Result<[f32; 3]> {
            if index >= local_count.saturating_add(shared_count) {
                return Err(invalid(
                    section.at(),
                    format!(
                        "vertex index {index} is past the section's {local_count} packed vertices and {shared_count} shared ones"
                    ),
                ));
            }
            if index < local_count {
                let index = first
                    .checked_add(index)
                    .ok_or_else(|| invalid(section.at(), "packed vertex index overflow"))?;
                let value = u32::try_from(packed.get(index)?.unsigned()?)
                    .map_err(|_| invalid(section.at(), "packed vertex exceeds u32"))?;
                primase_foundations::vertex::packed_position32(value, scale, offset)
                    .map_err(|e| invalid(section.at(), e.to_string()))
            } else {
                let position = index
                    .checked_sub(local_count)
                    .and_then(|i| i.checked_add(shared_base))
                    .ok_or_else(|| invalid(section.at(), "shared vertex index overflow"))?;
                let entry = self::index(shared_indices.get(position)?, section.at())?;
                let position = page
                    .checked_mul(PAGE_SIZE)
                    .and_then(|base| base.checked_add(entry))
                    .ok_or_else(|| invalid(section.at(), "shared vertex page overflow"))?;
                primase_foundations::vertex::packed_position64(shared.get(position)?.unsigned()?, min, max)
                    .map_err(|e| invalid(section.at(), e.to_string()))
            }
        };
        for i in 0..count {
            let primitive = start
                .checked_add(i)
                .ok_or_else(|| invalid(section.at(), "primitive index overflow"))?;
            let indices = primitives.get(primitive)?.field("indices")?.array()?;
            if indices.len() != 4 {
                return Err(invalid(section.at(), "primitive must have four indices"));
            }
            let mut ix = [0usize; 4];
            for (j, x) in ix.iter_mut().enumerate() {
                *x = index(indices.get(j)?, section.at())?;
            }
            if ix == [0xde, 0xad, 0xde, 0xad] {
                result.dead_primitives = result.dead_primitives.saturating_add(1);
                continue;
            }
            // the section number above the quad half and the primitive's seven bits (bounded by the check above)
            let key = u32::try_from(section_index)
                .ok()
                .and_then(|section| section.checked_mul(256))
                .zip(
                    u32::try_from(i)
                        .ok()
                        .and_then(|primitive| primitive.checked_mul(2)),
                )
                .map(|(section, primitive)| section | primitive)
                .ok_or_else(|| invalid(section.at(), "shape key range overflow"))?;
            if ix[1] == ix[3] {
                // a custom primitive `[k, x, n, x]`: its data from vertex index k on in the shared-vertex index
                let k = ix[0];
                if k >= local_count.saturating_add(shared_count) {
                    return Err(invalid(
                        section.at(),
                        format!(
                            "custom primitive {ix:?} starts past the section's shared-vertex index entries"
                        ),
                    ));
                }
                let start = k
                    .checked_sub(local_count)
                    .and_then(|o| o.checked_add(shared_base))
                    .ok_or_else(|| {
                        invalid(
                            section.at(),
                            format!("custom primitive {ix:?} names a packed vertex"),
                        )
                    })?;
                let word = |j: usize| -> Result<u32> {
                    let at = start
                        .checked_add(j)
                        .ok_or_else(|| invalid(section.at(), "custom primitive word index overflow"))?;
                    u32::try_from(shared_indices.get(at)?.unsigned()?)
                        .map_err(|_| invalid(section.at(), "custom primitive word exceeds u32"))
                };
                let header = word(0)?;
                let kind = match header {
                    EXTERN_SHAPE_TAG if ix == [k, k, 4, k] => {
                        let (low, high) = (word(2)?, word(3)?);
                        if low > 0xffff || high > 0xffff {
                            return Err(invalid(section.at(), "extern shape index exceeds 32 bits"));
                        }
                        CustomKind::Extern((low | (high << 16)) as usize)
                    }
                    first if first != EXTERN_SHAPE_TAG && first & 0xff == POINT_SET_HEADER => {
                        CustomKind::Unread
                    }
                    first => {
                        return Err(invalid(
                            section.at(),
                            format!("custom primitive {ix:?} of an unknown kind (first word {first})"),
                        ));
                    }
                };
                let points = if kind == CustomKind::Unread {
                    // the points are coded over the box of the primitive's own leaf in the section's tree
                    let leaf = trees
                        .sections
                        .get(section_index)
                        .and_then(|tree| tree.leaf(i))
                        .ok_or_else(|| {
                            invalid(section.at(), "a point set without a leaf in its section's tree")
                        })?;
                    Some(point_set(
                        section.at(),
                        header,
                        word(1)?,
                        word(2)?,
                        page,
                        shared,
                        &leaf.aabb,
                    )?)
                } else {
                    None
                };
                result.custom.push(CustomPrimitive {
                    key,
                    indices: ix,
                    kind,
                    points,
                });
                live.push(LivePrimitive { key, vertices: None });
                continue;
            }
            let a = vertex(ix[0])?;
            let b = vertex(ix[1])?;
            let c = vertex(ix[2])?;
            let d = if ix[2] != ix[3] {
                Some(vertex(ix[3])?)
            } else {
                None
            };
            // the primitive lies inside the box of its leaf in the section's tree
            let tree = trees
                .sections
                .get(section_index)
                .ok_or_else(|| invalid(section.at(), "a section without a tree"))?;
            let mut excess = 0.0f64;
            for v in [Some(a), Some(b), Some(c), d].into_iter().flatten() {
                let ulps = tree.excess_ulps(i, v).ok_or_else(|| {
                    invalid(section.at(), "a primitive without a leaf in its section's tree")
                })?;
                excess = excess.max(ulps);
            }
            result.bounds.record(excess);
            let (mut low, mut high) = (a, a);
            for v in [b, c].into_iter().chain(d) {
                for ((low, high), v) in low.iter_mut().zip(high.iter_mut()).zip(v) {
                    *low = low.min(v);
                    *high = high.max(v);
                }
            }
            live.push(LivePrimitive {
                key,
                vertices: Some(([low, high], range.domain.map(|corner| corner.map(f64::from)))),
            });
            result.triangles.push(Triangle {
                key,
                vertices: [a, b, c],
            });
            if let Some(d) = d {
                result.triangles.push(Triangle {
                    key: key | 1,
                    vertices: [a, c, d],
                });
            }
        }
    }
    check_section_flags(&ranges, &result.custom, &sections)?;
    result.simd = check_against(&simd_tree(data)?, &mut live, data)?;
    Ok(result)
}

/// The triangles and quads of a compressed mesh as the pass that sets the interior flags reads them
/// ([`interior_keys`](crate::interior_keys)): each primitive with the identity and the position of its four vertices, the
/// identity being what the stock tool names a vertex by (a packed vertex by its section and slot, a shared vertex by its
/// entry in the shared-vertex index, whatever page it is on) and the positions what the codes decode to in single
/// precision (`code * scale + offset`). Custom and removed primitives have no keys and are left out. The tree must store
/// its vertex arrays in the configuration [`compressed_mesh`] reads.
pub fn interior_primitives(data: Value<'_, '_>) -> Result<Vec<InteriorPrimitive>> {
    let tree = data.field("meshTree")?;
    let domain = aabb(tree.field("domain")?)?;
    let primitives = tree.field("primitives")?.array()?;
    let packed = array_of_width(tree, "packedVertices", 4)?;
    let shared = array_of_width(tree, "sharedVertices", 8)?;
    let shared_indices = array_of_width(tree, "sharedVerticesIndex", 2)?;
    let ranges = mesh_sections(data)?;
    let at = tree.at();
    let mut slots: Vec<[u8; 4]> = Vec::with_capacity(primitives.len());
    for i in 0..primitives.len() {
        let indices = primitives.get(i)?.field("indices")?.array()?;
        if indices.len() != 4 {
            return Err(invalid(at, "primitive must have four indices"));
        }
        let mut four = [0u8; 4];
        for (j, slot) in four.iter_mut().enumerate() {
            *slot = u8::try_from(index(indices.get(j)?, at)?)
                .map_err(|_| invalid(at, "a primitive names a vertex slot past a byte"))?;
        }
        slots.push(four);
    }
    let mut packed_codes: Vec<u32> = Vec::with_capacity(packed.len());
    for i in 0..packed.len() {
        packed_codes.push(
            u32::try_from(packed.get(i)?.unsigned()?)
                .map_err(|_| invalid(at, "packed vertex exceeds u32"))?,
        );
    }
    let mut shared_codes: Vec<u64> = Vec::with_capacity(shared.len());
    for i in 0..shared.len() {
        shared_codes.push(shared.get(i)?.unsigned()?);
    }
    let mut entries: Vec<u16> = Vec::with_capacity(shared_indices.len());
    for i in 0..shared_indices.len() {
        entries.push(
            u16::try_from(shared_indices.get(i)?.unsigned()?)
                .map_err(|_| invalid(at, "shared-vertex index entry exceeds 16 bits"))?,
        );
    }
    let sections: Vec<SectionView> = ranges
        .iter()
        .map(|r| SectionView {
            codec: r.codec,
            first_packed_vertex: r.first_packed_vertex,
            packed_vertices: r.packed_vertices,
            first_shared_index: r.first_shared_vertex,
            page: r.page,
            first_primitive: r.first_primitive,
            primitives: r.primitives,
        })
        .collect();
    let flat_mask = tree
        .field("primitiveStoresIsFlatConvex")
        .ok()
        .and_then(|v| v.unsigned().ok())
        .and_then(|v| u8::try_from(v).ok())
        .unwrap_or(0);
    let view = MeshView {
        domain,
        sections: &sections,
        primitives: &slots,
        packed_vertices: &packed_codes,
        shared_vertices: &shared_codes,
        shared_index: &entries,
        flat_mask,
    };
    primitives_of(&view, false)
}

/// Each section's [`SectionFlags::REQUIRE_TREE`] bit must be set exactly when the section holds a custom primitive: so it
/// is on all 167 flagged sections of the two installs and none of the other 979,040. `sections` are the tree's section
/// values, where a refusal is placed.
fn check_section_flags(
    ranges: &[MeshSection],
    custom: &[CustomPrimitive],
    sections: &crate::Array<'_, '_>,
) -> Result<()> {
    let mut holds = vec![false; ranges.len()];
    for c in custom {
        if let Some(slot) = usize::try_from(c.key >> 8).ok().and_then(|i| holds.get_mut(i)) {
            *slot = true;
        }
    }
    for (i, (range, &has_custom)) in ranges.iter().zip(&holds).enumerate() {
        let flagged = range.flags.contains(SectionFlags::REQUIRE_TREE);
        if flagged != has_custom {
            let detail = if flagged {
                format!("section {i} is flagged as needing its tree but holds no custom primitive")
            } else {
                format!("section {i} holds a custom primitive but is not flagged as needing its tree")
            };
            return Err(invalid(sections.get(i)?.at(), detail));
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::disallowed_methods,
    reason = "a test reads the corpus's files; the library itself takes bytes"
)]
mod tests {
    use crate::inputs::{Game, OracleSet, Query, expect_found, resolve};
    use crate::{Compendium, SectionFlags, Tagfile, compressed_mesh, mesh_sections};

    /// The tagfile, with its own types or the first compendium that has them.
    fn read<'a>(bytes: &'a [u8], compendia: &[Compendium]) -> Tagfile<'a> {
        std::iter::once(None)
            .chain(compendia.iter().map(Some))
            .find_map(|c| Tagfile::read(bytes, c).ok())
            .unwrap()
    }

    /// A real mesh tree whose section counts no longer tile its arrays is refused: one of the corpus's trees of two or
    /// more sections, with the packed-vertex count of section 0 and then the first packed vertex of section 1 moved
    /// by one. Skips without the Sekiro corpus.
    #[test]
    fn sections_that_do_not_tile_the_arrays_are_refused() {
        let corpus = Query::Oracle(OracleSet::Corpus, Game::Sekiro, "havok");
        let Some(dir) = resolve(corpus) else {
            return;
        };
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        files.sort();
        let with = |ext: &str| {
            files
                .iter()
                .filter(|p| p.extension().is_some_and(|e| e == ext))
                .map(|p| std::fs::read(p).unwrap())
                .collect::<Vec<_>>()
        };
        let compendia = with("compendium");
        let compendia: Vec<_> = compendia.iter().map(|b| Compendium::read(b).unwrap()).collect();
        let tagfiles = with("hkx");
        expect_found(corpus, ".hkx files", tagfiles.len());
        // the first tree of two or more sections: its file, item, and the file offsets of the two counts
        let (bytes, item, packed_count, second_first) = tagfiles
            .iter()
            .find_map(|bytes| {
                let tag = read(bytes, &compendia);
                (1..tag.items.len()).find_map(|i| {
                    let data = tag.item(i).ok()?.get(0).ok()?;
                    let sections = data
                        .field("meshTree")
                        .ok()?
                        .field("sections")
                        .ok()?
                        .array()
                        .ok()?;
                    if data.type_name() != "hknpCompressedMeshShapeData" || sections.len() < 2 {
                        return None;
                    }
                    assert!(mesh_sections(data).is_ok());
                    let at = |s: usize, name: &str| sections.get(s).unwrap().field(name).unwrap().at();
                    Some((
                        bytes.clone(),
                        i,
                        at(0, "numPackedVertices"),
                        at(1, "firstPackedVertex"),
                    ))
                })
            })
            .expect("a mesh tree of two or more sections");
        for (at, want) in [
            (packed_count, "the count in sharedVertices.data"),
            (second_first, "its first packed vertex is"),
        ] {
            let mut patched = bytes.clone();
            patched[at] = patched[at].wrapping_add(1);
            let tag = read(&patched, &compendia);
            let e = mesh_sections(tag.item(item).unwrap().get(0).unwrap())
                .unwrap_err()
                .to_string();
            assert!(e.contains(want), "{e}");
        }
    }

    /// The tagfiles and compendia of a game's `havok-enums` corpus; `None` without it.
    fn enums_corpus(game: Game) -> Option<(Vec<Vec<u8>>, Vec<Compendium>)> {
        let corpus = Query::Oracle(OracleSet::Corpus, game, "havok-enums");
        let dir = resolve(corpus)?;
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        files.sort();
        let with = |ext: &str| {
            files
                .iter()
                .filter(|p| p.extension().is_some_and(|e| e == ext))
                .map(|p| std::fs::read(p).unwrap())
                .collect::<Vec<_>>()
        };
        let compendia = with("compendium");
        let compendia: Vec<_> = compendia.iter().map(|b| Compendium::read(b).unwrap()).collect();
        let tagfiles = with("hkx");
        expect_found(corpus, ".hkx files", tagfiles.len());
        Some((tagfiles, compendia))
    }

    /// The first mesh of the corpus with a section that is flagged (or, with `flagged` false, one that is not): its
    /// file's bytes, the item of its data and the section's index.
    fn find_section(
        tagfiles: &[Vec<u8>],
        compendia: &[Compendium],
        flagged: bool,
    ) -> (Vec<u8>, usize, usize) {
        for bytes in tagfiles {
            let tag = read(bytes, compendia);
            for item in 1..tag.items.len() {
                let Some(data) = tag.item(item).ok().and_then(|a| a.get(0).ok()) else {
                    continue;
                };
                if data.type_name() != "hknpCompressedMeshShapeData" {
                    continue;
                }
                let sections = mesh_sections(data).unwrap();
                if let Some(i) = sections
                    .iter()
                    .position(|s| s.flags.contains(SectionFlags::REQUIRE_TREE) == flagged)
                {
                    return (bytes.clone(), item, i);
                }
            }
        }
        panic!("no mesh with a section of flagged = {flagged}");
    }

    /// A section's `flags` and `layerData` are checked. In the Sekiro corpus (the smallest tagfile with each kind of
    /// mesh): a flagged section holds the two custom primitives its mesh has, and an unflagged one none. A flags byte
    /// with a bit beyond bit 0, a layerData other than 0, and a flag that disagrees with the custom primitives are each
    /// refused by name. Skips without the corpus.
    #[test]
    fn section_flags_and_layer_data_are_checked() {
        let Some((tagfiles, compendia)) = enums_corpus(Game::Sekiro) else {
            return;
        };
        let field_at = |bytes: &[u8], item: usize, section: usize, name: &str| {
            let tag = read(bytes, &compendia);
            tag.item(item)
                .unwrap()
                .get(0)
                .unwrap()
                .field("meshTree")
                .unwrap()
                .field("sections")
                .unwrap()
                .array()
                .unwrap()
                .get(section)
                .unwrap()
                .field(name)
                .unwrap()
                .at()
        };
        let read_mesh = |bytes: &[u8], item: usize| {
            let tag = read(bytes, &compendia);
            compressed_mesh(tag.item(item).unwrap().get(0).unwrap()).map(|m| m.custom.len())
        };
        let patched = |bytes: &[u8], at: usize, value: u8| {
            let mut copy = bytes.to_vec();
            *copy.get_mut(at).unwrap() = value;
            copy
        };

        // a flagged section: the mesh reads, with two custom primitives; its flag cleared, the section is refused
        let (bytes, item, section) = find_section(&tagfiles, &compendia, true);
        assert_eq!(read_mesh(&bytes, item).unwrap(), 2);
        let at = field_at(&bytes, item, section, "flags");
        assert_eq!(bytes.get(at), Some(&1));
        let e = read_mesh(&patched(&bytes, at, 0), item).unwrap_err().to_string();
        assert!(e.contains("holds a custom primitive but is not flagged"), "{e}");
        // a bit beyond bit 0
        for value in [2, 3, 0x80] {
            let e = read_mesh(&patched(&bytes, at, value), item)
                .unwrap_err()
                .to_string();
            assert!(
                e.contains("flags are not a bit set within 0x1"),
                "{value:#x}: {e}"
            );
        }
        // layerData
        let at = field_at(&bytes, item, section, "layerData");
        assert_eq!(bytes.get(at), Some(&0));
        let e = read_mesh(&patched(&bytes, at, 1), item).unwrap_err().to_string();
        assert!(e.contains("layerData is not 0"), "{e}");

        // an unflagged section: the mesh reads with none; flagged, the section is refused
        let (bytes, item, section) = find_section(&tagfiles, &compendia, false);
        assert_eq!(read_mesh(&bytes, item).unwrap(), 0);
        let at = field_at(&bytes, item, section, "flags");
        assert_eq!(bytes.get(at), Some(&0));
        let e = read_mesh(&patched(&bytes, at, 1), item).unwrap_err().to_string();
        assert!(
            e.contains("flagged as needing its tree but holds no custom primitive"),
            "{e}"
        );
    }
}
