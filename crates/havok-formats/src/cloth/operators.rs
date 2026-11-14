//! A cloth data's operators (`hclOperator` and the classes derived from it), with the deformers and local blocks the
//! skinning and mesh deformation operators store.

use super::buffers::{BufferAccess, TransformSetAccess, buffer_access, transform_set_access};
use super::{
    Cloth, ConstraintKind, bounded, byte, fixed, flag, index16, items, matrix, small_signed, text, uint,
    words16,
};
use crate::{Result, Value, invalid};

/// Which vertex components an operator writes: the class name's `P`, `PN` or `PNT` suffix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Components {
    /// Positions.
    P,
    /// Positions and normals.
    PN,
    /// Positions, normals and tangents.
    PNT,
}

/// One block of `hclObjectSpaceDeformer`'s blend entries: sixteen vertices, each blended from the same number of bones.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectSpaceBlock {
    /// `vertexIndices`.
    pub vertices: [u16; 16],
    /// `boneIndices`: the bones of each vertex in turn (bones-per-vertex times 16).
    pub bones: Vec<u16>,
    /// `boneWeights`, widened to 16 bits: 8-bit in the two-, three- and four-bone blocks, 16-bit in the five- to
    /// eight-bone blocks, absent (empty) in the one-bone block.
    pub weights: Vec<u16>,
}

/// `hclObjectSpaceDeformer`.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectSpaceDeformer {
    /// The blend entry blocks, by bones per vertex: index 0 holds `oneBlendEntries`, index 7 `eightBlendEntries`.
    pub blocks: [Vec<ObjectSpaceBlock>; 8],
    /// `controlBytes`.
    pub control_bytes: Vec<u8>,
    /// `startVertexIndex`.
    pub start_vertex: u16,
    /// `endVertexIndex`.
    pub end_vertex: u16,
    /// `partialWrite`.
    pub partial_write: bool,
}

/// One block of `hclBoneSpaceDeformer`'s blend entries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoneSpaceBlock {
    /// `vertexIndices`: 16, 8, 5 or 4 vertices for the one-, two-, three- and four-bone blocks.
    pub vertices: Vec<u16>,
    /// `boneIndices`: 16, 16, 15 or 16 bones.
    pub bones: Vec<u16>,
}

/// `hclBoneSpaceDeformer`.
#[derive(Clone, Debug, PartialEq)]
pub struct BoneSpaceDeformer {
    /// The blend entry blocks, by bones per vertex: index 0 holds `oneBlendEntries`, index 3 `fourBlendEntries`.
    pub blocks: [Vec<BoneSpaceBlock>; 4],
    /// `controlBytes`.
    pub control_bytes: Vec<u8>,
    /// `startVertexIndex`.
    pub start_vertex: u16,
    /// `endVertexIndex`.
    pub end_vertex: u16,
    /// `partialWrite`.
    pub partial_write: bool,
}

/// A local block of a deformer: sixteen vertices' positions, and their normals and tangents when the operator writes
/// them. `P` and `N` are the stored element types: an `hkPackedVector3` (`[i16; 4]`, its four stored words) or an
/// `hkVector4` (`[f32; 4]`).
#[derive(Clone, Debug, PartialEq)]
pub struct LocalBlock<P, N> {
    /// `localPosition`.
    pub positions: [P; 16],
    /// `localNormal` (PN and PNT).
    pub normals: Option<[N; 16]>,
    /// `localTangent` (PNT).
    pub tangents: Option<[N; 16]>,
}

/// An object space local block (`hclObjectSpaceDeformer::LocalBlockP/PN/PNT`): packed vectors throughout.
pub type ObjectSpaceLocalBlock = LocalBlock<[i16; 4], [i16; 4]>;
/// A bone space local block (`hclBoneSpaceDeformer::LocalBlockP/PN/PNT`): `hkVector4` positions, packed normals and
/// tangents.
pub type BoneSpaceLocalBlock = LocalBlock<[f32; 4], [i16; 4]>;
/// An unpacked local block of either deformer (`LocalBlockUnpackedP/PN/PNT`): `hkVector4` throughout.
pub type UnpackedLocalBlock = LocalBlock<[f32; 4], [f32; 4]>;

/// `hclRuntimeConversionInfo::SlotConversion`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotConversion {
    /// `elements`.
    pub elements: [u8; 4],
    /// `numElements`.
    pub num_elements: u8,
    /// `index`.
    pub index: u8,
    /// `partialWrite`.
    pub partial_write: bool,
}

/// `hclRuntimeConversionInfo::ElementConversion`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ElementConversion {
    /// `index`.
    pub index: u8,
    /// `offset`.
    pub offset: u8,
    /// `conversion` (`hclRuntimeConversionInfo::VectorConversion`), as stored.
    pub conversion: u8,
}

/// `hclRuntimeConversionInfo`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConversionInfo {
    /// `slotConversions`.
    pub slots: [SlotConversion; 4],
    /// `elementConversions`.
    pub elements: [ElementConversion; 4],
    /// `numSlotsConverted`.
    pub num_slots: u8,
    /// `numElementsConverted`.
    pub num_elements: u8,
}

/// `hclSimulateOperator::Config`.
#[derive(Clone, Debug, PartialEq)]
pub struct SimulateConfig {
    /// `name`.
    pub name: String,
    /// `constraintExecution`: constraint set indices in solve order, -1 for collision; empty: every set in order.
    pub constraint_execution: Vec<i32>,
    /// `instanceCollidablesUsed`.
    pub instance_collidables_used: Vec<bool>,
    /// `subSteps`.
    pub sub_steps: u8,
    /// `numberOfSolveIterations`.
    pub solve_iterations: u8,
    /// `useAllInstanceCollidables`.
    pub use_all_instance_collidables: bool,
    /// `adaptConstraintStiffness`.
    pub adapt_constraint_stiffness: bool,
}

/// `hclMeshBoneDeformOperator::TriangleBonePair`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeshBoneTriangle {
    /// `localBoneTransform` (an `hkMatrix4`, four columns of four).
    pub local_bone_transform: [f32; 16],
    /// `weight`.
    pub weight: f32,
    /// `triangleIndex`.
    pub triangle: u16,
}

/// What an operator does, by its class, with the members the class adds to `hclOperator`.
#[derive(Clone, Debug, PartialEq)]
pub enum OperatorKind {
    /// `hclSimulateOperator`.
    Simulate {
        /// `simClothIndex`.
        sim_cloth: u32,
        /// `simulateOpConfigs`.
        configs: Vec<SimulateConfig>,
    },
    /// `hclMoveParticlesOperator`: sets particles from vertices of a buffer.
    MoveParticles {
        /// `vertexParticlePairs`: `(vertexIndex, particleIndex)`.
        pairs: Vec<(u16, u16)>,
        /// `simClothIndex`.
        sim_cloth: u32,
        /// `refBufferIdx`.
        reference_buffer: u32,
    },
    /// `hclObjectSpaceSkinPOperator`, `...PNOperator`, `...PNTOperator`.
    ObjectSpaceSkin {
        /// The class's suffix.
        components: Components,
        /// `boneFromSkinMeshTransforms` (`hkMatrix4`, four columns of four).
        bone_from_skin_mesh: Vec<[f32; 16]>,
        /// `transformSubset`.
        transform_subset: Vec<u16>,
        /// `outputBufferIndex`.
        output_buffer: u32,
        /// `transformSetIndex`.
        transform_set: u32,
        /// `objectSpaceDeformer`.
        deformer: ObjectSpaceDeformer,
        /// `localPs`, `localPNs` or `localPNTs`.
        local: Vec<ObjectSpaceLocalBlock>,
        /// `localUnpackedPs`, `localUnpackedPNs` or `localUnpackedPNTs`.
        local_unpacked: Vec<UnpackedLocalBlock>,
    },
    /// `hclBoneSpaceSkinPOperator`, `...PNOperator`, `...PNTOperator`.
    BoneSpaceSkin {
        /// The class's suffix.
        components: Components,
        /// `transformSubset`.
        transform_subset: Vec<u16>,
        /// `outputBufferIndex`.
        output_buffer: u32,
        /// `transformSetIndex`.
        transform_set: u32,
        /// `boneSpaceDeformer`.
        deformer: BoneSpaceDeformer,
        /// `localPs`, `localPNs` or `localPNTs`.
        local: Vec<BoneSpaceLocalBlock>,
        /// `localUnpackedPs`, `localUnpackedPNs` or `localUnpackedPNTs`.
        local_unpacked: Vec<UnpackedLocalBlock>,
    },
    /// `hclObjectSpaceMeshMeshDeformPOperator`, `...PNOperator`, `...PNTOperator`.
    ObjectSpaceMeshMeshDeform {
        /// The class's suffix.
        components: Components,
        /// `inputBufferIdx`.
        input_buffer: u32,
        /// `outputBufferIdx`.
        output_buffer: u32,
        /// `scaleNormalBehaviour`, as stored.
        scale_normal_behaviour: u32,
        /// `inputTrianglesSubset`.
        input_triangles_subset: Vec<u16>,
        /// `triangleFromMeshTransforms` (`hkMatrix4`, four columns of four).
        triangle_from_mesh: Vec<[f32; 16]>,
        /// `objectSpaceDeformer`.
        deformer: ObjectSpaceDeformer,
        /// `customSkinDeform`.
        custom_skin_deform: bool,
        /// `localPs`, `localPNs` or `localPNTs`.
        local: Vec<ObjectSpaceLocalBlock>,
        /// `localUnpackedPs`, `localUnpackedPNs` or `localUnpackedPNTs`.
        local_unpacked: Vec<UnpackedLocalBlock>,
    },
    /// `hclBoneSpaceMeshMeshDeformPOperator`, `...PNOperator`, `...PNTOperator`.
    BoneSpaceMeshMeshDeform {
        /// The class's suffix.
        components: Components,
        /// `inputBufferIdx`.
        input_buffer: u32,
        /// `outputBufferIdx`.
        output_buffer: u32,
        /// `scaleNormalBehaviour`, as stored.
        scale_normal_behaviour: u32,
        /// `inputTrianglesSubset`.
        input_triangles_subset: Vec<u16>,
        /// `boneSpaceDeformer`.
        deformer: BoneSpaceDeformer,
        /// `localPs`, `localPNs` or `localPNTs`.
        local: Vec<BoneSpaceLocalBlock>,
        /// `localUnpackedPs`, `localUnpackedPNs` or `localUnpackedPNTs`.
        local_unpacked: Vec<UnpackedLocalBlock>,
    },
    /// `hclSimpleMeshBoneDeformOperator`: sets bones from triangles of a buffer.
    SimpleMeshBoneDeform {
        /// `inputBufferIdx`.
        input_buffer: u32,
        /// `outputTransformSetIdx`.
        output_transform_set: u32,
        /// `triangleBonePairs`: `(boneOffset, triangleOffset)`.
        triangle_bone_pairs: Vec<(u16, u16)>,
        /// `localBoneTransforms` (`hkMatrix4`, four columns of four).
        local_bone_transforms: Vec<[f32; 16]>,
    },
    /// `hclMeshBoneDeformOperator`: sets bones from weighted triangles of a buffer.
    MeshBoneDeform {
        /// `inputBufferIdx`.
        input_buffer: u32,
        /// `outputTransformSetIdx`.
        output_transform_set: u32,
        /// `triangleBonePairs`.
        triangle_bone_pairs: Vec<MeshBoneTriangle>,
        /// `triangleBoneStartForBone`.
        triangle_bone_start_for_bone: Vec<u16>,
    },
    /// `hclCopyVerticesOperator`.
    CopyVertices {
        /// `inputBufferIdx`.
        input_buffer: u32,
        /// `outputBufferIdx`.
        output_buffer: u32,
        /// `numberOfVertices`.
        vertices: u32,
        /// `startVertexIn`.
        start_in: u32,
        /// `startVertexOut`.
        start_out: u32,
        /// `copyNormals`.
        copy_normals: bool,
    },
    /// `hclGatherAllVerticesOperator`.
    GatherAllVertices {
        /// `vertexInputFromVertexOutput`: for each output vertex, its input vertex (-1: none).
        input_from_output: Vec<i16>,
        /// `inputBufferIdx`.
        input_buffer: u32,
        /// `outputBufferIdx`.
        output_buffer: u32,
        /// `gatherNormals`.
        gather_normals: bool,
        /// `partialGather`.
        partial_gather: bool,
    },
    /// `hclGatherSomeVerticesOperator`.
    GatherSomeVertices {
        /// `vertexPairs`: `(indexInput, indexOutput)`.
        pairs: Vec<(u16, u16)>,
        /// `inputBufferIdx`.
        input_buffer: u32,
        /// `outputBufferIdx`.
        output_buffer: u32,
        /// `gatherNormals`.
        gather_normals: bool,
    },
    /// `hclInputConvertOperator`: the game's vertex buffer into the cloth's layout.
    InputConvert {
        /// `userBufferIndex`.
        user_buffer: u32,
        /// `shadowBufferIndex`.
        shadow_buffer: u32,
        /// `conversionInfo`.
        conversion: ConversionInfo,
    },
    /// `hclOutputConvertOperator`: the cloth's buffer back into the game's layout.
    OutputConvert {
        /// `userBufferIndex`.
        user_buffer: u32,
        /// `shadowBufferIndex`.
        shadow_buffer: u32,
        /// `conversionInfo`.
        conversion: ConversionInfo,
    },
    /// `hclUpdateAllVertexFramesOperator`.
    UpdateAllVertexFrames {
        /// `vertToNormalID`.
        vertex_to_normal_id: Vec<u16>,
        /// `triangleFlips`.
        triangle_flips: Vec<u8>,
        /// `referenceVertices`.
        reference_vertices: Vec<u16>,
        /// `tangentEdgeCosAngle`.
        tangent_edge_cos: Vec<f32>,
        /// `tangentEdgeSinAngle`.
        tangent_edge_sin: Vec<f32>,
        /// `biTangentFlip`.
        bitangent_flip: Vec<f32>,
        /// `bufferIdx`.
        buffer: u32,
        /// `numUniqueNormalIDs`.
        unique_normal_ids: u32,
        /// `updateNormals`.
        update_normals: bool,
        /// `updateTangents`.
        update_tangents: bool,
        /// `updateBiTangents`.
        update_bitangents: bool,
    },
    /// `hclUpdateSomeVertexFramesOperator`.
    UpdateSomeVertexFrames {
        /// `involvedTriangles`.
        involved_triangles: Vec<[u16; 3]>,
        /// `involvedVertices`.
        involved_vertices: Vec<u16>,
        /// `selectionVertexToInvolvedVertex`.
        selection_vertex_to_involved_vertex: Vec<u16>,
        /// `involvedVertexToNormalID`.
        involved_vertex_to_normal_id: Vec<u16>,
        /// `triangleFlips`.
        triangle_flips: Vec<u8>,
        /// `referenceVertices`.
        reference_vertices: Vec<u16>,
        /// `tangentEdgeCosAngle`.
        tangent_edge_cos: Vec<f32>,
        /// `tangentEdgeSinAngle`.
        tangent_edge_sin: Vec<f32>,
        /// `biTangentFlip`.
        bitangent_flip: Vec<f32>,
        /// `bufferIdx`.
        buffer: u32,
        /// `numUniqueNormalIDs`.
        unique_normal_ids: u32,
        /// `updateNormals`.
        update_normals: bool,
        /// `updateTangents`.
        update_tangents: bool,
        /// `updateBiTangents`.
        update_bitangents: bool,
    },
    /// A class this reader has no layout for, kept by its name.
    Other {
        /// The class name.
        class: String,
    },
}

/// An operator of a cloth data.
#[derive(Clone, Debug, PartialEq)]
pub struct Operator {
    /// `name`.
    pub name: String,
    /// `operatorID`.
    pub id: u32,
    /// `type`, as stored.
    pub operator_type: u32,
    /// `usedBuffers`.
    pub used_buffers: Vec<BufferAccess>,
    /// `usedTransformSets`.
    pub used_transform_sets: Vec<TransformSetAccess>,
    /// What the operator does.
    pub kind: OperatorKind,
}

/// The stored words of an `hkPackedVector3`.
fn packed(v: Value<'_, '_>) -> Result<[i16; 4]> {
    fixed(v, "values", |x| {
        i16::try_from(x.signed()?).map_err(|_| invalid(x.at(), "packed vector word outside i16"))
    })
}

fn bytes(v: Value<'_, '_>, name: &str) -> Result<Vec<u8>> {
    items(v, name, |b| {
        u8::try_from(b.unsigned()?).map_err(|_| invalid(b.at(), format!("{name}: value outside u8")))
    })
}

fn reals(v: Value<'_, '_>, name: &str) -> Result<Vec<f32>> {
    items(v, name, |x| x.float())
}

fn word(v: Value<'_, '_>, name: &str) -> Result<u16> {
    index16(v.field(name)?)
}

/// The values of a fixed-size array member as 16-bit words (stored as 8- or 16-bit integers).
fn widened(v: Value<'_, '_>, name: &str) -> Result<Vec<u16>> {
    items(v, name, |x| {
        u16::try_from(x.unsigned()?).map_err(|_| invalid(x.at(), format!("{name}: value outside u16")))
    })
}

const OBJECT_SPACE_BLOCKS: [&str; 8] = [
    "oneBlendEntries",
    "twoBlendEntries",
    "threeBlendEntries",
    "fourBlendEntries",
    "fiveBlendEntries",
    "sixBlendEntries",
    "sevenBlendEntries",
    "eightBlendEntries",
];

fn object_space_deformer(v: Value<'_, '_>) -> Result<ObjectSpaceDeformer> {
    let mut blocks: [Vec<ObjectSpaceBlock>; 8] = Default::default();
    for (blends, (name, slot)) in OBJECT_SPACE_BLOCKS.iter().zip(blocks.iter_mut()).enumerate() {
        *slot = items(v, name, |b| {
            let block = ObjectSpaceBlock {
                vertices: fixed(b, "vertexIndices", index16)?,
                bones: widened(b, "boneIndices")?,
                weights: if blends == 0 {
                    Vec::new()
                } else {
                    widened(b, "boneWeights")?
                },
            };
            let expected = blends.saturating_add(1).saturating_mul(16);
            if block.bones.len() != expected || (blends > 0 && block.weights.len() != expected) {
                return Err(invalid(
                    b.at(),
                    format!(
                        "{name}: {} bones and {} weights, not {expected}",
                        block.bones.len(),
                        block.weights.len()
                    ),
                ));
            }
            Ok(block)
        })?;
    }
    Ok(ObjectSpaceDeformer {
        blocks,
        control_bytes: bytes(v, "controlBytes")?,
        start_vertex: word(v, "startVertexIndex")?,
        end_vertex: word(v, "endVertexIndex")?,
        partial_write: flag(v, "partialWrite")?,
    })
}

fn bone_space_deformer(v: Value<'_, '_>) -> Result<BoneSpaceDeformer> {
    let mut blocks: [Vec<BoneSpaceBlock>; 4] = Default::default();
    for (name, slot) in [
        "oneBlendEntries",
        "twoBlendEntries",
        "threeBlendEntries",
        "fourBlendEntries",
    ]
    .iter()
    .zip(blocks.iter_mut())
    {
        *slot = items(v, name, |b| {
            Ok(BoneSpaceBlock {
                vertices: words16(b, "vertexIndices")?,
                bones: words16(b, "boneIndices")?,
            })
        })?;
    }
    Ok(BoneSpaceDeformer {
        blocks,
        control_bytes: bytes(v, "controlBytes")?,
        start_vertex: word(v, "startVertexIndex")?,
        end_vertex: word(v, "endVertexIndex")?,
        partial_write: flag(v, "partialWrite")?,
    })
}

/// The local blocks of member `name`, positions read by `position` and normals and tangents by `normal`.
fn local_blocks<'t, 'a, P, N>(
    v: Value<'t, 'a>,
    name: &str,
    components: Components,
    position: impl Fn(Value<'t, 'a>) -> Result<P> + Copy,
    normal: impl Fn(Value<'t, 'a>) -> Result<N> + Copy,
) -> Result<Vec<LocalBlock<P, N>>> {
    items(v, name, |b| {
        Ok(LocalBlock {
            positions: fixed(b, "localPosition", position)?,
            normals: match components {
                Components::P => None,
                Components::PN | Components::PNT => Some(fixed(b, "localNormal", normal)?),
            },
            tangents: match components {
                Components::P | Components::PN => None,
                Components::PNT => Some(fixed(b, "localTangent", normal)?),
            },
        })
    })
}

/// The member names of the local blocks of a `components` operator: packed, then unpacked.
fn local_names(components: Components) -> (&'static str, &'static str) {
    match components {
        Components::P => ("localPs", "localUnpackedPs"),
        Components::PN => ("localPNs", "localUnpackedPNs"),
        Components::PNT => ("localPNTs", "localUnpackedPNTs"),
    }
}

fn vector(v: Value<'_, '_>) -> Result<[f32; 4]> {
    v.vector::<4>()
}

fn conversion(v: Value<'_, '_>) -> Result<ConversionInfo> {
    Ok(ConversionInfo {
        slots: fixed(v, "slotConversions", |s| {
            Ok(SlotConversion {
                elements: fixed(s, "elements", |b| {
                    u8::try_from(b.unsigned()?).map_err(|_| invalid(b.at(), "slot element outside u8"))
                })?,
                num_elements: byte(s, "numElements")?,
                index: byte(s, "index")?,
                partial_write: flag(s, "partialWrite")?,
            })
        })?,
        elements: fixed(v, "elementConversions", |e| {
            Ok(ElementConversion {
                index: byte(e, "index")?,
                offset: byte(e, "offset")?,
                conversion: byte(e, "conversion")?,
            })
        })?,
        num_slots: byte(v, "numSlotsConverted")?,
        num_elements: byte(v, "numElementsConverted")?,
    })
}

/// The class's component suffix after `prefix`, and whether the class is one of `prefix`'s.
fn suffix(class: &str, prefix: &str) -> Option<Components> {
    match class.strip_prefix(prefix)? {
        "POperator" => Some(Components::P),
        "PNOperator" => Some(Components::PN),
        "PNTOperator" => Some(Components::PNT),
        _ => None,
    }
}

fn kind(v: Value<'_, '_>) -> Result<OperatorKind> {
    let class = v.type_name();
    if let Some(components) = suffix(class, "hclObjectSpaceSkin") {
        let (packed_name, unpacked_name) = local_names(components);
        return Ok(OperatorKind::ObjectSpaceSkin {
            components,
            bone_from_skin_mesh: items(v, "boneFromSkinMeshTransforms", matrix)?,
            transform_subset: words16(v, "transformSubset")?,
            output_buffer: uint(v, "outputBufferIndex")?,
            transform_set: uint(v, "transformSetIndex")?,
            deformer: object_space_deformer(v.field("objectSpaceDeformer")?)?,
            local: local_blocks(v, packed_name, components, packed, packed)?,
            local_unpacked: local_blocks(v, unpacked_name, components, vector, vector)?,
        });
    }
    if let Some(components) = suffix(class, "hclBoneSpaceSkin") {
        let (packed_name, unpacked_name) = local_names(components);
        return Ok(OperatorKind::BoneSpaceSkin {
            components,
            transform_subset: words16(v, "transformSubset")?,
            output_buffer: uint(v, "outputBufferIndex")?,
            transform_set: uint(v, "transformSetIndex")?,
            deformer: bone_space_deformer(v.field("boneSpaceDeformer")?)?,
            local: local_blocks(v, packed_name, components, vector, packed)?,
            local_unpacked: local_blocks(v, unpacked_name, components, vector, vector)?,
        });
    }
    if let Some(components) = suffix(class, "hclObjectSpaceMeshMeshDeform") {
        let (packed_name, unpacked_name) = local_names(components);
        return Ok(OperatorKind::ObjectSpaceMeshMeshDeform {
            components,
            input_buffer: uint(v, "inputBufferIdx")?,
            output_buffer: uint(v, "outputBufferIdx")?,
            scale_normal_behaviour: uint(v, "scaleNormalBehaviour")?,
            input_triangles_subset: words16(v, "inputTrianglesSubset")?,
            triangle_from_mesh: items(v, "triangleFromMeshTransforms", matrix)?,
            deformer: object_space_deformer(v.field("objectSpaceDeformer")?)?,
            custom_skin_deform: flag(v, "customSkinDeform")?,
            local: local_blocks(v, packed_name, components, packed, packed)?,
            local_unpacked: local_blocks(v, unpacked_name, components, vector, vector)?,
        });
    }
    if let Some(components) = suffix(class, "hclBoneSpaceMeshMeshDeform") {
        let (packed_name, unpacked_name) = local_names(components);
        return Ok(OperatorKind::BoneSpaceMeshMeshDeform {
            components,
            input_buffer: uint(v, "inputBufferIdx")?,
            output_buffer: uint(v, "outputBufferIdx")?,
            scale_normal_behaviour: uint(v, "scaleNormalBehaviour")?,
            input_triangles_subset: words16(v, "inputTrianglesSubset")?,
            deformer: bone_space_deformer(v.field("boneSpaceDeformer")?)?,
            local: local_blocks(v, packed_name, components, vector, packed)?,
            local_unpacked: local_blocks(v, unpacked_name, components, vector, vector)?,
        });
    }
    Ok(match class {
        "hclSimulateOperator" => OperatorKind::Simulate {
            sim_cloth: uint(v, "simClothIndex")?,
            configs: items(v, "simulateOpConfigs", |c| {
                Ok(SimulateConfig {
                    name: text(c, "name")?,
                    constraint_execution: items(c, "constraintExecution", small_signed)?,
                    instance_collidables_used: items(c, "instanceCollidablesUsed", |b| {
                        Ok(b.unsigned()? != 0)
                    })?,
                    sub_steps: byte(c, "subSteps")?,
                    solve_iterations: byte(c, "numberOfSolveIterations")?,
                    use_all_instance_collidables: flag(c, "useAllInstanceCollidables")?,
                    adapt_constraint_stiffness: flag(c, "adaptConstraintStiffness")?,
                })
            })?,
        },
        "hclMoveParticlesOperator" => OperatorKind::MoveParticles {
            pairs: items(v, "vertexParticlePairs", |p| {
                Ok((word(p, "vertexIndex")?, word(p, "particleIndex")?))
            })?,
            sim_cloth: uint(v, "simClothIndex")?,
            reference_buffer: uint(v, "refBufferIdx")?,
        },
        "hclSimpleMeshBoneDeformOperator" => OperatorKind::SimpleMeshBoneDeform {
            input_buffer: uint(v, "inputBufferIdx")?,
            output_transform_set: uint(v, "outputTransformSetIdx")?,
            triangle_bone_pairs: items(v, "triangleBonePairs", |p| {
                Ok((word(p, "boneOffset")?, word(p, "triangleOffset")?))
            })?,
            local_bone_transforms: items(v, "localBoneTransforms", matrix)?,
        },
        "hclMeshBoneDeformOperator" => OperatorKind::MeshBoneDeform {
            input_buffer: uint(v, "inputBufferIdx")?,
            output_transform_set: uint(v, "outputTransformSetIdx")?,
            triangle_bone_pairs: items(v, "triangleBonePairs", |p| {
                Ok(MeshBoneTriangle {
                    local_bone_transform: matrix(p.field("localBoneTransform")?)?,
                    weight: super::real(p, "weight")?,
                    triangle: word(p, "triangleIndex")?,
                })
            })?,
            triangle_bone_start_for_bone: words16(v, "triangleBoneStartForBone")?,
        },
        "hclCopyVerticesOperator" => OperatorKind::CopyVertices {
            input_buffer: uint(v, "inputBufferIdx")?,
            output_buffer: uint(v, "outputBufferIdx")?,
            vertices: uint(v, "numberOfVertices")?,
            start_in: uint(v, "startVertexIn")?,
            start_out: uint(v, "startVertexOut")?,
            copy_normals: flag(v, "copyNormals")?,
        },
        "hclGatherAllVerticesOperator" => OperatorKind::GatherAllVertices {
            input_from_output: items(v, "vertexInputFromVertexOutput", |x| {
                i16::try_from(x.signed()?).map_err(|_| invalid(x.at(), "vertex index outside i16"))
            })?,
            input_buffer: uint(v, "inputBufferIdx")?,
            output_buffer: uint(v, "outputBufferIdx")?,
            gather_normals: flag(v, "gatherNormals")?,
            partial_gather: flag(v, "partialGather")?,
        },
        "hclGatherSomeVerticesOperator" => OperatorKind::GatherSomeVertices {
            pairs: items(v, "vertexPairs", |p| {
                Ok((word(p, "indexInput")?, word(p, "indexOutput")?))
            })?,
            input_buffer: uint(v, "inputBufferIdx")?,
            output_buffer: uint(v, "outputBufferIdx")?,
            gather_normals: flag(v, "gatherNormals")?,
        },
        "hclInputConvertOperator" => OperatorKind::InputConvert {
            user_buffer: uint(v, "userBufferIndex")?,
            shadow_buffer: uint(v, "shadowBufferIndex")?,
            conversion: conversion(v.field("conversionInfo")?)?,
        },
        "hclOutputConvertOperator" => OperatorKind::OutputConvert {
            user_buffer: uint(v, "userBufferIndex")?,
            shadow_buffer: uint(v, "shadowBufferIndex")?,
            conversion: conversion(v.field("conversionInfo")?)?,
        },
        "hclUpdateAllVertexFramesOperator" => OperatorKind::UpdateAllVertexFrames {
            vertex_to_normal_id: words16(v, "vertToNormalID")?,
            triangle_flips: bytes(v, "triangleFlips")?,
            reference_vertices: words16(v, "referenceVertices")?,
            tangent_edge_cos: reals(v, "tangentEdgeCosAngle")?,
            tangent_edge_sin: reals(v, "tangentEdgeSinAngle")?,
            bitangent_flip: reals(v, "biTangentFlip")?,
            buffer: uint(v, "bufferIdx")?,
            unique_normal_ids: uint(v, "numUniqueNormalIDs")?,
            update_normals: flag(v, "updateNormals")?,
            update_tangents: flag(v, "updateTangents")?,
            update_bitangents: flag(v, "updateBiTangents")?,
        },
        "hclUpdateSomeVertexFramesOperator" => OperatorKind::UpdateSomeVertexFrames {
            involved_triangles: items(v, "involvedTriangles", |t| fixed(t, "indices", index16))?,
            involved_vertices: words16(v, "involvedVertices")?,
            selection_vertex_to_involved_vertex: words16(v, "selectionVertexToInvolvedVertex")?,
            involved_vertex_to_normal_id: words16(v, "involvedVertexToNormalID")?,
            triangle_flips: bytes(v, "triangleFlips")?,
            reference_vertices: words16(v, "referenceVertices")?,
            tangent_edge_cos: reals(v, "tangentEdgeCosAngle")?,
            tangent_edge_sin: reals(v, "tangentEdgeSinAngle")?,
            bitangent_flip: reals(v, "biTangentFlip")?,
            buffer: uint(v, "bufferIdx")?,
            unique_normal_ids: uint(v, "numUniqueNormalIDs")?,
            update_normals: flag(v, "updateNormals")?,
            update_tangents: flag(v, "updateTangents")?,
            update_bitangents: flag(v, "updateBiTangents")?,
        },
        other => OperatorKind::Other {
            class: other.to_owned(),
        },
    })
}

pub(super) fn operator(v: Value<'_, '_>) -> Result<Operator> {
    Ok(Operator {
        name: text(v, "name")?,
        id: uint(v, "operatorID")?,
        operator_type: uint(v, "type")?,
        used_buffers: items(v, "usedBuffers", buffer_access)?,
        used_transform_sets: items(v, "usedTransformSets", transform_set_access)?,
        kind: kind(v)?,
    })
}

/// The buffer, transform set, sim cloth and particle indices an operator's own members store, checked against the
/// cloth data's lists (`at` names the cloth data in a refusal).
pub(super) fn check_operator(at: Value<'_, '_>, op: &Operator, cloth: &Cloth) -> Result<()> {
    let buffers = cloth.buffers.len();
    let sets = cloth.transform_sets.len();
    let vertices = |buffer: u32| {
        usize::try_from(buffer)
            .ok()
            .and_then(|b| cloth.buffers.get(b))
            .map_or(0, |b| usize::try_from(b.vertices).unwrap_or(usize::MAX))
    };
    let name = op.name.as_str();
    match &op.kind {
        OperatorKind::Simulate { sim_cloth, configs } => {
            bounded(
                at,
                "simulate operator sim cloth",
                *sim_cloth,
                cloth.sim_cloths.len(),
            )?;
            let sets = usize::try_from(*sim_cloth)
                .ok()
                .and_then(|s| cloth.sim_cloths.get(s))
                .map_or(0, |s| s.constraint_sets.len());
            for config in configs {
                for &c in &config.constraint_execution {
                    if c != -1 {
                        bounded(
                            at,
                            "constraint execution index",
                            u32::try_from(c).unwrap_or(u32::MAX),
                            sets,
                        )?;
                    }
                }
            }
        }
        OperatorKind::MoveParticles {
            pairs,
            sim_cloth,
            reference_buffer,
        } => {
            bounded(at, "move particles sim cloth", *sim_cloth, cloth.sim_cloths.len())?;
            bounded(at, "move particles reference buffer", *reference_buffer, buffers)?;
            let particles = usize::try_from(*sim_cloth)
                .ok()
                .and_then(|s| cloth.sim_cloths.get(s))
                .map_or(0, |s| s.particles.len());
            let reference = vertices(*reference_buffer);
            for &(vertex, particle) in pairs {
                bounded(at, "move particles particle", u32::from(particle), particles)?;
                bounded(at, "move particles vertex", u32::from(vertex), reference)?;
            }
        }
        OperatorKind::ObjectSpaceSkin {
            output_buffer,
            transform_set,
            ..
        }
        | OperatorKind::BoneSpaceSkin {
            output_buffer,
            transform_set,
            ..
        } => {
            bounded(at, "skin output buffer", *output_buffer, buffers)?;
            bounded(at, "skin transform set", *transform_set, sets)?;
        }
        OperatorKind::ObjectSpaceMeshMeshDeform {
            input_buffer,
            output_buffer,
            ..
        }
        | OperatorKind::BoneSpaceMeshMeshDeform {
            input_buffer,
            output_buffer,
            ..
        } => {
            bounded(at, "mesh deform input buffer", *input_buffer, buffers)?;
            bounded(at, "mesh deform output buffer", *output_buffer, buffers)?;
        }
        OperatorKind::SimpleMeshBoneDeform {
            input_buffer,
            output_transform_set,
            ..
        }
        | OperatorKind::MeshBoneDeform {
            input_buffer,
            output_transform_set,
            ..
        } => {
            bounded(at, "mesh bone deform input buffer", *input_buffer, buffers)?;
            bounded(at, "mesh bone deform transform set", *output_transform_set, sets)?;
        }
        OperatorKind::CopyVertices {
            input_buffer,
            output_buffer,
            vertices: count,
            start_in,
            start_out,
            ..
        } => {
            bounded(at, "copy input buffer", *input_buffer, buffers)?;
            bounded(at, "copy output buffer", *output_buffer, buffers)?;
            let end_in = start_in.checked_add(*count).unwrap_or(u32::MAX);
            let end_out = start_out.checked_add(*count).unwrap_or(u32::MAX);
            if usize::try_from(end_in).map_or(true, |e| e > vertices(*input_buffer))
                || usize::try_from(end_out).map_or(true, |e| e > vertices(*output_buffer))
            {
                return Err(invalid(
                    at.at(),
                    format!(
                        "copy operator {name}: vertices {start_in}+{count} in, {start_out}+{count} out, past a buffer"
                    ),
                ));
            }
        }
        OperatorKind::GatherAllVertices {
            input_buffer,
            output_buffer,
            ..
        }
        | OperatorKind::GatherSomeVertices {
            input_buffer,
            output_buffer,
            ..
        } => {
            bounded(at, "gather input buffer", *input_buffer, buffers)?;
            bounded(at, "gather output buffer", *output_buffer, buffers)?;
        }
        OperatorKind::InputConvert {
            user_buffer,
            shadow_buffer,
            ..
        }
        | OperatorKind::OutputConvert {
            user_buffer,
            shadow_buffer,
            ..
        } => {
            bounded(at, "convert user buffer", *user_buffer, buffers)?;
            bounded(at, "convert shadow buffer", *shadow_buffer, buffers)?;
        }
        OperatorKind::UpdateAllVertexFrames { buffer, .. }
        | OperatorKind::UpdateSomeVertexFrames { buffer, .. } => {
            bounded(at, "vertex frames buffer", *buffer, buffers)?;
        }
        OperatorKind::Other { .. } => {}
    }
    Ok(())
}

/// The reference buffer and vertices a local range or transition set names, checked against the cloth's buffers.
pub(super) fn check_reference_vertices(at: Value<'_, '_>, cloth: &Cloth) -> Result<()> {
    for sim in &cloth.sim_cloths {
        for set in &sim.constraint_sets {
            let (buffer, vertices): (u32, Vec<u16>) = match &set.kind {
                ConstraintKind::LocalRange {
                    reference_mesh_buffer,
                    constraints,
                    ..
                } => (
                    *reference_mesh_buffer,
                    constraints.iter().map(|c| c.reference_vertex).collect(),
                ),
                ConstraintKind::Transition {
                    reference_mesh_buffer,
                    particles,
                    ..
                } => (
                    *reference_mesh_buffer,
                    particles.iter().map(|p| p.reference_vertex).collect(),
                ),
                _ => continue,
            };
            bounded(at, "constraint set reference buffer", buffer, cloth.buffers.len())?;
            let count = usize::try_from(buffer)
                .ok()
                .and_then(|b| cloth.buffers.get(b))
                .map_or(0, |b| usize::try_from(b.vertices).unwrap_or(usize::MAX));
            for vertex in vertices {
                bounded(at, "reference vertex", u32::from(vertex), count)?;
            }
        }
    }
    Ok(())
}
