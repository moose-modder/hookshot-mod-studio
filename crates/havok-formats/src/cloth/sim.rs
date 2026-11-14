//! `hclSimClothData`: the particles a simulate operator moves, with their constraints, collidables, rest poses and
//! virtual collision points.

use super::{
    collide, constraints, fixed, flag, index16, int, invalid, items, matrix, particle, real, text, uint,
    vec4, words16,
};
use crate::{Result, Value};

/// `hclSimClothData::ParticleData`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Particle {
    /// `mass`.
    pub mass: f32,
    /// `invMass`, as stored (the reciprocal of `mass` for a free particle).
    pub inv_mass: f32,
    /// `radius`.
    pub radius: f32,
    /// `friction`.
    pub friction: f32,
}

/// A rest pose of the particles (`hclSimClothPose`): one position (`x y z w`, as stored) a particle.
#[derive(Clone, Debug, PartialEq)]
pub struct Pose {
    /// `name`.
    pub name: String,
    /// `positions`: `x y z` and the fourth lane as stored, one for each particle.
    pub positions: Vec<[f32; 4]>,
}

/// `hclSimClothData::CollidableTransformMap`, when its `transformSetIndex` is not -1: the bones that drive the sim
/// cloth's collidables.
#[derive(Clone, Debug, PartialEq)]
pub struct CollidableTransformMap {
    /// `transformSetIndex`: the transform set the bones are in.
    pub transform_set: u32,
    /// `transformIndices`: for each collidable, its bone in that set.
    pub transform_indices: Vec<u32>,
    /// `offsets`: for each collidable, the bone-from-collidable matrix (`hkMatrix4`, four columns of four).
    pub offsets: Vec<[f32; 16]>,
}

/// `hclSimClothData::TransferMotionData`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransferMotion {
    /// `transformSetIndex`.
    pub transform_set: u32,
    /// `transformIndex`.
    pub transform: u32,
    /// `transferTranslationMotion`.
    pub transfer_translation: bool,
    /// `minTranslationSpeed`.
    pub min_translation_speed: f32,
    /// `maxTranslationSpeed`.
    pub max_translation_speed: f32,
    /// `minTranslationBlend`.
    pub min_translation_blend: f32,
    /// `maxTranslationBlend`.
    pub max_translation_blend: f32,
    /// `transferRotationMotion`.
    pub transfer_rotation: bool,
    /// `minRotationSpeed`.
    pub min_rotation_speed: f32,
    /// `maxRotationSpeed`.
    pub max_rotation_speed: f32,
    /// `minRotationBlend`.
    pub min_rotation_blend: f32,
    /// `maxRotationBlend`.
    pub max_rotation_blend: f32,
}

/// `hclSimClothData::LandscapeCollisionData`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LandscapeCollision {
    /// `landscapeRadius`.
    pub radius: f32,
    /// `enableStuckParticleDetection`.
    pub stuck_particle_detection: bool,
    /// `stuckParticlesStretchFactorSq`.
    pub stuck_particles_stretch_factor_sq: f32,
    /// `pinchDetectionEnabled`.
    pub pinch_detection: bool,
    /// `pinchDetectionPriority`.
    pub pinch_detection_priority: i8,
    /// `pinchDetectionRadius`.
    pub pinch_detection_radius: f32,
    /// `collisionTolerance`.
    pub collision_tolerance: f32,
}

/// `hclSimClothData::CollidablePinchingData`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CollidablePinching {
    /// `pinchDetectionEnabled`.
    pub enabled: bool,
    /// `pinchDetectionPriority`.
    pub priority: i8,
    /// `pinchDetectionRadius`.
    pub radius: f32,
}

/// `hclVirtualCollisionPointsData::Block`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VirtualCollisionPointsBlock {
    /// `safeDisplacementRadius`.
    pub safe_displacement_radius: f32,
    /// `startingVCPIndex`.
    pub start: u16,
    /// `numVCPs`.
    pub count: u8,
}

/// `hclVirtualCollisionPointsData::BarycentricDictionaryEntry`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VirtualDictionaryEntry {
    /// `startingBarycentricIndex`.
    pub start: u16,
    /// `numBarycentrics`.
    pub count: u8,
}

/// `hclVirtualCollisionPointsData::EdgeFanSection`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VirtualEdgeSection {
    /// `oppositeRealParticleIndex`: a particle index.
    pub opposite: u16,
    /// `barycentricDictionaryIndex`.
    pub dictionary: u16,
}

/// `hclVirtualCollisionPointsData::EdgeFan`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VirtualEdgeFan {
    /// `realParticleIndex`: a particle index.
    pub particle: u16,
    /// `edgeStartIndex`.
    pub edge_start: u16,
    /// `numEdges`.
    pub edges: u8,
}

/// `hclVirtualCollisionPointsData::EdgeFanLandscape`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VirtualEdgeFanLandscape {
    /// `realParticleIndex`: a particle index.
    pub particle: u16,
    /// `edgeStartIndex`.
    pub edge_start: u16,
    /// `vcpStartIndex`.
    pub vcp_start: u16,
    /// `numEdges`.
    pub edges: u8,
}

/// `hclVirtualCollisionPointsData::TriangleFanSection`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VirtualTriangleSection {
    /// `oppositeRealParticleIndices`: two particle indices.
    pub opposite: [u16; 2],
    /// `barycentricDictionaryIndex`.
    pub dictionary: u16,
}

/// `hclVirtualCollisionPointsData::TriangleFan`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VirtualTriangleFan {
    /// `realParticleIndex`: a particle index.
    pub particle: u16,
    /// `vcpStartIndex`.
    pub vcp_start: u16,
    /// `numTriangles`.
    pub triangles: u8,
}

/// `hclVirtualCollisionPointsData::TriangleFanLandscape`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VirtualTriangleFanLandscape {
    /// `realParticleIndex`: a particle index.
    pub particle: u16,
    /// `triangleStartIndex`.
    pub triangle_start: u16,
    /// `vcpStartIndex`.
    pub vcp_start: u16,
    /// `numTriangles`.
    pub triangles: u8,
}

/// `hclVirtualCollisionPointsData`: collision points placed on the sim cloth's edges and triangles between its
/// particles. Empty (no points) in most sim cloths.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VirtualCollisionPoints {
    /// `blocks`.
    pub blocks: Vec<VirtualCollisionPointsBlock>,
    /// `numVCPoints`.
    pub points: u16,
    /// `landscapeParticlesBlockIndex`.
    pub landscape_particles_block_index: Vec<u16>,
    /// `numLandscapeVCPoints`.
    pub landscape_points: u16,
    /// `edgeBarycentricsDictionary`.
    pub edge_barycentrics: Vec<f32>,
    /// `edgeDictionaryEntries`.
    pub edge_dictionary: Vec<VirtualDictionaryEntry>,
    /// `triangleBarycentricsDictionary`: `(u, v)` pairs.
    pub triangle_barycentrics: Vec<(f32, f32)>,
    /// `triangleDictionaryEntries`.
    pub triangle_dictionary: Vec<VirtualDictionaryEntry>,
    /// `edges`.
    pub edges: Vec<VirtualEdgeSection>,
    /// `edgeFans`.
    pub edge_fans: Vec<VirtualEdgeFan>,
    /// `triangles`.
    pub triangles: Vec<VirtualTriangleSection>,
    /// `triangleFans`.
    pub triangle_fans: Vec<VirtualTriangleFan>,
    /// `edgesLandscape`.
    pub edges_landscape: Vec<VirtualEdgeSection>,
    /// `edgeFansLandscape`.
    pub edge_fans_landscape: Vec<VirtualEdgeFanLandscape>,
    /// `trianglesLandscape`.
    pub triangles_landscape: Vec<VirtualTriangleSection>,
    /// `triangleFansLandscape`.
    pub triangle_fans_landscape: Vec<VirtualTriangleFanLandscape>,
    /// `edgeFanIndices`.
    pub edge_fan_indices: Vec<u16>,
    /// `triangleFanIndices`.
    pub triangle_fan_indices: Vec<u16>,
    /// `edgeFanIndicesLandscape`.
    pub edge_fan_indices_landscape: Vec<u16>,
    /// `triangleFanIndicesLandscape`.
    pub triangle_fan_indices_landscape: Vec<u16>,
}

/// `hclSimClothData`.
#[derive(Clone, Debug, PartialEq)]
pub struct SimCloth {
    /// `name`.
    pub name: String,
    /// `simulationInfo.gravity` (`x y z w` as stored).
    pub gravity: [f32; 4],
    /// `simulationInfo.globalDampingPerSecond`.
    pub global_damping_per_second: f32,
    /// `particleDatas`.
    pub particles: Vec<Particle>,
    /// The particles that do not move (`fixedParticles`): indices into `particles`.
    pub fixed_particles: Vec<u16>,
    /// `doNormals`.
    pub do_normals: bool,
    /// `simOpIds`: the simulate operators that move this sim cloth, as indices into the cloth data's operators.
    pub sim_op_ids: Vec<u32>,
    /// `simClothPoses`.
    pub poses: Vec<Pose>,
    /// `staticConstraintSets`.
    pub constraint_sets: Vec<ConstraintSet>,
    /// `antiPinchConstraintSets`.
    pub anti_pinch_constraint_sets: Vec<ConstraintSet>,
    /// `collidableTransformMap`; `None` when its `transformSetIndex` is -1.
    pub collidable_transform_map: Option<CollidableTransformMap>,
    /// `perInstanceCollidables`.
    pub collidables: Vec<Collidable>,
    /// `maxParticleRadius`.
    pub max_particle_radius: f32,
    /// `staticCollisionMasks`.
    pub static_collision_masks: Vec<u32>,
    /// `actions`: the class of each action object.
    pub actions: Vec<String>,
    /// `totalMass`.
    pub total_mass: f32,
    /// `transferMotionData`.
    pub transfer_motion: TransferMotion,
    /// `transferMotionEnabled`.
    pub transfer_motion_enabled: bool,
    /// `landscapeCollisionEnabled`.
    pub landscape_collision_enabled: bool,
    /// `landscapeCollisionData`.
    pub landscape_collision: LandscapeCollision,
    /// `numLandscapeCollidableParticles`.
    pub landscape_collidable_particles: u32,
    /// `triangleIndices`: three particle indices a triangle.
    pub triangle_indices: Vec<u16>,
    /// `triangleFlips`: one bit a triangle, as stored.
    pub triangle_flips: Vec<u8>,
    /// `pinchDetectionEnabled`.
    pub pinch_detection_enabled: bool,
    /// `perParticlePinchDetectionEnabledFlags`.
    pub per_particle_pinch_detection: Vec<bool>,
    /// `collidablePinchingDatas`.
    pub collidable_pinching: Vec<CollidablePinching>,
    /// `minPinchedParticleIndex`, as stored.
    pub min_pinched_particle: u16,
    /// `maxPinchedParticleIndex`, as stored.
    pub max_pinched_particle: u16,
    /// `maxCollisionPairs`.
    pub max_collision_pairs: u32,
    /// `virtualCollisionPointsData`.
    pub virtual_collision_points: VirtualCollisionPoints,
}

use super::{Collidable, ConstraintSet};

fn transfer_motion(v: Value<'_, '_>) -> Result<TransferMotion> {
    Ok(TransferMotion {
        transform_set: uint(v, "transformSetIndex")?,
        transform: uint(v, "transformIndex")?,
        transfer_translation: flag(v, "transferTranslationMotion")?,
        min_translation_speed: real(v, "minTranslationSpeed")?,
        max_translation_speed: real(v, "maxTranslationSpeed")?,
        min_translation_blend: real(v, "minTranslationBlend")?,
        max_translation_blend: real(v, "maxTranslationBlend")?,
        transfer_rotation: flag(v, "transferRotationMotion")?,
        min_rotation_speed: real(v, "minRotationSpeed")?,
        max_rotation_speed: real(v, "maxRotationSpeed")?,
        min_rotation_blend: real(v, "minRotationBlend")?,
        max_rotation_blend: real(v, "maxRotationBlend")?,
    })
}

fn landscape(v: Value<'_, '_>) -> Result<LandscapeCollision> {
    Ok(LandscapeCollision {
        radius: real(v, "landscapeRadius")?,
        stuck_particle_detection: flag(v, "enableStuckParticleDetection")?,
        stuck_particles_stretch_factor_sq: real(v, "stuckParticlesStretchFactorSq")?,
        pinch_detection: flag(v, "pinchDetectionEnabled")?,
        pinch_detection_priority: signed8(v, "pinchDetectionPriority")?,
        pinch_detection_radius: real(v, "pinchDetectionRadius")?,
        collision_tolerance: real(v, "collisionTolerance")?,
    })
}

/// Member `name` of `v`, a signed 8-bit integer.
pub(super) fn signed8(v: Value<'_, '_>, name: &str) -> Result<i8> {
    let f = v.field(name)?;
    i8::try_from(f.signed()?).map_err(|_| invalid(f.at(), format!("{name}: value outside i8")))
}

/// Member `name` of `v`, an unsigned 16-bit count or index (any 16-bit width check is the caller's).
fn word(v: Value<'_, '_>, name: &str) -> Result<u16> {
    index16(v.field(name)?)
}

fn dictionary_entry(v: Value<'_, '_>) -> Result<VirtualDictionaryEntry> {
    Ok(VirtualDictionaryEntry {
        start: word(v, "startingBarycentricIndex")?,
        count: super::byte(v, "numBarycentrics")?,
    })
}

fn edge_section(v: Value<'_, '_>, count: usize) -> Result<VirtualEdgeSection> {
    Ok(VirtualEdgeSection {
        opposite: particle(v.field("oppositeRealParticleIndex")?, count)?,
        dictionary: word(v, "barycentricDictionaryIndex")?,
    })
}

fn triangle_section(v: Value<'_, '_>, count: usize) -> Result<VirtualTriangleSection> {
    Ok(VirtualTriangleSection {
        opposite: fixed(v, "oppositeRealParticleIndices", |p| particle(p, count))?,
        dictionary: word(v, "barycentricDictionaryIndex")?,
    })
}

fn virtual_collision_points(v: Value<'_, '_>, count: usize) -> Result<VirtualCollisionPoints> {
    Ok(VirtualCollisionPoints {
        blocks: items(v, "blocks", |b| {
            Ok(VirtualCollisionPointsBlock {
                safe_displacement_radius: real(b, "safeDisplacementRadius")?,
                start: word(b, "startingVCPIndex")?,
                count: super::byte(b, "numVCPs")?,
            })
        })?,
        points: word(v, "numVCPoints")?,
        landscape_particles_block_index: words16(v, "landscapeParticlesBlockIndex")?,
        landscape_points: word(v, "numLandscapeVCPoints")?,
        edge_barycentrics: items(v, "edgeBarycentricsDictionary", |x| x.float())?,
        edge_dictionary: items(v, "edgeDictionaryEntries", dictionary_entry)?,
        triangle_barycentrics: items(v, "triangleBarycentricsDictionary", |p| {
            Ok((real(p, "u")?, real(p, "v")?))
        })?,
        triangle_dictionary: items(v, "triangleDictionaryEntries", dictionary_entry)?,
        edges: items(v, "edges", |e| edge_section(e, count))?,
        edge_fans: items(v, "edgeFans", |f| {
            Ok(VirtualEdgeFan {
                particle: particle(f.field("realParticleIndex")?, count)?,
                edge_start: word(f, "edgeStartIndex")?,
                edges: super::byte(f, "numEdges")?,
            })
        })?,
        triangles: items(v, "triangles", |t| triangle_section(t, count))?,
        triangle_fans: items(v, "triangleFans", |f| {
            Ok(VirtualTriangleFan {
                particle: particle(f.field("realParticleIndex")?, count)?,
                vcp_start: word(f, "vcpStartIndex")?,
                triangles: super::byte(f, "numTriangles")?,
            })
        })?,
        edges_landscape: items(v, "edgesLandscape", |e| edge_section(e, count))?,
        edge_fans_landscape: items(v, "edgeFansLandscape", |f| {
            Ok(VirtualEdgeFanLandscape {
                particle: particle(f.field("realParticleIndex")?, count)?,
                edge_start: word(f, "edgeStartIndex")?,
                vcp_start: word(f, "vcpStartIndex")?,
                edges: super::byte(f, "numEdges")?,
            })
        })?,
        triangles_landscape: items(v, "trianglesLandscape", |t| triangle_section(t, count))?,
        triangle_fans_landscape: items(v, "triangleFansLandscape", |f| {
            Ok(VirtualTriangleFanLandscape {
                particle: particle(f.field("realParticleIndex")?, count)?,
                triangle_start: word(f, "triangleStartIndex")?,
                vcp_start: word(f, "vcpStartIndex")?,
                triangles: super::byte(f, "numTriangles")?,
            })
        })?,
        edge_fan_indices: words16(v, "edgeFanIndices")?,
        triangle_fan_indices: words16(v, "triangleFanIndices")?,
        edge_fan_indices_landscape: words16(v, "edgeFanIndicesLandscape")?,
        triangle_fan_indices_landscape: words16(v, "triangleFanIndicesLandscape")?,
    })
}

pub(super) fn sim_cloth(v: Value<'_, '_>) -> Result<SimCloth> {
    let particles = items(v, "particleDatas", |p| {
        Ok(Particle {
            mass: real(p, "mass")?,
            inv_mass: real(p, "invMass")?,
            radius: real(p, "radius")?,
            friction: real(p, "friction")?,
        })
    })?;
    let count = particles.len();
    let info = v.field("simulationInfo")?;
    let map = v.field("collidableTransformMap")?;
    let collidable_transform_map = match int(map, "transformSetIndex")? {
        -1 => None,
        set => Some(CollidableTransformMap {
            transform_set: u32::try_from(set)
                .map_err(|_| invalid(map.at(), format!("collidable transform set index {set}")))?,
            transform_indices: items(map, "transformIndices", super::small)?,
            offsets: items(map, "offsets", matrix)?,
        }),
    };
    let collidables = items(v, "perInstanceCollidables", |c| {
        collide::collidable(c.dereference()?)
    })?;
    if let Some(m) = &collidable_transform_map
        && (m.transform_indices.len() != collidables.len() || m.offsets.len() != collidables.len())
    {
        return Err(invalid(
            map.at(),
            format!(
                "{} collidables, {} transform indices, {} offsets",
                collidables.len(),
                m.transform_indices.len(),
                m.offsets.len()
            ),
        ));
    }
    let triangle_indices = items(v, "triangleIndices", |p| particle(p, count))?;
    if !triangle_indices.len().is_multiple_of(3) {
        return Err(invalid(
            v.at(),
            format!("{} triangle indices: not whole triangles", triangle_indices.len()),
        ));
    }
    Ok(SimCloth {
        name: text(v, "name")?,
        gravity: vec4(info, "gravity")?,
        global_damping_per_second: real(info, "globalDampingPerSecond")?,
        fixed_particles: items(v, "fixedParticles", |p| particle(p, count))?,
        do_normals: flag(v, "doNormals")?,
        sim_op_ids: items(v, "simOpIds", super::small)?,
        poses: items(v, "simClothPoses", |p| {
            let p = p.dereference()?;
            Ok(Pose {
                name: text(p, "name")?,
                positions: items(p, "positions", |x| x.vector::<4>())?,
            })
        })?,
        constraint_sets: items(v, "staticConstraintSets", |s| {
            constraints::constraint_set(s.dereference()?, count)
        })?,
        anti_pinch_constraint_sets: items(v, "antiPinchConstraintSets", |s| {
            constraints::constraint_set(s.dereference()?, count)
        })?,
        collidable_transform_map,
        collidables,
        max_particle_radius: real(v, "maxParticleRadius")?,
        static_collision_masks: items(v, "staticCollisionMasks", super::small)?,
        actions: super::classes(v, "actions")?,
        total_mass: real(v, "totalMass")?,
        transfer_motion: transfer_motion(v.field("transferMotionData")?)?,
        transfer_motion_enabled: flag(v, "transferMotionEnabled")?,
        landscape_collision_enabled: flag(v, "landscapeCollisionEnabled")?,
        landscape_collision: landscape(v.field("landscapeCollisionData")?)?,
        landscape_collidable_particles: uint(v, "numLandscapeCollidableParticles")?,
        triangle_indices,
        triangle_flips: items(v, "triangleFlips", |b| {
            u8::try_from(b.unsigned()?).map_err(|_| invalid(b.at(), "triangle flips: value outside u8"))
        })?,
        pinch_detection_enabled: flag(v, "pinchDetectionEnabled")?,
        per_particle_pinch_detection: items(v, "perParticlePinchDetectionEnabledFlags", |b| {
            Ok(b.unsigned()? != 0)
        })?,
        collidable_pinching: items(v, "collidablePinchingDatas", |c| {
            Ok(CollidablePinching {
                enabled: flag(c, "pinchDetectionEnabled")?,
                priority: signed8(c, "pinchDetectionPriority")?,
                radius: real(c, "pinchDetectionRadius")?,
            })
        })?,
        min_pinched_particle: word(v, "minPinchedParticleIndex")?,
        max_pinched_particle: word(v, "maxPinchedParticleIndex")?,
        max_collision_pairs: uint(v, "maxCollisionPairs")?,
        virtual_collision_points: virtual_collision_points(v.field("virtualCollisionPointsData")?, count)?,
        particles,
    })
}
