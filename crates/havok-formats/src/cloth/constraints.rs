//! The constraint sets of a sim cloth (`hclConstraintSet` and the classes derived from it).

use super::{fixed, flag, index16, items, particle, real, text, uint, vec4};
use crate::{Result, Value, invalid};

/// One link of a standard or stretch link set (`hclStandardLinkConstraintSet::Link`,
/// `hclStretchLinkConstraintSet::Link`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Link {
    /// `particleA`: a particle index.
    pub particle_a: u16,
    /// `particleB`: a particle index.
    pub particle_b: u16,
    /// `restLength`.
    pub rest_length: f32,
    /// `stiffness`.
    pub stiffness: f32,
}

/// `hclCompressibleLinkConstraintSet::Link`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CompressibleLink {
    /// `particleA`: a particle index.
    pub particle_a: u16,
    /// `particleB`: a particle index.
    pub particle_b: u16,
    /// `restLength`.
    pub rest_length: f32,
    /// `compressionLength`.
    pub compression_length: f32,
    /// `stiffness`.
    pub stiffness: f32,
}

/// `hclBendLinkConstraintSet::Link`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BendLink {
    /// `particleA`: a particle index.
    pub particle_a: u16,
    /// `particleB`: a particle index.
    pub particle_b: u16,
    /// `bendMinLength`.
    pub bend_min_length: f32,
    /// `stretchMaxLength`.
    pub stretch_max_length: f32,
    /// `bendStiffness`.
    pub bend_stiffness: f32,
    /// `stretchStiffness`.
    pub stretch_stiffness: f32,
}

/// `hclBendStiffnessConstraintSet::Link`: two triangles sharing the edge from `particle_c` to `particle_d`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BendStiffnessLink {
    /// `weightA`, `weightB`, `weightC`, `weightD`.
    pub weights: [f32; 4],
    /// `bendStiffness`.
    pub bend_stiffness: f32,
    /// `restCurvature`.
    pub rest_curvature: f32,
    /// `particleA`, `particleB`, `particleC`, `particleD`: particle indices.
    pub particles: [u16; 4],
}

/// One entry of a local range constraint set (`hclLocalRangeConstraintSet::LocalConstraint`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocalRange {
    /// `particleIndex`: a particle index.
    pub particle: u16,
    /// `referenceVertex`: the vertex index the particle is limited relative to (16-bit; a vertex of the reference mesh
    /// buffer, whose size this reader checks against the buffer's definition).
    pub reference_vertex: u16,
    /// `maximumDistance`.
    pub maximum_distance: f32,
    /// `maxNormalDistance`.
    pub max_normal_distance: f32,
    /// `minNormalDistance`.
    pub min_normal_distance: f32,
}

/// The shape a local range constraint limits a particle to (`hclLocalRangeConstraintSet`'s `shapeType`, a 32-bit enum).
/// The values are the files': a set stores 0 (656 of 656 in Sekiro's cloth, 4,527 of 4,644 in Elden Ring's) or 1 (the
/// other 117); any other value is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalRangeShape {
    /// 0: a sphere around the reference vertex.
    Sphere,
    /// 1: a cylinder along the reference vertex's normal.
    Cylinder,
}

/// `hclTransitionConstraintSet::PerParticle`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransitionParticle {
    /// `particleIndex`: a particle index.
    pub particle: u16,
    /// `referenceVertex`: a vertex of the reference mesh buffer.
    pub reference_vertex: u16,
    /// `toAnimDelay`.
    pub to_anim_delay: f32,
    /// `toSimDelay`.
    pub to_sim_delay: f32,
    /// `toSimMaxDistance`.
    pub to_sim_max_distance: f32,
}

/// `hclVolumeConstraintMx::FrameBatchData`: sixteen particles that frame a volume constraint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VolumeFrameBatch {
    /// `frameVector`.
    pub frame_vectors: [[f32; 4]; 16],
    /// `particleIndex`: particle indices.
    pub particles: [u16; 16],
    /// `weight`.
    pub weights: [f32; 16],
}

/// `hclVolumeConstraintMx::FrameSingleData`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VolumeFrameSingle {
    /// `frameVector`.
    pub frame_vector: [f32; 4],
    /// `particleIndex`: a particle index.
    pub particle: u16,
    /// `weight`.
    pub weight: f32,
}

/// `hclVolumeConstraintMx::ApplyBatchData`: sixteen particles a volume constraint moves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VolumeApplyBatch {
    /// `frameVector`.
    pub frame_vectors: [[f32; 4]; 16],
    /// `particleIndex`: particle indices.
    pub particles: [u16; 16],
    /// `stiffness`.
    pub stiffnesses: [f32; 16],
}

/// `hclVolumeConstraintMx::ApplySingleData`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VolumeApplySingle {
    /// `frameVector`.
    pub frame_vector: [f32; 4],
    /// `particleIndex`: a particle index.
    pub particle: u16,
    /// `stiffness`.
    pub stiffness: f32,
}

/// What a constraint set holds, by its class.
#[derive(Clone, Debug, PartialEq)]
pub enum ConstraintKind {
    /// `hclStandardLinkConstraintSet`.
    StandardLink(Vec<Link>),
    /// `hclStretchLinkConstraintSet`.
    StretchLink(Vec<Link>),
    /// `hclCompressibleLinkConstraintSet`.
    CompressibleLink(Vec<CompressibleLink>),
    /// `hclBendLinkConstraintSet`.
    BendLink(Vec<BendLink>),
    /// `hclBendStiffnessConstraintSet`.
    BendStiffness {
        /// `links`.
        links: Vec<BendStiffnessLink>,
        /// `maxRestPoseHeightSq`.
        max_rest_pose_height_sq: f32,
        /// `clampBendStiffness`.
        clamp_bend_stiffness: bool,
        /// `useRestPoseConfig`.
        use_rest_pose_config: bool,
    },
    /// `hclLocalRangeConstraintSet`.
    LocalRange {
        /// `referenceMeshBufferIdx`: which mesh buffer holds the reference vertices.
        reference_mesh_buffer: u32,
        /// `stiffness`.
        stiffness: f32,
        /// `shapeType`.
        shape_type: LocalRangeShape,
        /// `applyNormalComponent`.
        apply_normal_component: bool,
        /// `localConstraints`.
        constraints: Vec<LocalRange>,
    },
    /// `hclTransitionConstraintSet`.
    Transition {
        /// `perParticleData`.
        particles: Vec<TransitionParticle>,
        /// `toAnimPeriod`.
        to_anim_period: f32,
        /// `toAnimPlusDelayPeriod`.
        to_anim_plus_delay_period: f32,
        /// `toSimPeriod`.
        to_sim_period: f32,
        /// `toSimPlusDelayPeriod`.
        to_sim_plus_delay_period: f32,
        /// `referenceMeshBufferIdx`.
        reference_mesh_buffer: u32,
    },
    /// `hclVolumeConstraintMx`.
    VolumeMx {
        /// `frameBatchDatas`.
        frame_batches: Vec<VolumeFrameBatch>,
        /// `frameSingleDatas`.
        frame_singles: Vec<VolumeFrameSingle>,
        /// `applyBatchDatas`.
        apply_batches: Vec<VolumeApplyBatch>,
        /// `applySingleDatas`.
        apply_singles: Vec<VolumeApplySingle>,
    },
    /// A class this reader has no layout for, kept by its name.
    Other {
        /// The set's class name.
        class: String,
    },
}

/// A constraint set of a sim cloth.
#[derive(Clone, Debug, PartialEq)]
pub struct ConstraintSet {
    /// `name`.
    pub name: String,
    /// `constraintId` (an `hkHandle`): stored by Elden Ring's `hclConstraintSet` (version 1), not by Sekiro's.
    pub constraint_id: Option<u32>,
    /// What the set holds.
    pub kind: ConstraintKind,
}

fn link(v: Value<'_, '_>, count: usize) -> Result<Link> {
    Ok(Link {
        particle_a: particle(v.field("particleA")?, count)?,
        particle_b: particle(v.field("particleB")?, count)?,
        rest_length: real(v, "restLength")?,
        stiffness: real(v, "stiffness")?,
    })
}

fn local_range_shape(v: Value<'_, '_>) -> Result<LocalRangeShape> {
    match v.unsigned()? {
        0 => Ok(LocalRangeShape::Sphere),
        1 => Ok(LocalRangeShape::Cylinder),
        other => Err(invalid(
            v.at(),
            format!("local range shape type {other}, not 0 or 1"),
        )),
    }
}

pub(super) fn constraint_set(v: Value<'_, '_>, count: usize) -> Result<ConstraintSet> {
    let class = v.type_name().to_owned();
    let kind = match class.as_str() {
        "hclStandardLinkConstraintSet" => {
            ConstraintKind::StandardLink(items(v, "links", |l| link(l, count))?)
        }
        "hclStretchLinkConstraintSet" => ConstraintKind::StretchLink(items(v, "links", |l| link(l, count))?),
        "hclCompressibleLinkConstraintSet" => ConstraintKind::CompressibleLink(items(v, "links", |l| {
            Ok(CompressibleLink {
                particle_a: particle(l.field("particleA")?, count)?,
                particle_b: particle(l.field("particleB")?, count)?,
                rest_length: real(l, "restLength")?,
                compression_length: real(l, "compressionLength")?,
                stiffness: real(l, "stiffness")?,
            })
        })?),
        "hclBendLinkConstraintSet" => ConstraintKind::BendLink(items(v, "links", |l| {
            Ok(BendLink {
                particle_a: particle(l.field("particleA")?, count)?,
                particle_b: particle(l.field("particleB")?, count)?,
                bend_min_length: real(l, "bendMinLength")?,
                stretch_max_length: real(l, "stretchMaxLength")?,
                bend_stiffness: real(l, "bendStiffness")?,
                stretch_stiffness: real(l, "stretchStiffness")?,
            })
        })?),
        "hclBendStiffnessConstraintSet" => ConstraintKind::BendStiffness {
            links: items(v, "links", |l| {
                Ok(BendStiffnessLink {
                    weights: [
                        real(l, "weightA")?,
                        real(l, "weightB")?,
                        real(l, "weightC")?,
                        real(l, "weightD")?,
                    ],
                    bend_stiffness: real(l, "bendStiffness")?,
                    rest_curvature: real(l, "restCurvature")?,
                    particles: [
                        particle(l.field("particleA")?, count)?,
                        particle(l.field("particleB")?, count)?,
                        particle(l.field("particleC")?, count)?,
                        particle(l.field("particleD")?, count)?,
                    ],
                })
            })?,
            max_rest_pose_height_sq: real(v, "maxRestPoseHeightSq")?,
            clamp_bend_stiffness: flag(v, "clampBendStiffness")?,
            use_rest_pose_config: flag(v, "useRestPoseConfig")?,
        },
        "hclLocalRangeConstraintSet" => ConstraintKind::LocalRange {
            reference_mesh_buffer: uint(v, "referenceMeshBufferIdx")?,
            stiffness: real(v, "stiffness")?,
            shape_type: local_range_shape(v.field("shapeType")?)?,
            apply_normal_component: flag(v, "applyNormalComponent")?,
            constraints: items(v, "localConstraints", |c| {
                Ok(LocalRange {
                    particle: particle(c.field("particleIndex")?, count)?,
                    reference_vertex: index16(c.field("referenceVertex")?)?,
                    maximum_distance: real(c, "maximumDistance")?,
                    max_normal_distance: real(c, "maxNormalDistance")?,
                    min_normal_distance: real(c, "minNormalDistance")?,
                })
            })?,
        },
        "hclTransitionConstraintSet" => ConstraintKind::Transition {
            particles: items(v, "perParticleData", |p| {
                Ok(TransitionParticle {
                    particle: particle(p.field("particleIndex")?, count)?,
                    reference_vertex: index16(p.field("referenceVertex")?)?,
                    to_anim_delay: real(p, "toAnimDelay")?,
                    to_sim_delay: real(p, "toSimDelay")?,
                    to_sim_max_distance: real(p, "toSimMaxDistance")?,
                })
            })?,
            to_anim_period: real(v, "toAnimPeriod")?,
            to_anim_plus_delay_period: real(v, "toAnimPlusDelayPeriod")?,
            to_sim_period: real(v, "toSimPeriod")?,
            to_sim_plus_delay_period: real(v, "toSimPlusDelayPeriod")?,
            reference_mesh_buffer: uint(v, "referenceMeshBufferIdx")?,
        },
        "hclVolumeConstraintMx" => ConstraintKind::VolumeMx {
            frame_batches: items(v, "frameBatchDatas", |b| {
                Ok(VolumeFrameBatch {
                    frame_vectors: fixed(b, "frameVector", |x| x.vector::<4>())?,
                    particles: fixed(b, "particleIndex", |p| particle(p, count))?,
                    weights: fixed(b, "weight", |x| x.float())?,
                })
            })?,
            frame_singles: items(v, "frameSingleDatas", |s| {
                Ok(VolumeFrameSingle {
                    frame_vector: vec4(s, "frameVector")?,
                    particle: particle(s.field("particleIndex")?, count)?,
                    weight: real(s, "weight")?,
                })
            })?,
            apply_batches: items(v, "applyBatchDatas", |b| {
                Ok(VolumeApplyBatch {
                    frame_vectors: fixed(b, "frameVector", |x| x.vector::<4>())?,
                    particles: fixed(b, "particleIndex", |p| particle(p, count))?,
                    stiffnesses: fixed(b, "stiffness", |x| x.float())?,
                })
            })?,
            apply_singles: items(v, "applySingleDatas", |s| {
                Ok(VolumeApplySingle {
                    frame_vector: vec4(s, "frameVector")?,
                    particle: particle(s.field("particleIndex")?, count)?,
                    stiffness: real(s, "stiffness")?,
                })
            })?,
        },
        _ => ConstraintKind::Other { class: class.clone() },
    };
    Ok(ConstraintSet {
        name: text(v, "name")?,
        constraint_id: if v.has_field("constraintId") {
            Some(uint(v, "constraintId")?)
        } else {
            None
        },
        kind,
    })
}
