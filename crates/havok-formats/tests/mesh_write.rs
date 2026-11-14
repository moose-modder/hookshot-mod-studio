//! Compressed meshes and extern meshes built from triangles, written into the shapes of synthetic tagfiles (both SDK
//! layouts; the class layouts follow the member names, kinds and sizes of the Sekiro and Elden Ring type tables, no game
//! payload is embedded) and read back by the readers: the arrays the builder makes are the arrays the reader decodes, every
//! tree and simd check passes, and the geometry comes back within the quantisation of the codecs.
#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    reason = "a test fixture that does not parse is the test's failure"
)]
mod tagbuilder;

use havok_formats::{
    BuildOptions, BuiltMesh, BvTree, ExternSource, MeshSource, NodeWidth, Primitives, Tagfile,
    build_compressed_mesh, build_compressed_mesh_with, compressed_mesh, compressed_mesh_shapes, extern_mesh,
    extern_mesh_shapes, mesh_trees, replace_compressed_mesh, replace_extern_mesh, simd_tree,
};
use tagbuilder::{Built, Definition, Layout, fixed, primitive, record, tagfile};

const SEKIRO: &[u8; 8] = b"20160200";
const ELDEN_RING: &[u8; 8] = b"20180100";

// the type indices of the fixture (one-based, in definition order)
const U8: u32 = 1;
const U16: u32 = 2;
const U32: u32 = 3;
const U64: u32 = 4;
const REAL: u32 = 5;
const VEC4: u32 = 6;
const AABB: u32 = 7;
const REAL6: u32 = 8;
const CODEC4: u32 = 9;
const CODEC5: u32 = 10;
const PRIMITIVE: u32 = 11;
const RUN_VALUE: u32 = 12;
const RUN: u32 = 13;
const ARRAY_CODEC4: u32 = 14;
const ARRAY_CODEC5: u32 = 15;
const ARRAY_PRIMITIVE: u32 = 16;
const ARRAY_U16: u32 = 17;
const ARRAY_U32: u32 = 18;
const ARRAY_U64: u32 = 19;
const ARRAY_RUN: u32 = 20;
const WORD: u32 = 21;
const SECTION: u32 = 22;
const ARRAY_SECTION: u32 = 23;
const TREE: u32 = 24;
const U32X4: u32 = 25;
const SIMD_NODE: u32 = 26;
const ARRAY_SIMD: u32 = 27;
const SIMD_TREE: u32 = 28;
const DATA: u32 = 29;
const STORAGE: u32 = 30;
const BITFIELD: u32 = 31;
const POINTER_TO_DATA: u32 = 32;
const SHAPE: u32 = 33;
const U8X4: u32 = 34;

/// The shape's data pointer and the data object's size, as the fixture lays them out.
const SHAPE_SIZE: usize = 160;
const DATA_POINTER: usize = 72;
const DATA_SIZE: usize = 272;

fn definitions(sdk: &[u8; 8], class: &'static str) -> Vec<Definition> {
    let sekiro = sdk == SEKIRO;
    let section = if sekiro {
        vec![
            ("nodes", 0, ARRAY_CODEC4),
            ("domain", 16, AABB),
            ("codecParms", 48, REAL6),
            ("firstPackedVertex", 72, U32),
            ("sharedVertices", 76, WORD),
            ("primitives", 80, WORD),
            ("dataRuns", 84, WORD),
            ("numPackedVertices", 88, U8),
            ("numSharedIndices", 89, U8),
            ("leafIndex", 90, U16),
            ("page", 92, U8),
            ("flags", 93, U8),
            ("layerData", 94, U8),
            ("unusedData", 95, U8),
        ]
    } else {
        vec![
            ("nodes", 0, ARRAY_CODEC4),
            ("domain", 16, AABB),
            ("codecParms", 48, REAL6),
            ("firstPackedVertexIndex", 72, U32),
            ("firstSharedVertexIndex", 76, U32),
            ("firstPrimitiveIndex", 80, U32),
            ("firstDataRunIndex", 84, U32),
            ("numPackedVertices", 88, U8),
            ("numPrimitives", 89, U8),
            ("numDataRuns", 90, U8),
            ("page", 91, U8),
            ("leafIndex", 92, U16),
            ("layerData", 94, U8),
            ("flags", 95, U8),
        ]
    };
    let mut node = vec![
        ("lx", 0, VEC4),
        ("hx", 16, VEC4),
        ("ly", 32, VEC4),
        ("hy", 48, VEC4),
        ("lz", 64, VEC4),
        ("hz", 80, VEC4),
        ("data", 96, U32X4),
    ];
    let mut simd_tree = vec![("nodes", 8, ARRAY_SIMD)];
    let mut data = vec![("meshTree", 32, TREE), ("simdTree", 192, SIMD_TREE)];
    if !sekiro {
        node.push(("isLeaf", 112, U8));
        node.push(("isActive", 113, U8));
        simd_tree.push(("isCompact", 24, U8));
        data.push(("hasSimdTree", 264, U8));
    }
    vec![
        primitive("hkUint8", 0x2004, 1, None),                           // 1
        primitive("hkUint16", 0x4004, 2, None),                          // 2
        primitive("hkUint32", 0x8004, 4, None),                          // 3
        primitive("hkUint64", 0x10004, 8, None),                         // 4
        primitive("hkReal", 0x174605, 4, None),                          // 5
        fixed("hkVector4", 4, REAL, 4),                                  // 6
        record("hkAabb", 32, vec![("min", 0, VEC4), ("max", 16, VEC4)]), // 7
        fixed("T[N]<hkReal,6>", 6, REAL, 4),                             // 8
        record("hkcdCompressedAabbCodecs::Aabb4BytesCodec", 4, vec![]),  // 9
        record("hkcdCompressedAabbCodecs::Aabb5BytesCodec", 5, vec![]),  // 10
        record("hkcdStaticMeshTree::Primitive", 4, vec![("indices", 0, U8X4)]), // 11
        record(
            "hknpCompressedMeshShapeTreeDataRunData",
            2,
            vec![("data", 0, U16)],
        ), // 12
        record(
            "hkcdStaticMeshTree::PrimitiveDataRun",
            4,
            vec![
                ("value", 0, if sekiro { RUN_VALUE } else { U16 }),
                ("index", 2, U8),
                ("count", 3, U8),
            ],
        ), // 13
        primitive("hkArray", 8, 16, Some(CODEC4)),                       // 14
        primitive("hkArray", 8, 16, Some(CODEC5)),                       // 15
        primitive("hkArray", 8, 16, Some(PRIMITIVE)),                    // 16
        primitive("hkArray", 8, 16, Some(U16)),                          // 17
        primitive("hkArray", 8, 16, Some(U32)),                          // 18
        primitive("hkArray", 8, 16, Some(U64)),                          // 19
        primitive("hkArray", 8, 16, Some(RUN)),                          // 20
        record("hkcdStaticMeshTree::Section::Word", 4, vec![("data", 0, U32)]), // 21
        record("hkcdStaticMeshTree::Section", 96, section),              // 22
        primitive("hkArray", 8, 16, Some(SECTION)),                      // 23
        record(
            "hknpCompressedMeshShapeTree",
            160,
            vec![
                ("nodes", 0, ARRAY_CODEC5),
                ("domain", 16, AABB),
                ("numPrimitiveKeys", 48, U32),
                ("bitsPerKey", 52, U32),
                ("maxKeyValue", 56, U32),
                ("primitiveStoresIsFlatConvex", 60, U8),
                ("sections", 64, ARRAY_SECTION),
                ("primitives", 80, ARRAY_PRIMITIVE),
                ("sharedVerticesIndex", 96, ARRAY_U16),
                ("packedVertices", 112, ARRAY_U32),
                ("sharedVertices", 128, ARRAY_U64),
                ("primitiveDataRuns", 144, ARRAY_RUN),
            ],
        ), // 24
        fixed("T[N]<hkUint32,4>", 4, U32, 4),                            // 25
        record("hkcdSimdTree::Node", if sekiro { 112 } else { 128 }, node), // 26
        primitive("hkArray", 8, 16, Some(SIMD_NODE)),                    // 27
        record("hkcdSimdTree", 32, simd_tree),                           // 28
        record("hknpCompressedMeshShapeData", DATA_SIZE as u32, data),   // 29
        record(
            "hkBitFieldStorage",
            24,
            vec![("words", 0, ARRAY_U32), ("numBits", 16, U32)],
        ), // 30
        record("hkBitField", 24, vec![("storage", 0, STORAGE)]),         // 31
        primitive("T*", 6, 8, Some(DATA)),                               // 32
        record(
            class,
            SHAPE_SIZE as u32,
            vec![
                ("numShapeKeyBits", 35, U8),
                ("convexRadius", 40, REAL),
                ("data", DATA_POINTER as u32, POINTER_TO_DATA),
                ("triangleIsInterior", 80, BITFIELD),
                ("numTriangles", 104, U32),
                ("numConvexShapes", 108, U32),
                ("triangleIndexToShapeKey", 136, ARRAY_U32),
            ],
        ), // 33
        fixed("T[N]<hkUint8,4>", 4, U8, 1),                              // 34
    ]
}

/// A tagfile whose root is a compressed mesh shape (of class `class`) with an empty mesh tree.
fn empty_shape(sdk: &[u8; 8], class: &'static str) -> Built<Vec<u8>> {
    let mut layout = Layout::default();
    let (_, shape) = layout.alloc(SHAPE, SHAPE_SIZE, 1, false)?;
    let (data_item, _) = layout.alloc(DATA, DATA_SIZE, 1, false)?;
    layout.put(shape + DATA_POINTER, &data_item.to_le_bytes());
    // the convex radius of the map meshes
    layout.put(shape + 40, &0.01f32.to_le_bytes());
    tagfile(sdk, &definitions(sdk, class), layout.data, &layout.items)
}

/// A bumpy grid of `n` by `n` cells, two triangles each, `cell` apart.
fn grid(n: usize, cell: f32) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let mut vertices = Vec::new();
    for j in 0..=n {
        for i in 0..=n {
            let (x, y) = (i as f32 * cell, j as f32 * cell);
            vertices.push([x, 0.6 * (x * 0.7).sin() + 0.4 * (y * 0.5).cos(), y]);
        }
    }
    let at = |i: usize, j: usize| (j * (n + 1) + i) as u32;
    let mut triangles = Vec::new();
    for j in 0..n {
        for i in 0..n {
            triangles.push([at(i, j), at(i, j + 1), at(i + 1, j)]);
            triangles.push([at(i + 1, j), at(i, j + 1), at(i + 1, j + 1)]);
        }
    }
    (vertices, triangles)
}

#[test]
fn a_built_mesh_decodes_to_its_triangles() {
    let (vertices, triangles) = grid(30, 1.0);
    let built = build_compressed_mesh(&MeshSource {
        vertices: &vertices,
        triangles: &triangles,
        convex_radius: 0.01,
    })
    .unwrap();
    assert_eq!(built.triangle_keys.iter().flatten().count(), triangles.len());
    assert!(built.sections.len() >= 15, "{} sections", built.sections.len());
    assert!(
        built
            .sections
            .iter()
            .all(|s| s.primitives <= 127 && s.packed_vertices + s.shared_indices <= 255)
    );
    assert!(
        !built.shared_vertices.is_empty(),
        "the grid's section borders share vertices"
    );
    assert_eq!(built.num_primitive_keys as usize, triangles.len());
    // the key bits hold the largest key and no fewer
    let bits = built.bits_per_key;
    assert!(
        u64::from(built.max_key_value) + 1 >= 1u64 << (bits - 1)
            && u64::from(built.max_key_value) + 1 < 1u64 << bits,
        "bitsPerKey {bits} for maxKeyValue {}",
        built.max_key_value
    );
    let decoded = built.triangles().unwrap();
    assert_eq!(decoded.len(), triangles.len());
    let by_key: std::collections::BTreeMap<u32, _> = decoded.iter().map(|t| (t.key, t.vertices)).collect();
    for (i, t) in triangles.iter().enumerate() {
        let key = built.triangle_keys[i].unwrap();
        let got = by_key[&key];
        for (corner, &v) in got.iter().zip(t) {
            let want = vertices[v as usize];
            for axis in 0..3 {
                // a section is a few units wide, so a code step is a few thousandths
                assert!(
                    (corner[axis] - want[axis]).abs() < 0.006,
                    "triangle {i} axis {axis}: {} against {}",
                    corner[axis],
                    want[axis]
                );
            }
        }
    }
}

#[test]
fn a_built_mesh_written_into_either_layout_reads_back() {
    let (vertices, triangles) = grid(30, 1.0);
    let built = build_compressed_mesh(&MeshSource {
        vertices: &vertices,
        triangles: &triangles,
        convex_radius: 0.01,
    })
    .unwrap();
    let want: std::collections::BTreeMap<u32, _> = built
        .triangles()
        .unwrap()
        .iter()
        .map(|t| (t.key, t.vertices))
        .collect();
    for sdk in [SEKIRO, ELDEN_RING] {
        for class in ["fsnpCustomParamCompressedMeshShape", "hknpCompressedMeshShape"] {
            let bytes = empty_shape(sdk, class).unwrap();
            let tag = Tagfile::read(&bytes, None).unwrap();
            let mut image = tag.image().unwrap();
            let shapes = compressed_mesh_shapes(&image);
            assert_eq!(shapes, [1], "{sdk:?} {class}");
            replace_compressed_mesh(&mut image, shapes[0], &built).unwrap();
            let written = image.write().unwrap();
            let tag = Tagfile::read(&written, None).unwrap();
            let shape = tag.root().unwrap();
            let data = shape.field("data").unwrap().dereference().unwrap();
            let mesh = compressed_mesh(data).unwrap();
            assert_eq!(mesh.triangles.len(), triangles.len(), "{sdk:?} {class}");
            assert!(mesh.custom.is_empty() && mesh.dead_primitives == 0);
            // the reader decodes exactly what the builder says its arrays decode to
            for t in &mesh.triangles {
                assert_eq!(want[&t.key], t.vertices, "{sdk:?} key {}", t.key);
            }
            // the trees hold together and hold the geometry
            assert_eq!(mesh.bounds.outside, 0, "{sdk:?} {class}");
            assert_eq!(mesh.bounds.primitives, triangles.len());
            let trees = mesh_trees(data).unwrap();
            assert_eq!(trees.sections.len(), built.sections.len());
            // the simd tree names every primitive, with exact boxes, and every lane box is the union below
            assert_eq!(mesh.simd.leaves, triangles.len(), "{sdk:?} {class}");
            assert_eq!(mesh.simd.leaves_exact, triangles.len());
            assert_eq!((mesh.simd.leaves_outside, mesh.simd.leaves_looser), (0, 0));
            assert_eq!((mesh.simd.children_looser, mesh.simd.children_short), (0, 0));
            // the shape's key bits
            assert_eq!(
                shape.field("numShapeKeyBits").unwrap().unsigned().unwrap(),
                u64::from(built.bits_per_key),
                "{sdk:?} {class}"
            );
            let tree = data.field("meshTree").unwrap();
            assert_eq!(
                tree.field("maxKeyValue").unwrap().unsigned().unwrap(),
                u64::from(built.max_key_value)
            );
            assert_eq!(
                tree.field("bitsPerKey").unwrap().unsigned().unwrap(),
                u64::from(built.bits_per_key)
            );
            // the interior flags: one bit per key, the ones the pass over the built triangles names
            let bits = shape
                .field("triangleIsInterior")
                .unwrap()
                .field("storage")
                .unwrap();
            assert_eq!(
                bits.field("numBits").unwrap().unsigned().unwrap(),
                u64::from(built.max_key_value) + 1
            );
            let words = bits.field("words").unwrap().array().unwrap();
            assert_eq!(
                words.len() as u64,
                (u64::from(built.max_key_value) + 1).div_ceil(32)
            );
            let mut expected = vec![0u64; words.len()];
            for key in built.interior_keys().unwrap() {
                expected[(key / 32) as usize] |= 1 << (key % 32);
            }
            for (i, &want) in expected.iter().enumerate() {
                assert_eq!(
                    words.get(i).unwrap().unsigned().unwrap(),
                    want,
                    "{sdk:?} {class} word {i}"
                );
            }
            // the map from source triangle to shape key (the games' own layer has it; the plain class has none)
            if class.starts_with("fsnp") {
                let map = shape.field("triangleIndexToShapeKey").unwrap().array().unwrap();
                assert_eq!(map.len(), triangles.len());
                let fill = 32 - built.bits_per_key;
                for i in 0..map.len() {
                    let word = map.get(i).unwrap().unsigned().unwrap() as u32;
                    assert_eq!(word >> fill, built.triangle_keys[i].unwrap(), "triangle {i}");
                    assert_eq!(word & ((1u32 << fill) - 1), (1u32 << fill) - 1);
                }
            }
        }
    }
}

/// A bumpy grid like [`grid`], its cells' triangles listed as `a b c` and `a c d` so that each cell's diagonal is a third
/// edge and a first edge: the pairing joins each cell into a quad.
fn grid_quads(n: usize, cell: f32) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let (vertices, _) = grid(n, cell);
    let at = |i: usize, j: usize| (j * (n + 1) + i) as u32;
    let mut triangles = Vec::new();
    for j in 0..n {
        for i in 0..n {
            triangles.push([at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
            triangles.push([at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
        }
    }
    (vertices, triangles)
}

/// Write `built` into a mesh shape of each SDK layout and read it back: the reader decodes what the builder says its
/// arrays decode to, the trees hold every primitive's vertices, and the simd tree names every primitive exactly.
fn written_reads_back(built: &BuiltMesh, triangles: usize) {
    let want: std::collections::BTreeMap<u32, _> = built
        .triangles()
        .unwrap()
        .iter()
        .map(|t| (t.key, t.vertices))
        .collect();
    assert_eq!(want.len(), triangles);
    for sdk in [SEKIRO, ELDEN_RING] {
        let class = "fsnpCustomParamCompressedMeshShape";
        let bytes = empty_shape(sdk, class).unwrap();
        let tag = Tagfile::read(&bytes, None).unwrap();
        let mut image = tag.image().unwrap();
        let shapes = compressed_mesh_shapes(&image);
        replace_compressed_mesh(&mut image, shapes[0], built).unwrap();
        let written = image.write().unwrap();
        let tag = Tagfile::read(&written, None).unwrap();
        let shape = tag.root().unwrap();
        let data = shape.field("data").unwrap().dereference().unwrap();
        let mesh = compressed_mesh(data).unwrap();
        assert_eq!(mesh.triangles.len(), triangles, "{sdk:?}");
        assert!(mesh.custom.is_empty() && mesh.dead_primitives == 0);
        for t in &mesh.triangles {
            assert_eq!(want[&t.key], t.vertices, "{sdk:?} key {}", t.key);
        }
        assert_eq!(mesh.bounds.outside, 0, "{sdk:?}");
        assert_eq!(mesh.bounds.primitives, built.primitives.len());
        let trees = mesh_trees(data).unwrap();
        assert_eq!(trees.sections.len(), built.sections.len());
        assert_eq!(mesh.simd.leaves, built.primitives.len(), "{sdk:?}");
        assert_eq!(mesh.simd.leaves_exact, built.primitives.len());
        assert_eq!((mesh.simd.leaves_outside, mesh.simd.leaves_looser), (0, 0));
        assert_eq!((mesh.simd.children_looser, mesh.simd.children_short), (0, 0));
    }
}

#[test]
fn a_mesh_laid_out_as_the_stock_tool_does_reads_back() {
    let (vertices, triangles) = grid(30, 1.0);
    let options = BuildOptions {
        primitives: Primitives::Triangles,
        layout: havok_formats::Layout::Stock,
        ..BuildOptions::default()
    };
    let built = build_compressed_mesh_with(
        &MeshSource {
            vertices: &vertices,
            triangles: &triangles,
            convex_radius: 0.01,
        },
        &options,
    )
    .unwrap();
    assert_eq!(built.triangle_keys.iter().flatten().count(), triangles.len());
    assert!(built.sections.len() >= 15, "{} sections", built.sections.len());
    assert!(
        built
            .sections
            .iter()
            .all(|s| s.primitives <= 127 && s.packed_vertices + s.shared_indices <= 255)
    );
    assert_eq!(built.num_primitive_keys as usize, triangles.len());
    written_reads_back(&built, triangles.len());
}

#[test]
fn triangle_pairs_are_joined_into_quads_and_read_back() {
    let n = 30;
    let (vertices, triangles) = grid_quads(n, 1.0);
    for layout in [havok_formats::Layout::Sah, havok_formats::Layout::Stock] {
        let options = BuildOptions {
            primitives: Primitives::PairedQuads,
            layout,
            ..BuildOptions::default()
        };
        let built = build_compressed_mesh_with(
            &MeshSource {
                vertices: &vertices,
                triangles: &triangles,
                convex_radius: 0.01,
            },
            &options,
        )
        .unwrap();
        // every cell is one quad: four distinct slots, the first half at an even key and the second at the next
        assert_eq!(built.primitives.len(), n * n, "{layout:?}");
        assert!(built.primitives.iter().all(|p| p[2] != p[3]));
        assert_eq!(built.num_primitive_keys as usize, n * n);
        assert_eq!(built.triangle_keys.iter().flatten().count(), triangles.len());
        for pair in built.triangle_keys.chunks(2) {
            let (first, second) = (pair[0].unwrap(), pair[1].unwrap());
            assert_eq!((first & 1, second), (0, first | 1), "{layout:?}");
        }
        assert_eq!(built.max_key_value & 1, 1);
        written_reads_back(&built, triangles.len());
    }
}

#[test]
fn a_degenerate_triangle_is_left_out_only_when_asked() {
    let (mut vertices, mut triangles) = grid(2, 1.0);
    let first = vertices.len() as u32;
    // legs of 0.01: the squared cross product is 1e-8, under the stock tool's threshold
    vertices.extend([[10.0, 0.0, 10.0], [10.01, 0.0, 10.0], [10.0, 0.0, 10.01]]);
    triangles.push([first, first + 1, first + 2]);
    let source = MeshSource {
        vertices: &vertices,
        triangles: &triangles,
        convex_radius: 0.01,
    };
    let kept = build_compressed_mesh(&source).unwrap();
    assert!(kept.triangle_keys.iter().all(Option::is_some));
    let options = BuildOptions {
        drop_degenerate: Some(havok_formats::DEFAULT_DEGENERATE_THRESHOLD),
        ..BuildOptions::default()
    };
    let dropped = build_compressed_mesh_with(&source, &options).unwrap();
    assert_eq!(dropped.triangle_keys.last(), Some(&None));
    assert_eq!(
        dropped.triangle_keys.iter().flatten().count(),
        triangles.len() - 1
    );
}

#[test]
fn a_mesh_that_cannot_be_built_is_refused() {
    let (vertices, triangles) = grid(2, 1.0);
    let source = |vertices: &'static [[f32; 3]], triangles: &'static [[u32; 3]], radius| MeshSource {
        vertices,
        triangles,
        convex_radius: radius,
    };
    // no triangle
    let e = build_compressed_mesh(&source(&[], &[], 0.01))
        .unwrap_err()
        .to_string();
    assert!(e.contains("no triangle"), "{e}");
    // a triangle with a repeated index, or two vertices at one position, is left out; with nothing else there is no mesh
    let e = build_compressed_mesh(&source(&[[0., 0., 0.], [1., 0., 0.]], &[[0, 1, 1]], 0.01))
        .unwrap_err()
        .to_string();
    assert!(e.contains("no triangle"), "{e}");
    let e = build_compressed_mesh(&source(
        &[[0., 0., 0.], [0., 0., 0.], [1., 0., 0.]],
        &[[0, 1, 2]],
        0.01,
    ))
    .unwrap_err()
    .to_string();
    assert!(e.contains("no triangle"), "{e}");
    // an index past the vertices
    assert!(build_compressed_mesh(&source(&[[0., 0., 0.]], &[[0, 1, 2]], 0.01)).is_err());
    // a vertex that is not finite, a bad radius
    assert!(build_compressed_mesh(&source(&[[f32::NAN, 0., 0.]], &[[0, 0, 0]], 0.01)).is_err());
    assert!(
        build_compressed_mesh(&source(
            &[[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
            &[[0, 1, 2]],
            -1.0
        ))
        .is_err()
    );
    // the built mesh keeps the good triangles and says which it left out
    let mut with_bad = triangles.clone();
    with_bad.insert(1, [0, 0, 1]);
    let built = build_compressed_mesh(&MeshSource {
        vertices: &vertices,
        triangles: &with_bad,
        convex_radius: 0.0,
    })
    .unwrap();
    assert_eq!(built.triangle_keys.len(), with_bad.len());
    assert_eq!(built.triangle_keys[1], None);
    assert_eq!(built.triangle_keys.iter().flatten().count(), triangles.len());
    // the shape writer refuses an item that is not a compressed mesh shape
    let bytes = empty_shape(ELDEN_RING, "fsnpCustomParamCompressedMeshShape").unwrap();
    let tag = Tagfile::read(&bytes, None).unwrap();
    let mut image = tag.image().unwrap();
    let e = replace_compressed_mesh(&mut image, 2, &built)
        .unwrap_err()
        .to_string();
    assert!(e.contains("not a compressed mesh shape"), "{e}");
}

// ---- extern meshes ------------------------------------------------------------------------------------------------

/// A tagfile whose root is an extern mesh shape, with either bounding-volume form.
fn extern_fixture(sdk: &[u8; 8], aabb_tree: bool) -> Built<Vec<u8>> {
    let sekiro = sdk == SEKIRO;
    // 1 u8, 2 u32, 3 real, 4 vec4, 5 aabb, 6 triangle, 7 hkArray<vec4>, 8 hkArray<triangle>, 9 hkGeometry,
    // 10 geometry class pointer, 11 hkGeometry pointer, 12 u32x4, 13 simd node, 14 hkArray<simd node>, 15 simd tree,
    // 16 codec6, 17 hkArray<codec6>, 18 aabb tree, 19 data, 20 data pointer, 21 shape
    let mut node = vec![
        ("lx", 0, 4),
        ("hx", 16, 4),
        ("ly", 32, 4),
        ("hy", 48, 4),
        ("lz", 64, 4),
        ("hz", 80, 4),
        ("data", 96, 12),
    ];
    let mut simd = vec![("nodes", 8, 14)];
    if !sekiro {
        node.push(("isLeaf", 112, 1));
        node.push(("isActive", 113, 1));
        simd.push(("isCompact", 24, 1));
    }
    let mut data = vec![("simdTree", if aabb_tree { 80 } else { 24 }, 15)];
    if aabb_tree {
        data.push(("aabbTree", 32, 18));
    }
    let definitions = vec![
        primitive("hkUint8", 0x2004, 1, None),                     // 1
        primitive("hkUint32", 0x8004, 4, None),                    // 2
        primitive("hkReal", 0x174605, 4, None),                    // 3
        fixed("hkVector4", 4, 3, 4),                               // 4
        record("hkAabb", 32, vec![("min", 0, 4), ("max", 16, 4)]), // 5
        record(
            "hkGeometry::Triangle",
            16,
            vec![("a", 0, 23), ("b", 4, 23), ("c", 8, 23), ("material", 12, 23)],
        ), // 6
        primitive("hkArray", 8, 16, Some(4)),                      // 7
        primitive("hkArray", 8, 16, Some(6)),                      // 8
        record("hkGeometry", 56, vec![("vertices", 24, 7), ("triangles", 40, 8)]), // 9
        primitive("T*", 6, 8, Some(21)),                           // 10 (to the geometry class, set below)
        primitive("hkRefPtr", 6, 8, Some(9)),                      // 11
        fixed("T[N]<hkUint32,4>", 4, 2, 4),                        // 12
        record("hkcdSimdTree::Node", if sekiro { 112 } else { 128 }, node), // 13
        primitive("hkArray", 8, 16, Some(13)),                     // 14
        record("hkcdSimdTree", 32, simd),                          // 15
        record("hkcdStaticTree::Codec3Axis6", 6, vec![]),          // 16
        primitive("hkArray", 8, 16, Some(16)),                     // 17
        record(
            "hkcdStaticTree::DefaultTree6",
            48,
            vec![("nodes", 0, 17), ("domain", 16, 5)],
        ), // 18
        record("hknpExternMeshShapeData", 128, data),              // 19
        primitive("T*", 6, 8, Some(19)),                           // 20
        record(
            "hknpDefaultExternMeshShapeGeometry",
            48,
            vec![("geometry", 32, 11), ("useTriangleMaterialAsShapeTag", 40, 1)],
        ), // 21
        record(
            "hknpExternMeshShape",
            96,
            vec![
                ("numShapeKeyBits", 35, 1),
                ("geometry", 80, 10),
                ("boundingVolumeData", 88, 20),
            ],
        ), // 22
        primitive("int", 0x8204, 4, None),                         // 23
    ];
    let mut layout = Layout::default();
    let (_, shape) = layout.alloc(22, 96, 1, false)?;
    let (geometry, _) = layout.alloc(21, 48, 1, false)?;
    let (hk, _) = layout.alloc(9, 56, 1, false)?;
    let (bv, _) = layout.alloc(19, 128, 1, false)?;
    layout.put(shape + 80, &geometry.to_le_bytes());
    layout.put(shape + 88, &bv.to_le_bytes());
    // the geometry object's pointer to the hkGeometry
    let geometry_at = layout.items[geometry as usize - 1].offset as usize;
    layout.put(geometry_at + 32, &hk.to_le_bytes());
    tagfile(sdk, &definitions, layout.data, &layout.items)
}

#[test]
fn an_extern_mesh_is_written_in_either_form_and_reads_back() {
    let (vertices, triangles) = grid(12, 2.0);
    for sdk in [SEKIRO, ELDEN_RING] {
        for aabb_tree in [true, false] {
            let bytes = extern_fixture(sdk, aabb_tree).unwrap();
            let tag = Tagfile::read(&bytes, None).unwrap();
            let mut image = tag.image().unwrap();
            let shapes = extern_mesh_shapes(&image);
            assert_eq!(shapes, [1]);
            let materials: Vec<i32> = (0..triangles.len() as i32).map(|i| i % 3 - 1).collect();
            replace_extern_mesh(
                &mut image,
                shapes[0],
                &ExternSource {
                    vertices: &vertices,
                    triangles: &triangles,
                    materials: Some(&materials),
                },
            )
            .unwrap();
            let written = image.write().unwrap();
            let tag = Tagfile::read(&written, None).unwrap();
            let shape = tag.root().unwrap();
            let mesh = extern_mesh(shape).unwrap();
            assert_eq!(mesh.vertices, vertices);
            assert_eq!(mesh.triangles, triangles);
            assert_eq!(mesh.materials, materials);
            let bits = 64 - (triangles.len() as u64).leading_zeros() + u32::from(!aabb_tree);
            assert_eq!(
                shape.field("numShapeKeyBits").unwrap().unsigned().unwrap(),
                u64::from(bits),
                "{sdk:?} aabb {aabb_tree}"
            );
            let bv = shape.field("boundingVolumeData").unwrap().dereference().unwrap();
            let simd = simd_tree(bv).unwrap();
            if aabb_tree {
                // only the two empty nodes of the simd tree; the aabb tree has a leaf for each triangle
                assert_eq!(simd.nodes.len(), 2);
                let tree = bv.field("aabbTree").unwrap();
                let nodes = tree.field("nodes").unwrap().array().unwrap();
                assert_eq!(nodes.len(), 2 * triangles.len() - 1);
                let first = nodes.get(0).unwrap();
                let raw = tag.bytes(first.offset, nodes.len() * 6).unwrap();
                let domain = {
                    let d = tree.field("domain").unwrap();
                    let lo = d.field("min").unwrap().vector::<4>().unwrap();
                    let hi = d.field("max").unwrap().vector::<4>().unwrap();
                    [[lo[0], lo[1], lo[2]], [hi[0], hi[1], hi[2]]]
                };
                let decoded = BvTree::decode(raw, NodeWidth::Six, domain, 0).unwrap();
                let leaves = decoded.leaf_nodes(triangles.len()).unwrap();
                for (t, corners) in triangles.iter().enumerate() {
                    for &v in corners {
                        let ulps = decoded.excess_ulps(leaves[t], vertices[v as usize]).unwrap();
                        assert_eq!(ulps, 0.0, "triangle {t} vertex {v}");
                    }
                }
            } else {
                // a leaf per triangle keyed by its index, exact boxes, the files' numbering
                assert_eq!(simd.leaves().count(), triangles.len());
                let mut keys: Vec<u32> = simd.leaves().map(|(k, _)| k).collect();
                keys.sort_unstable();
                assert_eq!(keys, (0..triangles.len() as u32).collect::<Vec<_>>());
                assert_eq!(simd.has_simd_tree, None);
            }
        }
    }
}

#[test]
fn an_extern_mesh_that_cannot_be_written_is_refused() {
    let (vertices, triangles) = grid(2, 1.0);
    let bytes = extern_fixture(ELDEN_RING, false).unwrap();
    let tag = Tagfile::read(&bytes, None).unwrap();
    let image = tag.image().unwrap();
    let source = |triangles: &'static [[u32; 3]], materials: Option<&'static [i32]>| ExternSource {
        vertices: &[[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
        triangles,
        materials,
    };
    let try_write = |s: ExternSource<'_>, item: usize| {
        replace_extern_mesh(&mut image.clone(), item, &s)
            .unwrap_err()
            .to_string()
    };
    assert!(try_write(source(&[], None), 1).contains("no triangle"));
    assert!(try_write(source(&[[0, 1, 3]], None), 1).contains("names vertex 3"));
    assert!(try_write(source(&[[0, 1, 2]], Some(&[1, 2])), 1).contains("2 materials for 1 triangles"));
    assert!(try_write(source(&[[0, 1, 2]], None), 2).contains("not an extern mesh shape"));
    let _ = (vertices, triangles);
}
