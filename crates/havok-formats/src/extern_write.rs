//! Writing extern meshes: [`replace_extern_mesh`] puts triangles into the `hknpExternMeshShape` of an editable
//! [`TagfileImage`], with the bounding-volume data the shape's class stores.
//!
//! An extern mesh is a plain `hkGeometry` (vertices with `w` 1, triangles `a b c material`) and the bounding-volume data
//! the broad phase queries, which comes in two forms in the installs' map `f` collision:
//!
//! - **Class version 2** (Sekiro's 12,688 shapes, Elden Ring's 40 oldest): a 6-byte-node aabb tree (`aabbTree`, one leaf per
//!   triangle, leaf payload the triangle index, rooted at the box of the vertices) and a simd tree that is only its two
//!   empty nodes. `numShapeKeyBits` is `bitlen(triangles)` in all 12,728.
//! - **Class version 3** (Elden Ring's 264,816 others): the four-lane simd tree alone, a leaf per triangle keyed by the
//!   triangle index, `isCompact` 1. `numShapeKeyBits` is `bitlen(triangles) + 1` in all 264,816.
//!
//! The bytes of the nodes are written as [`crate::bvtree_write`] and [`crate::simd_write`] document: the stock topology is
//! not reproduced (in the version 3 meshes of up to 32 triangles the leaves are four consecutive keys to a node; the
//! writer's aabb tree is a [`sah_shape`](crate::bvtree_write::sah_shape) and its simd tree splits at medians), the boxes
//! and the numbering are the files'.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use crate::bvtree::NodeWidth;
use crate::bvtree_write::{Aabb, Shape, encode_tree, sah_shape};
use crate::schema::{Schema, put_f32s, put_uint};
use crate::simd_write::{SimdEncoding, SimdLeaf, build_simd_tree, node_bytes};
use crate::{Result, TagfileImage, invalid};

/// The triangles to write into an extern mesh shape.
#[derive(Debug, Clone, Copy)]
pub struct ExternSource<'a> {
    /// The vertices, shape-local (finite).
    pub vertices: &'a [[f32; 3]],
    /// The triangles as indices of `vertices`.
    pub triangles: &'a [[u32; 3]],
    /// Each triangle's `material` (−1 where the files give none); `None` gives every triangle −1, as the stock files do.
    pub materials: Option<&'a [i32]>,
}

/// The object items of `image` that are extern mesh shapes.
pub fn extern_mesh_shapes(image: &TagfileImage) -> Vec<usize> {
    image.objects_named("hknpExternMeshShape")
}

/// The bit length of `x` (0 has none).
fn bitlen(x: u64) -> u32 {
    64u32.saturating_sub(x.leading_zeros())
}

/// Replace the triangles and bounding-volume data of the extern mesh shape in item `shape_item` of `image` with
/// `source`'s.
///
/// The shape's `hkGeometry` gets the vertices and triangles (the vertices as they are, in order: a vertex no triangle uses
/// stays), and its bounding-volume data the tree its class version stores (see the module documentation), rooted at
/// the exact box of the vertices the triangles use. `numShapeKeyBits` is set. The shape's pose, its geometry class's
/// `useTriangleMaterialAsShapeTag` and everything else are left alone.
///
/// Refused: an item that is not an extern mesh shape; a geometry or bounding-volume class this writer does not know (the
/// geometry must be an `hknpDefaultExternMeshShapeGeometry` holding an `hkGeometry`); no triangle; a vertex that is not
/// finite; an index past the vertices; `materials` of another length than the triangles; a type table without a member
/// the write needs. Nothing is written until every check has passed.
pub fn replace_extern_mesh(
    image: &mut TagfileImage,
    shape_item: usize,
    source: &ExternSource<'_>,
) -> Result<()> {
    let types = image.types.clone();
    let schema = Schema { types: &types };
    let shape_type = image
        .items
        .get(shape_item)
        .map(|i| i.type_index)
        .ok_or_else(|| invalid(0, format!("no item {shape_item}")))?;
    if schema.name(shape_type) != "hknpExternMeshShape" {
        return Err(invalid(
            0,
            format!(
                "item {shape_item} is a {}, not an extern mesh shape",
                schema.name(shape_type)
            ),
        ));
    }
    if source.triangles.is_empty() {
        return Err(invalid(0, "no triangle to write"));
    }
    if source.vertices.iter().flatten().any(|c| !c.is_finite()) {
        return Err(invalid(0, "a vertex that is not finite"));
    }
    if let Some(m) = source.materials
        && m.len() != source.triangles.len()
    {
        return Err(invalid(
            0,
            format!("{} materials for {} triangles", m.len(), source.triangles.len()),
        ));
    }
    // the exact boxes of the triangles
    let mut boxes: Vec<Aabb> = Vec::with_capacity(source.triangles.len());
    let mut domain = [[f32::MAX; 3], [f32::MIN; 3]];
    for (i, t) in source.triangles.iter().enumerate() {
        let mut low = [f64::MAX; 3];
        let mut high = [f64::MIN; 3];
        for &v in t {
            let p = usize::try_from(v)
                .ok()
                .and_then(|v| source.vertices.get(v))
                .ok_or_else(|| {
                    invalid(
                        0,
                        format!("triangle {i} names vertex {v} of {}", source.vertices.len()),
                    )
                })?;
            for axis in 0..3 {
                let x = p.get(axis).copied().unwrap_or(0.0);
                if let (Some(l), Some(dl)) = (low.get_mut(axis), domain[0].get_mut(axis)) {
                    *l = l.min(f64::from(x));
                    *dl = dl.min(x);
                }
                if let (Some(h), Some(dh)) = (high.get_mut(axis), domain[1].get_mut(axis)) {
                    *h = h.max(f64::from(x));
                    *dh = dh.max(x);
                }
            }
        }
        boxes.push([low, high]);
    }
    // the geometry
    let geometry_ptr = schema.field(shape_type, "geometry")?;
    let geometry_item = image.reference_at(shape_item, geometry_ptr.offset)?;
    let geometry_type = image
        .items
        .get(geometry_item)
        .filter(|_| geometry_item != 0)
        .map(|i| i.type_index)
        .ok_or_else(|| invalid(0, "the shape has no geometry"))?;
    if schema.name(geometry_type) != "hknpDefaultExternMeshShapeGeometry" {
        return Err(invalid(
            0,
            format!(
                "the geometry is a {}, not an hknpDefaultExternMeshShapeGeometry",
                schema.name(geometry_type)
            ),
        ));
    }
    let hk_ptr = schema.field(geometry_type, "geometry")?;
    let hk_item = image.reference_at(geometry_item, hk_ptr.offset)?;
    let hk_type = image
        .items
        .get(hk_item)
        .filter(|_| hk_item != 0)
        .map(|i| i.type_index)
        .ok_or_else(|| invalid(0, "the geometry has no hkGeometry"))?;
    if schema.name(hk_type) != "hkGeometry" {
        return Err(invalid(
            0,
            format!("the geometry holds a {}, not an hkGeometry", schema.name(hk_type)),
        ));
    }
    let vertices_member = schema.field(hk_type, "vertices")?;
    let vertex_type = schema.subtype_of(vertices_member.type_index)?;
    if schema.size_of(vertex_type)? != 16 {
        return Err(invalid(0, "geometry vertices are not 16 bytes"));
    }
    let triangles_member = schema.field(hk_type, "triangles")?;
    let triangle_type = schema.subtype_of(triangles_member.type_index)?;
    let triangle_size = schema.size_of(triangle_type)?;
    let mut vertex_bytes = schema.zeroed(vertex_type, source.vertices.len())?;
    for (i, v) in source.vertices.iter().enumerate() {
        put_f32s(&mut vertex_bytes, i.saturating_mul(16), &[v[0], v[1], v[2], 1.0])?;
    }
    let mut triangle_bytes = schema.zeroed(triangle_type, source.triangles.len())?;
    for (i, t) in source.triangles.iter().enumerate() {
        let base = i.saturating_mul(triangle_size);
        for (name, value) in [("a", t[0]), ("b", t[1]), ("c", t[2])] {
            let m = schema.field(triangle_type, name)?;
            put_uint(
                &mut triangle_bytes,
                base.saturating_add(m.offset),
                4,
                u64::from(value),
            )?;
        }
        let material = source.materials.and_then(|m| m.get(i).copied()).unwrap_or(-1);
        let m = schema.field(triangle_type, "material")?;
        put_uint(
            &mut triangle_bytes,
            base.saturating_add(m.offset),
            4,
            u64::from(material.cast_unsigned()),
        )?;
    }
    // the bounding-volume data
    let bv_ptr = schema.field(shape_type, "boundingVolumeData")?;
    let bv_item = image.reference_at(shape_item, bv_ptr.offset)?;
    let bv_type = image
        .items
        .get(bv_item)
        .filter(|_| bv_item != 0)
        .map(|i| i.type_index)
        .ok_or_else(|| invalid(0, "the shape has no bounding-volume data"))?;
    if schema.name(bv_type) != "hknpExternMeshShapeData" {
        return Err(invalid(
            0,
            format!(
                "the bounding-volume data is a {}, not an hknpExternMeshShapeData",
                schema.name(bv_type)
            ),
        ));
    }
    let count = source.triangles.len();
    let aabb = schema.has_field(bv_type, "aabbTree");
    let simd_member = schema.path(bv_type, &["simdTree", "nodes"])?;
    let simd_node_type = schema.subtype_of(simd_member.type_index)?;
    let encoding = SimdEncoding::of_node(&schema, simd_node_type);
    let leaves: Vec<SimdLeaf> = if aabb {
        Vec::new()
    } else {
        boxes
            .iter()
            .enumerate()
            .map(|(i, b)| {
                Ok(SimdLeaf {
                    key: u32::try_from(i).map_err(|_| invalid(0, "more triangles than 2^32"))?,
                    aabb: [b[0].map(narrow), b[1].map(narrow)],
                })
            })
            .collect::<Result<Vec<_>>>()?
    };
    let simd = build_simd_tree(&leaves, encoding)?;
    let simd_bytes = node_bytes(&schema, simd_node_type, &simd, encoding)?;
    let mut aabb_nodes: Option<(usize, usize, Vec<u8>, usize, usize)> = None;
    if aabb {
        let nodes_member = schema.path(bv_type, &["aabbTree", "nodes"])?;
        let node_type = schema.subtype_of(nodes_member.type_index)?;
        if schema.size_of(node_type)? != 6 {
            return Err(invalid(0, "the aabb tree's nodes are not 6 bytes"));
        }
        let (shape, order) = sah_shape(&boxes)?;
        // the leaf payload is the triangle index
        let shape: Vec<Shape> = shape
            .into_iter()
            .map(|s| match s {
                Shape::Leaf { payload } => Shape::Leaf {
                    payload: usize::try_from(payload)
                        .ok()
                        .and_then(|p| order.get(p))
                        .and_then(|&t| u32::try_from(t).ok())
                        .unwrap_or(u32::MAX),
                },
                internal => internal,
            })
            .collect();
        let tree = encode_tree(NodeWidth::Six, domain, &shape, &|payload| {
            usize::try_from(payload).ok().and_then(|t| boxes.get(t)).copied()
        })?;
        let min_at = schema.path(bv_type, &["aabbTree", "domain", "min"])?.offset;
        let max_at = schema.path(bv_type, &["aabbTree", "domain", "max"])?.offset;
        aabb_nodes = Some((nodes_member.offset, node_type, tree.bytes, min_at, max_at));
    }
    // the key bits
    let bits = bitlen(u64::try_from(count).map_err(|_| invalid(0, "too many triangles"))?)
        .saturating_add(u32::from(!aabb));
    let bits_member = schema.field(shape_type, "numShapeKeyBits")?;
    let bits_width = schema.int_width(bits_member.type_index)?;
    let mut bits_bytes = vec![0u8; bits_width];
    put_uint(&mut bits_bytes, 0, bits_width, u64::from(bits))?;
    let compact = schema.path(bv_type, &["simdTree", "isCompact"]).ok();

    // everything checked: write
    image.patch(shape_item, bits_member.offset, &bits_bytes)?;
    image.set_array(
        hk_item,
        vertices_member.offset,
        vertex_type,
        source.vertices.len(),
        vertex_bytes,
    )?;
    image.set_array(
        hk_item,
        triangles_member.offset,
        triangle_type,
        source.triangles.len(),
        triangle_bytes,
    )?;
    if let Some((nodes_at, node_type, bytes, min_at, max_at)) = aabb_nodes {
        image.set_array(
            bv_item,
            nodes_at,
            node_type,
            bytes.len().checked_div(6).unwrap_or(0),
            bytes,
        )?;
        for (at, corner) in [(min_at, domain[0]), (max_at, domain[1])] {
            let mut lanes = vec![0u8; 16];
            put_f32s(&mut lanes, 0, &[corner[0], corner[1], corner[2], 1.0])?;
            image.patch(bv_item, at, &lanes)?;
        }
    }
    image.set_array(
        bv_item,
        simd_member.offset,
        simd_node_type,
        simd.len(),
        simd_bytes,
    )?;
    if let Some(m) = compact {
        let width = schema.int_width(m.type_index)?;
        let mut bytes = vec![0u8; width];
        put_uint(&mut bytes, 0, width, 1)?;
        image.patch(bv_item, m.offset, &bytes)?;
    }
    Ok(())
}

/// A value narrowed to `f32`.
#[allow(clippy::cast_possible_truncation, reason = "the stored boxes are f32")]
fn narrow(x: f64) -> f32 {
    x as f32
}
