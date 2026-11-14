//! A synthetic polytope shape checks the rule the Sekiro and Elden Ring polytopes all keep: one plane for each face.
//! The layout follows the field names of `hknpConvexPolytopeShape` in their type tables; no game payload is embedded.
#![allow(
    clippy::unwrap_used,
    reason = "a test fixture that does not parse is the test's failure"
)]
mod tagbuilder;

use havok_formats::Tagfile;
use havok_formats::convex::{ConvexKind, ConvexShape};
use tagbuilder::{Built, Layout, fixed, primitive, record, tagfile};

/// A tagfile whose root is a polytope of one triangle face, with `planes` planes.
fn polytope(planes: usize) -> Built<Vec<u8>> {
    let definitions = vec![
        primitive("hkUint8", 0x2004, 1, None),  // 1
        primitive("hkUint16", 0x4004, 2, None), // 2
        primitive("hkReal", 0x174605, 4, None), // 3
        fixed("T[N]<hkReal,4>", 4, 3, 4),       // 4
        primitive("hkArray", 8, 16, Some(4)),   // 5
        primitive("hkArray", 8, 16, Some(1)),   // 6
        record(
            "hknpConvexPolytopeShape::Face",
            4,
            vec![("firstIndex", 0, 2), ("numIndices", 2, 1)],
        ), // 7
        primitive("hkArray", 8, 16, Some(7)),   // 8
        record(
            "hknpConvexPolytopeShape",
            80,
            vec![
                ("convexRadius", 0, 3),
                ("vertices", 16, 5),
                ("planes", 32, 5),
                ("faces", 48, 8),
                ("indices", 64, 6),
            ],
        ), // 9
    ];
    let mut layout = Layout::default();
    let (_, root) = layout.alloc(9, 80, 1, false)?;
    layout.put(root, &0.05f32.to_le_bytes());
    for (field, type_index, size, count) in [
        (16, 4, 48, 3),
        (32, 4, 16 * planes, planes),
        (48, 7, 4, 1),
        (64, 1, 3, 3),
    ] {
        let (item, offset) = layout.alloc(type_index, size, u32::try_from(count)?, true)?;
        layout.put(root + field, &item.to_le_bytes());
        match field {
            16 => {
                for (i, v) in [
                    [0.0f32, 0.0, 0.0, 0.0],
                    [1.0, 0.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0, 0.0],
                ]
                .iter()
                .enumerate()
                {
                    for (k, x) in v.iter().enumerate() {
                        layout.put(offset + 16 * i + 4 * k, &x.to_le_bytes());
                    }
                }
            }
            32 => {
                for i in 0..planes {
                    layout.put(offset + 16 * i + 8, &1.0f32.to_le_bytes());
                }
            }
            48 => layout.put(offset, &[0, 0, 3, 0]),
            _ => layout.put(offset, &[0, 1, 2]),
        }
    }
    tagfile(b"20180100", &definitions, layout.data, &layout.items)
}

#[test]
fn a_polytope_stores_one_plane_for_each_face() -> Built<()> {
    let bytes = polytope(1)?;
    let tag = Tagfile::read(&bytes, None).unwrap();
    let shape = ConvexShape::read(tag.root().unwrap()).unwrap();
    assert_eq!(shape.kind, ConvexKind::Polytope);
    assert_eq!(shape.faces, [vec![0, 1, 2]]);
    assert_eq!(shape.planes, [[0.0, 0.0, 1.0, 0.0]]);
    assert_eq!(shape.vertices.len(), 3);
    Ok(())
}

#[test]
fn a_polytope_with_another_number_of_planes_is_refused() -> Built<()> {
    for planes in [0, 2] {
        let bytes = polytope(planes)?;
        let tag = Tagfile::read(&bytes, None).unwrap();
        let e = ConvexShape::read(tag.root().unwrap()).unwrap_err().to_string();
        assert!(e.contains(&format!("{planes} planes and 1 faces")), "{e}");
    }
    Ok(())
}
