//! A synthetic extern mesh shape checks what `extern_mesh` reads and refuses. The layout follows the field names of
//! `hknpExternMeshShape`, `hknpDefaultExternMeshShapeGeometry` and `hkGeometry` in the games' type tables; no game
//! payload is embedded.
#![allow(
    clippy::unwrap_used,
    reason = "a test fixture that does not parse is the test's failure"
)]
mod tagbuilder;

use havok_formats::{Tagfile, extern_mesh};
use tagbuilder::{Built, Layout, fixed, primitive, record, tagfile};

/// A tagfile whose root is an extern mesh shape of one triangle over `vertices`: its corner indices `corners`, its
/// material, a geometry class named `geometry_class`, and the 2018.1 tag flag where `tag` has it.
fn shape(
    vertices: &[[f32; 4]],
    corners: [i32; 3],
    material: i32,
    geometry_class: &'static str,
    tag: Option<u8>,
) -> Built<Vec<u8>> {
    let definitions = vec![
        primitive("hkReal", 0x174605, 4, None), // 1
        fixed("T[N]<hkReal,4>", 4, 1, 4),       // 2
        primitive("hkArray", 8, 16, Some(2)),   // 3
        primitive("int", 0x8204, 4, None),      // 4
        record(
            "hkGeometry::Triangle",
            16,
            vec![("a", 0, 4), ("b", 4, 4), ("c", 8, 4), ("material", 12, 4)],
        ), // 5
        primitive("hkArray", 8, 16, Some(5)),   // 6
        record("hkGeometry", 56, vec![("vertices", 24, 3), ("triangles", 40, 6)]), // 7
        primitive("T*", 6, 8, Some(7)),         // 8
        primitive("hkBool", 0x2004, 1, None),   // 9
        record(
            geometry_class,
            48,
            vec![("geometry", 32, 8), ("useTriangleMaterialAsShapeTag", 40, 9)],
        ), // 10
        primitive("T*", 6, 8, Some(10)),        // 11
        record("hknpExternMeshShape", 96, vec![("geometry", 80, 11)]), // 12
    ];
    let mut layout = Layout::default();
    let (_, root) = layout.alloc(12, 96, 1, false)?;
    let (geometry, geometry_at) = layout.alloc(10, 48, 1, false)?;
    let (mesh, mesh_at) = layout.alloc(7, 56, 1, false)?;
    let (stored, stored_at) = layout.alloc(2, 16 * vertices.len(), u32::try_from(vertices.len())?, true)?;
    let (triangles, triangles_at) = layout.alloc(5, 16, 1, true)?;
    layout.put(root + 80, &u32::try_from(geometry)?.to_le_bytes());
    layout.put(geometry_at + 32, &u32::try_from(mesh)?.to_le_bytes());
    if let Some(flag) = tag {
        layout.put(geometry_at + 40, &[flag]);
    }
    layout.put(mesh_at + 24, &u32::try_from(stored)?.to_le_bytes());
    layout.put(mesh_at + 40, &u32::try_from(triangles)?.to_le_bytes());
    for (i, v) in vertices.iter().enumerate() {
        for (k, x) in v.iter().enumerate() {
            layout.put(stored_at + 16 * i + 4 * k, &x.to_le_bytes());
        }
    }
    for (k, x) in corners.iter().chain([&material]).enumerate() {
        layout.put(triangles_at + 4 * k, &x.to_le_bytes());
    }
    tagfile(b"20180100", &definitions, layout.data, &layout.items)
}

const TRIANGLE: [[f32; 4]; 3] = [[0., 0., 0., 1.], [1., 0., 2., 1.], [0., 3., 0., 1.]];
const GEOMETRY: &str = "hknpDefaultExternMeshShapeGeometry";

fn refusal(bytes: &[u8]) -> String {
    let tag = Tagfile::read(bytes, None).unwrap();
    extern_mesh(tag.root().unwrap()).unwrap_err().to_string()
}

#[test]
fn an_extern_mesh_is_its_vertices_triangles_and_materials() -> Built<()> {
    let bytes = shape(&TRIANGLE, [0, 2, 1], -1, GEOMETRY, Some(1))?;
    let tag = Tagfile::read(&bytes, None).unwrap();
    let mesh = extern_mesh(tag.root().unwrap()).unwrap();
    assert_eq!(mesh.vertices, [[0., 0., 0.], [1., 0., 2.], [0., 3., 0.]]);
    assert_eq!(mesh.triangles, [[0, 2, 1]]);
    assert_eq!(mesh.materials, [-1]);
    assert_eq!(mesh.material_as_shape_tag, Some(true));
    Ok(())
}

#[test]
fn an_index_off_the_vertices_is_refused() -> Built<()> {
    for corners in [[0, 1, 3], [0, -1, 2]] {
        let e = refusal(&shape(&TRIANGLE, corners, -1, GEOMETRY, None)?);
        assert!(e.contains("of 3"), "{e}");
    }
    Ok(())
}

#[test]
fn a_vertex_that_is_not_a_finite_point_with_w_one_is_refused() -> Built<()> {
    for bad in [
        [0., 0., 0., 0.],
        [0., 0., 0., 2.],
        [f32::NAN, 0., 0., 1.],
        [f32::INFINITY, 0., 0., 1.],
    ] {
        let vertices = [bad, TRIANGLE[1], TRIANGLE[2]];
        let e = refusal(&shape(&vertices, [0, 1, 2], -1, GEOMETRY, None)?);
        assert!(e.contains("extern mesh vertex 0"), "{e}");
    }
    Ok(())
}

#[test]
fn another_geometry_class_is_refused() -> Built<()> {
    let e = refusal(&shape(
        &TRIANGLE,
        [0, 1, 2],
        -1,
        "hknpOtherExternMeshShapeGeometry",
        None,
    )?);
    assert!(e.contains("hknpOtherExternMeshShapeGeometry"), "{e}");
    Ok(())
}
