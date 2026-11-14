//! Havok AI navigation data as the navmesh binders (`*.nvmhktbnd.dcx`) of Sekiro and Elden Ring store it: a polygon mesh
//! ([`NavMesh`], `hkaiNavMesh`), in Sekiro the cluster graph over it ([`NavGraph`], `hkaiDirectedGraphExplicitCost`), in
//! Elden Ring the bounding-volume tree its queries walk ([`NavTree`], the `hkcdStaticAabbTree` of an
//! `hkaiStaticTreeNavMeshQueryMediator` with 6-byte nodes), and the user-edge setups that say how two boxes in the world
//! are joined by user edges ([`UserEdgeSetup`]). [`nav_data`] reads a whole file; the other readers take one object.
//!
//! # The mesh
//!
//! A face is a ring of 3 or more edges that share their endpoints (`b` of an edge is `a` of the next, and the last
//! closes on the first), and each edge belongs to exactly one face. The edges of a mesh are its *regular* edges, the
//! face boundaries in face order, and then its *user* edges: edges that join a face to a face elsewhere over a link
//! (Elden Ring only; their endpoints are vertices of the mesh that are not corners of the face that owns the edge). A
//! regular edge's `opposite` is the edge of the neighbouring face it meets, and that edge points back; an edge on the
//! mesh's boundary has none. The opposite of an edge is not always its reverse (`a` and `b` swapped): 274,614 of Elden
//! Ring's regular opposites and 266 of Sekiro's are not. Faces wind counter-clockwise seen from above (+y), except 8,835
//! of Elden Ring's and 199 of Sekiro's.
//!
//! The faces are not always convex or planar, so [`NavMesh::triangulate`] clips ears in the plane of each face instead
//! of fanning it.
//!
//! # What is checked
//!
//! Each rule above and below holds in every tagfile of the Sekiro (1,509) and Elden Ring (7,438) navmesh binders. The
//! readers refuse what they do not know: streaming sets (none occurs), edge or vertex indices outside the mesh, a face
//! whose edges do not form a closed ring, an edge in two faces or none, an opposite edge that does not point back, data
//! arrays that are not faces or edges times the stride, a graph whose edge ranges do not tile its edges, a tree that is
//! not a valid pre-order tree. The fields whose meaning the files do not show (`flags`, the face and edge data words)
//! are kept as stored; the padding words are not read.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use std::collections::BTreeSet;
use std::ops::Range;

use crate::polygon::triangulate;
use crate::{Result, Value, invalid};

/// The value a packed key stores for "no link".
const NO_KEY: u32 = u32::MAX;

/// A face: a ring of edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NavFace {
    /// `startEdgeIndex`: the face's first regular edge.
    pub first_edge: usize,
    /// `numEdges`: its regular edges (3 or more), which run from `first_edge` and close a ring.
    pub edge_count: usize,
    /// `startUserEdgeIndex`: the face's first user edge; 0 when it has none (the file stores -1).
    pub first_user_edge: usize,
    /// `numUserEdges`: its user edges.
    pub user_edge_count: usize,
    /// `clusterIndex`: the node of the [`NavGraph`] this face belongs to (Sekiro); `None` where the file stores -1
    /// (every face of Elden Ring's).
    pub cluster: Option<usize>,
}

impl NavFace {
    /// The indices of the face's regular edges.
    pub fn edges(&self) -> Range<usize> {
        self.first_edge..self.first_edge.saturating_add(self.edge_count)
    }

    /// The indices of the face's user edges (empty when it has none).
    pub fn user_edges(&self) -> Range<usize> {
        if self.user_edge_count == 0 {
            0..0
        } else {
            self.first_user_edge..self.first_user_edge.saturating_add(self.user_edge_count)
        }
    }
}

/// The edge on the other side of an edge, and the face that owns it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NavLink {
    /// The opposite edge's index.
    pub edge: usize,
    /// The face that owns it: always [`NavEdge::face`] of that edge.
    pub face: usize,
}

/// An edge of a face's ring.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavEdge {
    /// `a`: the vertex the edge starts at.
    pub a: usize,
    /// `b`: the vertex it ends at, which is the next edge's `a` in the face's ring (user edges form no ring).
    pub b: usize,
    /// `oppositeEdge` and `oppositeFace`; `None` for an edge on the mesh's boundary (both stored as `0xFFFFFFFF`).
    pub opposite: Option<NavLink>,
    /// `flags`: 4 on every regular edge; 20 or 52 on user edges (the values 16 and 32 are set on user edges only; what
    /// the bits are is not shown by the files).
    pub flags: u8,
    /// `userEdgeCost`: 0 on every edge of both installs.
    pub user_cost: f32,
    /// The face that owns the edge, from the faces' edge ranges.
    pub face: usize,
    /// Whether it is one of its face's user edges.
    pub user: bool,
}

/// A mesh's box, as `aabb` stores it: four lanes a corner. It contains every vertex (all 4,826 meshes with faces) and is
/// not tight. The fourth lanes hold a margin with opposite signs (`min[3] == -max[3]` in every mesh with faces of both
/// installs; 1.1 in all of Sekiro's); the box is the vertices' bounds grown by it in 785 of Sekiro's 1,415 meshes with
/// faces and 39 of Elden Ring's 3,411, and larger in the others.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavAabb {
    /// The minimum corner, `w` last.
    pub min: [f32; 4],
    /// The maximum corner, `w` last.
    pub max: [f32; 4],
}

/// The triangles of one face, from [`NavMesh::triangulate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaceTriangles {
    /// Triangles as vertex indices, wound like the face. A face with no area gives none.
    pub triangles: Vec<[usize; 3]>,
    /// Whether ear clipping finished; false only for faces that are not simple polygons (two zero-area faces of
    /// Elden Ring), whose remaining corners are fanned.
    pub complete: bool,
}

/// What a `hkaiNavMesh` stores, before it is checked: the form [`NavMesh::from_parts`] takes.
#[derive(Debug, Clone, PartialEq)]
pub struct RawNavMesh {
    /// Per face: `startEdgeIndex`, `startUserEdgeIndex`, `numEdges`, `numUserEdges`, `clusterIndex`.
    pub faces: Vec<[i64; 5]>,
    /// Per edge: `a`, `b`, `oppositeEdge`, `oppositeFace`, `flags`.
    pub edges: Vec<[i64; 5]>,
    /// Per edge: `userEdgeCost`.
    pub edge_costs: Vec<f32>,
    /// `vertices`, xyz (the fourth lane is not part of the data).
    pub vertices: Vec<[f32; 3]>,
    /// `faceData`.
    pub face_data: Vec<i32>,
    /// `edgeData`.
    pub edge_data: Vec<i32>,
    /// `faceDataStriding`.
    pub face_data_stride: usize,
    /// `edgeDataStriding`.
    pub edge_data_stride: usize,
    /// `aabb`.
    pub aabb: NavAabb,
    /// `erosionRadius`.
    pub erosion_radius: f32,
    /// `userData`.
    pub user_data: u64,
    /// `flags`.
    pub flags: u8,
    /// The number of `streamingSets` stored.
    pub streaming_sets: usize,
}

/// A checked `hkaiNavMesh`.
#[derive(Debug, Clone, PartialEq)]
pub struct NavMesh {
    /// The faces.
    pub faces: Vec<NavFace>,
    /// The edges: every face's regular edges in face order, then the user edges.
    pub edges: Vec<NavEdge>,
    /// The vertices.
    pub vertices: Vec<[f32; 3]>,
    /// `faceData`: [`face_data_stride`](Self::face_data_stride) words a face (1 in every mesh). The words are bit sets:
    /// Sekiro's are 0, 1, 4 and 5; Elden Ring's are 18 distinct values built from bits 0, 6, 12, 16, 17, 20, 24 and 28.
    pub face_data: Vec<i32>,
    /// `edgeData`: [`edge_data_stride`](Self::edge_data_stride) words an edge (1 in all but 32 Elden Ring and 94 Sekiro
    /// files). 0 on every regular edge; on user edges the low byte is 2, 4, 18 or 20 and the second byte has 163
    /// values; nothing above the second byte is set.
    pub edge_data: Vec<i32>,
    /// `faceDataStriding`.
    pub face_data_stride: usize,
    /// `edgeDataStriding`.
    pub edge_data_stride: usize,
    /// `aabb`.
    pub aabb: NavAabb,
    /// `erosionRadius` (0 in both installs).
    pub erosion_radius: f32,
    /// `userData` (0 except in 40 Elden Ring meshes, which store 1).
    pub user_data: u64,
    /// `flags` (0).
    pub flags: u8,
}

impl NavMesh {
    /// Check a mesh's stored form and make the typed one.
    ///
    /// Refused: streaming sets; an edge range or a vertex index outside the mesh; a face of fewer than 3 edges, whose
    /// edges do not close a ring, or that has no user edges but a start index other than -1; an edge in two faces or
    /// in none; a vertex that is not finite; an opposite edge or face out of range, only one of the two given, not
    /// owned by the face named, or that does not point back at the edge; a data array that is not the faces (or
    /// edges) times the stride. `at` is where the mesh sits in the file, for the refusals.
    pub fn from_parts(raw: RawNavMesh, at: usize) -> Result<Self> {
        if raw.streaming_sets != 0 {
            return Err(invalid(
                at,
                format!(
                    "the navmesh has {} streaming sets, which no navmesh of Sekiro or Elden Ring has",
                    raw.streaming_sets
                ),
            ));
        }
        let index = |n: i64, what: &str| -> Result<usize> {
            usize::try_from(n).map_err(|_| invalid(at, format!("{what} is {n}, not an index")))
        };
        let edge_total = raw.edges.len();
        let mut owner: Vec<Option<usize>> = vec![None; edge_total];
        let mut user_edge = vec![false; edge_total];
        let mut faces = Vec::with_capacity(raw.faces.len());
        for (i, &[start, start_user, count, user_count, cluster]) in raw.faces.iter().enumerate() {
            let edge_count = index(count, &format!("face {i}'s numEdges"))?;
            if edge_count < 3 {
                return Err(invalid(
                    at,
                    format!("face {i} has {edge_count} edges, fewer than 3"),
                ));
            }
            let first_edge = index(start, &format!("face {i}'s startEdgeIndex"))?;
            claim(
                &mut owner,
                &mut user_edge,
                (first_edge, edge_count),
                (i, false),
                at,
            )?;
            let user_edge_count = index(user_count, &format!("face {i}'s numUserEdges"))?;
            let first_user_edge = if user_edge_count == 0 {
                if start_user != -1 {
                    return Err(invalid(
                        at,
                        format!(
                            "face {i} has no user edges but its startUserEdgeIndex is {start_user}, not -1"
                        ),
                    ));
                }
                0
            } else {
                let first = index(start_user, &format!("face {i}'s startUserEdgeIndex"))?;
                claim(
                    &mut owner,
                    &mut user_edge,
                    (first, user_edge_count),
                    (i, true),
                    at,
                )?;
                first
            };
            let cluster = if cluster == -1 {
                None
            } else {
                Some(index(cluster, &format!("face {i}'s clusterIndex"))?)
            };
            faces.push(NavFace {
                first_edge,
                edge_count,
                first_user_edge,
                user_edge_count,
                cluster,
            });
        }
        if raw.edge_costs.len() != edge_total {
            return Err(invalid(at, "an edge cost is missing"));
        }
        if let Some(v) = raw.vertices.iter().position(|p| p.iter().any(|c| !c.is_finite())) {
            return Err(invalid(at, format!("vertex {v} is not finite")));
        }
        let mut edges = Vec::with_capacity(edge_total);
        for (i, (&[a, b, opposite_edge, opposite_face, flags], &user_cost)) in
            raw.edges.iter().zip(&raw.edge_costs).enumerate()
        {
            let vertex = |n: i64, what: &str| -> Result<usize> {
                usize::try_from(n)
                    .ok()
                    .filter(|&v| v < raw.vertices.len())
                    .ok_or_else(|| {
                        invalid(
                            at,
                            format!(
                                "edge {i}'s {what} is {n}, outside the {} vertices",
                                raw.vertices.len()
                            ),
                        )
                    })
            };
            let key =
                |n: i64| u32::try_from(n).map_err(|_| invalid(at, format!("edge {i} stores the key {n}")));
            let opposite = match (key(opposite_edge)?, key(opposite_face)?) {
                (NO_KEY, NO_KEY) => None,
                (NO_KEY, _) | (_, NO_KEY) => {
                    return Err(invalid(
                        at,
                        format!("edge {i} has only one of its opposite edge and face"),
                    ));
                }
                (edge, face) => {
                    let to_index = |k: u32, limit: usize, what: &str| -> Result<usize> {
                        usize::try_from(k).ok().filter(|&n| n < limit).ok_or_else(|| {
                            invalid(
                                at,
                                format!("edge {i}'s opposite {what} {k} is outside the {limit} there are"),
                            )
                        })
                    };
                    Some(NavLink {
                        edge: to_index(edge, edge_total, "edge")?,
                        face: to_index(face, faces.len(), "face")?,
                    })
                }
            };
            let flags = u8::try_from(flags)
                .map_err(|_| invalid(at, format!("edge {i}'s flags {flags} exceed a byte")))?;
            let (Some(Some(face)), Some(&user)) = (owner.get(i), user_edge.get(i)) else {
                return Err(invalid(at, format!("edge {i} belongs to no face")));
            };
            edges.push(NavEdge {
                a: vertex(a, "a")?,
                b: vertex(b, "b")?,
                opposite,
                flags,
                user_cost,
                face: *face,
                user,
            });
        }
        // an opposite edge is owned by the face named (checked for every edge first: a wrong owner also breaks the link
        // back, and the owner is what is wrong) and points back
        for (i, edge) in edges.iter().enumerate() {
            let Some(link) = edge.opposite else { continue };
            if edges.get(link.edge).map(|o| o.face) != Some(link.face) {
                return Err(invalid(
                    at,
                    format!(
                        "edge {i}'s opposite edge {} is not owned by face {}",
                        link.edge, link.face
                    ),
                ));
            }
        }
        for (i, edge) in edges.iter().enumerate() {
            let Some(link) = edge.opposite else { continue };
            let back = edges.get(link.edge).and_then(|o| o.opposite);
            if back
                != Some(NavLink {
                    edge: i,
                    face: edge.face,
                })
            {
                return Err(invalid(
                    at,
                    format!("edge {i}'s opposite edge {} does not point back at it", link.edge),
                ));
            }
        }
        // every face's regular edges close a ring
        for (i, face) in faces.iter().enumerate() {
            let ring = edges.get(face.edges()).unwrap_or_default();
            if ring
                .iter()
                .zip(ring.iter().cycle().skip(1))
                .any(|(edge, next)| edge.b != next.a)
            {
                return Err(invalid(at, format!("the edges of face {i} do not close a ring")));
            }
        }
        for (what, data, stride, count) in [
            ("faceData", raw.face_data.len(), raw.face_data_stride, faces.len()),
            ("edgeData", raw.edge_data.len(), raw.edge_data_stride, edge_total),
        ] {
            if count.checked_mul(stride) != Some(data) {
                return Err(invalid(
                    at,
                    format!("{what} has {data} words, not {count} times the stride {stride}"),
                ));
            }
        }
        Ok(Self {
            faces,
            edges,
            vertices: raw.vertices,
            face_data: raw.face_data,
            edge_data: raw.edge_data,
            face_data_stride: raw.face_data_stride,
            edge_data_stride: raw.edge_data_stride,
            aabb: raw.aabb,
            erosion_radius: raw.erosion_radius,
            user_data: raw.user_data,
            flags: raw.flags,
        })
    }

    /// Read a `hkaiNavMesh` object (the `variant` of a root container's named variant).
    pub fn read(mesh: Value<'_, '_>) -> Result<Self> {
        expect_class(mesh, "hkaiNavMesh")?;
        let at = mesh.at();
        let stored = mesh.field("faces")?.array()?;
        let mut faces = Vec::with_capacity(stored.len());
        for i in 0..stored.len() {
            let face = stored.get(i)?;
            faces.push([
                face.field("startEdgeIndex")?.signed()?,
                face.field("startUserEdgeIndex")?.signed()?,
                face.field("numEdges")?.signed()?,
                face.field("numUserEdges")?.signed()?,
                face.field("clusterIndex")?.signed()?,
            ]);
        }
        let stored = mesh.field("edges")?.array()?;
        let mut edges = Vec::with_capacity(stored.len());
        let mut edge_costs = Vec::with_capacity(stored.len());
        for i in 0..stored.len() {
            let edge = stored.get(i)?;
            edges.push([
                edge.field("a")?.signed()?,
                edge.field("b")?.signed()?,
                i64::from(
                    u32::try_from(edge.field("oppositeEdge")?.unsigned()?)
                        .map_err(|_| invalid(edge.at(), "an opposite edge key does not fit 32 bits"))?,
                ),
                i64::from(
                    u32::try_from(edge.field("oppositeFace")?.unsigned()?)
                        .map_err(|_| invalid(edge.at(), "an opposite face key does not fit 32 bits"))?,
                ),
                i64::try_from(edge.field("flags")?.unsigned()?)
                    .map_err(|_| invalid(edge.at(), "edge flags do not fit"))?,
            ]);
            edge_costs.push(edge.field("userEdgeCost")?.half()?);
        }
        let stored = mesh.field("vertices")?.array()?;
        let mut vertices = Vec::with_capacity(stored.len());
        for i in 0..stored.len() {
            // the fourth lane is not part of the data (it holds leftovers: 1 in most, anything else in some)
            let [x, y, z, _] = stored.get(i)?.vector::<4>()?;
            vertices.push([x, y, z]);
        }
        let words = |name: &str| -> Result<Vec<i32>> {
            let stored = mesh.field(name)?.array()?;
            let mut out = Vec::with_capacity(stored.len());
            for i in 0..stored.len() {
                let word = stored.get(i)?;
                out.push(
                    i32::try_from(word.signed()?)
                        .map_err(|_| invalid(word.at(), format!("{name} holds a word wider than 32 bits")))?,
                );
            }
            Ok(out)
        };
        let count = |name: &str| -> Result<usize> {
            let n = mesh.field(name)?.signed()?;
            usize::try_from(n).map_err(|_| invalid(at, format!("{name} is {n}")))
        };
        Self::from_parts(
            RawNavMesh {
                faces,
                edges,
                edge_costs,
                vertices,
                face_data: words("faceData")?,
                edge_data: words("edgeData")?,
                face_data_stride: count("faceDataStriding")?,
                edge_data_stride: count("edgeDataStriding")?,
                aabb: read_aabb(mesh.field("aabb")?)?,
                erosion_radius: mesh.field("erosionRadius")?.float()?,
                user_data: mesh.field("userData")?.unsigned()?,
                flags: u8::try_from(mesh.field("flags")?.unsigned()?)
                    .map_err(|_| invalid(at, "navmesh flags exceed a byte"))?,
                streaming_sets: mesh.field("streamingSets")?.array()?.len(),
            },
            at,
        )
    }

    /// The vertex indices of face `face`'s corners, in ring order; `None` for a face the mesh does not have.
    pub fn corners(&self, face: usize) -> Option<Vec<usize>> {
        let ring = self.edges.get(self.faces.get(face)?.edges())?;
        Some(ring.iter().map(|e| e.a).collect())
    }

    /// The words of `faceData` for face `face` (empty where the stride is 0).
    pub fn face_words(&self, face: usize) -> Option<&[i32]> {
        let start = face.checked_mul(self.face_data_stride)?;
        self.face_data
            .get(start..start.checked_add(self.face_data_stride)?)
    }

    /// The words of `edgeData` for edge `edge` (empty where the stride is 0).
    pub fn edge_words(&self, edge: usize) -> Option<&[i32]> {
        let start = edge.checked_mul(self.edge_data_stride)?;
        self.edge_data
            .get(start..start.checked_add(self.edge_data_stride)?)
    }

    /// Triangulate face `face` by ear clipping in its plane; `None` for a face the mesh does not have. Convex or not,
    /// planar or not, each triangle turns the way the face does (checked on all 6,596,147 faces of both installs, whose
    /// projected triangle areas add up to the face's within 1e-9).
    pub fn triangulate(&self, face: usize) -> Option<FaceTriangles> {
        let corners = self.corners(face)?;
        let points: Vec<[f64; 3]> = corners
            .iter()
            .map(|&v| self.vertices.get(v).map_or([0.0; 3], |p| p.map(f64::from)))
            .collect();
        let (local, complete) = triangulate(&points);
        let triangles = local
            .into_iter()
            .filter_map(|t| {
                let [a, b, c] = t.map(|i| corners.get(i).copied());
                Some([a?, b?, c?])
            })
            .collect();
        Some(FaceTriangles { triangles, complete })
    }
}

/// Give edges `first..first + count` to `face` (`(index, is user)`); a refusal for a range outside the edges or an edge
/// that already has an owner.
fn claim(
    owner: &mut [Option<usize>],
    user_edge: &mut [bool],
    (first, count): (usize, usize),
    (face, is_user): (usize, bool),
    at: usize,
) -> Result<()> {
    let total = owner.len();
    let end = first
        .checked_add(count)
        .filter(|&end| end <= total)
        .ok_or_else(|| {
            invalid(
                at,
                format!("face {face}'s edges {first} and {count} more run past the {total} edges"),
            )
        })?;
    for index in first..end {
        if let (Some(slot), Some(flag)) = (owner.get_mut(index), user_edge.get_mut(index)) {
            if let Some(other) = slot {
                return Err(invalid(
                    at,
                    format!("edge {index} belongs to faces {other} and {face}"),
                ));
            }
            *slot = Some(face);
            *flag = is_user;
        }
    }
    Ok(())
}

fn expect_class(value: Value<'_, '_>, class: &str) -> Result<()> {
    if value.type_name() == class {
        Ok(())
    } else {
        Err(invalid(
            value.at(),
            format!("expected a {class}, not a {}", value.type_name()),
        ))
    }
}

/// An `hkAabb` with its fourth lanes.
fn read_aabb(v: Value<'_, '_>) -> Result<NavAabb> {
    Ok(NavAabb {
        min: v.field("min")?.vector::<4>()?,
        max: v.field("max")?.vector::<4>()?,
    })
}

/// A node of the cluster graph: the range of [`NavGraph::edges`] that leave it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NavGraphNode {
    /// `startEdgeIndex`.
    pub first_edge: usize,
    /// `numEdges`.
    pub edge_count: usize,
}

/// An edge of the cluster graph.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavGraphEdge {
    /// `cost`: a half float.
    pub cost: f32,
    /// `flags`: 0 on every edge of Sekiro.
    pub flags: u16,
    /// `target`: the node the edge leads to.
    pub target: usize,
}

/// A checked `hkaiDirectedGraphExplicitCost`. In Sekiro it is the cluster graph of the navmesh in its file: one node per
/// `clusterIndex` of the mesh's faces, an edge for every pair of clusters whose faces share an edge, in both directions
/// ([`NavData::graph_is_cluster_graph`]). Elden Ring has no such graph.
#[derive(Debug, Clone, PartialEq)]
pub struct NavGraph {
    /// `positions`: one point a node (`w` is not part of the data).
    pub positions: Vec<[f32; 3]>,
    /// `nodes`: their edge ranges tile `edges` in node order.
    pub nodes: Vec<NavGraphNode>,
    /// `edges`.
    pub edges: Vec<NavGraphEdge>,
    /// `nodeData`: `node_data_stride` words a node (0 words in both installs).
    pub node_data: Vec<u32>,
    /// `edgeData`: `edge_data_stride` words an edge (0 words in both installs).
    pub edge_data: Vec<u32>,
    /// `nodeDataStriding`.
    pub node_data_stride: usize,
    /// `edgeDataStriding`.
    pub edge_data_stride: usize,
}

/// What a `hkaiDirectedGraphExplicitCost` stores, before it is checked: the form [`NavGraph::from_parts`] takes.
#[derive(Debug, Clone, PartialEq)]
pub struct RawNavGraph {
    /// `positions`, xyz.
    pub positions: Vec<[f32; 3]>,
    /// Per node: `startEdgeIndex`, `numEdges`.
    pub nodes: Vec<[i64; 2]>,
    /// Per edge: `cost`, `flags`, `target` (the packed key as stored).
    pub edges: Vec<(f32, u16, u32)>,
    /// `nodeData`.
    pub node_data: Vec<u32>,
    /// `edgeData`.
    pub edge_data: Vec<u32>,
    /// `nodeDataStriding`.
    pub node_data_stride: usize,
    /// `edgeDataStriding`.
    pub edge_data_stride: usize,
    /// The number of `streamingSets` stored.
    pub streaming_sets: usize,
}

impl NavGraph {
    /// Check a graph's stored form and make the typed one.
    ///
    /// Refused: streaming sets; positions that are not one a node or not finite; node edge ranges that do not tile the
    /// edges in node order; an edge target that is not a node; a cost that is not finite; data arrays that are not the
    /// nodes (or edges) times the stride.
    pub fn from_parts(raw: RawNavGraph, at: usize) -> Result<Self> {
        if raw.streaming_sets != 0 {
            return Err(invalid(
                at,
                format!(
                    "the graph has {} streaming sets, which no graph of Sekiro has",
                    raw.streaming_sets
                ),
            ));
        }
        if raw.positions.len() != raw.nodes.len() {
            return Err(invalid(
                at,
                format!("{} positions for {} nodes", raw.positions.len(), raw.nodes.len()),
            ));
        }
        if let Some(i) = raw
            .positions
            .iter()
            .position(|p| p.iter().any(|c| !c.is_finite()))
        {
            return Err(invalid(at, format!("position {i} is not finite")));
        }
        let mut nodes = Vec::with_capacity(raw.nodes.len());
        let mut running = 0usize;
        for (i, &[start, count]) in raw.nodes.iter().enumerate() {
            let (Ok(first_edge), Ok(edge_count)) = (usize::try_from(start), usize::try_from(count)) else {
                return Err(invalid(
                    at,
                    format!("node {i}'s edge range {start}, {count} is not an index range"),
                ));
            };
            if first_edge != running {
                return Err(invalid(
                    at,
                    format!(
                        "node {i}'s edges start at {first_edge}, not at {running} where the earlier nodes' end"
                    ),
                ));
            }
            running = running
                .checked_add(edge_count)
                .ok_or_else(|| invalid(at, "node edge counts overflow"))?;
            nodes.push(NavGraphNode {
                first_edge,
                edge_count,
            });
        }
        if running != raw.edges.len() {
            return Err(invalid(
                at,
                format!(
                    "the nodes' edge ranges cover {running} of the {} edges",
                    raw.edges.len()
                ),
            ));
        }
        let mut edges = Vec::with_capacity(raw.edges.len());
        for (i, &(cost, flags, key)) in raw.edges.iter().enumerate() {
            let target = usize::try_from(key)
                .ok()
                .filter(|&t| t < nodes.len())
                .ok_or_else(|| {
                    invalid(
                        at,
                        format!("edge {i} leads to {key}, outside the {} nodes", nodes.len()),
                    )
                })?;
            if !cost.is_finite() {
                return Err(invalid(at, format!("edge {i}'s cost is not finite")));
            }
            edges.push(NavGraphEdge { cost, flags, target });
        }
        for (what, data, stride, count) in [
            ("nodeData", raw.node_data.len(), raw.node_data_stride, nodes.len()),
            ("edgeData", raw.edge_data.len(), raw.edge_data_stride, edges.len()),
        ] {
            if count.checked_mul(stride) != Some(data) {
                return Err(invalid(
                    at,
                    format!("{what} has {data} words, not {count} times the stride {stride}"),
                ));
            }
        }
        Ok(Self {
            positions: raw.positions,
            nodes,
            edges,
            node_data: raw.node_data,
            edge_data: raw.edge_data,
            node_data_stride: raw.node_data_stride,
            edge_data_stride: raw.edge_data_stride,
        })
    }

    /// Read a `hkaiDirectedGraphExplicitCost` object.
    pub fn read(graph: Value<'_, '_>) -> Result<Self> {
        expect_class(graph, "hkaiDirectedGraphExplicitCost")?;
        let at = graph.at();
        let stored = graph.field("positions")?.array()?;
        let mut positions = Vec::with_capacity(stored.len());
        for i in 0..stored.len() {
            let [x, y, z, _] = stored.get(i)?.vector::<4>()?;
            positions.push([x, y, z]);
        }
        let stored = graph.field("nodes")?.array()?;
        let mut nodes = Vec::with_capacity(stored.len());
        for i in 0..stored.len() {
            let node = stored.get(i)?;
            nodes.push([
                node.field("startEdgeIndex")?.signed()?,
                node.field("numEdges")?.signed()?,
            ]);
        }
        let stored = graph.field("edges")?.array()?;
        let mut edges = Vec::with_capacity(stored.len());
        for i in 0..stored.len() {
            let edge = stored.get(i)?;
            let flags = u16::try_from(edge.field("flags")?.unsigned()?)
                .map_err(|_| invalid(edge.at(), "graph edge flags exceed 16 bits"))?;
            let target = u32::try_from(edge.field("target")?.unsigned()?)
                .map_err(|_| invalid(edge.at(), "a graph edge target exceeds 32 bits"))?;
            edges.push((edge.field("cost")?.half()?, flags, target));
        }
        let words = |name: &str| -> Result<Vec<u32>> {
            let stored = graph.field(name)?.array()?;
            let mut out = Vec::with_capacity(stored.len());
            for i in 0..stored.len() {
                let word = stored.get(i)?;
                out.push(
                    u32::try_from(word.unsigned()?)
                        .map_err(|_| invalid(word.at(), format!("{name} holds a word wider than 32 bits")))?,
                );
            }
            Ok(out)
        };
        let stride = |name: &str| -> Result<usize> {
            let n = graph.field(name)?.signed()?;
            usize::try_from(n).map_err(|_| invalid(at, format!("{name} is {n}")))
        };
        Self::from_parts(
            RawNavGraph {
                positions,
                nodes,
                edges,
                node_data: words("nodeData")?,
                edge_data: words("edgeData")?,
                node_data_stride: stride("nodeDataStriding")?,
                edge_data_stride: stride("edgeDataStriding")?,
                streaming_sets: graph.field("streamingSets")?.array()?.len(),
            },
            at,
        )
    }
}

/// How a node of a [`NavTree`] continues.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavTreeKind {
    /// The first child is the next node; the second is at this index.
    Internal {
        /// Index of the second child.
        second_child: usize,
    },
    /// A leaf: one face of the mesh.
    Leaf {
        /// The face's index.
        face: usize,
    },
}

/// A node of a [`NavTree`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavTreeNode {
    /// The three stored axis bytes: per axis the high nibble (which shrinks the box from its minimum side) and the low
    /// nibble (from its maximum side).
    pub axes: [u8; 3],
    /// The node's box, minimum then maximum (xyz), decoded in double precision from the tree's domain.
    pub aabb: [[f64; 3]; 2],
    /// Internal (with its second child) or leaf (with its face).
    pub kind: NavTreeKind,
}

/// What one 6-byte node stores: three axis bytes, `hiData` and `loData`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawNavTreeNode {
    /// The `xyz` bytes.
    pub axes: [u8; 3],
    /// `hiData`.
    pub hi: u8,
    /// `loData`.
    pub lo: u16,
}

/// The nibble squares divide by this: a nibble `n` takes `n² / 226` of the parent's extent from its side (the divisor of
/// the 4- and 5-byte trees of compressed meshes too).
const NIBBLE_DIVISOR: f64 = 226.0;

/// A decoded `hkcdStaticAabbTree` with 6-byte nodes: a binary tree over the faces of a [`NavMesh`], one leaf for each,
/// stored in pre-order (the first child of an internal node is the next node, the second starts after the first
/// child's subtree; a leaf is the last node of its subtree). A node's data is `hiData << 16 | loData`: bit 23 set is
/// an internal node whose second child is `2 × (data & 0x7FFFFF)` nodes on, clear is a leaf for face `data`. A node's
/// axis bytes shrink its parent's box exactly as in the 4- and 5-byte trees of compressed meshes.
#[derive(Debug, Clone, PartialEq)]
pub struct NavTree {
    /// The nodes in file order. Empty for a mesh without faces (2,610 of Elden Ring's trees, whose domain is
    /// `±f32::MAX`).
    pub nodes: Vec<NavTreeNode>,
    /// `domain`: the box the root decodes from (xyz).
    pub domain: [[f32; 3]; 2],
}

impl NavTree {
    /// Decode a tree from its nodes and domain (`at` is where it sits in the file, for refusals).
    ///
    /// Refused: a second child that is not a later node inside its parent's subtree, a leaf that does not end its
    /// subtree (so every node is reached exactly once), a domain that is not finite and ordered when there are nodes.
    pub fn decode(raw: &[RawNavTreeNode], domain: [[f32; 3]; 2], at: usize) -> Result<Self> {
        let count = raw.len();
        if count == 0 {
            return Ok(Self {
                nodes: Vec::new(),
                domain,
            });
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
        let placeholder = NavTreeNode {
            axes: [0; 3],
            aabb: root,
            kind: NavTreeKind::Leaf { face: 0 },
        };
        let mut nodes = vec![placeholder; count];
        // (node, end of its subtree, the parent's box); the second child is pushed first so the first pops first
        let mut stack = vec![(0usize, count, root)];
        let mut reached = 0usize;
        while let Some((i, end, parent)) = stack.pop() {
            let node = raw.get(i).filter(|_| i < end).ok_or_else(|| {
                invalid(
                    at,
                    format!("node {i} lies outside its subtree, which ends at {end}"),
                )
            })?;
            let [mut min, mut max] = parent;
            for ((&pair, low), high) in node.axes.iter().zip(&mut min).zip(&mut max) {
                let extent = *high - *low;
                let (hi, lo) = (f64::from(pair >> 4), f64::from(pair & 15));
                *low += extent * hi * hi / NIBBLE_DIVISOR;
                *high -= extent * lo * lo / NIBBLE_DIVISOR;
            }
            let aabb = [min, max];
            let data = u32::from(node.hi) << 16 | u32::from(node.lo);
            let internal = data & 0x80_0000 != 0;
            let payload =
                usize::try_from(data & 0x7f_ffff).map_err(|_| invalid(at, "a node payload overflows"))?;
            reached = reached.saturating_add(1);
            let kind = if internal {
                let second = payload
                    .checked_mul(2)
                    .and_then(|step| i.checked_add(step))
                    .filter(|&second| payload >= 1 && second < end)
                    .ok_or_else(|| {
                        invalid(
                            at,
                            format!(
                                "node {i}: its second child is not inside its subtree, which ends at {end}"
                            ),
                        )
                    })?;
                stack.push((second, end, aabb));
                stack.push((i.saturating_add(1), second, aabb));
                NavTreeKind::Internal { second_child: second }
            } else {
                if i.saturating_add(1) != end {
                    return Err(invalid(
                        at,
                        format!(
                            "leaf {i} does not end its subtree, which ends at {end}: the nodes after it are not reached once"
                        ),
                    ));
                }
                NavTreeKind::Leaf { face: payload }
            };
            if let Some(slot) = nodes.get_mut(i) {
                *slot = NavTreeNode {
                    axes: node.axes,
                    aabb,
                    kind,
                };
            }
        }
        if reached != count {
            return Err(invalid(at, format!("{reached} of {count} nodes reached")));
        }
        Ok(Self { nodes, domain })
    }

    /// Read the tree of an `hkaiStaticTreeNavMeshQueryMediator` object.
    pub fn read(mediator: Value<'_, '_>) -> Result<Self> {
        expect_class(mediator, "hkaiStaticTreeNavMeshQueryMediator")?;
        let tree = mediator.field("tree")?.dereference()?;
        let implementation = tree.field("treePtr")?.dereference()?;
        let tree = implementation.field("tree")?;
        let stored = tree.field("nodes")?.array()?;
        let mut raw = Vec::with_capacity(stored.len());
        for i in 0..stored.len() {
            let node = stored.get(i)?;
            let xyz = node.field("xyz")?.array()?;
            let byte = |k: usize| -> Result<u8> {
                u8::try_from(xyz.get(k)?.unsigned()?)
                    .map_err(|_| invalid(node.at(), "an axis byte exceeds 8 bits"))
            };
            raw.push(RawNavTreeNode {
                axes: [byte(0)?, byte(1)?, byte(2)?],
                hi: u8::try_from(node.field("hiData")?.unsigned()?)
                    .map_err(|_| invalid(node.at(), "hiData exceeds 8 bits"))?,
                lo: u16::try_from(node.field("loData")?.unsigned()?)
                    .map_err(|_| invalid(node.at(), "loData exceeds 16 bits"))?,
            });
        }
        let domain = tree.field("domain")?;
        let [min, max] = [
            domain.field("min")?.vector::<4>()?,
            domain.field("max")?.vector::<4>()?,
        ];
        Self::decode(
            &raw,
            [[min[0], min[1], min[2]], [max[0], max[1], max[2]]],
            tree.at(),
        )
    }

    /// Whether the leaves name every face of a mesh of `faces` faces exactly once (true of all 3,411 Elden Ring trees of
    /// a mesh with faces, and of the 2,610 empty trees of meshes without).
    pub fn covers_faces_once(&self, faces: usize) -> bool {
        let mut seen = vec![false; faces];
        let mut leaves = 0usize;
        for node in &self.nodes {
            if let NavTreeKind::Leaf { face } = node.kind {
                leaves = leaves.saturating_add(1);
                match seen.get_mut(face) {
                    Some(flag) if !*flag => *flag = true,
                    _ => return false,
                }
            }
        }
        leaves == faces
    }

    /// The largest distance, in metres, a corner of a face lies outside its leaf's box (0 when every corner is inside;
    /// 4.9e-4 at most in the 3,411 trees of Elden Ring, f32 and nibble rounding). Faces the mesh lacks are skipped.
    pub fn corner_excess(&self, mesh: &NavMesh) -> f64 {
        let mut worst = 0.0f64;
        for node in &self.nodes {
            let NavTreeKind::Leaf { face } = node.kind else {
                continue;
            };
            let Some(corners) = mesh.corners(face) else {
                continue;
            };
            for vertex in corners {
                let Some(p) = mesh.vertices.get(vertex) else {
                    continue;
                };
                for ((&c, &low), &high) in p.iter().zip(&node.aabb[0]).zip(&node.aabb[1]) {
                    let c = f64::from(c);
                    worst = worst.max(low - c).max(c - high);
                }
            }
        }
        worst
    }
}

/// An oriented box of a [`UserEdgeSetup`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UserEdgeBox {
    /// The rotation's three columns (xyz each; the stored fourth float of a column is padding and not part of the data).
    pub columns: [[f32; 3]; 3],
    /// The translation (xyz).
    pub translation: [f32; 3],
    /// The half extents (xyz).
    pub half_extents: [f32; 3],
}

/// One entry of a `hkaiUserEdgeSetupArray`: how two boxes in the world are joined by user edges.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UserEdgeSetup {
    /// `obbA`.
    pub box_a: UserEdgeBox,
    /// `obbB`.
    pub box_b: UserEdgeBox,
    /// `userDataA`.
    pub user_data_a: u32,
    /// `userDataB`.
    pub user_data_b: u32,
    /// `costAtoB`.
    pub cost_a_to_b: f32,
    /// `costBtoA`.
    pub cost_b_to_a: f32,
    /// `worldUpA` (xyz): (0, 1, 0) in every setup of both installs.
    pub world_up_a: [f32; 3],
    /// `worldUpB`.
    pub world_up_b: [f32; 3],
    /// `direction`, as stored: 1 or 3.
    pub direction: u8,
    /// `space`, as stored: 0 or 1.
    pub space: u8,
    /// `forceAlign`.
    pub force_align: bool,
}

impl UserEdgeSetup {
    /// Read the setups of a `hkaiUserEdgeSetupArray` object.
    pub fn read_array(array: Value<'_, '_>) -> Result<Vec<Self>> {
        expect_class(array, "hkaiUserEdgeSetupArray")?;
        let stored = array.field("edgeSetups")?.array()?;
        let mut out = Vec::with_capacity(stored.len());
        for i in 0..stored.len() {
            let setup = stored.get(i)?;
            let byte = |name: &str| -> Result<u8> {
                u8::try_from(setup.field(name)?.unsigned()?)
                    .map_err(|_| invalid(setup.at(), format!("{name} exceeds 8 bits")))
            };
            let word = |name: &str| -> Result<u32> {
                u32::try_from(setup.field(name)?.unsigned()?)
                    .map_err(|_| invalid(setup.at(), format!("{name} exceeds 32 bits")))
            };
            let up = |name: &str| -> Result<[f32; 3]> {
                let [x, y, z, _] = setup.field(name)?.vector::<4>()?;
                Ok([x, y, z])
            };
            out.push(Self {
                box_a: read_box(setup.field("obbA")?)?,
                box_b: read_box(setup.field("obbB")?)?,
                user_data_a: word("userDataA")?,
                user_data_b: word("userDataB")?,
                cost_a_to_b: setup.field("costAtoB")?.float()?,
                cost_b_to_a: setup.field("costBtoA")?.float()?,
                world_up_a: up("worldUpA")?,
                world_up_b: up("worldUpB")?,
                direction: byte("direction")?,
                space: byte("space")?,
                force_align: setup.field("forceAlign")?.unsigned()? != 0,
            });
        }
        Ok(out)
    }
}

fn read_box(obb: Value<'_, '_>) -> Result<UserEdgeBox> {
    // an `hkTransform` is a flat array of 16 floats in the file: the rotation's three columns of four (the fourth float
    // of each is padding: 0 in Elden Ring, stale values in all but two of Sekiro's boxes), then the translation and one
    // more float (0 in Sekiro, not 0 in Elden Ring)
    let [a0, a1, a2, _, b0, b1, b2, _, c0, c1, c2, _, tx, ty, tz, _] =
        obb.field("transform")?.vector::<16>()?;
    let [hx, hy, hz, _] = obb.field("halfExtents")?.vector::<4>()?;
    Ok(UserEdgeBox {
        columns: [[a0, a1, a2], [b0, b1, b2], [c0, c1, c2]],
        translation: [tx, ty, tz],
        half_extents: [hx, hy, hz],
    })
}

/// Everything a navmesh tagfile holds: its mesh and whichever of the other objects it stores.
#[derive(Debug, Clone, PartialEq)]
pub struct NavData {
    /// The `hkaiNavMesh`: in every file, with no faces in 94 of Sekiro's and 4,027 of Elden Ring's.
    pub mesh: NavMesh,
    /// The `hkaiDirectedGraphExplicitCost`: in every Sekiro file (empty where the mesh is), in none of Elden Ring's.
    pub graph: Option<NavGraph>,
    /// The tree of the `hkaiStaticTreeNavMeshQueryMediator`: in 6,021 Elden Ring files, none of Sekiro's.
    pub tree: Option<NavTree>,
    /// The `hkaiUserEdgeSetupArray`'s setups: in 430 Sekiro files and 188 Elden Ring ones.
    pub user_edge_setups: Option<Vec<UserEdgeSetup>>,
}

/// Read a navmesh tagfile's root `hkRootLevelContainer`.
///
/// The container holds the objects named `hkaiNavMesh`, `hkaiDirectedGraphExplicitCost`,
/// `hkaiStaticTreeNavMeshQueryMediator` and `hkaiUserEdgeSetupArray` (each named for its class), exactly one mesh and at
/// most one of the others. Refused: any other object, a name that is not the object's class, a second object of a
/// class, no mesh, a mediator that names another mesh than the file's, a tree whose leaves are not the mesh's faces
/// once each, a graph with a different number of nodes than the mesh has clusters.
pub fn nav_data(root: Value<'_, '_>) -> Result<NavData> {
    expect_class(root, "hkRootLevelContainer")?;
    let variants = root.field("namedVariants")?.array()?;
    let (mut mesh, mut graph, mut mediator, mut setups) = (None, None, None, None);
    for i in 0..variants.len() {
        let named = variants.get(i)?;
        let object = named.field("variant")?.dereference()?;
        let class = named.field("className")?.string()?;
        if class != object.type_name() || named.field("name")?.string()? != class {
            return Err(invalid(
                named.at(),
                format!("the variant `{class}` holds a {}", object.type_name()),
            ));
        }
        let taken = match class.as_str() {
            "hkaiNavMesh" => mesh.replace(object),
            "hkaiDirectedGraphExplicitCost" => graph.replace(object),
            "hkaiStaticTreeNavMeshQueryMediator" => mediator.replace(object),
            "hkaiUserEdgeSetupArray" => setups.replace(object),
            other => {
                return Err(invalid(
                    named.at(),
                    format!("a navmesh file holds a {other}, which is not read"),
                ));
            }
        };
        if taken.is_some() {
            return Err(invalid(named.at(), format!("two {class} objects")));
        }
    }
    let mesh_object = mesh.ok_or_else(|| invalid(root.at(), "the file holds no hkaiNavMesh"))?;
    let navmesh = NavMesh::read(mesh_object)?;
    let tree = match mediator {
        Some(mediator) => {
            let named = mediator.field("navMesh")?.pointer()?;
            if named.map(|m| m.offset) != Some(mesh_object.offset) {
                return Err(invalid(
                    mediator.at(),
                    "the mediator names another navmesh than the file's",
                ));
            }
            let tree = NavTree::read(mediator)?;
            if !tree.covers_faces_once(navmesh.faces.len()) {
                return Err(invalid(
                    mediator.at(),
                    "the tree's leaves are not the mesh's faces once each",
                ));
            }
            Some(tree)
        }
        None => None,
    };
    let graph = graph.map(NavGraph::read).transpose()?;
    let data = NavData {
        mesh: navmesh,
        graph,
        tree,
        user_edge_setups: setups.map(UserEdgeSetup::read_array).transpose()?,
    };
    if data.graph.is_some() && !data.graph_is_cluster_graph() {
        return Err(invalid(
            root.at(),
            "the graph is not the cluster graph of the mesh",
        ));
    }
    Ok(data)
}

impl NavData {
    /// Whether the graph is the mesh's cluster graph: as many nodes as clusters (numbered from 0, every face in
    /// one), and an edge from cluster `x` to cluster `y` exactly when an edge of a face of `x` has its opposite in a
    /// face of `y`, with no edge repeated. True for all 1,509 Sekiro files; also true when there is no graph.
    pub fn graph_is_cluster_graph(&self) -> bool {
        let Some(graph) = &self.graph else { return true };
        let mut clusters = BTreeSet::new();
        for face in &self.mesh.faces {
            match face.cluster {
                Some(c) => {
                    clusters.insert(c);
                }
                None => return graph.nodes.is_empty() && self.mesh.faces.is_empty(),
            }
        }
        if clusters.len() != graph.nodes.len() || clusters.iter().enumerate().any(|(n, &c)| n != c) {
            return false;
        }
        let mut adjacent = BTreeSet::new();
        for edge in &self.mesh.edges {
            let Some(link) = edge.opposite else { continue };
            let from = self.mesh.faces.get(edge.face).and_then(|f| f.cluster);
            let to = self.mesh.faces.get(link.face).and_then(|f| f.cluster);
            if let (Some(from), Some(to)) = (from, to)
                && from != to
            {
                adjacent.insert((from, to));
            }
        }
        let mut stored = BTreeSet::new();
        for (node, n) in graph.nodes.iter().enumerate() {
            for edge in graph
                .edges
                .get(n.first_edge..n.first_edge.saturating_add(n.edge_count))
                .unwrap_or_default()
            {
                if !stored.insert((node, edge.target)) {
                    return false;
                }
            }
        }
        stored == adjacent
    }
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    reason = "test fixtures built by hand"
)]
mod tests {
    use super::*;

    const BOX: NavAabb = NavAabb {
        min: [0.0, 0.0, 0.0, -1.0],
        max: [2.0, 0.0, 1.0, 1.0],
    };

    /// Two triangles (0, 1, 2) and (0, 2, 3) of the unit-ish quad, sharing the diagonal 0-2, in Elden Ring's layout.
    fn quad() -> RawNavMesh {
        // vertices 0..3 around the quad
        let face = |start| [start, -1, 3, 0, -1];
        // edges: face 0 = 0->1, 1->2, 2->0; face 1 = 0->2, 2->3, 3->0. Edge 2 (2->0) and edge 3 (0->2) are opposite.
        let edge = |a, b, edge, face| [a, b, edge, face, 4];
        let none = 0xFFFF_FFFFi64;
        RawNavMesh {
            faces: vec![face(0), face(3)],
            edges: vec![
                edge(0, 1, none, none),
                edge(1, 2, none, none),
                edge(2, 0, 3, 1),
                edge(0, 2, 2, 0),
                edge(2, 3, none, none),
                edge(3, 0, none, none),
            ],
            edge_costs: vec![0.0; 6],
            vertices: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 0.0, 1.0], [0.0, 0.0, 1.0]],
            face_data: vec![0, 1],
            edge_data: vec![0; 6],
            face_data_stride: 1,
            edge_data_stride: 1,
            aabb: BOX,
            erosion_radius: 0.0,
            user_data: 0,
            flags: 0,
            streaming_sets: 0,
        }
    }

    #[test]
    fn a_quad_of_two_faces_reads_with_its_links() {
        let mesh = NavMesh::from_parts(quad(), 0).unwrap();
        assert_eq!(mesh.faces.len(), 2);
        assert_eq!(mesh.faces[1].edges(), 3..6);
        assert_eq!(mesh.faces[0].user_edges(), 0..0);
        assert_eq!(mesh.edges[2].opposite, Some(NavLink { edge: 3, face: 1 }));
        assert_eq!(mesh.edges[3].opposite, Some(NavLink { edge: 2, face: 0 }));
        assert_eq!(mesh.edges[0].opposite, None);
        assert_eq!(mesh.edges[4].face, 1);
        assert!(mesh.edges.iter().all(|e| !e.user));
        assert_eq!(mesh.corners(1), Some(vec![0, 2, 3]));
        assert_eq!(mesh.face_words(1), Some(&[1][..]));
        assert_eq!(mesh.edge_words(5), Some(&[0][..]));
        assert_eq!(mesh.corners(2), None);
    }

    #[test]
    fn a_face_is_triangulated_in_its_own_plane() {
        // one face, a square seen as a quad ring
        let mut raw = quad();
        raw.faces = vec![[0, -1, 4, 0, -1]];
        raw.edges = vec![
            [0, 1, 0xFFFF_FFFF, 0xFFFF_FFFF, 4],
            [1, 2, 0xFFFF_FFFF, 0xFFFF_FFFF, 4],
            [2, 3, 0xFFFF_FFFF, 0xFFFF_FFFF, 4],
            [3, 0, 0xFFFF_FFFF, 0xFFFF_FFFF, 4],
        ];
        raw.edge_costs = vec![0.0; 4];
        raw.face_data = vec![0];
        raw.edge_data = vec![0; 4];
        let mesh = NavMesh::from_parts(raw, 0).unwrap();
        let cut = mesh.triangulate(0).unwrap();
        assert!(cut.complete);
        assert_eq!(cut.triangles.len(), 2);
        assert!(cut.triangles.iter().flatten().all(|&v| v < 4));
        assert!(mesh.triangulate(1).is_none());
    }

    #[test]
    fn broken_meshes_are_refused_by_what_is_wrong() {
        let refused = |raw: RawNavMesh, what: &str| {
            let error = NavMesh::from_parts(raw, 0x10).unwrap_err();
            assert_eq!(error.offset, 0x10);
            assert!(error.detail.contains(what), "{what}: {error}");
        };
        let mut raw = quad();
        raw.streaming_sets = 1;
        refused(raw, "streaming sets");
        let mut raw = quad();
        raw.faces[1][2] = 2;
        refused(raw, "fewer than 3");
        let mut raw = quad();
        raw.faces[1][0] = 2;
        refused(raw, "belongs to faces");
        let mut raw = quad();
        raw.faces[1][0] = 4;
        refused(raw, "run past");
        let mut raw = quad();
        raw.edges[5][1] = 9;
        refused(raw, "outside the 4 vertices");
        let mut raw = quad();
        raw.edges[1][1] = 3;
        refused(raw, "do not close a ring");
        let mut raw = quad();
        raw.edges[3][3] = 1;
        refused(raw, "not owned by face");
        let mut raw = quad();
        raw.edges[3][2] = 0xFFFF_FFFF;
        refused(raw, "only one");
        let mut raw = quad();
        raw.edges[2][2] = 5;
        refused(raw, "does not point back");
        let mut raw = quad();
        raw.faces[0][1] = 4;
        refused(raw, "startUserEdgeIndex");
        let mut raw = quad();
        raw.vertices[2][1] = f32::NAN;
        refused(raw, "not finite");
        let mut raw = quad();
        raw.face_data.push(0);
        refused(raw, "faceData has 3 words");
        let mut raw = quad();
        raw.edge_data_stride = 2;
        refused(raw, "edgeData has 6 words");
        // an edge no face owns
        let mut raw = quad();
        raw.edges.push([0, 1, 0xFFFF_FFFF, 0xFFFF_FFFF, 4]);
        raw.edge_costs.push(0.0);
        raw.edge_data.push(0);
        refused(raw, "belongs to no face");
    }

    #[test]
    fn user_edges_follow_the_regular_ones_and_are_marked() {
        // face 0 gets one user edge (edge 3), joined to face 1's user edge (edge 4) over a link; each face has its ring
        let mut raw = quad();
        let none = 0xFFFF_FFFFi64;
        raw.faces = vec![[0, 6, 3, 1, -1], [3, 7, 3, 1, -1]];
        raw.edges = vec![
            [0, 1, none, none, 4],
            [1, 2, none, none, 4],
            [2, 0, none, none, 4],
            [0, 2, none, none, 4],
            [2, 3, none, none, 4],
            [3, 0, none, none, 4],
            [0, 2, 7, 1, 20],
            [2, 3, 6, 0, 20],
        ];
        raw.edge_costs = vec![0.0; 8];
        raw.edge_data = vec![0, 0, 0, 0, 0, 0, 4, 4];
        let mesh = NavMesh::from_parts(raw, 0).unwrap();
        assert_eq!(mesh.faces[0].user_edges(), 6..7);
        assert!(mesh.edges[6].user && mesh.edges[7].user && !mesh.edges[5].user);
        assert_eq!(mesh.edges[6].face, 0);
        assert_eq!(mesh.edges[6].opposite, Some(NavLink { edge: 7, face: 1 }));
        // the ring check covers the regular edges only: the user edges do not chain
        assert_eq!(mesh.corners(0), Some(vec![0, 1, 2]));
    }

    #[test]
    fn a_graph_must_tile_its_edges_and_lead_to_its_nodes() {
        let graph = || RawNavGraph {
            positions: vec![[0.0; 3], [1.0, 0.0, 0.0]],
            nodes: vec![[0, 1], [1, 1]],
            edges: vec![(16.5, 0, 1), (16.5, 0, 0)],
            node_data: Vec::new(),
            edge_data: Vec::new(),
            node_data_stride: 0,
            edge_data_stride: 0,
            streaming_sets: 0,
        };
        let ok = NavGraph::from_parts(graph(), 0).unwrap();
        assert_eq!(ok.edges[0].target, 1);
        assert_eq!(
            ok.nodes[1],
            NavGraphNode {
                first_edge: 1,
                edge_count: 1
            }
        );
        let refused = |raw: RawNavGraph, what: &str| {
            let error = NavGraph::from_parts(raw, 0x20).unwrap_err();
            assert_eq!(error.offset, 0x20);
            assert!(error.detail.contains(what), "{what}: {error}");
        };
        let mut raw = graph();
        raw.nodes[1] = [0, 1];
        refused(raw, "not at 1");
        let mut raw = graph();
        raw.edges.push((1.0, 0, 0));
        refused(raw, "cover 2 of the 3");
        let mut raw = graph();
        raw.edges[0].2 = 2;
        refused(raw, "outside the 2 nodes");
        let mut raw = graph();
        raw.positions.pop();
        refused(raw, "1 positions for 2 nodes");
        let mut raw = graph();
        raw.edges[1].0 = f32::INFINITY;
        refused(raw, "cost is not finite");
        let mut raw = graph();
        raw.node_data_stride = 1;
        refused(raw, "nodeData has 0 words");
        let mut raw = graph();
        raw.streaming_sets = 2;
        refused(raw, "streaming sets");
    }

    fn node(axes: [u8; 3], data: u32) -> RawNavTreeNode {
        RawNavTreeNode {
            axes,
            hi: (data >> 16) as u8,
            lo: data as u16,
        }
    }

    #[test]
    fn a_tree_of_three_nodes_decodes_its_boxes_and_leaves() {
        // root (internal, second child at 0 + 2 * 2 = 4 would be outside): with 3 nodes the second child is 1 + 1 = 2
        // nodes on, so the data is 0x800001 + ... = bit 23 and a payload of 1 (2 nodes)
        let domain = [[0.0, 0.0, 0.0], [10.0, 10.0, 10.0]];
        // the first child is a leaf for face 0 whose axis bytes cut nothing, the second a leaf for face 1 that takes
        // 1/226 * 15^2 = 0.995 of the extent from the maximum side of y
        let nodes = [
            node([0, 0, 0], 0x80_0001),
            node([0, 0, 0], 0),
            node([0, 0x0f, 0], 1),
        ];
        let tree = NavTree::decode(&nodes, domain, 0).unwrap();
        assert_eq!(tree.nodes[0].kind, NavTreeKind::Internal { second_child: 2 });
        assert_eq!(tree.nodes[1].kind, NavTreeKind::Leaf { face: 0 });
        assert_eq!(tree.nodes[2].kind, NavTreeKind::Leaf { face: 1 });
        assert_eq!(tree.nodes[2].aabb[0], [0.0, 0.0, 0.0]);
        let y = 10.0 - 10.0 * 225.0 / 226.0;
        assert!(
            (tree.nodes[2].aabb[1][1] - y).abs() < 1e-12,
            "{:?}",
            tree.nodes[2].aabb
        );
        assert!(tree.covers_faces_once(2));
        assert!(!tree.covers_faces_once(3));
        assert!(!tree.covers_faces_once(1));
    }

    #[test]
    fn broken_trees_are_refused_and_an_empty_one_is_kept() {
        let domain = [[0.0, 0.0, 0.0], [1.0, 1.0, 1.0]];
        // a second child past the subtree, a leaf that does not end its subtree, an internal node whose step is 0
        for (nodes, what) in [
            (
                vec![node([0; 3], 0x80_0002), node([0; 3], 0), node([0; 3], 1)],
                "its second child",
            ),
            (vec![node([0; 3], 0), node([0; 3], 1)], "does not end its subtree"),
            (vec![node([0; 3], 0x80_0000), node([0; 3], 0)], "its second child"),
        ] {
            let error = NavTree::decode(&nodes, domain, 7).unwrap_err();
            assert_eq!(error.offset, 7);
            assert!(error.detail.contains(what), "{what}: {error}");
        }
        let error = NavTree::decode(&[node([0; 3], 0)], [[f32::NAN; 3], [1.0; 3]], 0).unwrap_err();
        assert!(error.detail.contains("cannot decode"), "{error}");
        // a mesh without faces stores no nodes and a domain of ±f32::MAX
        let empty = NavTree::decode(&[], [[f32::MAX; 3], [-f32::MAX; 3]], 0).unwrap();
        assert!(empty.nodes.is_empty());
        assert!(empty.covers_faces_once(0));
        assert!(!empty.covers_faces_once(1));
    }

    #[test]
    fn the_cluster_graph_must_be_the_adjacency_of_the_clusters() {
        let mut raw = quad();
        raw.faces[0][4] = 0;
        raw.faces[1][4] = 1;
        let mesh = NavMesh::from_parts(raw, 0).unwrap();
        let graph = |edges: Vec<(f32, u16, u32)>, nodes: Vec<[i64; 2]>| {
            NavGraph::from_parts(
                RawNavGraph {
                    positions: vec![[0.0; 3]; nodes.len()],
                    nodes,
                    edges,
                    node_data: Vec::new(),
                    edge_data: Vec::new(),
                    node_data_stride: 0,
                    edge_data_stride: 0,
                    streaming_sets: 0,
                },
                0,
            )
            .unwrap()
        };
        let both = graph(vec![(1.0, 0, 1), (1.0, 0, 0)], vec![[0, 1], [1, 1]]);
        let data = NavData {
            mesh: mesh.clone(),
            graph: Some(both),
            tree: None,
            user_edge_setups: None,
        };
        assert!(data.graph_is_cluster_graph());
        let one_way = graph(vec![(1.0, 0, 1)], vec![[0, 1], [1, 0]]);
        let data = NavData {
            graph: Some(one_way),
            ..data
        };
        assert!(!data.graph_is_cluster_graph());
        let none = NavData { graph: None, ..data };
        assert!(none.graph_is_cluster_graph());
    }
}
