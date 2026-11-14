//! Synthetic cloth data: a cloth with one object of every class the reader knows, laid out by the member names and
//! types the classes have in Sekiro's and Elden Ring's tagfiles (flattened onto each class), and the refusals of the
//! indices it checks. No game payload is embedded here.
#![allow(
    clippy::unwrap_used,
    reason = "a test fixture that does not parse is the test's failure"
)]
mod graphbuilder;

use graphbuilder::{Graph, Schema, Ty, V, arr, build, fixed, floats, ints, mat4, r, s, vec4};
use havok_formats::Tagfile;
use havok_formats::cloth::{
    self, BufferKind, Components, ConstraintKind, Link, LocalRangeShape, OperatorKind, Shape,
};

const SEKIRO: &[u8; 8] = b"20160200";
const ELDEN: &[u8; 8] = b"20180100";

fn class(schema: &mut Schema, name: &'static str, members: Vec<(&'static str, Ty)>) {
    schema.insert(name, members);
}

/// The members every operator class starts with.
fn operator_members(mut own: Vec<(&'static str, Ty)>) -> Vec<(&'static str, Ty)> {
    let mut m = vec![
        ("name", Ty::Str),
        ("operatorID", Ty::U32),
        ("type", Ty::U32),
        ("usedBuffers", arr(Ty::Class("hclClothState::BufferAccess"))),
        (
            "usedTransformSets",
            arr(Ty::Class("hclClothState::TransformSetAccess")),
        ),
    ];
    m.append(&mut own);
    m
}

/// The classes, as Elden Ring stores them when `elden` (Sekiro's `hclConstraintSet` has no `constraintId`).
fn schema(elden: bool) -> Schema {
    let mut c = Schema::new();
    let u16a = || arr(Ty::U16);
    class(&mut c, "hkHandle", vec![("value", Ty::U32)]);
    class(&mut c, "hkPackedVector3", vec![("values", fixed(Ty::I16, 4))]);
    class(
        &mut c,
        "hkBitFieldStorage",
        vec![("words", arr(Ty::U32)), ("numBits", Ty::I32)],
    );
    class(
        &mut c,
        "hkBitField",
        vec![("storage", Ty::Class("hkBitFieldStorage"))],
    );
    class(&mut c, "hkSphere", vec![("pos", vec4())]);
    class(&mut c, "hkAabb", vec![("min", vec4()), ("max", vec4())]);
    for base in [
        "hclOperator",
        "hclConstraintSet",
        "hclShape",
        "hclAction",
        "hclBufferDefinition",
    ] {
        c.entry(base).or_default();
    }
    class(
        &mut c,
        "hclClothData",
        vec![
            ("name", Ty::Str),
            ("simClothDatas", arr(Ty::Ptr("hclSimClothData"))),
            ("bufferDefinitions", arr(Ty::Ptr("hclBufferDefinition"))),
            (
                "transformSetDefinitions",
                arr(Ty::Ptr("hclTransformSetDefinition")),
            ),
            ("operators", arr(Ty::Ptr("hclOperator"))),
            ("clothStateDatas", arr(Ty::Ptr("hclClothState"))),
            ("stateTransitions", arr(Ty::Ptr("hclStateTransition"))),
            ("actions", arr(Ty::Ptr("hclAction"))),
            ("targetPlatform", Ty::U32),
        ],
    );
    class(&mut c, "hclSimpleWindAction", vec![("active", Ty::Bool)]);
    class(
        &mut c,
        "hclSimClothData::OverridableSimulationInfo",
        vec![("gravity", vec4()), ("globalDampingPerSecond", Ty::F32)],
    );
    class(
        &mut c,
        "hclSimClothData::ParticleData",
        vec![
            ("mass", Ty::F32),
            ("invMass", Ty::F32),
            ("radius", Ty::F32),
            ("friction", Ty::F32),
        ],
    );
    class(
        &mut c,
        "hclSimClothData::CollidableTransformMap",
        vec![
            ("transformSetIndex", Ty::I32),
            ("transformIndices", arr(Ty::U32)),
            ("offsets", arr(mat4())),
        ],
    );
    class(
        &mut c,
        "hclSimClothData::TransferMotionData",
        vec![
            ("transformSetIndex", Ty::U32),
            ("transformIndex", Ty::U32),
            ("transferTranslationMotion", Ty::Bool),
            ("minTranslationSpeed", Ty::F32),
            ("maxTranslationSpeed", Ty::F32),
            ("minTranslationBlend", Ty::F32),
            ("maxTranslationBlend", Ty::F32),
            ("transferRotationMotion", Ty::Bool),
            ("minRotationSpeed", Ty::F32),
            ("maxRotationSpeed", Ty::F32),
            ("minRotationBlend", Ty::F32),
            ("maxRotationBlend", Ty::F32),
        ],
    );
    class(
        &mut c,
        "hclSimClothData::LandscapeCollisionData",
        vec![
            ("landscapeRadius", Ty::F32),
            ("enableStuckParticleDetection", Ty::Bool),
            ("stuckParticlesStretchFactorSq", Ty::F32),
            ("pinchDetectionEnabled", Ty::Bool),
            ("pinchDetectionPriority", Ty::I8),
            ("pinchDetectionRadius", Ty::F32),
            ("collisionTolerance", Ty::F32),
        ],
    );
    class(
        &mut c,
        "hclSimClothData::CollidablePinchingData",
        vec![
            ("pinchDetectionEnabled", Ty::Bool),
            ("pinchDetectionPriority", Ty::I8),
            ("pinchDetectionRadius", Ty::F32),
        ],
    );
    let vcp = "hclVirtualCollisionPointsData";
    class(
        &mut c,
        "hclVirtualCollisionPointsData::Block",
        vec![
            ("safeDisplacementRadius", Ty::F32),
            ("startingVCPIndex", Ty::U16),
            ("numVCPs", Ty::U8),
        ],
    );
    class(
        &mut c,
        "hclVirtualCollisionPointsData::BarycentricDictionaryEntry",
        vec![("startingBarycentricIndex", Ty::U16), ("numBarycentrics", Ty::U8)],
    );
    class(
        &mut c,
        "hclVirtualCollisionPointsData::BarycentricPair",
        vec![("u", Ty::F32), ("v", Ty::F32)],
    );
    class(
        &mut c,
        "hclVirtualCollisionPointsData::EdgeFanSection",
        vec![
            ("oppositeRealParticleIndex", Ty::U16),
            ("barycentricDictionaryIndex", Ty::U16),
        ],
    );
    class(
        &mut c,
        "hclVirtualCollisionPointsData::EdgeFan",
        vec![
            ("realParticleIndex", Ty::U16),
            ("edgeStartIndex", Ty::U16),
            ("numEdges", Ty::U8),
        ],
    );
    class(
        &mut c,
        "hclVirtualCollisionPointsData::EdgeFanLandscape",
        vec![
            ("realParticleIndex", Ty::U16),
            ("edgeStartIndex", Ty::U16),
            ("vcpStartIndex", Ty::U16),
            ("numEdges", Ty::U8),
        ],
    );
    class(
        &mut c,
        "hclVirtualCollisionPointsData::TriangleFanSection",
        vec![
            ("oppositeRealParticleIndices", fixed(Ty::U16, 2)),
            ("barycentricDictionaryIndex", Ty::U16),
        ],
    );
    class(
        &mut c,
        "hclVirtualCollisionPointsData::TriangleFan",
        vec![
            ("realParticleIndex", Ty::U16),
            ("vcpStartIndex", Ty::U16),
            ("numTriangles", Ty::U8),
        ],
    );
    class(
        &mut c,
        "hclVirtualCollisionPointsData::TriangleFanLandscape",
        vec![
            ("realParticleIndex", Ty::U16),
            ("triangleStartIndex", Ty::U16),
            ("vcpStartIndex", Ty::U16),
            ("numTriangles", Ty::U8),
        ],
    );
    let entry = || {
        arr(Ty::Class(
            "hclVirtualCollisionPointsData::BarycentricDictionaryEntry",
        ))
    };
    let edge = || arr(Ty::Class("hclVirtualCollisionPointsData::EdgeFanSection"));
    let triangle = || arr(Ty::Class("hclVirtualCollisionPointsData::TriangleFanSection"));
    class(
        &mut c,
        vcp,
        vec![
            ("blocks", arr(Ty::Class("hclVirtualCollisionPointsData::Block"))),
            ("numVCPoints", Ty::U16),
            ("landscapeParticlesBlockIndex", u16a()),
            ("numLandscapeVCPoints", Ty::U16),
            ("edgeBarycentricsDictionary", arr(Ty::F32)),
            ("edgeDictionaryEntries", entry()),
            (
                "triangleBarycentricsDictionary",
                arr(Ty::Class("hclVirtualCollisionPointsData::BarycentricPair")),
            ),
            ("triangleDictionaryEntries", entry()),
            ("edges", edge()),
            (
                "edgeFans",
                arr(Ty::Class("hclVirtualCollisionPointsData::EdgeFan")),
            ),
            ("triangles", triangle()),
            (
                "triangleFans",
                arr(Ty::Class("hclVirtualCollisionPointsData::TriangleFan")),
            ),
            ("edgesLandscape", edge()),
            (
                "edgeFansLandscape",
                arr(Ty::Class("hclVirtualCollisionPointsData::EdgeFanLandscape")),
            ),
            ("trianglesLandscape", triangle()),
            (
                "triangleFansLandscape",
                arr(Ty::Class("hclVirtualCollisionPointsData::TriangleFanLandscape")),
            ),
            ("edgeFanIndices", u16a()),
            ("triangleFanIndices", u16a()),
            ("edgeFanIndicesLandscape", u16a()),
            ("triangleFanIndicesLandscape", u16a()),
        ],
    );
    class(
        &mut c,
        "hclSimClothData",
        vec![
            ("name", Ty::Str),
            (
                "simulationInfo",
                Ty::Class("hclSimClothData::OverridableSimulationInfo"),
            ),
            ("particleDatas", arr(Ty::Class("hclSimClothData::ParticleData"))),
            ("fixedParticles", u16a()),
            ("doNormals", Ty::Bool),
            ("simOpIds", arr(Ty::U32)),
            ("simClothPoses", arr(Ty::Ptr("hclSimClothPose"))),
            ("staticConstraintSets", arr(Ty::Ptr("hclConstraintSet"))),
            ("antiPinchConstraintSets", arr(Ty::Ptr("hclConstraintSet"))),
            (
                "collidableTransformMap",
                Ty::Class("hclSimClothData::CollidableTransformMap"),
            ),
            ("perInstanceCollidables", arr(Ty::Ptr("hclCollidable"))),
            ("maxParticleRadius", Ty::F32),
            ("staticCollisionMasks", arr(Ty::U32)),
            ("actions", arr(Ty::Ptr("hclAction"))),
            ("totalMass", Ty::F32),
            (
                "transferMotionData",
                Ty::Class("hclSimClothData::TransferMotionData"),
            ),
            ("transferMotionEnabled", Ty::Bool),
            ("landscapeCollisionEnabled", Ty::Bool),
            (
                "landscapeCollisionData",
                Ty::Class("hclSimClothData::LandscapeCollisionData"),
            ),
            ("numLandscapeCollidableParticles", Ty::U32),
            ("triangleIndices", u16a()),
            ("triangleFlips", arr(Ty::U8)),
            ("pinchDetectionEnabled", Ty::Bool),
            ("perParticlePinchDetectionEnabledFlags", arr(Ty::Bool)),
            (
                "collidablePinchingDatas",
                arr(Ty::Class("hclSimClothData::CollidablePinchingData")),
            ),
            ("minPinchedParticleIndex", Ty::U16),
            ("maxPinchedParticleIndex", Ty::U16),
            ("maxCollisionPairs", Ty::U32),
            ("virtualCollisionPointsData", Ty::Class(vcp)),
        ],
    );
    class(
        &mut c,
        "hclSimClothPose",
        vec![("name", Ty::Str), ("positions", arr(vec4()))],
    );
    // constraint sets
    let set = |own: Vec<(&'static str, Ty)>| {
        let mut m = vec![("name", Ty::Str)];
        if elden {
            m.push(("constraintId", Ty::Class("hkHandle")));
        }
        m.push(("type", Ty::U32));
        m.extend(own);
        m
    };
    let link = vec![
        ("particleA", Ty::U16),
        ("particleB", Ty::U16),
        ("restLength", Ty::F32),
        ("stiffness", Ty::F32),
    ];
    class(&mut c, "hclStandardLinkConstraintSet::Link", link.clone());
    class(&mut c, "hclStretchLinkConstraintSet::Link", link);
    class(
        &mut c,
        "hclStandardLinkConstraintSet",
        set(vec![(
            "links",
            arr(Ty::Class("hclStandardLinkConstraintSet::Link")),
        )]),
    );
    class(
        &mut c,
        "hclStretchLinkConstraintSet",
        set(vec![(
            "links",
            arr(Ty::Class("hclStretchLinkConstraintSet::Link")),
        )]),
    );
    class(
        &mut c,
        "hclCompressibleLinkConstraintSet::Link",
        vec![
            ("particleA", Ty::U16),
            ("particleB", Ty::U16),
            ("restLength", Ty::F32),
            ("compressionLength", Ty::F32),
            ("stiffness", Ty::F32),
        ],
    );
    class(
        &mut c,
        "hclCompressibleLinkConstraintSet",
        set(vec![(
            "links",
            arr(Ty::Class("hclCompressibleLinkConstraintSet::Link")),
        )]),
    );
    class(
        &mut c,
        "hclBendLinkConstraintSet::Link",
        vec![
            ("particleA", Ty::U16),
            ("particleB", Ty::U16),
            ("bendMinLength", Ty::F32),
            ("stretchMaxLength", Ty::F32),
            ("bendStiffness", Ty::F32),
            ("stretchStiffness", Ty::F32),
        ],
    );
    class(
        &mut c,
        "hclBendLinkConstraintSet",
        set(vec![("links", arr(Ty::Class("hclBendLinkConstraintSet::Link")))]),
    );
    class(
        &mut c,
        "hclBendStiffnessConstraintSet::Link",
        vec![
            ("weightA", Ty::F32),
            ("weightB", Ty::F32),
            ("weightC", Ty::F32),
            ("weightD", Ty::F32),
            ("bendStiffness", Ty::F32),
            ("restCurvature", Ty::F32),
            ("particleA", Ty::U16),
            ("particleB", Ty::U16),
            ("particleC", Ty::U16),
            ("particleD", Ty::U16),
        ],
    );
    class(
        &mut c,
        "hclBendStiffnessConstraintSet",
        set(vec![
            ("links", arr(Ty::Class("hclBendStiffnessConstraintSet::Link"))),
            ("maxRestPoseHeightSq", Ty::F32),
            ("clampBendStiffness", Ty::Bool),
            ("useRestPoseConfig", Ty::Bool),
        ]),
    );
    class(
        &mut c,
        "hclLocalRangeConstraintSet::LocalConstraint",
        vec![
            ("particleIndex", Ty::U16),
            ("referenceVertex", Ty::U16),
            ("maximumDistance", Ty::F32),
            ("maxNormalDistance", Ty::F32),
            ("minNormalDistance", Ty::F32),
        ],
    );
    class(
        &mut c,
        "hclLocalRangeConstraintSet",
        set(vec![
            (
                "localConstraints",
                arr(Ty::Class("hclLocalRangeConstraintSet::LocalConstraint")),
            ),
            ("referenceMeshBufferIdx", Ty::U32),
            ("stiffness", Ty::F32),
            ("shapeType", Ty::U32),
            ("applyNormalComponent", Ty::Bool),
        ]),
    );
    class(
        &mut c,
        "hclTransitionConstraintSet::PerParticle",
        vec![
            ("particleIndex", Ty::U16),
            ("referenceVertex", Ty::U16),
            ("toAnimDelay", Ty::F32),
            ("toSimDelay", Ty::F32),
            ("toSimMaxDistance", Ty::F32),
        ],
    );
    class(
        &mut c,
        "hclTransitionConstraintSet",
        set(vec![
            (
                "perParticleData",
                arr(Ty::Class("hclTransitionConstraintSet::PerParticle")),
            ),
            ("toAnimPeriod", Ty::F32),
            ("toAnimPlusDelayPeriod", Ty::F32),
            ("toSimPeriod", Ty::F32),
            ("toSimPlusDelayPeriod", Ty::F32),
            ("referenceMeshBufferIdx", Ty::U32),
        ]),
    );
    let batch = |last: &'static str| {
        vec![
            ("frameVector", fixed(vec4(), 16)),
            ("particleIndex", fixed(Ty::U16, 16)),
            (last, fixed(Ty::F32, 16)),
        ]
    };
    let single = |last: &'static str| {
        vec![
            ("frameVector", vec4()),
            ("particleIndex", Ty::U16),
            (last, Ty::F32),
        ]
    };
    class(&mut c, "hclVolumeConstraintMx::FrameBatchData", batch("weight"));
    class(&mut c, "hclVolumeConstraintMx::FrameSingleData", single("weight"));
    class(
        &mut c,
        "hclVolumeConstraintMx::ApplyBatchData",
        batch("stiffness"),
    );
    class(
        &mut c,
        "hclVolumeConstraintMx::ApplySingleData",
        single("stiffness"),
    );
    class(
        &mut c,
        "hclVolumeConstraintMx",
        set(vec![
            (
                "frameBatchDatas",
                arr(Ty::Class("hclVolumeConstraintMx::FrameBatchData")),
            ),
            (
                "frameSingleDatas",
                arr(Ty::Class("hclVolumeConstraintMx::FrameSingleData")),
            ),
            (
                "applyBatchDatas",
                arr(Ty::Class("hclVolumeConstraintMx::ApplyBatchData")),
            ),
            (
                "applySingleDatas",
                arr(Ty::Class("hclVolumeConstraintMx::ApplySingleData")),
            ),
        ]),
    );
    class(&mut c, "hclBendStiffnessExperimentalSet", set(Vec::new()));
    // collision
    class(
        &mut c,
        "hclCollidable",
        vec![
            ("transform", mat4()),
            ("linearVelocity", vec4()),
            ("angularVelocity", vec4()),
            ("userData", Ty::U64),
            ("shape", Ty::Ptr("hclShape")),
            ("name", Ty::Str),
            ("pinchDetectionRadius", Ty::F32),
            ("pinchDetectionPriority", Ty::I8),
            ("pinchDetectionEnabled", Ty::Bool),
            ("virtualCollisionPointCollisionEnabled", Ty::Bool),
            ("enabled", Ty::Bool),
        ],
    );
    class(
        &mut c,
        "hclSphereShape",
        vec![("type", Ty::I32), ("sphere", Ty::Class("hkSphere"))],
    );
    class(
        &mut c,
        "hclCapsuleShape",
        vec![
            ("type", Ty::I32),
            ("start", vec4()),
            ("end", vec4()),
            ("dir", vec4()),
            ("radius", Ty::F32),
            ("capLenSqrdInv", Ty::F32),
        ],
    );
    class(
        &mut c,
        "hclTaperedCapsuleShape",
        vec![
            ("type", Ty::I32),
            ("small", vec4()),
            ("big", vec4()),
            ("coneApex", vec4()),
            ("coneAxis", vec4()),
            ("lVec", vec4()),
            ("dVec", vec4()),
            ("tanThetaVecNeg", vec4()),
            ("smallRadius", Ty::F32),
            ("bigRadius", Ty::F32),
            ("l", Ty::F32),
            ("d", Ty::F32),
            ("cosTheta", Ty::F32),
            ("sinTheta", Ty::F32),
            ("tanTheta", Ty::F32),
            ("tanThetaSqr", Ty::F32),
        ],
    );
    class(
        &mut c,
        "hclPlaneShape",
        vec![("type", Ty::I32), ("planeEquation", vec4())],
    );
    class(
        &mut c,
        "hclConvexGeometryShape",
        vec![
            ("type", Ty::I32),
            ("tetrahedraGrid", u16a()),
            ("gridCells", arr(Ty::U8)),
            ("tetrahedraEquations", arr(mat4())),
            ("localFromWorld", mat4()),
            ("worldFromLocal", mat4()),
            ("objAabb", Ty::Class("hkAabb")),
            ("geomCentroid", vec4()),
            ("invCellSize", vec4()),
            ("gridRes", Ty::U16),
        ],
    );
    class(&mut c, "hclConvexHeightFieldShape", vec![("type", Ty::I32)]);
    // buffers, transform sets, usage, states
    class(
        &mut c,
        "hclBufferLayout::BufferElement",
        vec![
            ("vectorConversion", Ty::U8),
            ("vectorSize", Ty::U8),
            ("slotId", Ty::U8),
            ("slotStart", Ty::U8),
        ],
    );
    class(
        &mut c,
        "hclBufferLayout::Slot",
        vec![("flags", Ty::U8), ("stride", Ty::U8)],
    );
    class(
        &mut c,
        "hclBufferLayout",
        vec![
            (
                "elementsLayout",
                fixed(Ty::Class("hclBufferLayout::BufferElement"), 4),
            ),
            ("slots", fixed(Ty::Class("hclBufferLayout::Slot"), 4)),
            ("numSlots", Ty::U8),
            ("triangleFormat", Ty::U8),
        ],
    );
    let buffer = |own: Vec<(&'static str, Ty)>| {
        let mut m = vec![
            ("name", Ty::Str),
            ("type", Ty::I32),
            ("subType", Ty::I32),
            ("numVertices", Ty::U32),
            ("numTriangles", Ty::U32),
            ("bufferLayout", Ty::Class("hclBufferLayout")),
        ];
        m.extend(own);
        m
    };
    c.insert("hclBufferDefinition", buffer(Vec::new()));
    class(
        &mut c,
        "hclScratchBufferDefinition",
        buffer(vec![
            ("triangleIndices", u16a()),
            ("storeNormals", Ty::Bool),
            ("storeTangentsAndBiTangents", Ty::Bool),
        ]),
    );
    class(
        &mut c,
        "hclShadowBufferDefinition",
        buffer(vec![
            ("triangleIndices", u16a()),
            ("shadowPositions", Ty::Bool),
            ("shadowNormals", Ty::Bool),
            ("shadowTangents", Ty::Bool),
            ("shadowBiTangents", Ty::Bool),
        ]),
    );
    class(
        &mut c,
        "hclTransformSetDefinition",
        vec![("name", Ty::Str), ("type", Ty::I32), ("numTransforms", Ty::U32)],
    );
    class(
        &mut c,
        "hclBufferUsage",
        vec![
            ("perComponentFlags", fixed(Ty::U8, 4)),
            ("trianglesRead", Ty::Bool),
        ],
    );
    class(
        &mut c,
        "hclClothState::BufferAccess",
        vec![
            ("bufferIndex", Ty::U32),
            ("bufferUsage", Ty::Class("hclBufferUsage")),
            ("shadowBufferIndex", Ty::U32),
        ],
    );
    class(
        &mut c,
        "hclTransformSetUsage::TransformTracker",
        vec![
            ("read", Ty::Class("hkBitField")),
            ("readBeforeWrite", Ty::Class("hkBitField")),
            ("written", Ty::Class("hkBitField")),
        ],
    );
    class(
        &mut c,
        "hclTransformSetUsage",
        vec![
            ("perComponentFlags", fixed(Ty::U8, 2)),
            (
                "perComponentTransformTrackers",
                arr(Ty::Class("hclTransformSetUsage::TransformTracker")),
            ),
        ],
    );
    class(
        &mut c,
        "hclClothState::TransformSetAccess",
        vec![
            ("transformSetIndex", Ty::U32),
            ("transformSetUsage", Ty::Class("hclTransformSetUsage")),
        ],
    );
    class(
        &mut c,
        "hclStateDependencyGraph::Branch",
        vec![
            ("branchId", Ty::I32),
            ("stateOperatorIndices", arr(Ty::I32)),
            ("parentBranches", arr(Ty::I32)),
            ("childBranches", arr(Ty::I32)),
        ],
    );
    class(
        &mut c,
        "hclStateDependencyGraph",
        vec![
            ("branches", arr(Ty::Class("hclStateDependencyGraph::Branch"))),
            ("rootBranchIds", arr(Ty::I32)),
            ("children", arr(arr(Ty::I32))),
            ("parents", arr(arr(Ty::I32))),
            ("multiThreadable", Ty::Bool),
        ],
    );
    class(
        &mut c,
        "hclClothState",
        vec![
            ("name", Ty::Str),
            ("operators", arr(Ty::U32)),
            ("usedBuffers", arr(Ty::Class("hclClothState::BufferAccess"))),
            (
                "usedTransformSets",
                arr(Ty::Class("hclClothState::TransformSetAccess")),
            ),
            ("usedSimCloths", arr(Ty::U32)),
            ("dependencyGraph", Ty::Ptr("hclStateDependencyGraph")),
        ],
    );
    let id = if elden { Ty::Class("hkHandle") } else { Ty::I32 };
    class(
        &mut c,
        "hclStateTransition::SimClothTransitionData",
        vec![
            ("isSimulated", Ty::Bool),
            ("transitionConstraints", arr(id.clone())),
            ("transitionType", Ty::U32),
        ],
    );
    class(
        &mut c,
        "hclStateTransition::BlendOpTransitionData",
        vec![
            ("bufferASimCloths", arr(Ty::I32)),
            ("bufferBSimCloths", arr(Ty::I32)),
            ("transitionType", Ty::U32),
            ("blendWeightType", Ty::U32),
            ("blendOperatorId", Ty::U32),
        ],
    );
    class(
        &mut c,
        "hclStateTransition::StateTransitionData",
        vec![
            (
                "simClothTransitionData",
                arr(Ty::Class("hclStateTransition::SimClothTransitionData")),
            ),
            (
                "blendOpTransitionData",
                arr(Ty::Class("hclStateTransition::BlendOpTransitionData")),
            ),
            ("simulatedState", Ty::Bool),
            ("emptyState", Ty::Bool),
        ],
    );
    class(
        &mut c,
        "hclStateTransition",
        vec![
            ("name", Ty::Str),
            ("stateIds", arr(Ty::U32)),
            (
                "stateTransitionData",
                arr(Ty::Class("hclStateTransition::StateTransitionData")),
            ),
            ("simClothTransitionConstraints", arr(arr(id))),
        ],
    );
    // operators and their parts
    class(
        &mut c,
        "hclSimulateOperator::Config",
        vec![
            ("name", Ty::Str),
            ("constraintExecution", arr(Ty::I32)),
            ("instanceCollidablesUsed", arr(Ty::Bool)),
            ("subSteps", Ty::U8),
            ("numberOfSolveIterations", Ty::U8),
            ("useAllInstanceCollidables", Ty::Bool),
            ("adaptConstraintStiffness", Ty::Bool),
        ],
    );
    class(
        &mut c,
        "hclSimulateOperator",
        operator_members(vec![
            ("simClothIndex", Ty::U32),
            ("simulateOpConfigs", arr(Ty::Class("hclSimulateOperator::Config"))),
        ]),
    );
    class(
        &mut c,
        "hclMoveParticlesOperator::VertexParticlePair",
        vec![("vertexIndex", Ty::U16), ("particleIndex", Ty::U16)],
    );
    class(
        &mut c,
        "hclMoveParticlesOperator",
        operator_members(vec![
            (
                "vertexParticlePairs",
                arr(Ty::Class("hclMoveParticlesOperator::VertexParticlePair")),
            ),
            ("simClothIndex", Ty::U32),
            ("refBufferIdx", Ty::U32),
        ]),
    );
    // object space deformer: n-bone blocks, weights 8-bit up to four bones and 16-bit above
    let names = ["One", "Two", "Three", "Four", "Five", "Six", "Seven", "Eight"];
    let block_names: [&'static str; 8] = [
        "hclObjectSpaceDeformer::OneBlendEntryBlock",
        "hclObjectSpaceDeformer::TwoBlendEntryBlock",
        "hclObjectSpaceDeformer::ThreeBlendEntryBlock",
        "hclObjectSpaceDeformer::FourBlendEntryBlock",
        "hclObjectSpaceDeformer::FiveBlendEntryBlock",
        "hclObjectSpaceDeformer::SixBlendEntryBlock",
        "hclObjectSpaceDeformer::SevenBlendEntryBlock",
        "hclObjectSpaceDeformer::EightBlendEntryBlock",
    ];
    for (n, block) in (1u32..=8).zip(block_names) {
        let mut m = vec![
            ("vertexIndices", fixed(Ty::U16, 16)),
            ("boneIndices", fixed(Ty::U16, 16 * n)),
        ];
        if n > 1 {
            m.push((
                "boneWeights",
                fixed(if n <= 4 { Ty::U8 } else { Ty::U16 }, 16 * n),
            ));
        }
        class(&mut c, block, m);
    }
    let _ = names;
    let member_names: [&'static str; 8] = [
        "oneBlendEntries",
        "twoBlendEntries",
        "threeBlendEntries",
        "fourBlendEntries",
        "fiveBlendEntries",
        "sixBlendEntries",
        "sevenBlendEntries",
        "eightBlendEntries",
    ];
    let mut object_space: Vec<(&'static str, Ty)> = member_names
        .iter()
        .rev()
        .zip(block_names.iter().rev())
        .map(|(m, b)| (*m, arr(Ty::Class(b))))
        .collect();
    object_space.extend([
        ("controlBytes", arr(Ty::U8)),
        ("startVertexIndex", Ty::U16),
        ("endVertexIndex", Ty::U16),
        ("partialWrite", Ty::Bool),
    ]);
    class(&mut c, "hclObjectSpaceDeformer", object_space);
    let bone_blocks: [(&'static str, u32, u32); 4] = [
        ("hclBoneSpaceDeformer::OneBlendEntryBlock", 16, 16),
        ("hclBoneSpaceDeformer::TwoBlendEntryBlock", 8, 16),
        ("hclBoneSpaceDeformer::ThreeBlendEntryBlock", 5, 15),
        ("hclBoneSpaceDeformer::FourBlendEntryBlock", 4, 16),
    ];
    for (name, vertices, bones) in bone_blocks {
        class(
            &mut c,
            name,
            vec![
                ("vertexIndices", fixed(Ty::U16, vertices)),
                ("boneIndices", fixed(Ty::U16, bones)),
            ],
        );
    }
    class(
        &mut c,
        "hclBoneSpaceDeformer",
        vec![
            (
                "fourBlendEntries",
                arr(Ty::Class("hclBoneSpaceDeformer::FourBlendEntryBlock")),
            ),
            (
                "threeBlendEntries",
                arr(Ty::Class("hclBoneSpaceDeformer::ThreeBlendEntryBlock")),
            ),
            (
                "twoBlendEntries",
                arr(Ty::Class("hclBoneSpaceDeformer::TwoBlendEntryBlock")),
            ),
            (
                "oneBlendEntries",
                arr(Ty::Class("hclBoneSpaceDeformer::OneBlendEntryBlock")),
            ),
            ("controlBytes", arr(Ty::U8)),
            ("startVertexIndex", Ty::U16),
            ("endVertexIndex", Ty::U16),
            ("partialWrite", Ty::Bool),
        ],
    );
    let packed = || fixed(Ty::Class("hkPackedVector3"), 16);
    let floats16 = || fixed(vec4(), 16);
    class(
        &mut c,
        "hclObjectSpaceDeformer::LocalBlockPN",
        vec![("localPosition", packed()), ("localNormal", packed())],
    );
    class(
        &mut c,
        "hclObjectSpaceDeformer::LocalBlockP",
        vec![("localPosition", packed())],
    );
    class(
        &mut c,
        "hclObjectSpaceDeformer::LocalBlockUnpackedPN",
        vec![("localPosition", floats16()), ("localNormal", floats16())],
    );
    class(
        &mut c,
        "hclObjectSpaceDeformer::LocalBlockUnpackedP",
        vec![("localPosition", floats16())],
    );
    class(
        &mut c,
        "hclBoneSpaceDeformer::LocalBlockPNT",
        vec![
            ("localPosition", floats16()),
            ("localNormal", packed()),
            ("localTangent", packed()),
        ],
    );
    class(
        &mut c,
        "hclBoneSpaceDeformer::LocalBlockUnpackedPNT",
        vec![
            ("localPosition", floats16()),
            ("localNormal", floats16()),
            ("localTangent", floats16()),
        ],
    );
    class(
        &mut c,
        "hclBoneSpaceDeformer::LocalBlockPN",
        vec![("localPosition", floats16()), ("localNormal", packed())],
    );
    class(
        &mut c,
        "hclBoneSpaceDeformer::LocalBlockUnpackedPN",
        vec![("localPosition", floats16()), ("localNormal", floats16())],
    );
    class(
        &mut c,
        "hclObjectSpaceSkinPNOperator",
        operator_members(vec![
            ("boneFromSkinMeshTransforms", arr(mat4())),
            ("transformSubset", u16a()),
            ("outputBufferIndex", Ty::U32),
            ("transformSetIndex", Ty::U32),
            ("objectSpaceDeformer", Ty::Class("hclObjectSpaceDeformer")),
            ("localPNs", arr(Ty::Class("hclObjectSpaceDeformer::LocalBlockPN"))),
            (
                "localUnpackedPNs",
                arr(Ty::Class("hclObjectSpaceDeformer::LocalBlockUnpackedPN")),
            ),
        ]),
    );
    class(
        &mut c,
        "hclBoneSpaceSkinPNTOperator",
        operator_members(vec![
            ("transformSubset", u16a()),
            ("outputBufferIndex", Ty::U32),
            ("transformSetIndex", Ty::U32),
            ("boneSpaceDeformer", Ty::Class("hclBoneSpaceDeformer")),
            ("localPNTs", arr(Ty::Class("hclBoneSpaceDeformer::LocalBlockPNT"))),
            (
                "localUnpackedPNTs",
                arr(Ty::Class("hclBoneSpaceDeformer::LocalBlockUnpackedPNT")),
            ),
        ]),
    );
    class(
        &mut c,
        "hclObjectSpaceMeshMeshDeformPOperator",
        operator_members(vec![
            ("inputBufferIdx", Ty::U32),
            ("outputBufferIdx", Ty::U32),
            ("scaleNormalBehaviour", Ty::U32),
            ("inputTrianglesSubset", u16a()),
            ("triangleFromMeshTransforms", arr(mat4())),
            ("objectSpaceDeformer", Ty::Class("hclObjectSpaceDeformer")),
            ("customSkinDeform", Ty::Bool),
            ("localPs", arr(Ty::Class("hclObjectSpaceDeformer::LocalBlockP"))),
            (
                "localUnpackedPs",
                arr(Ty::Class("hclObjectSpaceDeformer::LocalBlockUnpackedP")),
            ),
        ]),
    );
    class(
        &mut c,
        "hclBoneSpaceMeshMeshDeformPNOperator",
        operator_members(vec![
            ("inputBufferIdx", Ty::U32),
            ("outputBufferIdx", Ty::U32),
            ("scaleNormalBehaviour", Ty::U32),
            ("inputTrianglesSubset", u16a()),
            ("boneSpaceDeformer", Ty::Class("hclBoneSpaceDeformer")),
            ("localPNs", arr(Ty::Class("hclBoneSpaceDeformer::LocalBlockPN"))),
            (
                "localUnpackedPNs",
                arr(Ty::Class("hclBoneSpaceDeformer::LocalBlockUnpackedPN")),
            ),
        ]),
    );
    class(
        &mut c,
        "hclSimpleMeshBoneDeformOperator::TriangleBonePair",
        vec![("boneOffset", Ty::U16), ("triangleOffset", Ty::U16)],
    );
    class(
        &mut c,
        "hclSimpleMeshBoneDeformOperator",
        operator_members(vec![
            ("inputBufferIdx", Ty::U32),
            ("outputTransformSetIdx", Ty::U32),
            (
                "triangleBonePairs",
                arr(Ty::Class("hclSimpleMeshBoneDeformOperator::TriangleBonePair")),
            ),
            ("localBoneTransforms", arr(mat4())),
        ]),
    );
    class(
        &mut c,
        "hclMeshBoneDeformOperator::TriangleBonePair",
        vec![
            ("localBoneTransform", mat4()),
            ("weight", Ty::F32),
            ("triangleIndex", Ty::U16),
        ],
    );
    class(
        &mut c,
        "hclMeshBoneDeformOperator",
        operator_members(vec![
            ("inputBufferIdx", Ty::U32),
            ("outputTransformSetIdx", Ty::U32),
            (
                "triangleBonePairs",
                arr(Ty::Class("hclMeshBoneDeformOperator::TriangleBonePair")),
            ),
            ("triangleBoneStartForBone", u16a()),
        ]),
    );
    class(
        &mut c,
        "hclCopyVerticesOperator",
        operator_members(vec![
            ("inputBufferIdx", Ty::U32),
            ("outputBufferIdx", Ty::U32),
            ("numberOfVertices", Ty::U32),
            ("startVertexIn", Ty::U32),
            ("startVertexOut", Ty::U32),
            ("copyNormals", Ty::Bool),
        ]),
    );
    class(
        &mut c,
        "hclGatherAllVerticesOperator",
        operator_members(vec![
            ("vertexInputFromVertexOutput", arr(Ty::I16)),
            ("inputBufferIdx", Ty::U32),
            ("outputBufferIdx", Ty::U32),
            ("gatherNormals", Ty::Bool),
            ("partialGather", Ty::Bool),
        ]),
    );
    class(
        &mut c,
        "hclGatherSomeVerticesOperator::VertexPair",
        vec![("indexInput", Ty::U16), ("indexOutput", Ty::U16)],
    );
    class(
        &mut c,
        "hclGatherSomeVerticesOperator",
        operator_members(vec![
            (
                "vertexPairs",
                arr(Ty::Class("hclGatherSomeVerticesOperator::VertexPair")),
            ),
            ("inputBufferIdx", Ty::U32),
            ("outputBufferIdx", Ty::U32),
            ("gatherNormals", Ty::Bool),
        ]),
    );
    class(
        &mut c,
        "hclRuntimeConversionInfo::SlotConversion",
        vec![
            ("elements", fixed(Ty::U8, 4)),
            ("numElements", Ty::U8),
            ("index", Ty::U8),
            ("partialWrite", Ty::Bool),
        ],
    );
    class(
        &mut c,
        "hclRuntimeConversionInfo::ElementConversion",
        vec![("index", Ty::U8), ("offset", Ty::U8), ("conversion", Ty::U8)],
    );
    class(
        &mut c,
        "hclRuntimeConversionInfo",
        vec![
            (
                "slotConversions",
                fixed(Ty::Class("hclRuntimeConversionInfo::SlotConversion"), 4),
            ),
            (
                "elementConversions",
                fixed(Ty::Class("hclRuntimeConversionInfo::ElementConversion"), 4),
            ),
            ("numSlotsConverted", Ty::U8),
            ("numElementsConverted", Ty::U8),
        ],
    );
    for name in ["hclInputConvertOperator", "hclOutputConvertOperator"] {
        class(
            &mut c,
            name,
            operator_members(vec![
                ("userBufferIndex", Ty::U32),
                ("shadowBufferIndex", Ty::U32),
                ("conversionInfo", Ty::Class("hclRuntimeConversionInfo")),
            ]),
        );
    }
    let frames = |own: Vec<(&'static str, Ty)>| {
        let mut m = own;
        m.extend([
            ("triangleFlips", arr(Ty::U8)),
            ("referenceVertices", u16a()),
            ("tangentEdgeCosAngle", arr(Ty::F32)),
            ("tangentEdgeSinAngle", arr(Ty::F32)),
            ("biTangentFlip", arr(Ty::F32)),
            ("bufferIdx", Ty::U32),
            ("numUniqueNormalIDs", Ty::U32),
            ("updateNormals", Ty::Bool),
            ("updateTangents", Ty::Bool),
            ("updateBiTangents", Ty::Bool),
        ]);
        operator_members(m)
    };
    class(
        &mut c,
        "hclUpdateAllVertexFramesOperator",
        frames(vec![("vertToNormalID", u16a())]),
    );
    class(
        &mut c,
        "hclUpdateSomeVertexFramesOperator::Triangle",
        vec![("indices", fixed(Ty::U16, 3))],
    );
    class(
        &mut c,
        "hclUpdateSomeVertexFramesOperator",
        frames(vec![
            (
                "involvedTriangles",
                arr(Ty::Class("hclUpdateSomeVertexFramesOperator::Triangle")),
            ),
            ("involvedVertices", u16a()),
            ("selectionVertexToInvolvedVertex", u16a()),
            ("involvedVertexToNormalID", u16a()),
        ]),
    );
    class(
        &mut c,
        "hclBlendSomeVerticesOperator",
        operator_members(Vec::new()),
    );
    c
}

/// What the fixture changes from its valid defaults.
#[derive(Clone, Copy, Default)]
struct Change {
    /// A member of one object to overwrite: (object, member, value).
    member: Option<(&'static str, &'static str, i64)>,
}

/// A valid cloth: one sim cloth of three particles with one constraint set of each kind, two collidables, three
/// buffers of four vertices, one transform set of two bones, an operator of most classes, one state. `change`
/// overwrites one integer member of one object (named by its class).
fn graph(elden: bool, change: Change) -> Graph {
    let mut g: Graph = Vec::new();
    let put = |g: &mut Graph, class: &'static str, mut fields: Vec<(&'static str, V)>| -> usize {
        if !elden {
            // Sekiro's constraint sets have no constraintId
            fields.retain(|(m, _)| *m != "constraintId");
        }
        if let Some((target, member, value)) = change.member
            && target == class
        {
            fields.retain(|(m, _)| *m != member);
            fields.push((member, V::I(value)));
        }
        g.push((class, r(fields)));
        g.len() - 1
    };
    let id = |n: i64| {
        if elden {
            r(vec![("value", V::I(n))])
        } else {
            V::I(n)
        }
    };
    let cloth_index = put(&mut g, "hclClothData", Vec::new());
    // shapes and collidables
    let sphere = put(
        &mut g,
        "hclSphereShape",
        vec![("sphere", r(vec![("pos", floats(&[1.0, 2.0, 3.0, 0.25]))]))],
    );
    let capsule = put(
        &mut g,
        "hclCapsuleShape",
        vec![
            ("start", floats(&[0.0, 0.0, 0.0, 0.0])),
            ("end", floats(&[0.0, 1.0, 0.0, 0.0])),
            ("dir", floats(&[0.0, 1.0, 0.0, 0.0])),
            ("radius", V::F(0.5)),
            ("capLenSqrdInv", V::F(1.0)),
        ],
    );
    let collidable = |g: &mut Graph, name: &str, shape: usize| {
        g.push((
            "hclCollidable",
            r(vec![
                ("name", s(name)),
                (
                    "transform",
                    floats(&[
                        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
                    ]),
                ),
                ("userData", V::I(7)),
                ("shape", V::Obj(shape)),
                ("pinchDetectionRadius", V::F(0.01)),
                ("pinchDetectionPriority", V::I(-1)),
                ("enabled", V::I(1)),
            ]),
        ));
        g.len() - 1
    };
    let collidable_a = collidable(&mut g, "ball", sphere);
    let collidable_b = collidable(&mut g, "arm", capsule);
    // constraint sets
    let link = |a: i64, b: i64| {
        r(vec![
            ("particleA", V::I(a)),
            ("particleB", V::I(b)),
            ("restLength", V::F(0.5)),
            ("stiffness", V::F(0.75)),
        ])
    };
    let mut sets = vec![put(
        &mut g,
        "hclStandardLinkConstraintSet",
        vec![
            ("name", s("standard")),
            ("constraintId", id(1)),
            ("links", V::A(vec![link(0, 1), link(1, 2)])),
        ],
    )];
    sets.push(put(
        &mut g,
        "hclStretchLinkConstraintSet",
        vec![
            ("name", s("stretch")),
            ("constraintId", id(2)),
            ("links", V::A(vec![link(0, 2)])),
        ],
    ));
    sets.push(put(
        &mut g,
        "hclCompressibleLinkConstraintSet",
        vec![
            ("name", s("compressible")),
            ("constraintId", id(3)),
            (
                "links",
                V::A(vec![r(vec![
                    ("particleA", V::I(2)),
                    ("particleB", V::I(1)),
                    ("restLength", V::F(1.0)),
                    ("compressionLength", V::F(0.5)),
                    ("stiffness", V::F(0.25)),
                ])]),
            ),
        ],
    ));
    sets.push(put(
        &mut g,
        "hclBendLinkConstraintSet",
        vec![
            ("name", s("bend")),
            ("constraintId", id(4)),
            (
                "links",
                V::A(vec![r(vec![
                    ("particleA", V::I(0)),
                    ("particleB", V::I(2)),
                    ("bendMinLength", V::F(0.9)),
                    ("stretchMaxLength", V::F(1.1)),
                    ("bendStiffness", V::F(0.2)),
                    ("stretchStiffness", V::F(0.3)),
                ])]),
            ),
        ],
    ));
    sets.push(put(
        &mut g,
        "hclBendStiffnessConstraintSet",
        vec![
            ("name", s("bend stiffness")),
            ("constraintId", id(5)),
            (
                "links",
                V::A(vec![r(vec![
                    ("weightA", V::F(0.1)),
                    ("weightB", V::F(0.2)),
                    ("weightC", V::F(0.3)),
                    ("weightD", V::F(0.4)),
                    ("bendStiffness", V::F(0.5)),
                    ("restCurvature", V::F(0.6)),
                    ("particleA", V::I(0)),
                    ("particleB", V::I(1)),
                    ("particleC", V::I(2)),
                    ("particleD", V::I(1)),
                ])]),
            ),
            ("maxRestPoseHeightSq", V::F(4.0)),
            ("clampBendStiffness", V::I(1)),
        ],
    ));
    sets.push(put(
        &mut g,
        "hclLocalRangeConstraintSet",
        vec![
            ("name", s("local range")),
            ("constraintId", id(6)),
            (
                "localConstraints",
                V::A(vec![r(vec![
                    ("particleIndex", V::I(2)),
                    ("referenceVertex", V::I(3)),
                    ("maximumDistance", V::F(0.4)),
                    ("maxNormalDistance", V::F(0.3)),
                    ("minNormalDistance", V::F(0.2)),
                ])]),
            ),
            ("referenceMeshBufferIdx", V::I(1)),
            ("stiffness", V::F(1.0)),
            ("shapeType", V::I(1)),
            ("applyNormalComponent", V::I(1)),
        ],
    ));
    sets.push(put(
        &mut g,
        "hclTransitionConstraintSet",
        vec![
            ("name", s("transition")),
            ("constraintId", id(7)),
            (
                "perParticleData",
                V::A(vec![r(vec![
                    ("particleIndex", V::I(1)),
                    ("referenceVertex", V::I(2)),
                    ("toAnimDelay", V::F(0.05)),
                    ("toSimDelay", V::F(0.06)),
                    ("toSimMaxDistance", V::F(0.07)),
                ])]),
            ),
            ("toAnimPeriod", V::F(0.1)),
            ("toSimPeriod", V::F(0.1)),
            ("referenceMeshBufferIdx", V::I(0)),
        ],
    ));
    let sixteen = |x: i64| ints(&[x; 16]);
    sets.push(put(
        &mut g,
        "hclVolumeConstraintMx",
        vec![
            ("name", s("volume")),
            ("constraintId", id(8)),
            (
                "frameBatchDatas",
                V::A(vec![r(vec![
                    ("particleIndex", sixteen(2)),
                    ("weight", floats(&[0.0625; 16])),
                ])]),
            ),
            (
                "applySingleDatas",
                V::A(vec![r(vec![
                    ("frameVector", floats(&[1.0, 0.0, 0.0, 0.0])),
                    ("particleIndex", V::I(1)),
                    ("stiffness", V::F(0.5)),
                ])]),
            ),
        ],
    ));
    sets.push(put(
        &mut g,
        "hclBendStiffnessExperimentalSet",
        vec![("name", s("unknown"))],
    ));
    let pose = put(
        &mut g,
        "hclSimClothPose",
        vec![
            ("name", s("rest")),
            ("positions", V::A(vec![floats(&[0.0, 0.0, 0.0, 1.0]); 3])),
        ],
    );
    let particle = |m: f32| {
        r(vec![
            ("mass", V::F(m)),
            ("invMass", V::F(if m == 0.0 { 0.0 } else { 1.0 / m })),
            ("radius", V::F(0.01)),
            ("friction", V::F(0.5)),
        ])
    };
    let sim = put(
        &mut g,
        "hclSimClothData",
        vec![
            ("name", s("sim")),
            (
                "simulationInfo",
                r(vec![
                    ("gravity", floats(&[0.0, -9.8, 0.0, 0.0])),
                    ("globalDampingPerSecond", V::F(0.1)),
                ]),
            ),
            (
                "particleDatas",
                V::A(vec![particle(0.0), particle(2.0), particle(4.0)]),
            ),
            ("fixedParticles", ints(&[0])),
            ("doNormals", V::I(1)),
            ("simOpIds", ints(&[0])),
            ("simClothPoses", V::A(vec![V::Obj(pose)])),
            (
                "staticConstraintSets",
                V::A(sets.iter().map(|&i| V::Obj(i)).collect()),
            ),
            (
                "collidableTransformMap",
                r(vec![
                    ("transformSetIndex", V::I(0)),
                    ("transformIndices", ints(&[0, 1])),
                    ("offsets", V::A(vec![floats(&[1.0; 16]), floats(&[2.0; 16])])),
                ]),
            ),
            (
                "perInstanceCollidables",
                V::A(vec![V::Obj(collidable_a), V::Obj(collidable_b)]),
            ),
            ("maxParticleRadius", V::F(0.01)),
            ("staticCollisionMasks", ints(&[1, 1, 1])),
            ("totalMass", V::F(6.0)),
            (
                "landscapeCollisionData",
                r(vec![
                    ("landscapeRadius", V::F(0.05)),
                    ("collisionTolerance", V::F(0.2)),
                ]),
            ),
            ("triangleIndices", ints(&[0, 1, 2])),
            ("triangleFlips", ints(&[0])),
            (
                "virtualCollisionPointsData",
                r(vec![
                    ("numVCPoints", V::I(1)),
                    (
                        "triangleBarycentricsDictionary",
                        V::A(vec![r(vec![("u", V::F(0.25)), ("v", V::F(0.5))])]),
                    ),
                    (
                        "triangleDictionaryEntries",
                        V::A(vec![r(vec![
                            ("startingBarycentricIndex", V::I(0)),
                            ("numBarycentrics", V::I(1)),
                        ])]),
                    ),
                    (
                        "triangles",
                        V::A(vec![r(vec![
                            ("oppositeRealParticleIndices", ints(&[1, 2])),
                            ("barycentricDictionaryIndex", V::I(0)),
                        ])]),
                    ),
                    (
                        "triangleFans",
                        V::A(vec![r(vec![
                            ("realParticleIndex", V::I(0)),
                            ("vcpStartIndex", V::I(0)),
                            ("numTriangles", V::I(1)),
                        ])]),
                    ),
                    ("triangleFanIndices", ints(&[0, 65535])),
                ]),
            ),
        ],
    );
    // buffers and the transform set
    let layout = r(vec![("numSlots", V::I(1)), ("triangleFormat", V::I(2))]);
    let mut buffers = vec![put(
        &mut g,
        "hclBufferDefinition",
        vec![
            ("name", s("display")),
            ("numVertices", V::I(4)),
            ("numTriangles", V::I(2)),
            ("bufferLayout", layout.clone()),
        ],
    )];
    buffers.push(put(
        &mut g,
        "hclScratchBufferDefinition",
        vec![
            ("name", s("scratch")),
            ("numVertices", V::I(4)),
            ("bufferLayout", layout.clone()),
            ("triangleIndices", ints(&[0, 1, 2])),
            ("storeNormals", V::I(1)),
        ],
    ));
    buffers.push(put(
        &mut g,
        "hclShadowBufferDefinition",
        vec![
            ("name", s("shadow")),
            ("type", V::I(3)),
            ("numVertices", V::I(4)),
            ("bufferLayout", layout),
            ("shadowPositions", V::I(1)),
        ],
    ));
    let set_def = put(
        &mut g,
        "hclTransformSetDefinition",
        vec![("name", s("bones")), ("numTransforms", V::I(2))],
    );
    // operators
    let access = |b: i64| {
        r(vec![
            ("bufferIndex", V::I(b)),
            ("bufferUsage", r(vec![("perComponentFlags", ints(&[1, 0, 0, 0]))])),
            ("shadowBufferIndex", V::I(b)),
        ])
    };
    let set_access = r(vec![
        ("transformSetIndex", V::I(0)),
        (
            "transformSetUsage",
            r(vec![
                ("perComponentFlags", ints(&[9, 0])),
                (
                    "perComponentTransformTrackers",
                    V::A(vec![r(vec![(
                        "read",
                        r(vec![(
                            "storage",
                            r(vec![("words", ints(&[3])), ("numBits", V::I(2))]),
                        )]),
                    )])]),
                ),
            ]),
        ),
    ]);
    let op = |name: &str, id: i64, mut own: Vec<(&'static str, V)>| {
        let mut m = vec![
            ("name", s(name)),
            ("operatorID", V::I(id)),
            ("usedBuffers", V::A(vec![access(1)])),
            ("usedTransformSets", V::A(vec![set_access.clone()])),
        ];
        m.append(&mut own);
        m
    };
    let mut ops = vec![put(
        &mut g,
        "hclSimulateOperator",
        op(
            "Simulate",
            0,
            vec![(
                "simulateOpConfigs",
                V::A(vec![r(vec![
                    ("name", s("Default Config")),
                    ("constraintExecution", ints(&[0, -1, 1])),
                    ("subSteps", V::I(2)),
                    ("numberOfSolveIterations", V::I(1)),
                    ("useAllInstanceCollidables", V::I(1)),
                ])]),
            )],
        ),
    )];
    ops.push(put(
        &mut g,
        "hclMoveParticlesOperator",
        op(
            "MoveFixedParticles",
            1,
            vec![
                (
                    "vertexParticlePairs",
                    V::A(vec![r(vec![
                        ("vertexIndex", V::I(3)),
                        ("particleIndex", V::I(0)),
                    ])]),
                ),
                ("refBufferIdx", V::I(1)),
            ],
        ),
    ));
    let one_block = r(vec![
        ("vertexIndices", ints(&[0; 16])),
        ("boneIndices", ints(&[1; 16])),
    ]);
    let two_block = r(vec![
        ("vertexIndices", ints(&[1; 16])),
        ("boneIndices", ints(&[0; 32])),
        ("boneWeights", ints(&[255; 32])),
    ]);
    let five_block = r(vec![
        ("vertexIndices", ints(&[2; 16])),
        ("boneIndices", ints(&[1; 80])),
        ("boneWeights", ints(&[65535; 80])),
    ]);
    let deformer = r(vec![
        ("oneBlendEntries", V::A(vec![one_block])),
        ("twoBlendEntries", V::A(vec![two_block])),
        ("fiveBlendEntries", V::A(vec![five_block])),
        ("controlBytes", ints(&[0, 1, 4])),
        ("endVertexIndex", V::I(3)),
    ]);
    let packed_block = |x: i64| V::A(vec![r(vec![("values", ints(&[x, -x, 0, 12288]))]); 16]);
    ops.push(put(
        &mut g,
        "hclObjectSpaceSkinPNOperator",
        op(
            "Skin",
            2,
            vec![
                ("boneFromSkinMeshTransforms", V::A(vec![floats(&[0.5; 16])])),
                ("transformSubset", ints(&[0, 1])),
                ("outputBufferIndex", V::I(1)),
                ("objectSpaceDeformer", deformer.clone()),
                (
                    "localPNs",
                    V::A(vec![r(vec![
                        ("localPosition", packed_block(100)),
                        ("localNormal", packed_block(200)),
                    ])]),
                ),
            ],
        ),
    ));
    let bone_deformer = r(vec![
        (
            "threeBlendEntries",
            V::A(vec![r(vec![
                ("vertexIndices", ints(&[0, 1, 2, 3, 0])),
                ("boneIndices", ints(&[1; 15])),
            ])]),
        ),
        ("controlBytes", ints(&[2])),
    ]);
    let vectors16 = |x: f32| V::A(vec![floats(&[x, 0.0, 0.0, 1.0]); 16]);
    ops.push(put(
        &mut g,
        "hclBoneSpaceSkinPNTOperator",
        op(
            "Skin Display",
            3,
            vec![
                ("transformSubset", ints(&[1])),
                ("outputBufferIndex", V::I(0)),
                ("boneSpaceDeformer", bone_deformer.clone()),
                (
                    "localPNTs",
                    V::A(vec![r(vec![
                        ("localPosition", vectors16(0.5)),
                        ("localNormal", packed_block(3)),
                        ("localTangent", packed_block(4)),
                    ])]),
                ),
                (
                    "localUnpackedPNTs",
                    V::A(vec![r(vec![
                        ("localPosition", vectors16(1.0)),
                        ("localNormal", vectors16(2.0)),
                        ("localTangent", vectors16(3.0)),
                    ])]),
                ),
            ],
        ),
    ));
    ops.push(put(
        &mut g,
        "hclObjectSpaceMeshMeshDeformPOperator",
        op(
            "MeshDeform",
            4,
            vec![
                ("inputBufferIdx", V::I(1)),
                ("outputBufferIdx", V::I(0)),
                ("scaleNormalBehaviour", V::I(1)),
                ("inputTrianglesSubset", ints(&[0])),
                ("triangleFromMeshTransforms", V::A(vec![floats(&[0.25; 16])])),
                ("objectSpaceDeformer", deformer),
                ("customSkinDeform", V::I(1)),
                ("localPs", V::A(vec![r(vec![("localPosition", packed_block(9))])])),
            ],
        ),
    ));
    ops.push(put(
        &mut g,
        "hclBoneSpaceMeshMeshDeformPNOperator",
        op(
            "MeshDeform Bone",
            5,
            vec![
                ("inputBufferIdx", V::I(1)),
                ("outputBufferIdx", V::I(2)),
                ("boneSpaceDeformer", bone_deformer),
                (
                    "localPNs",
                    V::A(vec![r(vec![
                        ("localPosition", vectors16(0.0)),
                        ("localNormal", packed_block(1)),
                    ])]),
                ),
            ],
        ),
    ));
    ops.push(put(
        &mut g,
        "hclSimpleMeshBoneDeformOperator",
        op(
            "MeshBoneDeform Op",
            6,
            vec![
                ("inputBufferIdx", V::I(1)),
                ("outputTransformSetIdx", V::I(0)),
                (
                    "triangleBonePairs",
                    V::A(vec![r(vec![
                        ("boneOffset", V::I(1)),
                        ("triangleOffset", V::I(0)),
                    ])]),
                ),
                ("localBoneTransforms", V::A(vec![floats(&[3.0; 16])])),
            ],
        ),
    ));
    ops.push(put(
        &mut g,
        "hclMeshBoneDeformOperator",
        op(
            "Sim",
            7,
            vec![
                ("inputBufferIdx", V::I(1)),
                (
                    "triangleBonePairs",
                    V::A(vec![r(vec![
                        ("localBoneTransform", floats(&[4.0; 16])),
                        ("weight", V::F(1.0)),
                        ("triangleIndex", V::I(0)),
                    ])]),
                ),
                ("triangleBoneStartForBone", ints(&[0, 1])),
            ],
        ),
    ));
    ops.push(put(
        &mut g,
        "hclCopyVerticesOperator",
        op(
            "Gather",
            8,
            vec![
                ("inputBufferIdx", V::I(1)),
                ("outputBufferIdx", V::I(2)),
                ("numberOfVertices", V::I(3)),
                ("startVertexIn", V::I(1)),
                ("startVertexOut", V::I(0)),
                ("copyNormals", V::I(1)),
            ],
        ),
    ));
    ops.push(put(
        &mut g,
        "hclGatherAllVerticesOperator",
        op(
            "Gather All",
            9,
            vec![
                ("vertexInputFromVertexOutput", ints(&[0, -1, 2, 3])),
                ("inputBufferIdx", V::I(0)),
                ("outputBufferIdx", V::I(1)),
                ("gatherNormals", V::I(1)),
            ],
        ),
    ));
    ops.push(put(
        &mut g,
        "hclGatherSomeVerticesOperator",
        op(
            "Gather Some",
            10,
            vec![
                (
                    "vertexPairs",
                    V::A(vec![r(vec![("indexInput", V::I(3)), ("indexOutput", V::I(0))])]),
                ),
                ("inputBufferIdx", V::I(2)),
                ("outputBufferIdx", V::I(1)),
            ],
        ),
    ));
    let conversion = r(vec![
        (
            "slotConversions",
            V::A(vec![r(vec![
                ("elements", ints(&[0, 1, 255, 255])),
                ("numElements", V::I(2)),
                ("index", V::I(0)),
            ])]),
        ),
        (
            "elementConversions",
            V::A(vec![r(vec![
                ("index", V::I(0)),
                ("offset", V::I(12)),
                ("conversion", V::I(3)),
            ])]),
        ),
        ("numSlotsConverted", V::I(1)),
        ("numElementsConverted", V::I(1)),
    ]);
    ops.push(put(
        &mut g,
        "hclInputConvertOperator",
        op(
            "Convert Input Operator",
            11,
            vec![
                ("userBufferIndex", V::I(0)),
                ("shadowBufferIndex", V::I(2)),
                ("conversionInfo", conversion.clone()),
            ],
        ),
    ));
    ops.push(put(
        &mut g,
        "hclOutputConvertOperator",
        op(
            "Convert Output Operator",
            12,
            vec![
                ("userBufferIndex", V::I(0)),
                ("shadowBufferIndex", V::I(2)),
                ("conversionInfo", conversion),
            ],
        ),
    ));
    ops.push(put(
        &mut g,
        "hclUpdateAllVertexFramesOperator",
        op(
            "UpdateVertexFrames",
            13,
            vec![
                ("vertToNormalID", ints(&[0, 1, 2, 3])),
                ("triangleFlips", ints(&[1])),
                ("referenceVertices", ints(&[0, 1])),
                ("tangentEdgeCosAngle", floats(&[1.0, 0.5])),
                ("tangentEdgeSinAngle", floats(&[0.0, 0.5])),
                ("biTangentFlip", floats(&[1.0, -1.0])),
                ("bufferIdx", V::I(2)),
                ("numUniqueNormalIDs", V::I(4)),
                ("updateNormals", V::I(1)),
            ],
        ),
    ));
    ops.push(put(
        &mut g,
        "hclUpdateSomeVertexFramesOperator",
        op(
            "UpdateVertexFrames Some",
            14,
            vec![
                (
                    "involvedTriangles",
                    V::A(vec![r(vec![("indices", ints(&[0, 1, 2]))])]),
                ),
                ("involvedVertices", ints(&[0, 1, 2])),
                ("selectionVertexToInvolvedVertex", ints(&[0, 1, 2, 65535])),
                ("involvedVertexToNormalID", ints(&[0, 1, 2])),
                ("bufferIdx", V::I(2)),
                ("numUniqueNormalIDs", V::I(3)),
                ("updateTangents", V::I(1)),
            ],
        ),
    ));
    ops.push(put(
        &mut g,
        "hclBlendSomeVerticesOperator",
        op("Blend", 15, Vec::new()),
    ));
    // the state and its graph
    let graph_object = put(
        &mut g,
        "hclStateDependencyGraph",
        vec![
            (
                "branches",
                V::A(vec![r(vec![
                    ("branchId", V::I(0)),
                    ("stateOperatorIndices", ints(&[0, 1])),
                    ("childBranches", ints(&[1])),
                ])]),
            ),
            ("rootBranchIds", ints(&[0])),
            ("children", V::A(vec![ints(&[1]), ints(&[])])),
            ("parents", V::A(vec![ints(&[]), ints(&[0])])),
            ("multiThreadable", V::I(1)),
        ],
    );
    let state = put(
        &mut g,
        "hclClothState",
        vec![
            ("name", s("#01#Default")),
            ("operators", ints(&[11, 2, 1, 0, 6, 13])),
            ("usedBuffers", V::A(vec![access(0), access(1)])),
            ("usedTransformSets", V::A(vec![set_access])),
            ("usedSimCloths", ints(&[0])),
            ("dependencyGraph", V::Obj(graph_object)),
        ],
    );
    let transition = put(
        &mut g,
        "hclStateTransition",
        vec![
            ("name", s("to sim")),
            ("stateIds", ints(&[0])),
            (
                "stateTransitionData",
                V::A(vec![r(vec![
                    (
                        "simClothTransitionData",
                        V::A(vec![r(vec![
                            ("isSimulated", V::I(1)),
                            ("transitionConstraints", V::A(vec![id(6)])),
                            ("transitionType", V::I(2)),
                        ])]),
                    ),
                    ("simulatedState", V::I(1)),
                ])]),
            ),
            ("simClothTransitionConstraints", V::A(vec![V::A(vec![id(6)])])),
        ],
    );
    let action = put(&mut g, "hclSimpleWindAction", vec![("active", V::I(1))]);
    let objects = |list: &[usize]| V::A(list.iter().map(|&i| V::Obj(i)).collect());
    let mut cloth = vec![
        ("name", s("cape")),
        ("simClothDatas", objects(&[sim])),
        ("bufferDefinitions", objects(&buffers)),
        ("transformSetDefinitions", objects(&[set_def])),
        ("operators", objects(&ops)),
        ("clothStateDatas", objects(&[state])),
        ("stateTransitions", objects(&[transition])),
        ("actions", objects(&[action])),
        ("targetPlatform", V::I(2)),
    ];
    if let Some(("hclClothData", member, value)) = change.member {
        cloth.retain(|(m, _)| *m != member);
        cloth.push((member, V::I(value)));
    }
    g[cloth_index].1 = r(cloth);
    g
}

fn read(sdk: &[u8; 8], change: Change) -> Result<cloth::Physics, String> {
    let elden = sdk == ELDEN;
    let bytes = build(sdk, &schema(elden), &graph(elden, change));
    let tag = Tagfile::read(&bytes, None).map_err(|e| format!("tagfile: {e}"))?;
    cloth::read(&tag).map_err(|e| e.to_string())
}

fn change(class: &'static str, member: &'static str, value: i64) -> Change {
    Change {
        member: Some((class, member, value)),
    }
}

#[test]
fn every_class_of_the_cloth_data_reads_in_both_layouts() {
    for sdk in [SEKIRO, ELDEN] {
        let physics = read(sdk, Change::default()).unwrap();
        let cloth = &physics.cloths[0];
        assert_eq!((cloth.name.as_str(), cloth.target_platform), ("cape", 2));
        assert_eq!(cloth.actions, ["hclSimpleWindAction"]);
        assert_eq!(cloth.transform_sets[0].transforms, 2);
        // buffers by class
        assert_eq!(cloth.buffers.len(), 3);
        assert_eq!(cloth.buffers[0].kind, BufferKind::Plain);
        assert_eq!(cloth.buffers[0].layout.triangle_format, 2);
        assert!(matches!(
            &cloth.buffers[1].kind,
            BufferKind::Scratch { triangle_indices, store_normals: true, .. } if triangle_indices == &[0, 1, 2]
        ));
        assert!(matches!(
            cloth.buffers[2].kind,
            BufferKind::Shadow {
                positions: true,
                normals: false,
                ..
            }
        ));
        assert_eq!(cloth.buffers[2].buffer_type, 3);
        // the sim cloth
        let sim = &cloth.sim_cloths[0];
        assert_eq!(sim.gravity, [0.0, -9.8, 0.0, 0.0]);
        assert_eq!(sim.global_damping_per_second, 0.1);
        assert_eq!(sim.particles.len(), 3);
        assert_eq!((sim.particles[2].mass, sim.particles[2].inv_mass), (4.0, 0.25));
        assert_eq!(sim.fixed_particles, [0]);
        assert_eq!(sim.triangle_indices, [0, 1, 2]);
        assert_eq!(sim.poses[0].positions.len(), 3);
        let map = sim.collidable_transform_map.as_ref().unwrap();
        assert_eq!(
            (map.transform_set, map.transform_indices.as_slice()),
            (0, [0, 1].as_slice())
        );
        assert_eq!(map.offsets[1], [2.0; 16]);
        assert_eq!(sim.landscape_collision.collision_tolerance, 0.2);
        assert_eq!(sim.virtual_collision_points.points, 1);
        assert_eq!(sim.virtual_collision_points.triangles[0].opposite, [1, 2]);
        assert_eq!(sim.virtual_collision_points.triangle_fan_indices, [0, 65535]);
        // collidables and shapes
        assert_eq!(
            sim.collidables[0].shape,
            Shape::Sphere {
                pos: [1.0, 2.0, 3.0, 0.25]
            }
        );
        assert!(matches!(
            sim.collidables[1].shape,
            Shape::Capsule { radius: 0.5, .. }
        ));
        assert_eq!(
            (
                sim.collidables[1].pinch_detection_priority,
                sim.collidables[1].user_data
            ),
            (-1, 7)
        );
        // constraint sets: the ids only in Elden Ring's layout
        let ids: Vec<Option<u32>> = sim.constraint_sets.iter().map(|c| c.constraint_id).collect();
        if sdk == ELDEN {
            assert_eq!(
                ids[..8],
                [
                    Some(1),
                    Some(2),
                    Some(3),
                    Some(4),
                    Some(5),
                    Some(6),
                    Some(7),
                    Some(8)
                ]
            );
        } else {
            assert!(ids.iter().all(Option::is_none));
        }
        let kinds: Vec<&ConstraintKind> = sim.constraint_sets.iter().map(|c| &c.kind).collect();
        assert_eq!(
            kinds[0],
            &ConstraintKind::StandardLink(vec![
                Link {
                    particle_a: 0,
                    particle_b: 1,
                    rest_length: 0.5,
                    stiffness: 0.75
                },
                Link {
                    particle_a: 1,
                    particle_b: 2,
                    rest_length: 0.5,
                    stiffness: 0.75
                },
            ])
        );
        assert!(matches!(kinds[1], ConstraintKind::StretchLink(l) if l.len() == 1));
        assert!(matches!(kinds[2], ConstraintKind::CompressibleLink(l) if l[0].compression_length == 0.5));
        assert!(matches!(kinds[3], ConstraintKind::BendLink(l) if l[0].stretch_max_length == 1.1));
        assert!(matches!(
            kinds[4],
            ConstraintKind::BendStiffness { links, clamp_bend_stiffness: true, use_rest_pose_config: false, .. }
                if links[0].particles == [0, 1, 2, 1] && links[0].weights == [0.1, 0.2, 0.3, 0.4]
        ));
        assert!(matches!(
            kinds[5],
            ConstraintKind::LocalRange { shape_type: LocalRangeShape::Cylinder, reference_mesh_buffer: 1, constraints, .. }
                if constraints[0].reference_vertex == 3
        ));
        assert!(matches!(
            kinds[6],
            ConstraintKind::Transition { particles, to_sim_period, .. } if particles[0].to_sim_delay == 0.06 && *to_sim_period == 0.1
        ));
        assert!(matches!(
            kinds[7],
            ConstraintKind::VolumeMx { frame_batches, apply_singles, frame_singles, apply_batches }
                if frame_batches[0].particles == [2; 16] && apply_singles[0].particle == 1
                    && frame_singles.is_empty() && apply_batches.is_empty()
        ));
        assert_eq!(
            kinds[8],
            &ConstraintKind::Other {
                class: "hclBendStiffnessExperimentalSet".into()
            }
        );
        // operators
        let op = |i: usize| &cloth.operators[i].kind;
        assert!(matches!(
            op(0),
            OperatorKind::Simulate { sim_cloth: 0, configs } if configs[0].constraint_execution == [0, -1, 1] && configs[0].sub_steps == 2
        ));
        assert!(
            matches!(op(1), OperatorKind::MoveParticles { pairs, reference_buffer: 1, .. } if pairs == &[(3, 0)])
        );
        let OperatorKind::ObjectSpaceSkin {
            components,
            deformer,
            local,
            local_unpacked,
            transform_subset,
            ..
        } = op(2)
        else {
            panic!("operator 2: {:?}", op(2));
        };
        assert_eq!(*components, Components::PN);
        assert_eq!(transform_subset, &[0, 1]);
        assert_eq!(deformer.blocks[0][0].bones, [1; 16]);
        assert!(deformer.blocks[0][0].weights.is_empty());
        assert_eq!(deformer.blocks[1][0].weights, [255; 32]);
        assert_eq!(deformer.blocks[4][0].weights, [65535; 80]);
        assert!(deformer.blocks[7].is_empty());
        assert_eq!(deformer.control_bytes, [0, 1, 4]);
        assert_eq!(local[0].positions[0], [100, -100, 0, 12288]);
        assert_eq!(local[0].normals.unwrap()[15], [200, -200, 0, 12288]);
        assert!(local[0].tangents.is_none());
        assert!(local_unpacked.is_empty());
        let OperatorKind::BoneSpaceSkin {
            components,
            deformer,
            local,
            local_unpacked,
            ..
        } = op(3)
        else {
            panic!("operator 3: {:?}", op(3));
        };
        assert_eq!(*components, Components::PNT);
        assert_eq!(deformer.blocks[2][0].vertices, [0, 1, 2, 3, 0]);
        assert_eq!(local[0].positions[0], [0.5, 0.0, 0.0, 1.0]);
        assert_eq!(local[0].tangents.unwrap()[0], [4, -4, 0, 12288]);
        assert_eq!(local_unpacked[0].tangents.unwrap()[0], [3.0, 0.0, 0.0, 1.0]);
        assert!(matches!(
            op(4),
            OperatorKind::ObjectSpaceMeshMeshDeform { components: Components::P, custom_skin_deform: true, local, .. }
                if local[0].normals.is_none()
        ));
        assert!(matches!(
            op(5),
            OperatorKind::BoneSpaceMeshMeshDeform {
                components: Components::PN,
                output_buffer: 2,
                ..
            }
        ));
        assert!(matches!(
            op(6),
            OperatorKind::SimpleMeshBoneDeform { triangle_bone_pairs, local_bone_transforms, .. }
                if triangle_bone_pairs == &[(1, 0)] && local_bone_transforms[0] == [3.0; 16]
        ));
        assert!(matches!(
            op(7),
            OperatorKind::MeshBoneDeform { triangle_bone_pairs, triangle_bone_start_for_bone, .. }
                if triangle_bone_pairs[0].weight == 1.0 && triangle_bone_start_for_bone == &[0, 1]
        ));
        assert!(matches!(
            op(8),
            OperatorKind::CopyVertices {
                vertices: 3,
                start_in: 1,
                copy_normals: true,
                ..
            }
        ));
        assert!(
            matches!(op(9), OperatorKind::GatherAllVertices { input_from_output, .. } if input_from_output == &[0, -1, 2, 3])
        );
        assert!(matches!(op(10), OperatorKind::GatherSomeVertices { pairs, .. } if pairs == &[(3, 0)]));
        assert!(matches!(
            op(11),
            OperatorKind::InputConvert { conversion, shadow_buffer: 2, .. } if conversion.elements[0].offset == 12 && conversion.slots[0].elements == [0, 1, 255, 255]
        ));
        assert!(matches!(op(12), OperatorKind::OutputConvert { .. }));
        assert!(matches!(
            op(13),
            OperatorKind::UpdateAllVertexFrames { bitangent_flip, update_normals: true, update_tangents: false, .. } if bitangent_flip == &[1.0, -1.0]
        ));
        assert!(matches!(
            op(14),
            OperatorKind::UpdateSomeVertexFrames { involved_triangles, update_tangents: true, .. } if involved_triangles == &[[0, 1, 2]]
        ));
        assert_eq!(
            op(15),
            &OperatorKind::Other {
                class: "hclBlendSomeVerticesOperator".into()
            }
        );
        let usage = &cloth.operators[0].used_transform_sets[0].usage;
        assert_eq!(
            (usage.per_component_flags, usage.trackers[0].read.words.as_slice()),
            ([9, 0], [3].as_slice())
        );
        // the state, its graph and the transition
        let state = &cloth.states[0];
        assert_eq!(state.operators, [11, 2, 1, 0, 6, 13]);
        let graph = state.dependency_graph.as_ref().unwrap();
        assert_eq!(
            (
                graph.branches[0].state_operators.as_slice(),
                graph.multi_threadable
            ),
            ([0, 1].as_slice(), true)
        );
        assert_eq!(graph.parents, [vec![], vec![0]]);
        let transition = &cloth.state_transitions[0];
        assert_eq!(transition.data[0].sim_cloths[0].constraints, [6]);
        assert_eq!(transition.sim_cloth_transition_constraints, [vec![6]]);
    }
}

#[test]
fn local_range_shape_zero_is_a_sphere_and_others_are_refused() {
    let sphere = read(ELDEN, change("hclLocalRangeConstraintSet", "shapeType", 0)).unwrap();
    assert!(matches!(
        sphere.cloths[0].sim_cloths[0].constraint_sets[5].kind,
        ConstraintKind::LocalRange {
            shape_type: LocalRangeShape::Sphere,
            ..
        }
    ));
    let e = read(ELDEN, change("hclLocalRangeConstraintSet", "shapeType", 2)).unwrap_err();
    assert!(e.contains("shape type 2"), "{e}");
}

/// `graph` with member `member` of the first object of class `class` set to `value`.
fn with(class: &'static str, member: &'static str, value: V) -> Graph {
    let mut g = graph(false, Change::default());
    let object = g.iter().position(|(c, _)| *c == class).unwrap();
    let V::R(fields) = &mut g[object].1 else {
        panic!("{class} is not a record");
    };
    fields.retain(|(m, _)| *m != member);
    fields.push((member, value));
    g
}

fn read_graph(g: &Graph) -> Result<cloth::Physics, String> {
    let bytes = build(SEKIRO, &schema(false), g);
    let tag = Tagfile::read(&bytes, None).map_err(|e| format!("tagfile: {e}"))?;
    cloth::read(&tag).map_err(|e| e.to_string())
}

#[test]
fn a_collidable_transform_set_of_minus_one_means_no_map() {
    let g = with(
        "hclSimClothData",
        "collidableTransformMap",
        r(vec![("transformSetIndex", V::I(-1))]),
    );
    assert_eq!(
        read_graph(&g).unwrap().cloths[0].sim_cloths[0].collidable_transform_map,
        None
    );
    // a map must give each collidable a bone and an offset
    let g = with(
        "hclSimClothData",
        "collidableTransformMap",
        r(vec![
            ("transformSetIndex", V::I(0)),
            ("transformIndices", ints(&[0])),
            ("offsets", V::A(vec![floats(&[1.0; 16])])),
        ]),
    );
    let e = read_graph(&g).unwrap_err();
    assert!(e.contains("2 collidables, 1 transform indices, 1 offsets"), "{e}");
}

#[test]
fn particle_indices_past_the_particles_are_refused() {
    let link = |b: i64| V::A(vec![r(vec![("particleA", V::I(0)), ("particleB", V::I(b))])]);
    for (class, member, value) in [
        ("hclSimClothData", "fixedParticles", ints(&[3])),
        ("hclSimClothData", "triangleIndices", ints(&[0, 1, 3])),
        ("hclStandardLinkConstraintSet", "links", link(3)),
        ("hclStretchLinkConstraintSet", "links", link(3)),
        ("hclCompressibleLinkConstraintSet", "links", link(3)),
        ("hclBendLinkConstraintSet", "links", link(3)),
        (
            "hclBendStiffnessConstraintSet",
            "links",
            V::A(vec![r(vec![("particleD", V::I(3))])]),
        ),
        (
            "hclLocalRangeConstraintSet",
            "localConstraints",
            V::A(vec![r(vec![("particleIndex", V::I(3))])]),
        ),
        (
            "hclTransitionConstraintSet",
            "perParticleData",
            V::A(vec![r(vec![("particleIndex", V::I(3))])]),
        ),
        (
            "hclVolumeConstraintMx",
            "frameBatchDatas",
            V::A(vec![r(vec![("particleIndex", ints(&[0, 1, 2, 3]))])]),
        ),
        (
            "hclVolumeConstraintMx",
            "applySingleDatas",
            V::A(vec![r(vec![("particleIndex", V::I(3))])]),
        ),
    ] {
        let e = read_graph(&with(class, member, value)).unwrap_err();
        assert!(
            e.contains("particle index 3 of a sim cloth with 3 particles"),
            "{class}.{member}: {e}"
        );
    }
    // the virtual collision points and a move particles operator name particles too
    let vcp = |member: &'static str, value: V| r(vec![(member, value)]);
    for value in [
        vcp(
            "triangleFans",
            V::A(vec![r(vec![("realParticleIndex", V::I(3))])]),
        ),
        vcp(
            "triangles",
            V::A(vec![r(vec![("oppositeRealParticleIndices", ints(&[0, 3]))])]),
        ),
        vcp(
            "edges",
            V::A(vec![r(vec![("oppositeRealParticleIndex", V::I(3))])]),
        ),
        vcp("edgeFans", V::A(vec![r(vec![("realParticleIndex", V::I(3))])])),
    ] {
        let e = read_graph(&with("hclSimClothData", "virtualCollisionPointsData", value)).unwrap_err();
        assert!(
            e.contains("particle index 3 of a sim cloth with 3 particles"),
            "{e}"
        );
    }
    let e = read_graph(&with(
        "hclMoveParticlesOperator",
        "vertexParticlePairs",
        V::A(vec![r(vec![
            ("vertexIndex", V::I(0)),
            ("particleIndex", V::I(3)),
        ])]),
    ))
    .unwrap_err();
    assert!(e.contains("move particles particle 3 of 3"), "{e}");
}

#[test]
fn particle_indices_of_another_width_are_refused() {
    let mut schema = schema(false);
    for member in schema.get_mut("hclSimClothData").unwrap() {
        if member.0 == "fixedParticles" {
            member.1 = arr(Ty::U32);
        }
    }
    let bytes = build(SEKIRO, &schema, &graph(false, Change::default()));
    let tag = Tagfile::read(&bytes, None).unwrap();
    let e = cloth::read(&tag).unwrap_err().to_string();
    assert!(e.contains("16-bit index") && e.contains("32-bit"), "{e}");
}

#[test]
fn buffer_vertices_past_the_buffer_are_refused() {
    for (class, member, value, expected) in [
        (
            "hclMoveParticlesOperator",
            "vertexParticlePairs",
            V::A(vec![r(vec![
                ("vertexIndex", V::I(4)),
                ("particleIndex", V::I(0)),
            ])]),
            "move particles vertex 4 of 4",
        ),
        (
            "hclLocalRangeConstraintSet",
            "localConstraints",
            V::A(vec![r(vec![
                ("particleIndex", V::I(0)),
                ("referenceVertex", V::I(4)),
            ])]),
            "reference vertex 4 of 4",
        ),
        (
            "hclTransitionConstraintSet",
            "perParticleData",
            V::A(vec![r(vec![
                ("particleIndex", V::I(0)),
                ("referenceVertex", V::I(9)),
            ])]),
            "reference vertex 9 of 4",
        ),
    ] {
        let e = read_graph(&with(class, member, value)).unwrap_err();
        assert!(e.contains(expected), "{class}.{member}: {e}");
    }
    // three vertices from vertex 1 of a four-vertex buffer fit; from vertex 2 they do not
    let e = read(SEKIRO, change("hclCopyVerticesOperator", "startVertexIn", 2)).unwrap_err();
    assert!(
        e.contains("copy operator Gather") && e.contains("past a buffer"),
        "{e}"
    );
}

#[test]
fn state_and_operator_indices_past_their_lists_are_refused() {
    for (class, member, value, expected) in [
        (
            "hclClothState",
            "operators",
            ints(&[16]),
            "state operator 16 of 16",
        ),
        (
            "hclClothState",
            "usedSimCloths",
            ints(&[1]),
            "state sim cloth 1 of 1",
        ),
        (
            "hclSimClothData",
            "simOpIds",
            ints(&[16]),
            "sim cloth operator 16 of 16",
        ),
        (
            "hclSimulateOperator",
            "simulateOpConfigs",
            V::A(vec![r(vec![("constraintExecution", ints(&[-1, 9]))])]),
            "constraint execution index 9 of 9",
        ),
    ] {
        let e = read_graph(&with(class, member, value)).unwrap_err();
        assert!(e.contains(expected), "{class}.{member}: {e}");
    }
}

#[test]
fn a_sim_cloth_stores_whole_triangles() {
    let e = read_graph(&with("hclSimClothData", "triangleIndices", ints(&[0, 1, 2, 0]))).unwrap_err();
    assert!(e.contains("4 triangle indices"), "{e}");
}

#[test]
fn indices_into_the_cloth_datas_lists_are_refused_when_out_of_range() {
    for (class, member, value, expected) in [
        (
            "hclSimulateOperator",
            "simClothIndex",
            1,
            "simulate operator sim cloth 1 of 1",
        ),
        (
            "hclMoveParticlesOperator",
            "simClothIndex",
            3,
            "move particles sim cloth 3 of 1",
        ),
        (
            "hclMoveParticlesOperator",
            "refBufferIdx",
            3,
            "move particles reference buffer 3 of 3",
        ),
        (
            "hclObjectSpaceSkinPNOperator",
            "outputBufferIndex",
            9,
            "skin output buffer 9 of 3",
        ),
        (
            "hclBoneSpaceSkinPNTOperator",
            "transformSetIndex",
            1,
            "skin transform set 1 of 1",
        ),
        (
            "hclObjectSpaceMeshMeshDeformPOperator",
            "inputBufferIdx",
            3,
            "mesh deform input buffer 3 of 3",
        ),
        (
            "hclBoneSpaceMeshMeshDeformPNOperator",
            "outputBufferIdx",
            4,
            "mesh deform output buffer 4 of 3",
        ),
        (
            "hclSimpleMeshBoneDeformOperator",
            "outputTransformSetIdx",
            2,
            "mesh bone deform transform set 2 of 1",
        ),
        (
            "hclMeshBoneDeformOperator",
            "inputBufferIdx",
            5,
            "mesh bone deform input buffer 5 of 3",
        ),
        (
            "hclCopyVerticesOperator",
            "outputBufferIdx",
            3,
            "copy output buffer 3 of 3",
        ),
        (
            "hclGatherAllVerticesOperator",
            "inputBufferIdx",
            3,
            "gather input buffer 3 of 3",
        ),
        (
            "hclGatherSomeVerticesOperator",
            "outputBufferIdx",
            7,
            "gather output buffer 7 of 3",
        ),
        (
            "hclInputConvertOperator",
            "shadowBufferIndex",
            3,
            "convert shadow buffer 3 of 3",
        ),
        (
            "hclOutputConvertOperator",
            "userBufferIndex",
            3,
            "convert user buffer 3 of 3",
        ),
        (
            "hclUpdateAllVertexFramesOperator",
            "bufferIdx",
            3,
            "vertex frames buffer 3 of 3",
        ),
        (
            "hclUpdateSomeVertexFramesOperator",
            "bufferIdx",
            3,
            "vertex frames buffer 3 of 3",
        ),
        (
            "hclLocalRangeConstraintSet",
            "referenceMeshBufferIdx",
            3,
            "constraint set reference buffer 3 of 3",
        ),
        (
            "hclTransitionConstraintSet",
            "referenceMeshBufferIdx",
            4,
            "constraint set reference buffer 4 of 3",
        ),
    ] {
        let e = read(ELDEN, change(class, member, value)).unwrap_err();
        assert!(e.contains(expected), "{class}.{member}: {e}");
    }
}

/// A ragdoll (skeleton with no bones) and a constraint joining two bodies, beside no cloth.
fn ragdoll_graph() -> (Schema, Graph) {
    let mut c = Schema::new();
    let empty = || arr(Ty::U32);
    class(
        &mut c,
        "hkaSkeleton",
        vec![
            ("name", Ty::Str),
            ("parentIndices", empty()),
            ("bones", empty()),
            ("referencePose", empty()),
            ("referenceFloats", empty()),
            ("floatSlots", empty()),
        ],
    );
    class(
        &mut c,
        "hknpRagdollData",
        vec![
            ("skeleton", Ty::Ptr("hkaSkeleton")),
            ("boneToBodyMap", arr(Ty::I32)),
            ("bodyTags", arr(Ty::U32)),
        ],
    );
    class(&mut c, "hknpBodyId", vec![("serialAndIndex", Ty::U32)]);
    class(&mut c, "hknpConstraintId", vec![("value", Ty::U32)]);
    class(&mut c, "hknpConstraintGroupId", vec![("value", Ty::U32)]);
    class(&mut c, "hkpRagdollConstraintData", Vec::new());
    class(
        &mut c,
        "hknpConstraintCinfo",
        vec![
            ("constraintData", Ty::Ptr("hkpRagdollConstraintData")),
            ("bodyA", Ty::Class("hknpBodyId")),
            ("bodyB", Ty::U32),
            ("flags", Ty::U32),
            ("name", Ty::Str),
            ("desiredConstraintId", Ty::Class("hknpConstraintId")),
            ("constraintGroupId", Ty::Class("hknpConstraintGroupId")),
        ],
    );
    let g: Graph = vec![
        (
            "hknpRagdollData",
            r(vec![("skeleton", V::Obj(1)), ("bodyTags", ints(&[17, 23]))]),
        ),
        ("hkaSkeleton", r(vec![("name", s("ragdoll"))])),
        (
            "hknpConstraintCinfo",
            r(vec![
                ("constraintData", V::Obj(3)),
                ("bodyA", r(vec![("serialAndIndex", V::I(0x0102_0304))])),
                ("bodyB", V::I(12)),
                ("flags", V::I(5)),
                ("name", s("knee")),
                ("desiredConstraintId", r(vec![("value", V::I(99))])),
                ("constraintGroupId", r(vec![("value", V::I(7))])),
            ]),
        ),
        ("hkpRagdollConstraintData", r(Vec::new())),
    ];
    (c, g)
}

#[test]
fn ragdoll_data_retains_its_skeleton_and_body_tags() {
    let (schema, g) = ragdoll_graph();
    for sdk in [SEKIRO, ELDEN] {
        let bytes = build(sdk, &schema, &g);
        let physics = cloth::read(&Tagfile::read(&bytes, None).unwrap()).unwrap();
        assert!(physics.cloths.is_empty());
        let ragdoll = &physics.ragdolls[0];
        assert!(ragdoll.skeleton.bones.is_empty());
        assert!(ragdoll.bone_to_body.is_empty());
        assert_eq!(ragdoll.body_tags, [17, 23]);
    }
}

#[test]
fn constraint_cinfo_reads_record_handles_and_primitive_ids() {
    let (schema, g) = ragdoll_graph();
    let bytes = build(ELDEN, &schema, &g);
    let physics = cloth::read(&Tagfile::read(&bytes, None).unwrap()).unwrap();
    let constraint = &physics.constraints[0];
    assert_eq!((constraint.body_a, constraint.body_b), (0x0102_0304, 12));
    assert_eq!(
        (constraint.desired_constraint_id, constraint.constraint_group_id),
        (99, 7)
    );
    assert_eq!((constraint.flags, constraint.name.as_str()), (5, "knee"));
    assert_eq!(constraint.data_class, "hkpRagdollConstraintData");
}

#[test]
fn a_ragdoll_whose_body_map_does_not_match_its_bones_is_refused() {
    let (schema, mut g) = ragdoll_graph();
    g[0].1 = r(vec![("skeleton", V::Obj(1)), ("boneToBodyMap", ints(&[0]))]);
    let bytes = build(SEKIRO, &schema, &g);
    let e = cloth::read(&Tagfile::read(&bytes, None).unwrap())
        .unwrap_err()
        .to_string();
    assert!(e.contains("0 bones, 1 body map entries"), "{e}");
}
