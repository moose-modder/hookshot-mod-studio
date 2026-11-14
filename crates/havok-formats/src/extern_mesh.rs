//! The triangle meshes of `hknpExternMeshShape` bodies (the `f` collision binders of Sekiro and Elden Ring): a shape
//! that holds its triangles as a plain `hkGeometry`, read by [`extern_mesh`].
use crate::{Result, Value, invalid};

/// The class an extern mesh shape's geometry object is: the only one that occurs.
const GEOMETRY_CLASS: &str = "hknpDefaultExternMeshShapeGeometry";

/// An extern mesh shape's triangles as its `hkGeometry` stores them, in the shape's own space.
#[derive(Clone, Debug, PartialEq)]
pub struct ExternMesh {
    /// `vertices`, each `xyz` (the stored fourth lane is 1).
    pub vertices: Vec<[f32; 3]>,
    /// `triangles`, each as the indices `a`, `b`, `c` of `vertices`, all in range.
    pub triangles: Vec<[u32; 3]>,
    /// Each triangle's stored `material`, as stored (−1 where the file gives none).
    pub materials: Vec<i32>,
    /// `useTriangleMaterialAsShapeTag`, where the geometry class has it (2018.1's, not 2016.2's).
    pub material_as_shape_tag: Option<bool>,
}

/// The triangles of an `hknpExternMeshShape` object: its `geometry` must be an `hknpDefaultExternMeshShapeGeometry`
/// holding an `hkGeometry`. Every vertex must be finite with a fourth lane of exactly 1, and every triangle index
/// must name a vertex (all of the 12,688 Sekiro and 264,858 Elden Ring extern mesh shapes of the map collision binders
/// do); anything else is refused. The shape's pose belongs to the body that holds it.
pub fn extern_mesh(shape: Value<'_, '_>) -> Result<ExternMesh> {
    if shape.type_name() != "hknpExternMeshShape" {
        return Err(invalid(
            shape.at(),
            format!("expected an hknpExternMeshShape, not a {}", shape.type_name()),
        ));
    }
    let geometry = shape.field("geometry")?.dereference()?;
    if geometry.type_name() != GEOMETRY_CLASS {
        return Err(invalid(
            geometry.at(),
            format!(
                "an extern mesh shape's geometry is a {}, not an {GEOMETRY_CLASS}",
                geometry.type_name()
            ),
        ));
    }
    let material_as_shape_tag = if geometry.has_field("useTriangleMaterialAsShapeTag") {
        Some(geometry.field("useTriangleMaterialAsShapeTag")?.unsigned()? != 0)
    } else {
        None
    };
    let mesh = geometry.field("geometry")?.dereference()?;
    if mesh.type_name() != "hkGeometry" {
        return Err(invalid(
            mesh.at(),
            format!(
                "an extern mesh's geometry is a {}, not an hkGeometry",
                mesh.type_name()
            ),
        ));
    }
    let stored = mesh.field("vertices")?.array()?;
    let mut vertices = Vec::with_capacity(stored.len());
    for i in 0..stored.len() {
        let vertex = stored.get(i)?;
        let [x, y, z, w] = vertex.vector::<4>()?;
        if ![x, y, z].iter().all(|f| f.is_finite()) || w != 1.0 {
            return Err(invalid(
                vertex.at(),
                format!("extern mesh vertex {i} is ({x}, {y}, {z}, {w}), not a finite point with w 1"),
            ));
        }
        vertices.push([x, y, z]);
    }
    let stored = mesh.field("triangles")?.array()?;
    let mut triangles = Vec::with_capacity(stored.len());
    let mut materials = Vec::with_capacity(stored.len());
    for i in 0..stored.len() {
        let triangle = stored.get(i)?;
        let corner = |name: &str| -> Result<u32> {
            let index = triangle.field(name)?.signed()?;
            usize::try_from(index)
                .ok()
                .filter(|&n| n < vertices.len())
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| {
                    invalid(
                        triangle.at(),
                        format!(
                            "extern mesh triangle {i} names vertex {index} (`{name}`) of {}",
                            vertices.len()
                        ),
                    )
                })
        };
        triangles.push([corner("a")?, corner("b")?, corner("c")?]);
        let material = i32::try_from(triangle.field("material")?.signed()?)
            .map_err(|_| invalid(triangle.at(), "extern mesh triangle material exceeds i32"))?;
        materials.push(material);
    }
    Ok(ExternMesh {
        vertices,
        triangles,
        materials,
        material_as_shape_tag,
    })
}
