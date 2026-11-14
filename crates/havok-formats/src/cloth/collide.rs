//! A sim cloth's collidables (`hclCollidable`) and their shapes (`hclShape` and the classes derived from it).

use super::{flag, items, matrix, real, sim::signed8, text, vec4};
use crate::enums::hcl::ShapeType;
use crate::{Result, Value, invalid};

/// The shape of a collidable, by its class. The `type` member every shape stores is 0 in both installs; the class says
/// what the shape is.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    /// `hclSphereShape`: `sphere.pos`, the centre in `x y z` and the radius in `w`.
    Sphere {
        /// `sphere.pos`.
        pos: [f32; 4],
    },
    /// `hclCapsuleShape`.
    Capsule {
        /// `start`.
        start: [f32; 4],
        /// `end`.
        end: [f32; 4],
        /// `dir`.
        dir: [f32; 4],
        /// `radius`.
        radius: f32,
        /// `capLenSqrdInv`.
        cap_len_sqrd_inv: f32,
    },
    /// `hclTaperedCapsuleShape`: a capsule whose two ends have different radii.
    TaperedCapsule {
        /// `small`.
        small: [f32; 4],
        /// `big`.
        big: [f32; 4],
        /// `coneApex`.
        cone_apex: [f32; 4],
        /// `coneAxis`.
        cone_axis: [f32; 4],
        /// `lVec`.
        l_vec: [f32; 4],
        /// `dVec`.
        d_vec: [f32; 4],
        /// `tanThetaVecNeg`.
        tan_theta_vec_neg: [f32; 4],
        /// `smallRadius`.
        small_radius: f32,
        /// `bigRadius`.
        big_radius: f32,
        /// `l`.
        l: f32,
        /// `d`.
        d: f32,
        /// `cosTheta`.
        cos_theta: f32,
        /// `sinTheta`.
        sin_theta: f32,
        /// `tanTheta`.
        tan_theta: f32,
        /// `tanThetaSqr`.
        tan_theta_sqr: f32,
    },
    /// `hclPlaneShape`.
    Plane {
        /// `planeEquation`.
        equation: [f32; 4],
    },
    /// `hclConvexGeometryShape`: a convex mesh split into tetrahedra, looked up through a grid.
    ConvexGeometry {
        /// `tetrahedraGrid`.
        tetrahedra_grid: Vec<u16>,
        /// `gridCells`.
        grid_cells: Vec<u8>,
        /// `tetrahedraEquations`: one `hkMatrix4` a tetrahedron (four columns of four).
        tetrahedra_equations: Vec<[f32; 16]>,
        /// `localFromWorld` (an `hkTransform`, four columns of four).
        local_from_world: [f32; 16],
        /// `worldFromLocal`.
        world_from_local: [f32; 16],
        /// `objAabb.min`.
        aabb_min: [f32; 4],
        /// `objAabb.max`.
        aabb_max: [f32; 4],
        /// `geomCentroid`.
        centroid: [f32; 4],
        /// `invCellSize`.
        inv_cell_size: [f32; 4],
        /// `gridRes`.
        grid_res: u16,
    },
    /// A class this reader has no layout for, kept by its name.
    Other {
        /// The shape's class name.
        class: String,
    },
}

impl Shape {
    /// The `hclShapeType` of the shape's class. The shape's own `type` member is not stored, so this is the only way
    /// to name it; `None` for a class of the `Other` variant that is not one of the shape classes of HKLib's 2018.1
    /// reflection.
    pub fn shape_type(&self) -> Option<ShapeType> {
        match self {
            Self::Sphere { .. } => Some(ShapeType::Sphere),
            Self::Capsule { .. } => Some(ShapeType::Capsule),
            Self::TaperedCapsule { .. } => Some(ShapeType::TaperedCapsule),
            Self::Plane { .. } => Some(ShapeType::Plane),
            Self::ConvexGeometry { .. } => Some(ShapeType::ConvexGeometry),
            Self::Other { class } => ShapeType::of_class(class),
        }
    }
}

/// `hclCollidable`.
#[derive(Clone, Debug, PartialEq)]
pub struct Collidable {
    /// `name`.
    pub name: String,
    /// `transform` (an `hkTransform`, four columns of four).
    pub transform: [f32; 16],
    /// `linearVelocity`.
    pub linear_velocity: [f32; 4],
    /// `angularVelocity`.
    pub angular_velocity: [f32; 4],
    /// `userData`.
    pub user_data: u64,
    /// `shape`.
    pub shape: Shape,
    /// `pinchDetectionRadius`.
    pub pinch_detection_radius: f32,
    /// `pinchDetectionPriority`.
    pub pinch_detection_priority: i8,
    /// `pinchDetectionEnabled`.
    pub pinch_detection_enabled: bool,
    /// `virtualCollisionPointCollisionEnabled`.
    pub virtual_collision_point_collision: bool,
    /// `enabled`.
    pub enabled: bool,
}

fn shape(v: Value<'_, '_>) -> Result<Shape> {
    Ok(match v.type_name() {
        "hclSphereShape" => Shape::Sphere {
            pos: vec4(v.field("sphere")?, "pos")?,
        },
        "hclCapsuleShape" => Shape::Capsule {
            start: vec4(v, "start")?,
            end: vec4(v, "end")?,
            dir: vec4(v, "dir")?,
            radius: real(v, "radius")?,
            cap_len_sqrd_inv: real(v, "capLenSqrdInv")?,
        },
        "hclTaperedCapsuleShape" => Shape::TaperedCapsule {
            small: vec4(v, "small")?,
            big: vec4(v, "big")?,
            cone_apex: vec4(v, "coneApex")?,
            cone_axis: vec4(v, "coneAxis")?,
            l_vec: vec4(v, "lVec")?,
            d_vec: vec4(v, "dVec")?,
            tan_theta_vec_neg: vec4(v, "tanThetaVecNeg")?,
            small_radius: real(v, "smallRadius")?,
            big_radius: real(v, "bigRadius")?,
            l: real(v, "l")?,
            d: real(v, "d")?,
            cos_theta: real(v, "cosTheta")?,
            sin_theta: real(v, "sinTheta")?,
            tan_theta: real(v, "tanTheta")?,
            tan_theta_sqr: real(v, "tanThetaSqr")?,
        },
        "hclPlaneShape" => Shape::Plane {
            equation: vec4(v, "planeEquation")?,
        },
        "hclConvexGeometryShape" => {
            let aabb = v.field("objAabb")?;
            let grid = v.field("gridRes")?;
            Shape::ConvexGeometry {
                tetrahedra_grid: super::words16(v, "tetrahedraGrid")?,
                grid_cells: items(v, "gridCells", |b| {
                    u8::try_from(b.unsigned()?).map_err(|_| invalid(b.at(), "grid cell outside u8"))
                })?,
                tetrahedra_equations: items(v, "tetrahedraEquations", matrix)?,
                local_from_world: matrix(v.field("localFromWorld")?)?,
                world_from_local: matrix(v.field("worldFromLocal")?)?,
                aabb_min: vec4(aabb, "min")?,
                aabb_max: vec4(aabb, "max")?,
                centroid: vec4(v, "geomCentroid")?,
                inv_cell_size: vec4(v, "invCellSize")?,
                grid_res: u16::try_from(grid.unsigned()?)
                    .map_err(|_| invalid(grid.at(), "gridRes outside u16"))?,
            }
        }
        other => Shape::Other {
            class: other.to_owned(),
        },
    })
}

pub(super) fn collidable(v: Value<'_, '_>) -> Result<Collidable> {
    Ok(Collidable {
        name: text(v, "name")?,
        transform: matrix(v.field("transform")?)?,
        linear_velocity: vec4(v, "linearVelocity")?,
        angular_velocity: vec4(v, "angularVelocity")?,
        user_data: v.field("userData")?.unsigned()?,
        shape: shape(v.field("shape")?.dereference()?)?,
        pinch_detection_radius: real(v, "pinchDetectionRadius")?,
        pinch_detection_priority: signed8(v, "pinchDetectionPriority")?,
        pinch_detection_enabled: flag(v, "pinchDetectionEnabled")?,
        virtual_collision_point_collision: flag(v, "virtualCollisionPointCollisionEnabled")?,
        enabled: flag(v, "enabled")?,
    })
}
