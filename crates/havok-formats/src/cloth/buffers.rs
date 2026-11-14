//! What a cloth data's operators read and write: its buffer and transform set definitions, the usage records of
//! operators and states, the states themselves and their transitions.

use super::{byte, fixed, flag, int, invalid, items, list, small, small_signed, text, uint, words16};
use crate::{Result, Value};

/// `hclBufferLayout::BufferElement`: where one component (position, normal, tangent, bitangent) sits in a vertex.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferElement {
    /// `vectorConversion` (`hclRuntimeConversionInfo::VectorConversion`), as stored.
    pub vector_conversion: u8,
    /// `vectorSize`.
    pub vector_size: u8,
    /// `slotId`.
    pub slot: u8,
    /// `slotStart`.
    pub slot_start: u8,
}

/// `hclBufferLayout::Slot`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferSlot {
    /// `flags` (`hclBufferLayout::SlotFlags`), as stored.
    pub flags: u8,
    /// `stride`.
    pub stride: u8,
}

/// `hclBufferLayout`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferLayout {
    /// `elementsLayout`: position, normal, tangent, bitangent.
    pub elements: [BufferElement; 4],
    /// `slots`.
    pub slots: [BufferSlot; 4],
    /// `numSlots`.
    pub num_slots: u8,
    /// `triangleFormat` (`hclBufferLayout::TriangleFormat`), as stored.
    pub triangle_format: u8,
}

/// The class of a buffer definition and the members that class adds.
#[derive(Clone, Debug, PartialEq)]
pub enum BufferKind {
    /// `hclBufferDefinition` itself.
    Plain,
    /// `hclScratchBufferDefinition`.
    Scratch {
        /// `triangleIndices`.
        triangle_indices: Vec<u16>,
        /// `storeNormals`.
        store_normals: bool,
        /// `storeTangentsAndBiTangents`.
        store_tangents_and_bitangents: bool,
    },
    /// `hclShadowBufferDefinition`.
    Shadow {
        /// `triangleIndices`.
        triangle_indices: Vec<u16>,
        /// `shadowPositions`.
        positions: bool,
        /// `shadowNormals`.
        normals: bool,
        /// `shadowTangents`.
        tangents: bool,
        /// `shadowBiTangents`.
        bitangents: bool,
    },
    /// A class this reader has no layout for, kept by its name.
    Other {
        /// The class name.
        class: String,
    },
}

/// `hclBufferDefinition` and the classes derived from it.
#[derive(Clone, Debug, PartialEq)]
pub struct BufferDefinition {
    /// `name`.
    pub name: String,
    /// `type`, as stored.
    pub buffer_type: i32,
    /// `subType`, as stored.
    pub sub_type: i32,
    /// `numVertices`.
    pub vertices: u32,
    /// `numTriangles`.
    pub triangles: u32,
    /// `bufferLayout`.
    pub layout: BufferLayout,
    /// The class and what it adds.
    pub kind: BufferKind,
}

/// `hclTransformSetDefinition`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransformSetDefinition {
    /// `name`.
    pub name: String,
    /// `type`, as stored.
    pub set_type: i32,
    /// `numTransforms`.
    pub transforms: u32,
}

/// `hclBufferUsage`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferUsage {
    /// `perComponentFlags`: position, normal, tangent, bitangent; bits as stored.
    pub per_component_flags: [u8; 4],
    /// `trianglesRead`.
    pub triangles_read: bool,
}

/// `hclClothState::BufferAccess`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferAccess {
    /// `bufferIndex`.
    pub buffer: u32,
    /// `bufferUsage`.
    pub usage: BufferUsage,
    /// `shadowBufferIndex`.
    pub shadow_buffer: u32,
}

/// An `hkBitField`: `numBits` bits in 32-bit words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BitField {
    /// `storage.words`.
    pub words: Vec<u32>,
    /// `storage.numBits`.
    pub bits: i32,
}

/// `hclTransformSetUsage::TransformTracker`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransformTracker {
    /// `read`.
    pub read: BitField,
    /// `readBeforeWrite`.
    pub read_before_write: BitField,
    /// `written`.
    pub written: BitField,
}

/// `hclTransformSetUsage`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransformSetUsage {
    /// `perComponentFlags`: the transform and its inverse transpose; bits as stored.
    pub per_component_flags: [u8; 2],
    /// `perComponentTransformTrackers`.
    pub trackers: Vec<TransformTracker>,
}

/// `hclClothState::TransformSetAccess`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransformSetAccess {
    /// `transformSetIndex`.
    pub transform_set: u32,
    /// `transformSetUsage`.
    pub usage: TransformSetUsage,
}

/// `hclStateDependencyGraph::Branch`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DependencyBranch {
    /// `branchId`.
    pub id: i32,
    /// `stateOperatorIndices`.
    pub state_operators: Vec<i32>,
    /// `parentBranches`.
    pub parents: Vec<i32>,
    /// `childBranches`.
    pub children: Vec<i32>,
}

/// `hclStateDependencyGraph`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DependencyGraph {
    /// `branches`.
    pub branches: Vec<DependencyBranch>,
    /// `rootBranchIds`.
    pub root_branches: Vec<i32>,
    /// `children`.
    pub children: Vec<Vec<i32>>,
    /// `parents`.
    pub parents: Vec<Vec<i32>>,
    /// `multiThreadable`.
    pub multi_threadable: bool,
}

/// `hclClothState`.
#[derive(Clone, Debug, PartialEq)]
pub struct ClothState {
    /// `name`.
    pub name: String,
    /// `operators`: the operators the state runs, in order, as indices into the cloth data's operators.
    pub operators: Vec<u32>,
    /// `usedBuffers`.
    pub used_buffers: Vec<BufferAccess>,
    /// `usedTransformSets`.
    pub used_transform_sets: Vec<TransformSetAccess>,
    /// `usedSimCloths`.
    pub used_sim_cloths: Vec<u32>,
    /// `dependencyGraph`.
    pub dependency_graph: Option<DependencyGraph>,
}

/// `hclStateTransition::SimClothTransitionData`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SimClothTransition {
    /// `isSimulated`.
    pub simulated: bool,
    /// `transitionConstraints`.
    pub constraints: Vec<u32>,
    /// `transitionType`.
    pub transition_type: u32,
}

/// `hclStateTransition::BlendOpTransitionData`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlendTransition {
    /// `bufferASimCloths`.
    pub buffer_a_sim_cloths: Vec<i32>,
    /// `bufferBSimCloths`.
    pub buffer_b_sim_cloths: Vec<i32>,
    /// `transitionType`.
    pub transition_type: u32,
    /// `blendWeightType`.
    pub blend_weight_type: u32,
    /// `blendOperatorId`.
    pub blend_operator: u32,
}

/// `hclStateTransition::StateTransitionData`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateTransitionData {
    /// `simClothTransitionData`.
    pub sim_cloths: Vec<SimClothTransition>,
    /// `blendOpTransitionData`.
    pub blend_operators: Vec<BlendTransition>,
    /// `simulatedState`.
    pub simulated: bool,
    /// `emptyState`.
    pub empty: bool,
}

/// `hclStateTransition`. Neither install stores one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateTransition {
    /// `name`.
    pub name: String,
    /// `stateIds`.
    pub states: Vec<u32>,
    /// `stateTransitionData`.
    pub data: Vec<StateTransitionData>,
    /// `simClothTransitionConstraints`.
    pub sim_cloth_transition_constraints: Vec<Vec<u32>>,
}

fn layout(v: Value<'_, '_>) -> Result<BufferLayout> {
    Ok(BufferLayout {
        elements: fixed(v, "elementsLayout", |e| {
            Ok(BufferElement {
                vector_conversion: byte(e, "vectorConversion")?,
                vector_size: byte(e, "vectorSize")?,
                slot: byte(e, "slotId")?,
                slot_start: byte(e, "slotStart")?,
            })
        })?,
        slots: fixed(v, "slots", |s| {
            Ok(BufferSlot {
                flags: byte(s, "flags")?,
                stride: byte(s, "stride")?,
            })
        })?,
        num_slots: byte(v, "numSlots")?,
        triangle_format: byte(v, "triangleFormat")?,
    })
}

pub(super) fn buffer_definition(v: Value<'_, '_>) -> Result<BufferDefinition> {
    let kind = match v.type_name() {
        "hclBufferDefinition" => BufferKind::Plain,
        "hclScratchBufferDefinition" => BufferKind::Scratch {
            triangle_indices: words16(v, "triangleIndices")?,
            store_normals: flag(v, "storeNormals")?,
            store_tangents_and_bitangents: flag(v, "storeTangentsAndBiTangents")?,
        },
        "hclShadowBufferDefinition" => BufferKind::Shadow {
            triangle_indices: words16(v, "triangleIndices")?,
            positions: flag(v, "shadowPositions")?,
            normals: flag(v, "shadowNormals")?,
            tangents: flag(v, "shadowTangents")?,
            bitangents: flag(v, "shadowBiTangents")?,
        },
        other => BufferKind::Other {
            class: other.to_owned(),
        },
    };
    Ok(BufferDefinition {
        name: text(v, "name")?,
        buffer_type: int(v, "type")?,
        sub_type: int(v, "subType")?,
        vertices: uint(v, "numVertices")?,
        triangles: uint(v, "numTriangles")?,
        layout: layout(v.field("bufferLayout")?)?,
        kind,
    })
}

pub(super) fn transform_set_definition(v: Value<'_, '_>) -> Result<TransformSetDefinition> {
    Ok(TransformSetDefinition {
        name: text(v, "name")?,
        set_type: int(v, "type")?,
        transforms: uint(v, "numTransforms")?,
    })
}

pub(super) fn buffer_access(v: Value<'_, '_>) -> Result<BufferAccess> {
    let usage = v.field("bufferUsage")?;
    Ok(BufferAccess {
        buffer: uint(v, "bufferIndex")?,
        usage: BufferUsage {
            per_component_flags: fixed(usage, "perComponentFlags", |b| {
                u8::try_from(b.unsigned()?).map_err(|_| invalid(b.at(), "buffer usage flags outside u8"))
            })?,
            triangles_read: flag(usage, "trianglesRead")?,
        },
        shadow_buffer: uint(v, "shadowBufferIndex")?,
    })
}

fn bit_field(v: Value<'_, '_>) -> Result<BitField> {
    let storage = v.field("storage")?;
    Ok(BitField {
        words: items(storage, "words", small)?,
        bits: int(storage, "numBits")?,
    })
}

pub(super) fn transform_set_access(v: Value<'_, '_>) -> Result<TransformSetAccess> {
    let usage = v.field("transformSetUsage")?;
    Ok(TransformSetAccess {
        transform_set: uint(v, "transformSetIndex")?,
        usage: TransformSetUsage {
            per_component_flags: fixed(usage, "perComponentFlags", |b| {
                u8::try_from(b.unsigned()?)
                    .map_err(|_| invalid(b.at(), "transform set usage flags outside u8"))
            })?,
            trackers: items(usage, "perComponentTransformTrackers", |t| {
                Ok(TransformTracker {
                    read: bit_field(t.field("read")?)?,
                    read_before_write: bit_field(t.field("readBeforeWrite")?)?,
                    written: bit_field(t.field("written")?)?,
                })
            })?,
        },
    })
}

fn ints(v: Value<'_, '_>, name: &str) -> Result<Vec<i32>> {
    items(v, name, small_signed)
}

fn dependency_graph(v: Value<'_, '_>) -> Result<DependencyGraph> {
    Ok(DependencyGraph {
        branches: items(v, "branches", |b| {
            Ok(DependencyBranch {
                id: int(b, "branchId")?,
                state_operators: ints(b, "stateOperatorIndices")?,
                parents: ints(b, "parentBranches")?,
                children: ints(b, "childBranches")?,
            })
        })?,
        root_branches: ints(v, "rootBranchIds")?,
        children: items(v, "children", |c| list(c, small_signed))?,
        parents: items(v, "parents", |c| list(c, small_signed))?,
        multi_threadable: flag(v, "multiThreadable")?,
    })
}

pub(super) fn cloth_state(v: Value<'_, '_>) -> Result<ClothState> {
    Ok(ClothState {
        name: text(v, "name")?,
        operators: items(v, "operators", small)?,
        used_buffers: items(v, "usedBuffers", buffer_access)?,
        used_transform_sets: items(v, "usedTransformSets", transform_set_access)?,
        used_sim_cloths: items(v, "usedSimCloths", small)?,
        dependency_graph: v
            .field("dependencyGraph")?
            .pointer()?
            .map(dependency_graph)
            .transpose()?,
    })
}

/// An id stored as an `hkHandle` record (Elden Ring) or a plain `int` (Sekiro); a negative one is refused.
fn handle(v: Value<'_, '_>) -> Result<u32> {
    if v.format()? & 15 == 7 {
        return small(v);
    }
    let value = small_signed(v)?;
    u32::try_from(value).map_err(|_| invalid(v.at(), format!("negative id {value}")))
}

pub(super) fn state_transition(v: Value<'_, '_>) -> Result<StateTransition> {
    Ok(StateTransition {
        name: text(v, "name")?,
        states: items(v, "stateIds", small)?,
        data: items(v, "stateTransitionData", |d| {
            Ok(StateTransitionData {
                sim_cloths: items(d, "simClothTransitionData", |s| {
                    Ok(SimClothTransition {
                        simulated: flag(s, "isSimulated")?,
                        constraints: items(s, "transitionConstraints", handle)?,
                        transition_type: uint(s, "transitionType")?,
                    })
                })?,
                blend_operators: items(d, "blendOpTransitionData", |b| {
                    Ok(BlendTransition {
                        buffer_a_sim_cloths: ints(b, "bufferASimCloths")?,
                        buffer_b_sim_cloths: ints(b, "bufferBSimCloths")?,
                        transition_type: uint(b, "transitionType")?,
                        blend_weight_type: uint(b, "blendWeightType")?,
                        blend_operator: uint(b, "blendOperatorId")?,
                    })
                })?,
                simulated: flag(d, "simulatedState")?,
                empty: flag(d, "emptyState")?,
            })
        })?,
        sim_cloth_transition_constraints: items(v, "simClothTransitionConstraints", |c| list(c, handle))?,
    })
}
