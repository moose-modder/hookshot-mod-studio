//! Real-data check of the navigation readers (`havok_formats::navmesh`): twelve tagfiles of the Sekiro and Elden Ring
//! navmesh binders (`*.nvmhktbnd.dcx`), taken out of the user's own installs (see tests/inputs/README.md); skips
//! without them. No game data is included in the repository.
//!
//! The expected counts of each file are those a census of the binders measured with its own reading of the same
//! tagfiles (faces, edges, user edges, vertices, clustered faces, graph nodes and edges, tree nodes, user-edge setups,
//! data words) and, for the triangles, with a triangulator whose output the census checked face by face (no triangle
//! turns against its face, the projected areas add up). Two Elden Ring files' triangle counts are instead the count
//! a face of n corners whose clipping finishes must give, n - 2 less one per corner on the line of its neighbours in
//! 3-space: their faces include corners straight only seen from above, whose triangles the census's triangulator
//! dropped. The census read all 1,509 Sekiro and 7,438 Elden Ring
//! navmesh tagfiles of both installs; this test reads twelve of them. The files were picked as the smallest and the
//! largest of each combination of objects, an empty mesh of each, the largest with user-edge setups and the one with a
//! face that ear clipping cannot finish.
mod inputs;

use havok_formats::Tagfile;
use havok_formats::navmesh::{NavData, NavTreeKind, nav_data};
use inputs::{Game, OracleSet, Query, expect_found, resolve};

/// What the census found in one file.
struct Expected {
    file: &'static str,
    faces: usize,
    edges: usize,
    user_edges: usize,
    vertices: usize,
    clustered_faces: usize,
    triangles: usize,
    incomplete_faces: usize,
    graph: Option<(usize, usize)>,
    tree_nodes: Option<usize>,
    setups: Option<usize>,
    face_words: usize,
    edge_words: usize,
}

const SEKIRO: &[Expected] = &[
    Expected {
        file: "00-graph-small-n10_00_00_00_900200.hkx",
        faces: 1,
        edges: 3,
        user_edges: 0,
        vertices: 3,
        clustered_faces: 1,
        triangles: 1,
        incomplete_faces: 0,
        graph: Some((1, 0)),
        tree_nodes: None,
        setups: None,
        face_words: 1,
        edge_words: 3,
    },
    Expected {
        file: "01-graph-large-n20_00_00_00_900000.hkx",
        faces: 8667,
        edges: 40772,
        user_edges: 0,
        vertices: 11737,
        clustered_faces: 8667,
        triangles: 23438,
        incomplete_faces: 0,
        graph: Some((544, 3036)),
        tree_nodes: None,
        setups: None,
        face_words: 8667,
        edge_words: 40772,
    },
    Expected {
        file: "02-graph-empty-n10_00_00_00_000200.hkx",
        faces: 0,
        edges: 0,
        user_edges: 0,
        vertices: 0,
        clustered_faces: 0,
        triangles: 0,
        incomplete_faces: 0,
        graph: Some((0, 0)),
        tree_nodes: None,
        setups: None,
        face_words: 0,
        edge_words: 0,
    },
    Expected {
        file: "03-graph-setups-n17_00_00_00_005700.hkx",
        faces: 6,
        edges: 24,
        user_edges: 0,
        vertices: 14,
        clustered_faces: 6,
        triangles: 12,
        incomplete_faces: 0,
        graph: Some((1, 0)),
        tree_nodes: None,
        setups: Some(1),
        face_words: 6,
        edge_words: 24,
    },
    Expected {
        file: "04-graph-setups-large-n25_00_00_00_900500.hkx",
        faces: 5104,
        edges: 23505,
        user_edges: 0,
        vertices: 7284,
        clustered_faces: 5104,
        triangles: 13296,
        incomplete_faces: 0,
        graph: Some((323, 1450)),
        tree_nodes: None,
        setups: Some(21),
        face_words: 5104,
        edge_words: 23505,
    },
];

const ELDEN: &[Expected] = &[
    Expected {
        file: "00-empty-n10_00_00_00_000001.hkx",
        faces: 0,
        edges: 0,
        user_edges: 0,
        vertices: 0,
        clustered_faces: 0,
        triangles: 0,
        incomplete_faces: 0,
        graph: None,
        tree_nodes: None,
        setups: None,
        face_words: 0,
        edge_words: 0,
    },
    Expected {
        file: "01-tree-small-n10_00_00_00_020000.hkx",
        faces: 1,
        edges: 4,
        user_edges: 0,
        vertices: 4,
        clustered_faces: 0,
        triangles: 2,
        incomplete_faces: 0,
        graph: None,
        tree_nodes: Some(1),
        setups: None,
        face_words: 1,
        edge_words: 4,
    },
    Expected {
        file: "02-tree-large-n34_10_00_00_000200.hkx",
        faces: 25263,
        edges: 103860,
        user_edges: 5760,
        vertices: 50278,
        clustered_faces: 0,
        triangles: 47574,
        incomplete_faces: 0,
        graph: None,
        tree_nodes: Some(50525),
        setups: None,
        face_words: 25263,
        edge_words: 103860,
    },
    Expected {
        file: "03-tree-empty-n60_49_37_00_493700.hkx",
        faces: 0,
        edges: 0,
        user_edges: 0,
        vertices: 0,
        clustered_faces: 0,
        triangles: 0,
        incomplete_faces: 0,
        graph: None,
        tree_nodes: Some(0),
        setups: None,
        face_words: 0,
        edge_words: 0,
    },
    Expected {
        file: "04-user-edges-n60_47_38_00_473800.hkx",
        faces: 21435,
        edges: 91391,
        user_edges: 17017,
        vertices: 58710,
        clustered_faces: 0,
        triangles: 31504,
        incomplete_faces: 0,
        graph: None,
        tree_nodes: Some(42869),
        setups: Some(6),
        face_words: 21435,
        edge_words: 91391,
    },
    Expected {
        file: "05-user-edges-small-n13_00_00_00_006000.hkx",
        faces: 51,
        edges: 225,
        user_edges: 20,
        vertices: 135,
        clustered_faces: 0,
        triangles: 103,
        incomplete_faces: 0,
        graph: None,
        tree_nodes: Some(101),
        setups: Some(2),
        face_words: 51,
        edge_words: 225,
    },
    Expected {
        file: "06-incomplete-face-o12_05_00_00_003900.hkx",
        faces: 4666,
        edges: 15610,
        user_edges: 82,
        vertices: 5384,
        clustered_faces: 0,
        triangles: 6196,
        incomplete_faces: 1,
        graph: None,
        tree_nodes: Some(9331),
        setups: None,
        face_words: 4666,
        edge_words: 15610,
    },
];

fn read(game: Game, expected: &[Expected]) -> Result<(), Box<dyn std::error::Error>> {
    let corpus = Query::Oracle(OracleSet::Corpus, game, "havok-nav");
    let Some(dir) = resolve(corpus) else {
        return Ok(());
    };
    expect_found(corpus, "navmesh tagfiles", expected.len());
    for want in expected {
        let path = dir.join(want.file);
        let bytes = std::fs::read(&path)?;
        // the navmesh tagfiles carry their own type tables: no compendium is needed (census: all 8,947)
        let tag = Tagfile::read(&bytes, None)?;
        let data = nav_data(tag.root()?).map_err(|e| format!("{}: {e}", want.file))?;
        let mesh = &data.mesh;
        let name = want.file;
        assert_eq!(mesh.faces.len(), want.faces, "{name}: faces");
        assert_eq!(mesh.edges.len(), want.edges, "{name}: edges");
        assert_eq!(
            mesh.edges.iter().filter(|e| e.user).count(),
            want.user_edges,
            "{name}: user edges"
        );
        assert_eq!(mesh.vertices.len(), want.vertices, "{name}: vertices");
        assert_eq!(
            mesh.faces.iter().filter(|f| f.cluster.is_some()).count(),
            want.clustered_faces,
            "{name}: clustered faces"
        );
        assert_eq!(mesh.face_data.len(), want.face_words, "{name}: face words");
        assert_eq!(mesh.edge_data.len(), want.edge_words, "{name}: edge words");
        assert_eq!(
            data.graph.as_ref().map(|g| (g.nodes.len(), g.edges.len())),
            want.graph,
            "{name}: graph"
        );
        assert_eq!(
            data.tree.as_ref().map(|t| t.nodes.len()),
            want.tree_nodes,
            "{name}: tree nodes"
        );
        assert_eq!(
            data.user_edge_setups.as_ref().map(Vec::len),
            want.setups,
            "{name}: setups"
        );
        triangles(&data, want)?;
        trees(&data, name);
        setups(&data, name);
        // a cluster graph is the clusters' adjacency (checked by the reader; stated here for the Sekiro files)
        assert_eq!(
            data.graph.is_some(),
            game == Game::Sekiro,
            "{name}: only Sekiro has the graph"
        );
        assert!(data.graph_is_cluster_graph(), "{name}");
    }
    Ok(())
}

/// Twice the area vector of a polygon given by its corners in order (Newell's sum).
fn area_vector(corners: &[[f64; 3]]) -> [f64; 3] {
    let mut sum = [0.0f64; 3];
    for (i, p) in corners.iter().enumerate() {
        let q = &corners[(i + 1) % corners.len()];
        sum[0] += (p[1] - q[1]) * (p[2] + q[2]);
        sum[1] += (p[2] - q[2]) * (p[0] + q[0]);
        sum[2] += (p[0] - q[0]) * (p[1] + q[1]);
    }
    sum
}

/// Every face triangulated: the census's triangle and incomplete-face counts, every triangle made of the face's own
/// corners, and the triangles' area vectors adding up to the polygon's (which any triangulation of one boundary does,
/// whatever diagonals it picks, even for a face that is not flat).
fn triangles(data: &NavData, want: &Expected) -> Result<(), Box<dyn std::error::Error>> {
    let (mut total, mut incomplete) = (0usize, 0usize);
    for face in 0..data.mesh.faces.len() {
        let corners = data.mesh.corners(face).ok_or("a face without corners")?;
        let cut = data
            .mesh
            .triangulate(face)
            .ok_or("a face that does not triangulate")?;
        total += cut.triangles.len();
        incomplete += usize::from(!cut.complete);
        assert!(cut.triangles.len() < corners.len(), "{}: face {face}", want.file);
        for triangle in &cut.triangles {
            assert!(
                triangle.iter().all(|v| corners.contains(v)),
                "{}: face {face}",
                want.file
            );
        }
        let point = |v: usize| data.mesh.vertices[v].map(f64::from);
        let boundary: Vec<[f64; 3]> = corners.iter().map(|&v| point(v)).collect();
        let cut_points: Vec<[[f64; 3]; 3]> = cut.triangles.iter().map(|t| t.map(point)).collect();
        let whole = area_vector(&boundary);
        let mut parts = [0.0f64; 3];
        for triangle in &cut_points {
            let part = area_vector(triangle);
            for (sum, c) in parts.iter_mut().zip(part) {
                *sum += c;
            }
        }
        let size = whole.iter().map(|c| c.abs()).fold(0.0f64, f64::max);
        assert!(
            whole
                .iter()
                .zip(&parts)
                .all(|(w, p)| (w - p).abs() <= 1e-6 * size + 1e-9),
            "{}: face {face}: the triangles' area vector {parts:?} is not the polygon's {whole:?}",
            want.file
        );
    }
    assert_eq!(total, want.triangles, "{}: triangles", want.file);
    assert_eq!(
        incomplete, want.incomplete_faces,
        "{}: faces ear clipping could not finish",
        want.file
    );
    Ok(())
}

/// The user-edge setups: both boxes' rotation columns orthonormal (the census found them so in all 4,272 boxes of both
/// installs), positive half extents, `worldUpA` straight up, `forceAlign` set.
fn setups(data: &NavData, name: &str) {
    let dot = |u: [f32; 3], v: [f32; 3]| u[0] * v[0] + u[1] * v[1] + u[2] * v[2];
    for setup in data.user_edge_setups.iter().flatten() {
        for b in [&setup.box_a, &setup.box_b] {
            let [c0, c1, c2] = b.columns;
            for unit in [dot(c0, c0), dot(c1, c1), dot(c2, c2)] {
                assert!((unit - 1.0).abs() < 1e-3, "{name}: {unit}");
            }
            for orthogonal in [dot(c0, c1), dot(c0, c2), dot(c1, c2)] {
                assert!(orthogonal.abs() < 1e-3, "{name}: {orthogonal}");
            }
            assert!(b.half_extents.iter().all(|h| *h > 0.0), "{name}");
        }
        assert_eq!(setup.world_up_a, [0.0, 1.0, 0.0], "{name}");
        assert!(setup.force_align, "{name}");
        assert!(
            matches!(setup.direction, 1 | 3) && matches!(setup.space, 0 | 1),
            "{name}"
        );
    }
}

/// The tree: every face a leaf once, every face corner inside its leaf's box within the census's 0.5 mm.
fn trees(data: &NavData, name: &str) {
    let Some(tree) = &data.tree else { return };
    assert!(tree.covers_faces_once(data.mesh.faces.len()), "{name}");
    assert!(
        tree.corner_excess(&data.mesh) <= 1e-3,
        "{name}: {}",
        tree.corner_excess(&data.mesh)
    );
    let leaves = tree
        .nodes
        .iter()
        .filter(|n| matches!(n.kind, NavTreeKind::Leaf { .. }))
        .count();
    assert_eq!(leaves, data.mesh.faces.len(), "{name}");
}

#[test]
fn sekiros_navmeshes_read_with_the_census_counts() -> Result<(), Box<dyn std::error::Error>> {
    read(Game::Sekiro, SEKIRO)
}

#[test]
fn elden_rings_navmeshes_read_with_the_census_counts() -> Result<(), Box<dyn std::error::Error>> {
    read(Game::Elden, ELDEN)
}
