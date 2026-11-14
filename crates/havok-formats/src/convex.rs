//! Convex collision shapes of a tagfile as the vertices, planes and faces of their core (`hknpConvexShape` and the
//! classes built on it), read by [`ConvexShape::read`].

use crate::{Result, Value, invalid};

/// What a convex shape stores of its core.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConvexKind {
    /// Vertices, faces (as runs of an index array) and one plane a face (`hknpConvexPolytopeShape` and the classes
    /// built on it: box, capsule, cylinder, triangle).
    Polytope,
    /// Vertices only (`hknpConvexShape`): its faces are those of the vertices' convex hull, which the file does not
    /// store.
    Vertices,
    /// One vertex, the centre (`hknpSphereShape`): the radius is the sphere's.
    Sphere,
}

/// A convex shape's core as its file stores it: the vertices, the faces as vertex indices, one plane `[nx, ny, nz, w]` a
/// face (`n . v + w = 0` on the face), and the convex radius the surface lies at around the core. Values are as
/// stored: nothing is checked beyond the references between them and, for a polytope, that it has as many planes as
/// faces ([`ConvexShape::read`]).
#[derive(Clone, Debug, PartialEq)]
pub struct ConvexShape {
    /// Which core the shape stores.
    pub kind: ConvexKind,
    /// The shape object's offset in the file, for refusals that name it.
    pub offset: usize,
    /// `vertices`, each `xyz`.
    pub vertices: Vec<[f32; 3]>,
    /// Empty for [`ConvexKind::Vertices`] and [`ConvexKind::Sphere`].
    pub planes: Vec<[f32; 4]>,
    /// Empty for [`ConvexKind::Vertices`] and [`ConvexKind::Sphere`].
    pub faces: Vec<Vec<usize>>,
    /// `convexRadius`.
    pub radius: f32,
}

impl ConvexShape {
    /// The stored core of a convex shape object: `convexRadius` and the `xyz` of each `vertices` entry; for a polytope
    /// also `planes`, and each of `faces` as its `numIndices` entries of `indices` from `firstIndex`. A face running
    /// past the index array, or a count that does not fit the platform, is refused; a polytope must store one plane a
    /// face (every box, polytope, capsule, cylinder and triangle shape of the Sekiro and Elden Ring collision does:
    /// 8,373 and 20,948 shapes); a sphere must store exactly one vertex.
    pub fn read(shape: Value<'_, '_>) -> Result<Self> {
        let radius = shape.field("convexRadius")?.float()?;
        let stored = shape.field("vertices")?.array()?;
        let mut vertices = Vec::with_capacity(stored.len());
        for i in 0..stored.len() {
            let v = stored.get(i)?.vector::<4>()?;
            vertices.push([v[0], v[1], v[2]]);
        }
        let only = |kind| Self {
            kind,
            offset: shape.at(),
            vertices: Vec::new(),
            planes: Vec::new(),
            faces: Vec::new(),
            radius,
        };
        if shape.type_name() == "hknpConvexShape" {
            return Ok(Self {
                vertices,
                ..only(ConvexKind::Vertices)
            });
        }
        if shape.type_name() == "hknpSphereShape" {
            if vertices.len() != 1 {
                return Err(invalid(shape.at(), "a sphere shape without exactly one vertex"));
            }
            return Ok(Self {
                vertices,
                ..only(ConvexKind::Sphere)
            });
        }
        let stored = shape.field("planes")?.array()?;
        let mut planes = Vec::with_capacity(stored.len());
        for i in 0..stored.len() {
            planes.push(stored.get(i)?.vector::<4>()?);
        }
        let indices = shape.field("indices")?.array()?;
        let stored = shape.field("faces")?.array()?;
        let mut faces = Vec::with_capacity(stored.len());
        for i in 0..stored.len() {
            let face = stored.get(i)?;
            let start = usize::try_from(face.field("firstIndex")?.unsigned()?)
                .map_err(|_| invalid(face.at(), "face index overflow"))?;
            let count = usize::try_from(face.field("numIndices")?.unsigned()?)
                .map_err(|_| invalid(face.at(), "face size overflow"))?;
            let end = start
                .checked_add(count)
                .filter(|&e| e <= indices.len())
                .ok_or_else(|| invalid(face.at(), "face exceeds index array"))?;
            let mut polygon = Vec::with_capacity(count);
            for k in start..end {
                polygon.push(
                    usize::try_from(indices.get(k)?.unsigned()?)
                        .map_err(|_| invalid(face.at(), "vertex index overflow"))?,
                );
            }
            faces.push(polygon);
        }
        if planes.len() != faces.len() {
            return Err(invalid(
                shape.at(),
                format!(
                    "a polytope with {} planes and {} faces",
                    planes.len(),
                    faces.len()
                ),
            ));
        }
        Ok(Self {
            vertices,
            planes,
            faces,
            ..only(ConvexKind::Polytope)
        })
    }
}
