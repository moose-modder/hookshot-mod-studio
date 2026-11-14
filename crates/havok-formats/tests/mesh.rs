//! Synthetic hknpCompressedMeshShapeData trees (both SDK layouts of the sections) check how shared vertices are
//! addressed: a section's page, and the widths of the vertex arrays. The class layouts follow the field names and
//! sizes of the Sekiro and Elden Ring type tables; no game payload is embedded here.
#![allow(
    clippy::unwrap_used,
    reason = "a test fixture that does not parse is the test's failure"
)]
mod tagbuilder;

use havok_formats::{CustomKind, SimdLane, Tagfile, compressed_mesh, mesh_sections, mesh_trees, simd_tree};
use tagbuilder::{Built, Definition, Layout, fixed, primitive, record, tagfile};

const SEKIRO: &[u8; 8] = b"20160200";
const ELDEN_RING: &[u8; 8] = b"20180100";

/// The element types (type indices: 2 = 16 bits, 3 = 32, 4 = 64) of `sharedVerticesIndex`, `packedVertices` and
/// `sharedVertices`.
#[derive(Clone, Copy)]
struct Widths {
    index: u32,
    packed: u32,
    shared: u32,
}
const STORED: Widths = Widths {
    index: 2,
    packed: 3,
    shared: 4,
};

/// One section as the fixture stores it.
#[derive(Clone, Copy)]
struct Section {
    first_packed: u32,
    packed: u8,
    first_shared: u32,
    shared_indices: u8,
    first_primitive: u32,
    primitives: u8,
    page: u8,
    /// The section's flags (`SectionFlags`): `REQUIRE_TREE` exactly when it holds a custom primitive.
    flags: u8,
}

/// The `hkAabb` of every domain here: a shared vertex's three codes then decode to themselves (the step is exactly 1).
const DOMAIN: [f32; 8] = [0.0, 0.0, 0.0, 0.0, 2_097_151.0, 2_097_151.0, 4_194_303.0, 0.0];

/// A shared vertex's code: `x` in the low 21 bits, `y` in the next 21, `z` in the top 22.
const fn shared_code(x: u64, y: u64, z: u64) -> u64 {
    x | (y << 21) | (z << 42)
}

fn definitions(sdk: &[u8; 8], widths: Widths) -> Vec<Definition> {
    let section = if sdk == SEKIRO {
        // sharedVertices.data = first shared index << 8 | packed vertices, primitives.data = first primitive << 8 | count
        vec![
            ("codecParms", 0, 7),
            ("domain", 32, 9),
            ("firstPackedVertex", 64, 3),
            ("sharedVertices", 68, 12),
            ("primitives", 72, 13),
            ("dataRuns", 76, 14),
            ("numPackedVertices", 80, 1),
            ("numSharedIndices", 81, 1),
            ("leafIndex", 82, 2),
            ("nodes", 88, 23),
            ("page", 84, 1),
            ("flags", 85, 1),
            ("layerData", 86, 1),
            ("unusedData", 87, 1),
        ]
    } else {
        vec![
            ("codecParms", 0, 7),
            ("domain", 32, 9),
            ("firstPackedVertexIndex", 64, 3),
            ("firstSharedVertexIndex", 68, 3),
            ("firstPrimitiveIndex", 72, 3),
            ("firstDataRunIndex", 76, 3),
            ("numPackedVertices", 80, 1),
            ("numPrimitives", 81, 1),
            ("numDataRuns", 82, 1),
            ("page", 83, 1),
            ("leafIndex", 84, 2),
            ("nodes", 88, 23),
            ("layerData", 86, 1),
            ("flags", 87, 1),
        ]
    };
    // Sekiro's simd nodes are four boxes and four tagged words; Elden Ring's add the node flag, and its data a flag
    let mut node = vec![
        ("lx", 0, 6),
        ("hx", 16, 6),
        ("ly", 32, 6),
        ("hy", 48, 6),
        ("lz", 64, 6),
        ("hz", 80, 6),
        ("data", 96, 25),
    ];
    let mut data = vec![("meshTree", 0, 20), ("simdTree", 128, 28)];
    if sdk != SEKIRO {
        node.push(("isLeaf", 112, 1));
        data.push(("hasSimdTree", 152, 1));
    }
    vec![
        primitive("hkUint8", 0x2004, 1, None),                               // 1
        primitive("hkUint16", 0x4004, 2, None),                              // 2
        primitive("hkUint32", 0x8004, 4, None),                              // 3
        primitive("hkUint64", 0x10004, 8, None),                             // 4
        primitive("hkReal", 0x174605, 4, None),                              // 5
        fixed("T[N]<hkReal,4>", 4, 5, 4),                                    // 6
        fixed("T[N]<hkReal,6>", 6, 5, 4),                                    // 7
        fixed("T[N]<hkUint8,4>", 4, 1, 1),                                   // 8
        record("hkAabb", 32, vec![("min", 0, 6), ("max", 16, 6)]),           // 9
        record("hkcdStaticMeshTree::Primitive", 4, vec![("indices", 0, 8)]), // 10
        record("hkcdStaticMeshTree::Section", SECTION_SIZE as u32, section), // 11
        record(
            "hkcdStaticMeshTree::Section::SharedVertices",
            4,
            vec![("data", 0, 3)],
        ), // 12
        record("hkcdStaticMeshTree::Section::Primitives", 4, vec![("data", 0, 3)]), // 13
        record("hkcdStaticMeshTree::Section::DataRuns", 4, vec![("data", 0, 3)]), // 14
        primitive("hkArray", 8, 16, Some(11)),                               // 15 sections
        primitive("hkArray", 8, 16, Some(10)),                               // 16 primitives
        primitive("hkArray", 8, 16, Some(widths.index)),                     // 17 sharedVerticesIndex
        primitive("hkArray", 8, 16, Some(widths.packed)),                    // 18 packedVertices
        primitive("hkArray", 8, 16, Some(widths.shared)),                    // 19 sharedVertices
        record(
            "hkcdStaticMeshTree",
            128,
            vec![
                ("nodes", 112, 24),
                ("domain", 0, 9),
                ("sections", 32, 15),
                ("primitives", 48, 16),
                ("sharedVerticesIndex", 64, 17),
                ("packedVertices", 80, 18),
                ("sharedVertices", 96, 19),
            ],
        ), // 20
        record("hknpCompressedMeshShapeData", 160, data),                    // 21
        fixed("T[N]<hkUint8,5>", 5, 1, 1),                                   // 22 a mesh tree node
        primitive("hkArray", 8, 16, Some(8)), // 23 section nodes (4 bytes each)
        primitive("hkArray", 8, 16, Some(22)), // 24 mesh nodes (5 bytes each)
        fixed("T[N]<hkUint32,4>", 4, 3, 4),   // 25 a simd node's data words
        record("hkcdSimdTree::Node", if sdk == SEKIRO { 112 } else { 128 }, node), // 26
        primitive("hkArray", 8, 16, Some(26)), // 27 simd nodes
        record("hkcdSimdTree", 24, vec![("nodes", 0, 27)]), // 28
    ]
}

/// The bytes of a section of the fixtures' section records (the SDK layouts differ in fields, not in size).
const SECTION_SIZE: usize = 104;

/// A tree of `leaves` leaves (payloads `0..leaves`) with zero nibbles, every second child two nodes on, so each
/// internal node's first child is a leaf: `width`-byte nodes (4: a section tree, 5: a mesh tree).
fn chain(width: usize, leaves: usize) -> Vec<u8> {
    let node = |internal: bool, payload: usize| -> Vec<u8> {
        let mut node = vec![0u8; 3];
        if width == 4 {
            // internal: second child 2 nodes on; leaf: the payload in the high seven bits
            node.push(if internal { 3 } else { (payload << 1) as u8 });
        } else {
            let value: u16 = if internal { 0x8001 } else { payload as u16 };
            node.extend(value.to_be_bytes());
        }
        node
    };
    let mut out = Vec::new();
    for k in 0..leaves {
        if k + 1 < leaves {
            out.extend(node(true, 0));
            out.extend(node(false, k));
        } else {
            out.extend(node(false, k));
        }
    }
    out
}

/// The node of the leaf for payload `k` of a [`chain`] of `leaves` leaves.
fn chain_leaf(leaves: usize, k: usize) -> usize {
    if k + 1 < leaves { 2 * k + 1 } else { 2 * k }
}

/// The bytes an element of type `type_index` (2, 3 or 4) takes.
fn width(type_index: u32) -> usize {
    match type_index {
        2 => 2,
        3 => 4,
        _ => 8,
    }
}

/// A lane of a simd node as a test states it; each encoding below writes it as its SDK version stores it.
#[derive(Clone, Copy)]
enum Lane {
    Empty,
    Child(u32, [[f32; 3]; 2]),
    Leaf(u32, [[f32; 3]; 2]),
}

/// The box of an empty lane: its minimum above its maximum.
const EMPTY_BOX: [[f32; 3]; 2] = [[f32::MAX; 3], [-f32::MAX; 3]];

/// The values of a mesh tree's arrays: `shared` are the (slot, code) pairs set among `shared_len` shared vertices.
struct Tree<'a> {
    sections: &'a [Section],
    primitives: &'a [[u8; 4]],
    entries: &'a [u32],
    packed: &'a [u32],
    shared_len: usize,
    shared: &'a [(usize, u64)],
    /// The mesh tree's nodes, each section's, and each section's stored `leafIndex`.
    mesh_nodes: &'a [u8],
    section_nodes: &'a [Vec<u8>],
    leaf_indices: &'a [u16],
    /// The simd tree's nodes, and Elden Ring's `hasSimdTree`.
    simd: &'a [[Lane; 4]],
    has_simd_tree: u8,
}

/// A tagfile whose root is an hknpCompressedMeshShapeData holding `tree`.
fn mesh_fixture(sdk: &[u8; 8], widths: Widths, tree: &Tree<'_>) -> Built<Vec<u8>> {
    let sekiro = sdk == SEKIRO;
    let mut layout = Layout::default();
    let (_, root) = layout.alloc(21, 160, 1, false)?;
    for (i, v) in DOMAIN.iter().enumerate() {
        layout.put(root + 4 * i, &v.to_le_bytes());
    }
    // an array member of the tree at `field`: its item and the item's DATA offset (none for an empty array)
    let array =
        |layout: &mut Layout, field: usize, type_index: u32, size: usize, count: usize| -> Built<usize> {
            if count == 0 {
                return Ok(0);
            }
            let (item, offset) = layout.alloc(type_index, size, u32::try_from(count)?, true)?;
            layout.put(root + field, &item.to_le_bytes());
            Ok(offset)
        };
    let at = array(
        &mut layout,
        32,
        11,
        SECTION_SIZE * tree.sections.len(),
        tree.sections.len(),
    )?;
    let nodes = array(
        &mut layout,
        112,
        22,
        tree.mesh_nodes.len(),
        tree.mesh_nodes.len() / 5,
    )?;
    layout.put(nodes, tree.mesh_nodes);
    for (i, s) in tree.sections.iter().enumerate() {
        let base = at + SECTION_SIZE * i;
        let bytes = &tree.section_nodes[i];
        let (item, offset) = layout.alloc(8, bytes.len(), u32::try_from(bytes.len() / 4)?, true)?;
        layout.put(offset, bytes);
        layout.put(base + 88, &item.to_le_bytes());
        layout.put(
            base + if sekiro { 82 } else { 84 },
            &tree.leaf_indices[i].to_le_bytes(),
        );
        // codecParms: offset xyz, scale xyz (the packed vertices decode to their codes)
        for (k, v) in [0.0f32, 0.0, 0.0, 1.0, 1.0, 1.0].iter().enumerate() {
            layout.put(base + 4 * k, &v.to_le_bytes());
        }
        for (k, v) in DOMAIN.iter().enumerate() {
            layout.put(base + 32 + 4 * k, &v.to_le_bytes());
        }
        layout.put(base + 64, &s.first_packed.to_le_bytes());
        if sekiro {
            layout.put(
                base + 68,
                &((s.first_shared << 8) | u32::from(s.packed)).to_le_bytes(),
            );
            layout.put(
                base + 72,
                &((s.first_primitive << 8) | u32::from(s.primitives)).to_le_bytes(),
            );
            layout.put(base + 80, &[s.packed, s.shared_indices]);
            layout.put(base + 84, &[s.page, s.flags]);
        } else {
            layout.put(base + 68, &s.first_shared.to_le_bytes());
            layout.put(base + 72, &s.first_primitive.to_le_bytes());
            layout.put(base + 80, &[s.packed, s.primitives, 0, s.page]);
            layout.put(base + 87, &[s.flags]);
        }
    }
    // the simd tree: node by node, lanes as boxes and data words
    let node_size = if sekiro { 112 } else { 128 };
    let at = array(&mut layout, 128, 26, node_size * tree.simd.len(), tree.simd.len())?;
    if !sekiro {
        layout.put(root + 152, &[tree.has_simd_tree]);
    }
    for (i, lanes) in tree.simd.iter().enumerate() {
        let base = at + node_size * i;
        // Elden Ring flags a node whose lanes are all leaves (or empty); Sekiro tags each word
        let leaf_node = lanes.iter().any(|l| matches!(l, Lane::Leaf(..)))
            && lanes.iter().all(|l| matches!(l, Lane::Leaf(..) | Lane::Empty));
        for (lane, l) in lanes.iter().enumerate() {
            let (aabb, word) = match *l {
                Lane::Empty => (EMPTY_BOX, if sekiro || !leaf_node { 0 } else { u32::MAX }),
                Lane::Child(node, aabb) => (aabb, if sekiro { node << 1 } else { node }),
                Lane::Leaf(key, aabb) => (aabb, if sekiro { key << 1 | 1 } else { key }),
            };
            // lx hx ly hy lz hz, four lanes each
            for (k, value) in [
                aabb[0][0], aabb[1][0], aabb[0][1], aabb[1][1], aabb[0][2], aabb[1][2],
            ]
            .into_iter()
            .enumerate()
            {
                layout.put(base + 16 * k + 4 * lane, &value.to_le_bytes());
            }
            layout.put(base + 96 + 4 * lane, &word.to_le_bytes());
        }
        if !sekiro {
            layout.put(base + 112, &[u8::from(leaf_node)]);
        }
    }
    let at = array(
        &mut layout,
        48,
        10,
        4 * tree.primitives.len(),
        tree.primitives.len(),
    )?;
    for (i, p) in tree.primitives.iter().enumerate() {
        layout.put(at + 4 * i, p);
    }
    let n = width(widths.index);
    let at = array(
        &mut layout,
        64,
        widths.index,
        n * tree.entries.len(),
        tree.entries.len(),
    )?;
    for (i, e) in tree.entries.iter().enumerate() {
        layout.put(at + n * i, &e.to_le_bytes()[..n]);
    }
    let n = width(widths.packed);
    let at = array(
        &mut layout,
        80,
        widths.packed,
        n * tree.packed.len(),
        tree.packed.len(),
    )?;
    for (i, e) in tree.packed.iter().enumerate() {
        layout.put(at + n * i, &e.to_le_bytes()[..n]);
    }
    let n = width(widths.shared);
    let at = array(
        &mut layout,
        96,
        widths.shared,
        n * tree.shared_len,
        tree.shared_len,
    )?;
    for &(slot, code) in tree.shared {
        layout.put(at + n * slot, &code.to_le_bytes()[..n]);
    }
    tagfile(sdk, &definitions(sdk, widths), layout.data, &layout.items)
}

/// Shared vertices 0 to 2 (page 0) and 65,536 and 65,537 (page 1).
const SHARED: [(usize, u64); 5] = [
    (0, shared_code(1, 2, 3)),
    (1, shared_code(4, 5, 6)),
    (2, shared_code(7, 8, 9)),
    (65_536, shared_code(11, 12, 13)),
    (65_537, shared_code(14, 15, 16)),
];

/// Two sections: section 0 (page 0) has one triangle on shared entries 0, 1 and 2; section 1 (page `page`) has one
/// triangle on its packed vertex (code x 1, y 2, z 3) and its own shared entries 0 and 1, which store the same 16-bit
/// values as section 0's first two.
fn two_pages_with(sdk: &[u8; 8], widths: Widths, page: u8, trees: &Trees) -> Built<Vec<u8>> {
    let sections = [
        Section {
            first_packed: 0,
            packed: 0,
            first_shared: 0,
            shared_indices: 3,
            first_primitive: 0,
            primitives: 1,
            page: 0,
            flags: 0,
        },
        Section {
            first_packed: 0,
            packed: 1,
            first_shared: 3,
            shared_indices: 2,
            first_primitive: 1,
            primitives: 1,
            page,
            flags: 0,
        },
    ];
    mesh_fixture(
        sdk,
        widths,
        &Tree {
            sections: &sections,
            primitives: &[[0, 1, 2, 2], [0, 1, 2, 2]],
            entries: &[0, 1, 2, 0, 1],
            packed: &[1 | (2 << 11) | (3 << 22)],
            shared_len: 65_538,
            shared: &SHARED,
            mesh_nodes: &trees.mesh,
            section_nodes: &trees.sections,
            leaf_indices: &trees.leaf_indices,
            simd: &trees.simd,
            has_simd_tree: trees.has_simd_tree,
        },
    )
}

/// The trees of the two-section fixture: the mesh tree's nodes, each section's, and each section's stored `leafIndex`.
struct Trees {
    mesh: Vec<u8>,
    sections: [Vec<u8>; 2],
    leaf_indices: [u16; 2],
    simd: Vec<[Lane; 4]>,
    has_simd_tree: u8,
}

/// The box of the first triangle (section 0: its three shared vertices) and of the second (section 1: its packed
/// vertex and two shared ones).
const FIRST_BOX: [[f32; 3]; 2] = [[1., 2., 3.], [7., 8., 9.]];
const SECOND_BOX: [[f32; 3]; 2] = [[1., 2., 3.], [14., 15., 16.]];

impl Trees {
    /// Valid trees: zero nibbles throughout, so every box is the domain and holds the fixture's vertices.
    fn valid() -> Self {
        Self {
            mesh: chain(5, 2),
            sections: [chain(4, 1), chain(4, 1)],
            leaf_indices: [chain_leaf(2, 0) as u16, chain_leaf(2, 1) as u16],
            // the sentinel, then the root with a leaf for each triangle (shape keys 0 and 256)
            simd: vec![
                [Lane::Empty; 4],
                [
                    Lane::Leaf(0, FIRST_BOX),
                    Lane::Leaf(256, SECOND_BOX),
                    Lane::Empty,
                    Lane::Empty,
                ],
            ],
            has_simd_tree: 1,
        }
    }
}

fn two_pages(sdk: &[u8; 8], widths: Widths, page: u8) -> Built<Vec<u8>> {
    two_pages_with(sdk, widths, page, &Trees::valid())
}

#[test]
fn a_sections_page_selects_its_shared_vertices() -> Built<()> {
    for sdk in [SEKIRO, ELDEN_RING] {
        let bytes = two_pages(sdk, STORED, 1)?;
        let tag = Tagfile::read(&bytes, None).unwrap();
        let data = tag.root().unwrap();
        let sections = mesh_sections(data).unwrap();
        assert_eq!(sections.iter().map(|s| s.page).collect::<Vec<_>>(), [0, 1]);
        assert_eq!(
            sections[1].domain,
            [[0.0; 3], [2_097_151.0, 2_097_151.0, 4_194_303.0]]
        );
        let mesh = compressed_mesh(data).unwrap();
        let triangles: Vec<_> = mesh.triangles.iter().map(|t| (t.key, t.vertices)).collect();
        assert_eq!(
            triangles,
            [
                (0, [[1., 2., 3.], [4., 5., 6.], [7., 8., 9.]]),
                // the packed vertex, then shared entries 0 and 1 of page 1 (slots 65,536 and 65,537); a reader that
                // ignored the page would give slots 0 and 1 here
                (256, [[1., 2., 3.], [11., 12., 13.], [14., 15., 16.]]),
            ],
            "{sdk:?}"
        );
    }
    Ok(())
}

#[test]
fn a_page_past_the_shared_vertices_is_refused() -> Built<()> {
    for sdk in [SEKIRO, ELDEN_RING] {
        let bytes = two_pages(sdk, STORED, 2)?;
        let tag = Tagfile::read(&bytes, None).unwrap();
        let e = compressed_mesh(tag.root().unwrap()).unwrap_err().to_string();
        assert!(e.contains("out of bounds"), "{e}");
    }
    Ok(())
}

#[test]
fn vertex_arrays_of_another_width_are_refused() -> Built<()> {
    for sdk in [SEKIRO, ELDEN_RING] {
        for (widths, name) in [
            (Widths { index: 3, ..STORED }, "sharedVerticesIndex"),
            (Widths { packed: 2, ..STORED }, "packedVertices"),
            (Widths { shared: 3, ..STORED }, "sharedVertices"),
        ] {
            let bytes = two_pages(sdk, widths, 1)?;
            let tag = Tagfile::read(&bytes, None).unwrap();
            let e = compressed_mesh(tag.root().unwrap()).unwrap_err().to_string();
            assert!(e.contains(name) && e.contains("bytes, not the"), "{name}: {e}");
        }
    }
    Ok(())
}

/// Every triangle of the fixture sits inside the box of its leaf, so the trees are counted and nothing is outside.
#[test]
fn the_trees_of_a_mesh_are_counted_and_its_triangles_sit_in_their_leaf_boxes() -> Built<()> {
    for sdk in [SEKIRO, ELDEN_RING] {
        let bytes = two_pages(sdk, STORED, 1)?;
        let tag = Tagfile::read(&bytes, None).unwrap();
        let data = tag.root().unwrap();
        let trees = mesh_trees(data).unwrap();
        assert_eq!(trees.mesh.nodes().len(), 3);
        assert_eq!(trees.mesh_leaves, [1, 2]);
        assert_eq!(trees.sections.len(), 2);
        assert_eq!(
            trees.sections[1].leaf(0).unwrap().aabb,
            [[0.0; 3], [2_097_151.0, 2_097_151.0, 4_194_303.0]]
        );
        assert!(trees.sections[1].leaf(1).is_none());
        let b = compressed_mesh(data).unwrap().bounds;
        assert_eq!(
            (
                b.mesh_trees,
                b.section_trees,
                b.nodes,
                b.mesh_leaves,
                b.section_leaves
            ),
            (1, 2, 5, 2, 2),
            "{sdk:?}"
        );
        assert_eq!(
            (
                b.primitives,
                b.inside,
                b.rounding,
                b.outside,
                b.inverted_boxes,
                b.worst_ulps
            ),
            (2, 2, 0, 0, 0, 0.0),
            "{sdk:?}"
        );
    }
    Ok(())
}

/// A leaf box that leaves a vertex out is a finding, counted; the triangles are still read.
#[test]
fn a_triangle_outside_its_leaf_box_is_counted_and_still_read() -> Built<()> {
    for sdk in [SEKIRO, ELDEN_RING] {
        // section 0's only node shrinks x from the minimum side by (15 squared) 225/226 of the domain: its vertices (x 1, 4, 7)
        // lie far outside; section 1's tree is untouched
        let mut trees = Trees::valid();
        trees.sections[0][0] = 0xf0;
        let bytes = two_pages_with(sdk, STORED, 1, &trees)?;
        let tag = Tagfile::read(&bytes, None).unwrap();
        let mesh = compressed_mesh(tag.root().unwrap()).unwrap();
        assert_eq!(mesh.triangles.len(), 2);
        let b = mesh.bounds;
        assert_eq!(
            (b.primitives, b.inside, b.rounding, b.outside),
            (2, 1, 0, 1),
            "{sdk:?}"
        );
        assert!(b.worst_ulps > 1e6, "{}", b.worst_ulps);
    }
    Ok(())
}

#[test]
fn trees_that_do_not_hold_together_are_refused() -> Built<()> {
    for sdk in [SEKIRO, ELDEN_RING] {
        let read = |trees: &Trees| -> Built<String> {
            let bytes = two_pages_with(sdk, STORED, 1, trees)?;
            let tag = Tagfile::read(&bytes, None).unwrap();
            let e = compressed_mesh(tag.root().unwrap()).unwrap_err().to_string();
            // `mesh_trees` makes the same refusal
            assert_eq!(mesh_trees(tag.root().unwrap()).unwrap_err().to_string(), e);
            Ok(e)
        };
        // a section whose stored leafIndex is not its leaf's node in the mesh tree
        let mut trees = Trees::valid();
        trees.leaf_indices = [0, 2];
        let e = read(&trees)?;
        assert!(
            e.contains("section 0: its leafIndex is 0, its leaf in the mesh tree is node 1"),
            "{e}"
        );
        // a mesh tree with no leaf for one of the sections
        let mut trees = Trees::valid();
        trees.mesh = chain(5, 1);
        let e = read(&trees)?;
        assert!(e.contains("no leaf names 1 of 2"), "{e}");
        // a mesh tree whose leaves name a section twice
        let mut trees = Trees::valid();
        trees.mesh[14] = 0;
        let e = read(&trees)?;
        assert!(e.contains("both name 0"), "{e}");
        // a section tree with a leaf for a primitive the section does not have
        let mut trees = Trees::valid();
        trees.sections[0] = chain(4, 2);
        let e = read(&trees)?;
        assert!(
            e.contains("section 0:") && e.contains("names 1, outside the 1 it must cover"),
            "{e}"
        );
        // a section tree with no leaf for its primitive
        let mut trees = Trees::valid();
        trees.sections[1] = Vec::new();
        let e = read(&trees)?;
        assert!(
            e.contains("section 1:") && e.contains("no leaf names 0 of 1"),
            "{e}"
        );
        // a structurally broken mesh tree: a leaf that does not end its subtree
        let mut trees = Trees::valid();
        trees.mesh = [chain(5, 2), chain(5, 1)].concat();
        let e = read(&trees)?;
        assert!(e.contains("does not end its subtree"), "{e}");
    }
    Ok(())
}

/// The first and second triangle's keys as the simd tree names them: the shape key of the primitive's first half.
const KEYS: [u32; 2] = [0, 256];

#[test]
fn the_simd_tree_is_decoded_and_its_leaves_are_the_primitives() -> Built<()> {
    for sdk in [SEKIRO, ELDEN_RING] {
        let bytes = two_pages(sdk, STORED, 1)?;
        let tag = Tagfile::read(&bytes, None).unwrap();
        let data = tag.root().unwrap();
        let simd = simd_tree(data).unwrap();
        assert_eq!(simd.nodes.len(), 2);
        assert_eq!(simd.has_simd_tree, (sdk != SEKIRO).then_some(true));
        assert_eq!(simd.nodes[0].lanes, [SimdLane::Empty; 4]);
        assert_eq!(
            simd.leaves().collect::<Vec<_>>(),
            [(KEYS[0], FIRST_BOX), (KEYS[1], SECOND_BOX)],
            "{sdk:?}"
        );
        let c = compressed_mesh(data).unwrap().simd;
        assert_eq!((c.trees, c.leafless, c.nodes, c.leaves), (1, 0, 2, 2), "{sdk:?}");
        assert_eq!(
            (
                c.leaves_exact,
                c.leaves_looser,
                c.leaves_rounding,
                c.leaves_outside
            ),
            (2, 0, 0, 0),
            "{sdk:?}"
        );
        assert_eq!((c.children_exact, c.children_looser, c.children_short), (0, 0, 0));
    }
    Ok(())
}

#[test]
fn child_lanes_and_leaf_boxes_are_counted_against_what_they_enclose() -> Built<()> {
    let union = [FIRST_BOX[0], SECOND_BOX[1]];
    let looser = [[0., 2., 3.], SECOND_BOX[1]];
    let short = [FIRST_BOX[0], [14., 15., 15.]];
    let sentinel = [Lane::Empty; 4];
    let leaves = [
        Lane::Leaf(KEYS[0], FIRST_BOX),
        Lane::Leaf(KEYS[1], SECOND_BOX),
        Lane::Empty,
        Lane::Empty,
    ];
    for sdk in [SEKIRO, ELDEN_RING] {
        // a child lane's box against the union of the lanes below it: equal, larger, and short of it
        let mut trees = Trees::valid();
        for (aabb, want) in [(union, (1, 0, 0)), (looser, (0, 1, 0)), (short, (0, 0, 1))] {
            trees.simd = vec![
                sentinel,
                [Lane::Child(2, aabb), Lane::Empty, Lane::Empty, Lane::Empty],
                leaves,
            ];
            let bytes = two_pages_with(sdk, STORED, 1, &trees)?;
            let tag = Tagfile::read(&bytes, None).unwrap();
            let c = compressed_mesh(tag.root().unwrap()).unwrap().simd;
            assert_eq!(
                (c.children_exact, c.children_looser, c.children_short),
                want,
                "{sdk:?}"
            );
        }
        // leaf boxes: the first larger than its triangle, the second a point, far short of it
        let mut trees = Trees::valid();
        trees.simd[1][0] = Lane::Leaf(KEYS[0], [[0., 2., 3.], FIRST_BOX[1]]);
        trees.simd[1][1] = Lane::Leaf(KEYS[1], [SECOND_BOX[0], SECOND_BOX[0]]);
        let bytes = two_pages_with(sdk, STORED, 1, &trees)?;
        let tag = Tagfile::read(&bytes, None).unwrap();
        let c = compressed_mesh(tag.root().unwrap()).unwrap().simd;
        assert_eq!(
            (
                c.leaves_exact,
                c.leaves_looser,
                c.leaves_rounding,
                c.leaves_outside
            ),
            (0, 1, 0, 1),
            "{sdk:?}"
        );
    }
    Ok(())
}

#[test]
fn a_simd_tree_with_no_leaf_is_counted_and_the_triangles_are_read_all_the_same() -> Built<()> {
    for sdk in [SEKIRO, ELDEN_RING] {
        let mut trees = Trees::valid();
        trees.simd = vec![[Lane::Empty; 4], [Lane::Empty; 4]];
        trees.has_simd_tree = 0;
        let bytes = two_pages_with(sdk, STORED, 1, &trees)?;
        let tag = Tagfile::read(&bytes, None).unwrap();
        let mesh = compressed_mesh(tag.root().unwrap()).unwrap();
        assert_eq!((mesh.simd.trees, mesh.simd.leafless, mesh.simd.leaves), (1, 1, 0));
        assert_eq!(mesh.triangles.len(), 2, "the triangles are read all the same");
    }
    Ok(())
}

#[test]
fn simd_trees_that_do_not_hold_together_are_refused() -> Built<()> {
    for sdk in [SEKIRO, ELDEN_RING] {
        let read = |trees: &Trees| -> Built<String> {
            let bytes = two_pages_with(sdk, STORED, 1, trees)?;
            let tag = Tagfile::read(&bytes, None).unwrap();
            Ok(compressed_mesh(tag.root().unwrap()).unwrap_err().to_string())
        };
        let sentinel = [Lane::Empty; 4];
        let leaves = [
            Lane::Leaf(KEYS[0], FIRST_BOX),
            Lane::Leaf(KEYS[1], SECOND_BOX),
            Lane::Empty,
            Lane::Empty,
        ];
        // a primitive with no leaf
        let mut trees = Trees::valid();
        trees.simd = vec![
            sentinel,
            [
                Lane::Leaf(KEYS[0], FIRST_BOX),
                Lane::Empty,
                Lane::Empty,
                Lane::Empty,
            ],
        ];
        let e = read(&trees)?;
        assert!(e.contains("1 leaves for the mesh's 2"), "{sdk:?}: {e}");
        // a leaf that names no primitive
        let mut trees = Trees::valid();
        trees.simd = vec![
            sentinel,
            [
                Lane::Leaf(KEYS[0], FIRST_BOX),
                Lane::Leaf(257, SECOND_BOX),
                Lane::Empty,
                Lane::Empty,
            ],
        ];
        let e = read(&trees)?;
        assert!(
            e.contains("names shape key 257 where the mesh's next primitive is 256"),
            "{sdk:?}: {e}"
        );
        // a first node that is not the sentinel
        let mut trees = Trees::valid();
        trees.simd = vec![leaves, leaves];
        let e = read(&trees)?;
        assert!(
            e.contains("simd node 0 is not the empty sentinel"),
            "{sdk:?}: {e}"
        );
        // a node that no lane has as its child
        let mut trees = Trees::valid();
        trees.simd = vec![sentinel, leaves, sentinel];
        let e = read(&trees)?;
        assert!(
            e.contains("simd node 2 is the child of 0 lanes, not 1"),
            "{sdk:?}: {e}"
        );
        // a child that is not above its node
        let mut trees = Trees::valid();
        trees.simd = vec![
            sentinel,
            [Lane::Child(1, FIRST_BOX), Lane::Empty, Lane::Empty, Lane::Empty],
        ];
        let e = read(&trees)?;
        assert!(e.contains("is not a later node"), "{sdk:?}: {e}");
        // a lane holding a leaf whose box is empty
        let mut trees = Trees::valid();
        trees.simd = vec![
            sentinel,
            [
                Lane::Leaf(KEYS[0], EMPTY_BOX),
                Lane::Leaf(KEYS[1], SECOND_BOX),
                Lane::Empty,
                Lane::Empty,
            ],
        ];
        let e = read(&trees)?;
        assert!(e.contains("its box is empty but its data word"), "{sdk:?}: {e}");
        if sdk == ELDEN_RING {
            // the flag and the leaves must agree
            let mut trees = Trees::valid();
            trees.has_simd_tree = 0;
            let e = read(&trees)?;
            assert!(
                e.contains("hasSimdTree is clear and the simd tree has leaves"),
                "{e}"
            );
            let mut trees = Trees::valid();
            trees.simd = vec![sentinel, sentinel];
            let e = read(&trees)?;
            assert!(
                e.contains("hasSimdTree is set, the mesh has primitives and the simd tree has no leaf"),
                "{e}"
            );
        }
    }
    Ok(())
}

/// A point-set code: `x` in the low 11 bits, `y` in the next 11, `z` in the top 10.
const fn point_code(x: u64, y: u64, z: u64) -> u64 {
    x | (y << 11) | (z << 22)
}

/// A fixture of one section whose one primitive is a custom primitive `[0, 0, 3, 0]` with the record `entries` (header,
/// slot, tag) in the shared-vertex index, and `shared` shared vertices holding the point codes.
fn point_set_fixture(
    sdk: &[u8; 8],
    entries: &[u32],
    shared: &[(usize, u64)],
    shared_len: usize,
) -> Built<Vec<u8>> {
    let sections = [Section {
        first_packed: 0,
        packed: 0,
        first_shared: 0,
        shared_indices: 3,
        first_primitive: 0,
        primitives: 1,
        page: 0,
        flags: havok_formats::SectionFlags::REQUIRE_TREE,
    }];
    mesh_fixture(
        sdk,
        STORED,
        &Tree {
            sections: &sections,
            primitives: &[[0, 0, 3, 0]],
            entries,
            packed: &[],
            shared_len,
            shared,
            mesh_nodes: &chain(5, 1),
            section_nodes: &[chain(4, 1)],
            leaf_indices: &[0],
            simd: &[
                [Lane::Empty; 4],
                [Lane::Leaf(0, FIRST_BOX), Lane::Empty, Lane::Empty, Lane::Empty],
            ],
            has_simd_tree: 1,
        },
    )
}

/// A custom primitive with a `0x52` record decodes to the points of its record: 11/11/10-bit codes, two to a shared
/// vertex (the even point in the high half), over the box of its leaf in the section's tree (here the section's domain,
/// as the tree's nibbles are zero).
#[test]
fn a_point_set_decodes_over_its_leaf_box() -> Built<()> {
    for sdk in [SEKIRO, ELDEN_RING] {
        let first = (point_code(0, 0, 0) << 32) | point_code(2047, 2047, 1023);
        let second = point_code(1024, 512, 256) << 32;
        // header: three points, record type 2 with 32-bit codes and one tag word
        let bytes = point_set_fixture(sdk, &[0x0352, 0, 0x3d4d], &[(0, first), (1, second)], 2)?;
        let tag = Tagfile::read(&bytes, None).unwrap();
        let mesh = compressed_mesh(tag.root().unwrap()).unwrap();
        assert!(mesh.triangles.is_empty(), "{sdk:?}");
        assert_eq!(mesh.custom.len(), 1, "{sdk:?}");
        let custom = &mesh.custom[0];
        assert_eq!(custom.kind, CustomKind::Unread, "{sdk:?}");
        let points = custom.points.as_ref().unwrap();
        assert_eq!(points.tag, 0x3d4d, "{sdk:?}");
        // the leaf box is the fixture's domain, which starts at the origin
        let scale = [
            (2_097_151.0f64 / 2047.0) as f32,
            (2_097_151.0f64 / 2047.0) as f32,
            (4_194_303.0f64 / 1023.0) as f32,
        ];
        let want = |c: [u32; 3]| {
            [
                c[0] as f32 * scale[0],
                c[1] as f32 * scale[1],
                c[2] as f32 * scale[2],
            ]
        };
        assert_eq!(
            points.vertices,
            [want([0, 0, 0]), want([2047, 2047, 1023]), want([1024, 512, 256])],
            "{sdk:?}"
        );
    }
    Ok(())
}

/// A record with no points, or whose points run past the shared vertices, is refused.
#[test]
fn a_point_set_that_cannot_be_read_is_refused() -> Built<()> {
    for sdk in [SEKIRO, ELDEN_RING] {
        let code = point_code(1, 2, 3) << 32;
        let bytes = point_set_fixture(sdk, &[0x0052, 0, 0x3d4d], &[(0, code)], 1)?;
        let tag = Tagfile::read(&bytes, None).unwrap();
        let e = compressed_mesh(tag.root().unwrap()).unwrap_err().to_string();
        assert!(e.contains("a point set without points"), "{sdk:?}: {e}");
        // two points from slot 5 of one shared vertex
        let bytes = point_set_fixture(sdk, &[0x0252, 5, 0x3d4d], &[(0, code)], 1)?;
        let tag = Tagfile::read(&bytes, None).unwrap();
        let e = compressed_mesh(tag.root().unwrap()).unwrap_err().to_string();
        assert!(e.contains("out of bounds"), "{sdk:?}: {e}");
    }
    Ok(())
}
