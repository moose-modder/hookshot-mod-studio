//! Havok cloth and ragdoll data of a tagfile, as typed values. A character's or part's `hclClothData` is read whole:
//! its simulation cloths (particles, fixed particles, rest poses, constraint sets, collidables and their shapes,
//! virtual collision points), its buffer and transform set definitions, its operators (simulate, move particles,
//! skinning, mesh-to-mesh and mesh-to-bone deformation, vertex copies and gathers, conversions, vertex frames) and its
//! states. The ragdoll (`hknpRagdollData`: the skeleton, the map from bones to bodies and the body tags) and the
//! constraints that join bodies (`hknpConstraintCinfo`) are read beside it.
//!
//! This is reading: every number is the file's, read by the names of the class metadata the file carries; a class or
//! field the file does not have is an error naming it, never a default. Objects are found by their class among the
//! file's items, so the order of the object graph does not matter. What the simulation does with these values is not
//! here. A constraint set, operator or shape of a class this reader has no layout for is kept by its class name.
//!
//! The particle indices of a sim cloth (its fixed particles and triangles, the particles of its constraints, a move
//! particles operator's particles, the particles of its virtual collision points) are 16-bit in the Sekiro and Elden
//! Ring files, and each is checked against the sim cloth's particle count; a file that stores them in another width,
//! or one that names a particle it does not have, is refused. Indices into buffers, transform sets, operators and sim
//! cloths are checked against the cloth data's lists.

use crate::animation::{Skeleton, skeleton};
use crate::enums::hcl::Platform;
use crate::{Result, Tagfile, Value, invalid};

mod buffers;
mod collide;
mod constraints;
mod operators;
mod sim;

pub use buffers::{
    BitField, BlendTransition, BufferAccess, BufferDefinition, BufferElement, BufferKind, BufferLayout,
    BufferSlot, BufferUsage, ClothState, DependencyBranch, DependencyGraph, SimClothTransition,
    StateTransition, StateTransitionData, TransformSetAccess, TransformSetDefinition, TransformSetUsage,
    TransformTracker,
};
pub use collide::{Collidable, Shape};
pub use constraints::{
    BendLink, BendStiffnessLink, CompressibleLink, ConstraintKind, ConstraintSet, Link, LocalRange,
    LocalRangeShape, TransitionParticle, VolumeApplyBatch, VolumeApplySingle, VolumeFrameBatch,
    VolumeFrameSingle,
};
pub use operators::{
    BoneSpaceBlock, BoneSpaceDeformer, BoneSpaceLocalBlock, Components, ConversionInfo, ElementConversion,
    LocalBlock, MeshBoneTriangle, ObjectSpaceBlock, ObjectSpaceDeformer, ObjectSpaceLocalBlock, Operator,
    OperatorKind, SimulateConfig, SlotConversion, UnpackedLocalBlock,
};
pub use sim::{
    CollidablePinching, CollidableTransformMap, LandscapeCollision, Particle, Pose, SimCloth, TransferMotion,
    VirtualCollisionPoints, VirtualCollisionPointsBlock, VirtualDictionaryEntry, VirtualEdgeFan,
    VirtualEdgeFanLandscape, VirtualEdgeSection, VirtualTriangleFan, VirtualTriangleFanLandscape,
    VirtualTriangleSection,
};

/// `hclClothData`: one cloth of a character or part, with everything its runtime instances share.
#[derive(Clone, Debug, PartialEq)]
pub struct Cloth {
    /// `name`.
    pub name: String,
    /// `targetPlatform` (`hkEnum<Platform, hkUint32>`), as stored: 2 in all 610 `hclClothData` of Sekiro's and all 2,692 of
    /// Elden Ring's distinct tagfiles. The files do not say what the value means; HKLib's `Platform` names 2
    /// `HCL_PLATFORM_X64` (see [`Cloth::platform`]).
    pub target_platform: u32,
    /// `simClothDatas`.
    pub sim_cloths: Vec<SimCloth>,
    /// `bufferDefinitions`: the vertex buffers the operators read and write, by index.
    pub buffers: Vec<BufferDefinition>,
    /// `transformSetDefinitions`: the sets of bone transforms, by index.
    pub transform_sets: Vec<TransformSetDefinition>,
    /// `operators`, by index (a state lists the operators it runs by these indices).
    pub operators: Vec<Operator>,
    /// `clothStateDatas`.
    pub states: Vec<ClothState>,
    /// `stateTransitions`.
    pub state_transitions: Vec<StateTransition>,
    /// `actions`: the class of each action object.
    pub actions: Vec<String>,
}

impl Cloth {
    /// The target platform, named; `None` for a stored number the enumeration does not name.
    pub fn platform(&self) -> Option<Platform> {
        Platform::from_value(i64::from(self.target_platform))
    }
}

/// `hknpConstraintCinfo`: a constraint joining two bodies. Its atoms are not read: `data_class` names the constraint's
/// kind (`hkpRagdollConstraintData`, `hkpLimitedHingeConstraintData` ...).
#[derive(Clone, Debug, PartialEq)]
pub struct Constraint {
    /// `name`.
    pub name: String,
    /// The class of the object `constraintData` points to, which says what kind of constraint it is.
    pub data_class: String,
    /// `bodyA`: a body id (`hknpBodyId`'s `serialAndIndex` when the file stores a record).
    pub body_a: u32,
    /// `bodyB`, as `body_a`.
    pub body_b: u32,
    /// `flags`, as stored.
    pub flags: u32,
    /// `desiredConstraintId`.
    pub desired_constraint_id: u32,
    /// `constraintGroupId`.
    pub constraint_group_id: u32,
}

/// `hknpRagdollData`.
#[derive(Clone, Debug, PartialEq)]
pub struct Ragdoll {
    /// `skeleton`.
    pub skeleton: Skeleton,
    /// One a bone: the body that moves it, as stored (negative: none).
    pub bone_to_body: Vec<i32>,
    /// `bodyTags`, as stored.
    pub body_tags: Vec<u32>,
}

/// What a file holds of cloth and ragdoll.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Physics {
    /// Every `hclClothData` of the file, in item order.
    pub cloths: Vec<Cloth>,
    /// Every `hknpRagdollData`, in item order.
    pub ragdolls: Vec<Ragdoll>,
    /// Every `hknpConstraintCinfo`, in item order.
    pub constraints: Vec<Constraint>,
}

/// The elements of array `v`, each read by `each`.
fn list<'t, 'a, T>(v: Value<'t, 'a>, each: impl Fn(Value<'t, 'a>) -> Result<T>) -> Result<Vec<T>> {
    let a = v.array()?;
    (0..a.len()).map(|i| each(a.get(i)?)).collect()
}

/// The elements of array member `name` of `v`.
fn items<'t, 'a, T>(
    v: Value<'t, 'a>,
    name: &str,
    each: impl Fn(Value<'t, 'a>) -> Result<T>,
) -> Result<Vec<T>> {
    list(v.field(name)?, each)
}

/// The `N` elements of fixed-size array member `name` of `v`; another count is refused.
fn fixed<'t, 'a, T, const N: usize>(
    v: Value<'t, 'a>,
    name: &str,
    each: impl Fn(Value<'t, 'a>) -> Result<T>,
) -> Result<[T; N]> {
    let f = v.field(name)?;
    let values = list(f, each)?;
    let count = values.len();
    values
        .try_into()
        .map_err(|_| invalid(f.at(), format!("{name}: expected {N} elements, found {count}")))
}

fn small(v: Value<'_, '_>) -> Result<u32> {
    // Havok 2018 stores these ids as reflected records; 2016 may expose the
    // integer directly. Read only the established fields, never a record's first bytes.
    let v = match (v.format()? & 15, v.type_name()) {
        (7, "hknpBodyId") => v.field("serialAndIndex")?,
        (7, "hknpConstraintId" | "hknpConstraintGroupId" | "hkHandle") => v.field("value")?,
        _ => v,
    };
    u32::try_from(v.unsigned()?).map_err(|_| invalid(v.at(), "value outside u32"))
}

fn small_signed(v: Value<'_, '_>) -> Result<i32> {
    i32::try_from(v.signed()?).map_err(|_| invalid(v.at(), "value outside i32"))
}

/// Member `name` of `v` as a float.
fn real(v: Value<'_, '_>, name: &str) -> Result<f32> {
    v.field(name)?.float()
}

/// Member `name` of `v`, an unsigned integer (or id record) of at most 32 bits.
fn uint(v: Value<'_, '_>, name: &str) -> Result<u32> {
    small(v.field(name)?)
}

/// Member `name` of `v`, a signed integer of at most 32 bits.
fn int(v: Value<'_, '_>, name: &str) -> Result<i32> {
    small_signed(v.field(name)?)
}

/// Member `name` of `v`, an 8-bit integer.
fn byte(v: Value<'_, '_>, name: &str) -> Result<u8> {
    let f = v.field(name)?;
    u8::try_from(f.unsigned()?).map_err(|_| invalid(f.at(), format!("{name}: value outside u8")))
}

/// Member `name` of `v`, a boolean: any stored byte but 0 is true.
fn flag(v: Value<'_, '_>, name: &str) -> Result<bool> {
    Ok(v.field(name)?.unsigned()? != 0)
}

/// Member `name` of `v`, a string.
fn text(v: Value<'_, '_>, name: &str) -> Result<String> {
    v.field(name)?.string()
}

/// Member `name` of `v`, an `hkVector4` (`x y z w` as stored).
fn vec4(v: Value<'_, '_>, name: &str) -> Result<[f32; 4]> {
    v.field(name)?.vector::<4>()
}

/// An `hkMatrix4` or `hkTransform` value: its 16 floats as stored (four columns of four).
fn matrix(v: Value<'_, '_>) -> Result<[f32; 16]> {
    v.vector::<16>()
}

/// A 16-bit index stored as `hkUint16`; another width is refused.
fn index16(v: Value<'_, '_>) -> Result<u16> {
    let value = v.unsigned()?;
    let bits = v.format()? >> 10;
    if bits != 16 {
        return Err(invalid(
            v.at(),
            format!("expected a 16-bit index, found a {bits}-bit {}", v.type_name()),
        ));
    }
    u16::try_from(value).map_err(|_| invalid(v.at(), "value outside u16"))
}

/// Member `name` of `v`, an array of 16-bit indices or counts.
fn words16(v: Value<'_, '_>, name: &str) -> Result<Vec<u16>> {
    items(v, name, index16)
}

/// A 16-bit particle index that names one of the sim cloth's `count` particles.
fn particle(v: Value<'_, '_>, count: usize) -> Result<u16> {
    let index = index16(v)?;
    if usize::from(index) >= count {
        return Err(invalid(
            v.at(),
            format!("particle index {index} of a sim cloth with {count} particles"),
        ));
    }
    Ok(index)
}

/// Refuses index `index` of `what` (stored in `owner`) when it is not below `count`.
fn bounded(owner: Value<'_, '_>, what: &str, index: u32, count: usize) -> Result<()> {
    if usize::try_from(index).is_ok_and(|i| i < count) {
        Ok(())
    } else {
        Err(invalid(owner.at(), format!("{what} {index} of {count}")))
    }
}

/// Every object of class `class` among the file's items, in item order.
fn objects<'t, 'a>(tag: &'t Tagfile<'a>, class: &str) -> Result<Vec<Value<'t, 'a>>> {
    let mut out = Vec::new();
    for (index, item) in tag.items.iter().enumerate().skip(1) {
        if tag.types.get(item.type_index).is_some_and(|t| t.name == class) {
            let array = tag.item(index)?;
            for i in 0..array.len() {
                out.push(array.get(i)?);
            }
        }
    }
    Ok(out)
}

/// The class names of a pointer array's objects.
fn classes(v: Value<'_, '_>, name: &str) -> Result<Vec<String>> {
    items(v, name, |a| Ok(a.dereference()?.type_name().to_owned()))
}

fn cloth(data: Value<'_, '_>) -> Result<Cloth> {
    let sim_cloths = items(data, "simClothDatas", |s| sim::sim_cloth(s.dereference()?))?;
    let buffers = items(data, "bufferDefinitions", |b| {
        buffers::buffer_definition(b.dereference()?)
    })?;
    let transform_sets = items(data, "transformSetDefinitions", |t| {
        buffers::transform_set_definition(t.dereference()?)
    })?;
    let operators = items(data, "operators", |o| operators::operator(o.dereference()?))?;
    let states = items(data, "clothStateDatas", |s| {
        buffers::cloth_state(s.dereference()?)
    })?;
    let state_transitions = items(data, "stateTransitions", |s| {
        buffers::state_transition(s.dereference()?)
    })?;
    let cloth = Cloth {
        name: text(data, "name")?,
        target_platform: u32::try_from(data.field("targetPlatform")?.unsigned()?)
            .map_err(|_| invalid(data.at(), "targetPlatform outside u32"))?,
        sim_cloths,
        buffers,
        transform_sets,
        operators,
        states,
        state_transitions,
        actions: classes(data, "actions")?,
    };
    check_references(data, &cloth)?;
    Ok(cloth)
}

/// The indices one part of the cloth data stores into another are checked against the lists they index.
fn check_references(at: Value<'_, '_>, cloth: &Cloth) -> Result<()> {
    let buffers = cloth.buffers.len();
    let sets = cloth.transform_sets.len();
    let sims = cloth.sim_cloths.len();
    let operators = cloth.operators.len();
    for sim in &cloth.sim_cloths {
        for &op in &sim.sim_op_ids {
            bounded(at, "sim cloth operator", op, operators)?;
        }
        if let Some(map) = &sim.collidable_transform_map {
            bounded(at, "collidable transform set", map.transform_set, sets)?;
        }
    }
    for state in &cloth.states {
        for &op in &state.operators {
            bounded(at, "state operator", op, operators)?;
        }
        for &s in &state.used_sim_cloths {
            bounded(at, "state sim cloth", s, sims)?;
        }
        for access in &state.used_buffers {
            bounded(at, "state buffer", access.buffer, buffers)?;
            bounded(at, "state shadow buffer", access.shadow_buffer, buffers)?;
        }
        for access in &state.used_transform_sets {
            bounded(at, "state transform set", access.transform_set, sets)?;
        }
    }
    for op in &cloth.operators {
        for access in &op.used_buffers {
            bounded(at, "operator buffer", access.buffer, buffers)?;
            bounded(at, "operator shadow buffer", access.shadow_buffer, buffers)?;
        }
        for access in &op.used_transform_sets {
            bounded(at, "operator transform set", access.transform_set, sets)?;
        }
        operators::check_operator(at, op, cloth)?;
    }
    operators::check_reference_vertices(at, cloth)
}

/// The cloth, ragdoll and constraint objects of `tag`.
pub fn read(tag: &Tagfile<'_>) -> Result<Physics> {
    let mut out = Physics::default();
    for data in objects(tag, "hclClothData")? {
        out.cloths.push(cloth(data)?);
    }
    for data in objects(tag, "hknpRagdollData")? {
        let skeleton = skeleton(data.field("skeleton")?.dereference()?)?;
        let bone_to_body = items(data, "boneToBodyMap", small_signed)?;
        if bone_to_body.len() != skeleton.bones.len() {
            return Err(invalid(
                data.at(),
                format!(
                    "{} bones, {} body map entries",
                    skeleton.bones.len(),
                    bone_to_body.len()
                ),
            ));
        }
        out.ragdolls.push(Ragdoll {
            skeleton,
            bone_to_body,
            body_tags: items(data, "bodyTags", small)?,
        });
    }
    for c in objects(tag, "hknpConstraintCinfo")? {
        out.constraints.push(Constraint {
            name: text(c, "name")?,
            data_class: c.field("constraintData")?.dereference()?.type_name().to_owned(),
            body_a: uint(c, "bodyA")?,
            body_b: uint(c, "bodyB")?,
            flags: uint(c, "flags")?,
            desired_constraint_id: uint(c, "desiredConstraintId")?,
            constraint_group_id: uint(c, "constraintGroupId")?,
        });
    }
    Ok(out)
}
