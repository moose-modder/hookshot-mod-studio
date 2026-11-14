//! Havok 7.1.0-r1 physics class views over a [`Packfile`] (4-byte pointers, big endian). The level packfiles carry no
//! reflection (`__types__` is empty in 482 of the 490; the eight ragdoll files carry theirs, and every offset below that
//! they cover is the one [`super::reflection`] reads from them), so every member offset below is stated from the 7.1 member order (`hkVector4`
//! members 16-aligned) and **confirmed on the data**: the last column names the check that held on all 490 SHRED files
//! (12,789 bodies, 1,758 extended meshes, 6,445 subparts, 646,081 triangles, 8,780 convex shapes). The member lists
//! are the ones whose class signatures every file stores ([`super::CLASS_SIGNATURES`]; [`Packfile::read`] refuses a
//! class whose signature differs). A layout that does not hold is an error naming the offset, never a guess.
//!
//! | class | member | offset | confirmed by |
//! |---|---|---|---|
//! | hkpRigidBody | shape pointer | `+0x10` | never null; its class is a known shape |
//! | | shape key | `+0x14` | `0xFFFFFFFF` in every body |
//! | | broad-phase handle type (u8) | `+0x28` | 1 ([`BroadPhaseType::Entity`]) in all 12,789 bodies |
//! | | collisionFilterInfo (of the collidable's broad-phase handle) | `+0x2C` | 0, 2, 7, 11, 15 or 18 in the data; layers only ([`FilterInfo`]) |
//! | | quality type (i8) | `+0x2A` | 0 in the fixed bodies, 4 in the dynamic ones |
//! | | allowed penetration depth | `+0x5C` | `0x7F7FFFEE` in the fixed bodies, 0.015 to 0.635 in the dynamic ones |
//! | | name pointer | `+0x74` | C string in every body |
//! | | material: response type (u8), friction, restitution | `+0x8C`, `+0x90`, `+0x94` | type 1 ([`ResponseType::SimpleContact`]) in every body; finite |
//! | | storageIndex (u16), contactPointCallbackDelay (u16) | `+0xA8`, `+0xAA` | 540 distinct indices from 0; delay `0xFFFF` in every body |
//! | | motion type (u8) | `+0xE8` | 2, 3 or 5 ([`MotionType`]: sphere inertia, box inertia, fixed); fixed = all 1,758 meshes |
//! | | swept transform: centres of mass 0 and 1 (xyz), rotations 0 and 1 | `+0x130`, `+0x140`; `+0x150`, `+0x160` | equal pairs, in all 12,789 bodies |
//! | | centre of mass in the body's frame | `+0x170` | |
//! | | objectRadius, linear and angular damping | `+0x190`, `+0x194`, `+0x198` | |
//! | | inertiaAndMassInv: inverse inertia xyz, inverse mass w | `+0x1A0`, `+0x1AC` | all zero in the 2,021 fixed bodies, all positive in the 10,768 dynamic ones |
//! | | linear and angular velocity | `+0x1B0`, `+0x1C0` | zero (16 bytes each) |
//! | | gravityFactor (half) | `+0x1FE` | 1.0 in every body |
//! | | rotation, three columns of 16 bytes (xyz used) | `+0xF0` | orthonormal, det +1 in every body |
//! | | translation | `+0x120` | equals the swept centre `+0x130` unless the body has a centre-of-mass offset |
//! | hkpMoppBvTreeShape | child shape pointer | `+0x34` | a mesh (1,704) or a list (1) |
//! | hkpStorageExtendedMeshShape | embeddedTrianglesSubpart (96 B) | `+0x20` | a copy of subpart 0 but for its triangle offset (`+0x4C`, unread) |
//! | | scaling | `+0x80` | `(1, 1, 1, 1)` in every mesh |
//! | | aabb half extents, centre | `+0x90`, `+0xA0` | every vertex inside (tolerance 0.06) |
//! | | numBitsForSubpartIndex | `+0xB0` | 12 |
//! | | trianglesSubparts (hkArray, 96-byte elements) | `+0xB4` | |
//! | | weldingInfo (hkArray of u16) | `+0xCC` | length = total triangles |
//! | | weldingType (u8), defaultCollisionFilterInfo, cachedNumChildShapes, triangleRadius | `+0xD8`, `+0xDC`, `+0xE0`, `+0xE4` | 0 ([`WeldingType::Anticlockwise`]); 0; at most the triangle count (equal in 1,277 of 1,758); 0.001. `+0xD9..+0xDC` and `+0xE8..+0xF0` are zero |
//! | | shape storage (hkArray) | `+0xFC` | empty in every mesh; the body is 0x110 bytes |
//! | | meshStorage (hkArray of pointers) | `+0xF0` | length = subparts; each a MeshSubpartStorage |
//! | TrianglesSubpart (96 B) | type (u8), materialIndexStridingType (u8), materialStriding (i16) | `+0x00`, `+0x01`, `+0x02` | 0 ([`SubpartType::Triangles`]), 2 ([`MaterialIndexStridingType::Int16`]), 12 |
//! | | materialIndexStriding (u16), numMaterials (u16) | `+0x08`, `+0x0A` | 2; = storage material count |
//! | | triangle count, vertex striding, vertex count | `+0x14`, `+0x1C`, `+0x20` | = storage; 16 |
//! | | extrusion (vec4) | `+0x30` | zero |
//! | | index striding, striding type (u8), flipAlternateTriangles (i8) | `+0x44`, `+0x48`, `+0x49` | 16 bytes (4 u32 per triangle); 3 ([`IndexStridingType::Int32`]); 0 |
//! | | triangle offset | `+0x4C` | cumulative triangles of the earlier subparts |
//! | | userData | `+0x50` | zero |
//! | | the pointers materialIndexBase, materialBase, materialClass, vertexBase, indexBase | `+0x04`, `+0x0C`, `+0x10`, `+0x18`, `+0x40` | zero (the storage holds the arrays) |
//! | | every other byte (padding) | | zero |
//! | MeshSubpartStorage | vertices (hkVector4) | `+0x08` | count = subpart; w ignored |
//! | | indices8, indices16 | `+0x14`, `+0x20` | empty in every subpart |
//! | | indices32 | `+0x2C` | 4 per triangle; every index < vertex count; the 4th is 0 |
//! | | materialIndices8 | `+0x38` | empty |
//! | | materials (12-byte records: filterInfo u32, restitution and friction as halves, userData) | `+0x44` | halves finite (restitution 0, friction 1 in all 7,909 records) |
//! | | namedMaterials | `+0x50` | empty |
//! | | materialIndices16 | `+0x5C` | one per triangle, each < the material count |
//! | hkpConvexVerticesShape | radius, aabb half extents, centre | `+0x10`, `+0x20`, `+0x30` | every vertex inside the aabb |
//! | | rotatedVertices (FourVectors, 48 B: `x[4]`, `y[4]`, `z[4]`) | `+0x40` | |
//! | | vertex count | `+0x4C` | fits the FourVectors (`4 n-3 ..= 4 n`) |
//! | | a pointer slot | `+0x50` | zero with no fixup in all 8,780 shapes |
//! | | planeEquations (16 B) | `+0x54` | stored, **not exact hull faces**: normals are unit in 8,550 shapes, one other length in 221, mixed in 9; 222 shapes have a hull vertex outside their planes |
//! | | connectivity pointer | `+0x60` | an `hkpConvexVerticesConnectivity` (8,751; null in 29); `+0x64..+0x70` zero, the shape is 0x70 bytes |
//! | hkpBoxShape | radius, half extents | `+0x10`, `+0x20` | |
//! | hkpSphereShape | radius | `+0x10` | |
//! | hkpCapsuleShape | radius, vertex A, vertex B | `+0x10`, `+0x20`, `+0x30` | the w lanes `+0x2C`, `+0x3C` equal the radius in all 96 |
//! | hkpCylinderShape | radius, cylinder radius, base-radius factor, A, B | `+0x10`, `+0x14`, `+0x18`, `+0x20`, `+0x30` | |
//! | hkpConvexTranslateShape | child, translation | `+0x18`, `+0x20` | a box (1,384) or sphere (16) |
//! | hkpListShape | childInfo (hkArray of 16-byte records: shape pointer, collisionFilterInfo, two zero words) | `+0x18` | 41 records in 10 lists; the filter word is 0 in all |
//! | hkpMoppBvTreeShape | code pointer | `+0x14` | an `hkpMoppCode` |
//! | hkpMoppCode | `info.offset` (xyz), scale (w) | `+0x10` | offset = the mesh's minimum corner (1,704 codes over a mesh) |
//! | | code (hkArray of bytes) | `+0x20` | its data follows the 0x30-byte body, the object is exactly `0x30 + length` bytes (1,705) |
//! | | buildType (i8) | `+0x2C` | 0 ([`MoppBuildType::BuiltWithChunkSubdivision`]); `+0x00..+0x10` and `+0x2D..+0x30` are zero |
//! | hkpConvexVerticesConnectivity | vertexIndices (hkArray of u16), numVerticesPerFace (hkArray of u8) | `+0x08`, `+0x14` | the counts add up to the index count and every index names a vertex (8,751) |
//! | every shape | userData | `+0x08` | 0 but in 583 convex-vertices shapes of box-inertia bodies and 45 extended meshes (1) |
//! | | shape type | `+0x0C` | 0 in all 15,935 shapes ([`shape_user_data`]): the metadata of the eight reflection-bearing files marks the member `SERIALIZE_IGNORED`, so no value is stored |
//!
//! Every array header has the capacity word `size | 0x80000000` (an empty array has no fixup) and every pointer slot
//! holds zero whether or not it has a fixup; see [`Packfile::array_header`] and [`Packfile::pointer`].
//!
//! The constraint classes, the physics data and the systems that hold the bodies are read by
//! [`crate::constraint::packfile`]. A body's collision filter word splits as [`FilterInfo`] says.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use std::collections::BTreeMap;

use super::Packfile;
use crate::Result;
use crate::enums::hkp::{
    BroadPhaseType, IndexStridingType, MaterialIndexStridingType, MoppBuildType, MotionType, ResponseType,
    SubpartType, WeldingType,
};

/// Classes decoded by this module.
pub const DECODED: [&str; 20] = [
    "hkpRigidBody",
    "hkpMoppBvTreeShape",
    "hkpMoppCode",
    "hkpStorageExtendedMeshShape",
    "hkpStorageExtendedMeshShapeMeshSubpartStorage",
    "hkpConvexVerticesShape",
    "hkpConvexVerticesConnectivity",
    "hkpBoxShape",
    "hkpSphereShape",
    "hkpCapsuleShape",
    "hkpCylinderShape",
    "hkpListShape",
    "hkpConvexTranslateShape",
    "hkpConstraintInstance",
    "hkpRagdollConstraintData",
    "hkpLimitedHingeConstraintData",
    "hkpBreakableConstraintData",
    "hkRootLevelContainer",
    "hkpPhysicsData",
    "hkpPhysicsSystem",
];
/// Classes recognised by name and reported, not decoded: none is left in the corpus (every class of the 490 files has
/// a view).
pub const RECOGNISED: [&str; 0] = [];

/// What the objects of a packfile are: counts per class, split by what this module does with them.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Census {
    /// Classes with a view here.
    pub decoded: BTreeMap<String, usize>,
    /// Classes known by name, with no view.
    pub recognised: BTreeMap<String, usize>,
    /// Classes this module has never seen: nothing is read from them.
    pub unknown: BTreeMap<String, usize>,
}

/// Count every object of `pack` by class (the report of what was and was not read).
pub fn census(pack: &Packfile<'_>) -> Census {
    let mut out = Census::default();
    for (name, n) in pack.census() {
        let bucket = if DECODED.contains(&name) {
            &mut out.decoded
        } else if RECOGNISED.contains(&name) {
            &mut out.recognised
        } else {
            &mut out.unknown
        };
        bucket.insert(name.to_string(), n);
    }
    out
}

/// Offsets of every `hkpRigidBody`, in data order.
pub fn rigid_bodies(pack: &Packfile<'_>) -> Vec<usize> {
    pack.objects_of("hkpRigidBody").collect()
}

/// `hkpMotion::MotionType` 0: invalid. The values are 7.1's (the enum is part of the `hkpRigidBody` signature the
/// files store); other Havok versions and other engines number motions differently. The data holds only 2, 3 and 5,
/// and [`RigidBody::read`] accepts only those. The enumeration with its names is [`MotionType`]; these constants stay
/// for the `u8` the body stores.
pub const MOTION_INVALID: u8 = 0;
/// Motion type 1: dynamic.
pub const MOTION_DYNAMIC: u8 = 1;
/// Motion type 2: sphere inertia.
pub const MOTION_SPHERE_INERTIA: u8 = 2;
/// Motion type 3: box inertia.
pub const MOTION_BOX_INERTIA: u8 = 3;
/// Motion type 4: keyframed.
pub const MOTION_KEYFRAMED: u8 = 4;
/// Motion type 5: fixed.
pub const MOTION_FIXED: u8 = 5;
/// Motion type 6: thin box inertia.
pub const MOTION_THIN_BOX_INERTIA: u8 = 6;
/// Motion type 7: character.
pub const MOTION_CHARACTER: u8 = 7;

/// Fail unless the `len` bytes at `offset` are zero.
fn zero(pack: &Packfile<'_>, offset: usize, len: usize, what: &str) -> Result<()> {
    if pack.bytes(offset, len)?.iter().any(|&b| b != 0) {
        Err(pack.err(offset, format!("{what} is not zero")))
    } else {
        Ok(())
    }
}

/// The `userData` word (`+0x08`) of the shape object at `offset`, whatever its class: 1 in 583 convex-vertices shapes
/// (all on box-inertia bodies) and 45 extended meshes, 0 in the other 15,307 shapes. The word at `+0x0C` (the shape type)
/// is 0 in all 15,935 shapes and is refused otherwise.
pub fn shape_user_data(pack: &Packfile<'_>, offset: usize) -> Result<u32> {
    pack.require_710()?;
    if pack.u32(pack.at(offset, 0x0C)?)? != 0 {
        return Err(pack.err(pack.at(offset, 0x0C)?, "the shape type word is not zero"));
    }
    pack.u32(pack.at(offset, 0x08)?)
}

/// A collision filter word split the way Havok's group filter (`hkpGroupFilter`) reads it: the layer in the low five
/// bits, a sub-system id and the sub-system it does not collide with in the next two groups of five, a system group in
/// the high sixteen. Bit 15 is not part of the split.
///
/// The files hold no filter object (the physics data's world is null in all 490 files, so there is no layer table
/// either): what a layer number means, and which layers collide, is not in the data. What the data shows is which
/// words occur. The `collisionFilterInfo` of the 12,789 rigid bodies has the values 0 (148 bodies), 2 (1,933), 7
/// (8,741), 11 (1,953), 15 (1) and 18 (13), so the layer is all that is set: every body has sub-system id 0, no
/// sub-system to ignore and system group 0. The per-triangle `filterInfo` of an extended mesh's materials is **not** such
/// a word (its surface id sits in bits 3 to 8, across the layer and sub-system fields), and neither is the child
/// record word of a list shape (0 in all 41).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FilterInfo(pub u32);

impl FilterInfo {
    /// The bits of the layer.
    pub const LAYER_MASK: u32 = 0x1F;
    /// The layer (bits 0 to 4).
    pub fn layer(self) -> u8 {
        u8::try_from(self.0 & Self::LAYER_MASK).unwrap_or_default()
    }
    /// The sub-system id (bits 5 to 9).
    pub fn sub_system_id(self) -> u8 {
        u8::try_from((self.0 >> 5) & 0x1F).unwrap_or_default()
    }
    /// The sub-system id this word does not collide with (bits 10 to 14).
    pub fn sub_system_dont_collide_with(self) -> u8 {
        u8::try_from((self.0 >> 10) & 0x1F).unwrap_or_default()
    }
    /// The system group (bits 16 to 31).
    pub fn system_group(self) -> u16 {
        u16::try_from(self.0 >> 16).unwrap_or_default()
    }
    /// The word as stored.
    pub fn word(self) -> u32 {
        self.0
    }
}

/// An `hkpRigidBody`: where it is, what it is named and the shape it carries.
#[derive(Debug, Clone, PartialEq)]
pub struct RigidBody {
    /// The body's data-section offset.
    pub offset: usize,
    /// The collision shape's object offset.
    pub shape: usize,
    /// The body's name.
    pub name: String,
    /// `hkpMaterial` response type, as stored: 1 ([`ResponseType::SimpleContact`]) in every body; see
    /// [`RigidBody::response`].
    pub response_type: u8,
    /// Material friction.
    pub friction: f32,
    /// Material restitution.
    pub restitution: f32,
    /// The collidable's `collisionFilterInfo` (its broad-phase handle's, at `+0x2C`), kept whole; see
    /// [`RigidBody::filter`].
    pub collision_filter_info: u32,
    /// The type of the collidable's broad-phase handle (`+0x28`): [`BroadPhaseType::Entity`] in all 12,789 bodies, and
    /// a body that stores another value is refused.
    pub broad_phase_type: BroadPhaseType,
    /// `storageIndex` (`+0xA8`): the body's index in its storage, as stored.
    pub storage_index: u16,
    /// `hkpMotion::MotionType`, as stored; 2, 3 or 5 are accepted (see [`RigidBody::motion`]).
    pub motion_type: u8,
    /// Rotation as three columns: `world = x*c0 + y*c1 + z*c2 + translation`.
    pub rotation: [[f32; 3]; 3],
    /// The body's position.
    pub translation: [f32; 3],
    /// `qualityType` (`+0x2A`): 0 in the fixed bodies, 4 in the dynamic ones.
    pub quality_type: i8,
    /// `allowedPenetrationDepth` (`+0x5C`): `3.40282e38` (`0x7F7FFFEE`) in the fixed bodies, 0.015 to 0.635 in the
    /// dynamic ones.
    pub allowed_penetration_depth: f32,
    /// The motion's `objectRadius` (`+0x190`). It is not a bound of the shape's extent: 2,051 of the 10,758 dynamic
    /// bodies with a convex shape store less than the distance from the centre of mass to the shape's farthest point.
    pub object_radius: f32,
    /// The motion's linear damping (`+0x194`).
    pub linear_damping: f32,
    /// The motion's angular damping (`+0x198`).
    pub angular_damping: f32,
    /// The `xyz` of `inertiaAndMassInv` (`+0x1A0`): all zero in a fixed body, all positive in a dynamic one. They are
    /// the *inverse* inertia: for 494 of the 587 dynamic bodies that carry a box shape, `x` times the solid box's
    /// `m/3 (hy^2 + hz^2)` (`m` from [`inverse_mass`](Self::inverse_mass)) lies in 0.8 to 1.05, and for none is `x` close
    /// to that inertia itself.
    pub inverse_inertia: [f32; 3],
    /// The `w` of `inertiaAndMassInv` (`+0x1AC`): zero in a fixed body, positive in a dynamic one.
    pub inverse_mass: f32,
    /// The motion's centre of mass in the body's frame (`+0x170`, xyz).
    pub center_of_mass_local: [f32; 3],
    /// `gravityFactor` (`+0x1FE`, a half).
    pub gravity_factor: f32,
}

impl RigidBody {
    /// The rigid body at data-section `offset`; every member is checked as the table above states.
    pub fn read(pack: &Packfile<'_>, offset: usize) -> Result<Self> {
        pack.require_710()?;
        let at = |add| pack.at(offset, add);
        if pack.class_at(offset)? != "hkpRigidBody" {
            return Err(pack.err(offset, "not a hkpRigidBody"));
        }
        let shape = pack.target(at(0x10)?, "rigid body shape")?;
        pack.class_at(shape)?;
        if pack.u32(at(0x14)?)? != 0xFFFF_FFFF {
            return Err(pack.err(at(0x14)?, "shape key is not the invalid key"));
        }
        let name = pack.c_string(pack.target(at(0x74)?, "rigid body name")?)?;
        let response_type = pack.u8(at(0x8C)?)?;
        let handle_type = at(0x28)?;
        let broad_phase_type = BroadPhaseType::from_value(i64::from(pack.u8(handle_type)?))
            .filter(|&t| t == BroadPhaseType::Entity)
            .ok_or_else(|| {
                pack.err(
                    handle_type,
                    "the broad-phase handle type is not BROAD_PHASE_ENTITY",
                )
            })?;
        if pack.u16(at(0xAA)?)? != 0xFFFF {
            return Err(pack.err(at(0xAA)?, "contactPointCallbackDelay is not 0xFFFF"));
        }
        let motion_type = pack.u8(at(0xE8)?)?;
        if ![MOTION_SPHERE_INERTIA, MOTION_BOX_INERTIA, MOTION_FIXED].contains(&motion_type) {
            return Err(pack.err(at(0xE8)?, format!("motion type {motion_type} is not 2, 3 or 5")));
        }
        let inverse_inertia = pack.floats::<3>(at(0x1A0)?)?;
        let inverse_mass = pack.f32(at(0x1AC)?)?;
        if motion_type == MOTION_FIXED {
            if inverse_inertia != [0.0; 3] || inverse_mass != 0.0 {
                return Err(pack.err(at(0x1A0)?, "a fixed body has inverse inertia or inverse mass"));
            }
        } else if inverse_inertia.iter().any(|&x| x <= 0.0) || inverse_mass <= 0.0 {
            return Err(pack.err(
                at(0x1A0)?,
                "a dynamic body has no positive inverse inertia and inverse mass",
            ));
        }
        zero(pack, at(0x1B0)?, 16, "linear velocity")?;
        zero(pack, at(0x1C0)?, 16, "angular velocity")?;
        // the swept transform of a body at rest: both ends equal (the time lanes differ and are not compared)
        if pack.bytes(at(0x130)?, 12)? != pack.bytes(at(0x140)?, 12)? {
            return Err(pack.err(at(0x130)?, "the swept transform's two centres of mass differ"));
        }
        if pack.bytes(at(0x150)?, 16)? != pack.bytes(at(0x160)?, 16)? {
            return Err(pack.err(at(0x150)?, "the swept transform's two rotations differ"));
        }
        let mut rotation = [[0.0; 3]; 3];
        for (i, col) in rotation.iter_mut().enumerate() {
            *col = pack.floats::<3>(at(0xF0usize.saturating_add(i.saturating_mul(16)))?)?;
        }
        let dot = |a: &[f32; 3], b: &[f32; 3]| {
            a.iter()
                .zip(b)
                .map(|(x, y)| f64::from(*x) * f64::from(*y))
                .sum::<f64>()
        };
        for (i, a) in rotation.iter().enumerate() {
            for (j, b) in rotation.iter().enumerate() {
                let want = if i == j { 1.0 } else { 0.0 };
                if (dot(a, b) - want).abs() > 2e-3 {
                    return Err(pack.err(at(0xF0)?, "rotation is not orthonormal"));
                }
            }
        }
        let [c0, c1, c2] = &rotation;
        let det = c0[0] * (c1[1] * c2[2] - c1[2] * c2[1]) - c0[1] * (c1[0] * c2[2] - c1[2] * c2[0])
            + c0[2] * (c1[0] * c2[1] - c1[1] * c2[0]);
        if (det - 1.0).abs() > 2e-3 {
            return Err(pack.err(at(0xF0)?, "rotation determinant is not +1"));
        }
        Ok(Self {
            offset,
            shape,
            name,
            response_type,
            friction: pack.f32(at(0x90)?)?,
            restitution: pack.f32(at(0x94)?)?,
            collision_filter_info: pack.u32(at(0x2C)?)?,
            broad_phase_type,
            storage_index: pack.u16(at(0xA8)?)?,
            motion_type,
            rotation,
            translation: pack.floats::<3>(at(0x120)?)?,
            quality_type: i8::from_be_bytes([pack.u8(at(0x2A)?)?]),
            allowed_penetration_depth: pack.f32(at(0x5C)?)?,
            object_radius: pack.f32(at(0x190)?)?,
            linear_damping: pack.f32(at(0x194)?)?,
            angular_damping: pack.f32(at(0x198)?)?,
            inverse_inertia,
            inverse_mass,
            center_of_mass_local: pack.floats::<3>(at(0x170)?)?,
            gravity_factor: half(pack, at(0x1FE)?)?,
        })
    }

    /// The motion type, named; `None` for a stored number the enumeration does not name (the reader accepts only 2,
    /// 3 and 5).
    pub fn motion(&self) -> Option<MotionType> {
        MotionType::from_value(i64::from(self.motion_type))
    }

    /// The material's response type, named; `None` for a stored number the enumeration does not name.
    pub fn response(&self) -> Option<ResponseType> {
        ResponseType::from_value(i64::from(self.response_type))
    }

    /// The body's collision filter word, split (see [`FilterInfo`]).
    pub fn filter(&self) -> FilterInfo {
        FilterInfo(self.collision_filter_info)
    }

    /// A shape-local point in world space.
    pub fn to_world(&self, p: [f32; 3]) -> [f32; 3] {
        let mut out = self.translation;
        for (axis, col) in p.iter().zip(&self.rotation) {
            for (o, c) in out.iter_mut().zip(col) {
                *o += axis * c;
            }
        }
        out
    }
}

/// One triangle material record (`hkpStorageExtendedMeshShape::Material`, 12 bytes): `filterInfo`, `restitution` and
/// `friction` as halves (a half is the top 16 bits of an `f32`), then `userData`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshMaterial {
    /// `filterInfo`, the whole per-triangle word (kept intact; bits 3..=8 are a surface id, the other bits are opaque
    /// flags).
    pub filter_info: u32,
    /// `restitution` (0 in all data).
    pub restitution: f32,
    /// `friction` (1 in all data).
    pub friction: f32,
    /// `userData`, as stored: the writer does not zero it (nonzero in 6,279 of the 7,909 records).
    pub user_data: u32,
}

/// The half at `offset` (the top 16 bits of an `f32`) as the `f32` it stands for; a NaN or infinity is an error.
fn half(pack: &Packfile<'_>, offset: usize) -> Result<f32> {
    let x = f32::from_bits(u32::from(pack.u16(offset)?) << 16);
    if x.is_finite() {
        Ok(x)
    } else {
        Err(pack.err(offset, "non-finite half"))
    }
}

/// One subpart of an extended mesh: its vertices, triangles and materials.
#[derive(Debug, Clone, PartialEq)]
pub struct Subpart {
    /// Shape-local vertices (the stored w lane is dropped).
    pub vertices: Vec<[f32; 3]>,
    /// Triangles as stored (`i0, i1, i2`), indexing `vertices`.
    pub triangles: Vec<[u32; 3]>,
    /// Per triangle, an index into `materials`.
    pub material_indices: Vec<u16>,
    /// The material records the triangles index.
    pub materials: Vec<MeshMaterial>,
    /// Cumulative triangle count of the earlier subparts.
    pub triangle_offset: usize,
}

/// The bytes of the 96-byte TrianglesSubpart that hold nothing in the data: the pointers the storage replaces
/// (materialIndexBase, materialBase, materialClass, vertexBase, indexBase), `extrusion`, `flipAlternateTriangles`,
/// `userData` and the padding. Each range is zero in all 6,445 subparts.
const SUBPART_ZERO: [(usize, usize, &str); 6] = [
    (0x04, 0x08, "materialIndexBase"),
    (0x0C, 0x14, "materialBase or materialClass"),
    (0x18, 0x1C, "vertexBase"),
    (0x24, 0x44, "extrusion, indexBase or padding"),
    (0x49, 0x4C, "flipAlternateTriangles or padding"),
    (0x50, 0x60, "userData or padding"),
];
/// The size of a TrianglesSubpart.
const SUBPART: usize = 96;
/// Where a TrianglesSubpart keeps its triangle offset.
const TRIANGLE_OFFSET: usize = 0x4C;

/// An `hkpStorageExtendedMeshShape`: a triangle mesh stored in subparts.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtendedMesh {
    /// Half extents of the stored bounding box.
    pub aabb_half_extents: [f32; 3],
    /// Centre of the stored bounding box.
    pub aabb_center: [f32; 3],
    /// The subparts, in order.
    pub subparts: Vec<Subpart>,
    /// Per triangle (all subparts in order), the 16-bit welding info.
    pub welding: Vec<u16>,
    /// `weldingType` (`+0xD8`), as stored: 0 in every mesh of the corpus ([`WeldingType::Anticlockwise`]); see
    /// [`ExtendedMesh::welding`].
    pub welding_type: u8,
    /// `defaultCollisionFilterInfo` (`+0xDC`): 0 in every mesh of the corpus.
    pub default_collision_filter_info: u32,
    /// `cachedNumChildShapes` (`+0xE0`): at most the triangle count (equal in 1,277 of the 1,758 meshes, smaller in
    /// 481; which triangles those leave out is not known).
    pub cached_num_child_shapes: u32,
    /// `triangleRadius` (`+0xE4`): 0.001 in every mesh of the corpus.
    pub triangle_radius: f32,
    /// The shape's `userData` (`+0x08`, see [`shape_user_data`]): 1 in 45 meshes, 0 in the rest.
    pub user_data: u32,
}

impl ExtendedMesh {
    /// The extended mesh at data-section `offset`; refused when a member differs from the confirmed layout.
    pub fn read(pack: &Packfile<'_>, offset: usize) -> Result<Self> {
        pack.require_710()?;
        let at = |add| pack.at(offset, add);
        if pack.class_at(offset)? != "hkpStorageExtendedMeshShape" {
            return Err(pack.err(offset, "not a hkpStorageExtendedMeshShape"));
        }
        if pack.u32(at(0xB0)?)? != 12 {
            return Err(pack.err(at(0xB0)?, "numBitsForSubpartIndex is not 12"));
        }
        let subs = pack.array_header(at(0xB4)?)?;
        let storage = pack.array_header(at(0xF0)?)?;
        if storage.len != subs.len {
            return Err(pack.err(at(0xF0)?, "mesh storage count differs from the subpart count"));
        }
        if pack.array_header(at(0xC0)?)?.len != 0 {
            return Err(pack.err(at(0xC0)?, "shape subparts are not supported"));
        }
        if pack.array_header(at(0xFC)?)?.len != 0 {
            return Err(pack.err(at(0xFC)?, "shape storage is not supported"));
        }
        zero(pack, at(0xD9)?, 3, "the padding after weldingType")?;
        zero(pack, at(0xE8)?, 8, "the padding after triangleRadius")?;
        let user_data = shape_user_data(pack, offset)?;
        if pack.floats::<4>(at(0x80)?)? != [1.0; 4] {
            return Err(pack.err(at(0x80)?, "scaling is not (1, 1, 1, 1)"));
        }
        if let Some(first) = subs.data.filter(|_| subs.len > 0) {
            // the embedded subpart is a copy of subpart 0, but for a triangle offset nothing reads
            let embedded = pack.bytes(at(0x20)?, SUBPART)?;
            let first = pack.bytes(first, SUBPART)?;
            let same = |range: std::ops::Range<usize>| embedded.get(range.clone()) == first.get(range);
            if !same(0..TRIANGLE_OFFSET) || !same(TRIANGLE_OFFSET.saturating_add(4)..SUBPART) {
                return Err(pack.err(at(0x20)?, "the embedded subpart is not a copy of subpart 0"));
            }
        }
        let mut subparts = Vec::new();
        let mut total = 0usize;
        for i in 0..subs.len {
            let sub = pack.at(subs.data.unwrap_or(0), i.saturating_mul(SUBPART))?;
            let store = pack.target(
                pack.at(storage.data.unwrap_or(0), i.saturating_mul(4))?,
                "mesh storage",
            )?;
            if pack.class_at(store)? != "hkpStorageExtendedMeshShapeMeshSubpartStorage" {
                return Err(pack.err(store, "mesh storage is not a MeshSubpartStorage"));
            }
            let s = read_subpart(pack, sub, store, total)?;
            total = pack.at(total, s.triangles.len())?;
            subparts.push(s);
        }
        let weld = pack.array_header(at(0xCC)?)?;
        if weld.len != total {
            return Err(pack.err(at(0xCC)?, "welding info count differs from the triangle count"));
        }
        let mut welding = Vec::new();
        for i in 0..weld.len {
            welding.push(pack.u16(pack.at(weld.data.unwrap_or(0), i.saturating_mul(2))?)?);
        }
        let cached_num_child_shapes = pack.u32(at(0xE0)?)?;
        if usize::try_from(cached_num_child_shapes).map_or(true, |n| n > total) {
            return Err(pack.err(
                at(0xE0)?,
                format!("cachedNumChildShapes {cached_num_child_shapes} exceeds the {total} triangles"),
            ));
        }
        Ok(Self {
            aabb_half_extents: pack.floats::<3>(at(0x90)?)?,
            aabb_center: pack.floats::<3>(at(0xA0)?)?,
            subparts,
            welding,
            welding_type: pack.u8(at(0xD8)?)?,
            default_collision_filter_info: pack.u32(at(0xDC)?)?,
            cached_num_child_shapes,
            triangle_radius: pack.f32(at(0xE4)?)?,
            user_data,
        })
    }

    /// The welding type, named; `None` for a stored number the enumeration does not name.
    pub fn welding(&self) -> Option<WeldingType> {
        WeldingType::from_value(i64::from(self.welding_type))
    }

    /// The triangles of all subparts.
    pub fn triangle_count(&self) -> usize {
        self.subparts.iter().map(|s| s.triangles.len()).sum()
    }
}

fn read_subpart(pack: &Packfile<'_>, sub: usize, store: usize, expected_offset: usize) -> Result<Subpart> {
    let sa = |add| pack.at(sub, add);
    let ta = |add| pack.at(store, add);
    if SubpartType::from_value(i64::from(pack.u8(sa(0x00)?)?)) != Some(SubpartType::Triangles) {
        return Err(pack.err(sa(0x00)?, "subpart type is not triangles"));
    }
    if MaterialIndexStridingType::from_value(i64::from(pack.u8(sa(0x01)?)?))
        != Some(MaterialIndexStridingType::Int16)
    {
        return Err(pack.err(sa(0x01)?, "material index striding type is not 16-bit"));
    }
    if pack.u16(sa(0x02)?)? != 12 {
        return Err(pack.err(sa(0x02)?, "material striding is not 12"));
    }
    for &(from, to, what) in &SUBPART_ZERO {
        if pack
            .bytes(sa(from)?, to.saturating_sub(from))?
            .iter()
            .any(|&b| b != 0)
        {
            return Err(pack.err(sa(from)?, format!("subpart {what} is not zero")));
        }
    }
    if pack.u16(sa(0x08)?)? != 2 {
        return Err(pack.err(sa(0x08)?, "material index striding is not 2"));
    }
    let n_materials = usize::from(pack.u16(sa(0x0A)?)?);
    let n_tris = pack.count(sa(0x14)?)?;
    let n_verts = pack.count(sa(0x20)?)?;
    if pack.u32(sa(0x1C)?)? != 16 || pack.u32(sa(0x44)?)? != 16 {
        return Err(pack.err(sa(0x1C)?, "vertex or index striding is not 16"));
    }
    if IndexStridingType::from_value(i64::from(pack.u8(sa(0x48)?)?)) != Some(IndexStridingType::Int32) {
        return Err(pack.err(sa(0x48)?, "index striding type is not 32-bit"));
    }
    if pack.count(sa(TRIANGLE_OFFSET)?)? != expected_offset {
        return Err(pack.err(
            sa(TRIANGLE_OFFSET)?,
            "triangle offset is not the earlier subparts' triangle count",
        ));
    }
    let verts = pack.array_header(ta(0x08)?)?;
    if verts.len != n_verts {
        return Err(pack.err(ta(0x08)?, "storage vertex count differs from the subpart"));
    }
    for (off, what) in [
        (0x14, "indices8"),
        (0x20, "indices16"),
        (0x38, "materialIndices8"),
        (0x50, "namedMaterials"),
    ] {
        if pack.array_header(ta(off)?)?.len != 0 {
            return Err(pack.err(ta(off)?, format!("{what} is not empty")));
        }
    }
    let mut vertices = Vec::new();
    for i in 0..verts.len {
        let v = pack.at(verts.data.unwrap_or(0), i.saturating_mul(16))?;
        vertices.push(pack.floats::<3>(v)?);
        pack.f32(pack.at(v, 12)?)?;
    }
    let idx = pack.array_header(ta(0x2C)?)?;
    if Some(idx.len) != n_tris.checked_mul(4) {
        return Err(pack.err(ta(0x2C)?, "index count is not four per triangle"));
    }
    let mut triangles = Vec::new();
    for t in 0..n_tris {
        let at = pack.at(idx.data.unwrap_or(0), t.saturating_mul(16))?;
        let word = |k: usize| -> Result<u32> { pack.u32(pack.at(at, k.saturating_mul(4))?) };
        let tri = [word(0)?, word(1)?, word(2)?];
        if tri
            .iter()
            .any(|&i| usize::try_from(i).map_or(true, |i| i >= n_verts))
        {
            return Err(pack.err(at, "triangle index is outside the vertices"));
        }
        if word(3)? != 0 {
            return Err(pack.err(pack.at(at, 12)?, "fourth index word is not zero"));
        }
        triangles.push(tri);
    }
    let mats = pack.array_header(ta(0x44)?)?;
    if mats.len != n_materials {
        return Err(pack.err(ta(0x44)?, "storage material count differs from the subpart"));
    }
    let mut materials = Vec::new();
    for m in 0..mats.len {
        let r = pack.at(mats.data.unwrap_or(0), m.saturating_mul(12))?;
        materials.push(MeshMaterial {
            filter_info: pack.u32(r)?,
            restitution: half(pack, pack.at(r, 4)?)?,
            friction: half(pack, pack.at(r, 6)?)?,
            user_data: pack.u32(pack.at(r, 8)?)?,
        });
    }
    let mi = pack.array_header(ta(0x5C)?)?;
    if mi.len != n_tris {
        return Err(pack.err(ta(0x5C)?, "material index count differs from the triangle count"));
    }
    let mut material_indices = Vec::new();
    for t in 0..mi.len {
        let i = pack.u16(pack.at(mi.data.unwrap_or(0), t.saturating_mul(2))?)?;
        if usize::from(i) >= n_materials {
            return Err(pack.err(ta(0x5C)?, "material index is outside the materials"));
        }
        material_indices.push(i);
    }
    Ok(Subpart {
        vertices,
        triangles,
        material_indices,
        materials,
        triangle_offset: expected_offset,
    })
}

/// An `hkpConvexVerticesShape`.
#[derive(Debug, Clone, PartialEq)]
pub struct ConvexVertices {
    /// The convex radius.
    pub radius: f32,
    /// Half extents of the stored bounding box.
    pub aabb_half_extents: [f32; 3],
    /// Centre of the stored bounding box.
    pub aabb_center: [f32; 3],
    /// The vertices, unpacked from the `FourVectors` groups.
    pub vertices: Vec<[f32; 3]>,
    /// Stored plane equations `(nx, ny, nz, d)`: approximate, not exact hull faces.
    pub planes: Vec<[f32; 4]>,
    /// The faces of the hull as vertex indices, from the shape's connectivity object; empty when the shape has none
    /// (29 of the 8,780 shapes). There are at least as many [`planes`](Self::planes) as faces, and the first ones
    /// are the faces' planes in order in most shapes (every pair in 7,587 of the 8,751 with faces).
    pub faces: Vec<Vec<u16>>,
    /// The shape's `userData` (`+0x08`, see [`shape_user_data`]): 1 in the 583 convex shapes of box-inertia bodies.
    pub user_data: u32,
}

/// The faces of an `hkpConvexVerticesConnectivity` at `offset`: its vertex indices (`+0x08`, `u16`) cut into runs by
/// its per-face counts (`+0x14`, `u8`), which must use every index and name only vertices below `vertex_count`.
fn connectivity(pack: &Packfile<'_>, offset: usize, vertex_count: usize) -> Result<Vec<Vec<u16>>> {
    if pack.class_at(offset)? != "hkpConvexVerticesConnectivity" {
        return Err(pack.err(offset, "the connectivity is not a hkpConvexVerticesConnectivity"));
    }
    let indices = pack.array_header(pack.at(offset, 0x08)?)?;
    let counts = pack.array_header(pack.at(offset, 0x14)?)?;
    let mut next = 0usize;
    let mut faces = Vec::new();
    for f in 0..counts.len {
        let count = usize::from(pack.u8(pack.at(counts.data.unwrap_or(0), f)?)?);
        let end = next
            .checked_add(count)
            .filter(|&end| end <= indices.len)
            .ok_or_else(|| pack.err(offset, "the faces use more vertex indices than are stored"))?;
        let mut face = Vec::new();
        for k in next..end {
            let index = pack.u16(pack.at(indices.data.unwrap_or(0), k.saturating_mul(2))?)?;
            if usize::from(index) >= vertex_count {
                return Err(pack.err(offset, "a face names a vertex the shape does not have"));
            }
            face.push(index);
        }
        faces.push(face);
        next = end;
    }
    if next != indices.len {
        return Err(pack.err(offset, "the faces leave stored vertex indices unused"));
    }
    Ok(faces)
}

impl ConvexVertices {
    /// The convex vertices shape at data-section `offset`.
    pub fn read(pack: &Packfile<'_>, offset: usize) -> Result<Self> {
        pack.require_710()?;
        let at = |add| pack.at(offset, add);
        if pack.class_at(offset)? != "hkpConvexVerticesShape" {
            return Err(pack.err(offset, "not a hkpConvexVerticesShape"));
        }
        let user_data = shape_user_data(pack, offset)?;
        if pack.pointer(at(0x50)?)?.is_some() {
            return Err(pack.err(at(0x50)?, "the slot at +0x50 has a fixup"));
        }
        zero(pack, at(0x64)?, 12, "the end of the shape")?;
        let groups = pack.array_header(at(0x40)?)?;
        let n = pack.count(at(0x4C)?)?;
        let capacity = groups.len.saturating_mul(4);
        if n > capacity || n <= capacity.saturating_sub(4) {
            return Err(pack.err(at(0x4C)?, "vertex count does not fit the FourVectors"));
        }
        let mut vertices = Vec::new();
        for j in 0..n {
            let (g, lane) = (j / 4, j % 4);
            let base = pack.at(groups.data.unwrap_or(0), g.saturating_mul(48))?;
            let mut v = [0.0; 3];
            for (axis, slot) in v.iter_mut().enumerate() {
                *slot = pack.f32(pack.at(
                    base,
                    axis.saturating_mul(16).saturating_add(lane.saturating_mul(4)),
                )?)?;
            }
            vertices.push(v);
        }
        let pl = pack.array_header(at(0x54)?)?;
        let mut planes = Vec::new();
        for i in 0..pl.len {
            planes.push(pack.floats::<4>(pack.at(pl.data.unwrap_or(0), i.saturating_mul(16))?)?);
        }
        let connected = pack.pointer(at(0x60)?)?;
        let faces = match connected {
            Some(c) => connectivity(pack, c, n)?,
            None => Vec::new(),
        };
        if connected.is_some() && planes.len() < faces.len() {
            return Err(pack.err(
                at(0x54)?,
                format!("{} planes for {} faces", planes.len(), faces.len()),
            ));
        }
        Ok(Self {
            radius: pack.f32(at(0x10)?)?,
            aabb_half_extents: pack.floats::<3>(at(0x20)?)?,
            aabb_center: pack.floats::<3>(at(0x30)?)?,
            vertices,
            planes,
            faces,
            user_data,
        })
    }
}

/// An `hkpMoppCode`: the byte code of a MOPP tree and the box it is scaled into.
#[derive(Debug, Clone, PartialEq)]
pub struct MoppCode {
    /// `info.offset` (`+0x10`, xyz): the minimum corner of the box the code covers; the mesh's own minimum corner in
    /// the 1,704 codes over a mesh.
    pub offset: [f32; 3],
    /// `info.offset`'s `w` (`+0x1C`): the code's scale. Over a mesh, `scale` times the mesh box's longest extent over
    /// `2^24` rounds to 0.99 in all 1,704 codes.
    pub scale: f32,
    /// `buildType` (`+0x2C`), as stored: 0 in every code of the corpus ([`MoppBuildType::BuiltWithChunkSubdivision`]); see
    /// [`MoppCode::build`].
    pub build_type: i8,
    /// The code bytes (the array at `+0x20`; its data follows the 0x30-byte body).
    pub code: Vec<u8>,
}

impl MoppCode {
    /// The build type, named; `None` for a stored number the enumeration does not name.
    pub fn build(&self) -> Option<MoppBuildType> {
        MoppBuildType::from_value(i64::from(self.build_type))
    }

    /// The MOPP code at data-section `offset`.
    pub fn read(pack: &Packfile<'_>, offset: usize) -> Result<Self> {
        pack.require_710()?;
        let at = |add| pack.at(offset, add);
        if pack.class_at(offset)? != "hkpMoppCode" {
            return Err(pack.err(offset, "not a hkpMoppCode"));
        }
        zero(pack, offset, 0x10, "the start of a MOPP code")?;
        let code = pack.array_header(at(0x20)?)?;
        if code.data != Some(at(0x30)?) {
            return Err(pack.err(at(0x20)?, "the code does not follow the 0x30-byte body"));
        }
        zero(pack, at(0x2D)?, 3, "the padding after buildType")?;
        let bytes = pack.bytes(at(0x30)?, code.len)?;
        Ok(Self {
            offset: pack.floats::<3>(at(0x10)?)?,
            scale: pack.f32(at(0x1C)?)?,
            build_type: i8::from_be_bytes([pack.u8(at(0x2C)?)?]),
            code: bytes.to_vec(),
        })
    }
}

/// One entry of an `hkpListShape`'s `childInfo`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListChild {
    /// Data-section offset of the child shape (the pointer at the record's start).
    pub shape: usize,
    /// The record's `collisionFilterInfo` word (`+4`): 0 in the 41 records of the corpus. One `u32` or two `u16`
    /// cannot be told apart in zeros; the word is kept whole.
    pub collision_filter_info: u32,
}

/// A collision shape, one view per class.
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    /// A MOPP tree. The child holds the geometry.
    Mopp {
        /// Data-section offset of the child shape (`hkpMoppBvTreeShape`, the pointer at 0x34).
        child: usize,
        /// Data-section offset of the shape's `hkpMoppCode` (the pointer at 0x14); read it with [`MoppCode::read`].
        code: usize,
    },
    /// An `hkpStorageExtendedMeshShape`.
    ExtendedMesh(ExtendedMesh),
    /// An `hkpConvexVerticesShape`.
    ConvexVertices(ConvexVertices),
    /// An `hkpBoxShape`.
    Box {
        /// The shape's convex radius (at 0x10).
        radius: f32,
        /// The half extents `xyz` (at 0x20).
        half_extents: [f32; 3],
    },
    /// An `hkpSphereShape`.
    Sphere {
        /// The sphere's radius (at 0x10).
        radius: f32,
    },
    /// An `hkpCapsuleShape`.
    Capsule {
        /// The capsule's radius (at 0x10).
        radius: f32,
        /// One end of the axis, shape-local (at 0x20).
        a: [f32; 3],
        /// The other end of the axis, shape-local (at 0x30).
        b: [f32; 3],
    },
    /// An `hkpCylinderShape`.
    Cylinder {
        /// The shape's convex radius (at 0x10).
        radius: f32,
        /// The cylinder's radius (at 0x14).
        cylinder_radius: f32,
        /// The value stored at 0x18, as stored.
        base_radius_factor: f32,
        /// One end of the axis, shape-local (at 0x20).
        a: [f32; 3],
        /// The other end of the axis, shape-local (at 0x30).
        b: [f32; 3],
    },
    /// An `hkpConvexTranslateShape`: a child shape moved by a translation.
    ConvexTranslate {
        /// Data-section offset of the child shape (the pointer at 0x18).
        child: usize,
        /// The translation `xyz` (at 0x20).
        translation: [f32; 3],
    },
    /// An `hkpListShape`.
    List {
        /// The child shapes, in the stored order.
        children: Vec<ListChild>,
    },
}

impl Shape {
    /// The shape object at `offset`; a class this module has no view for is an error naming it.
    pub fn read(pack: &Packfile<'_>, offset: usize) -> Result<Self> {
        pack.require_710()?;
        let at = |add| pack.at(offset, add);
        let class = pack.class_at(offset)?;
        let shape = match class {
            "hkpMoppBvTreeShape" => {
                let code = pack.target(at(0x14)?, "MOPP code")?;
                if pack.class_at(code)? != "hkpMoppCode" {
                    return Err(pack.err(at(0x14)?, "the MOPP code is not a hkpMoppCode"));
                }
                Self::Mopp {
                    child: pack.target(at(0x34)?, "MOPP child shape")?,
                    code,
                }
            }
            "hkpStorageExtendedMeshShape" => Self::ExtendedMesh(ExtendedMesh::read(pack, offset)?),
            "hkpConvexVerticesShape" => Self::ConvexVertices(ConvexVertices::read(pack, offset)?),
            "hkpBoxShape" => Self::Box {
                radius: pack.f32(at(0x10)?)?,
                half_extents: pack.floats::<3>(at(0x20)?)?,
            },
            "hkpSphereShape" => Self::Sphere {
                radius: pack.f32(at(0x10)?)?,
            },
            "hkpCapsuleShape" => {
                let radius = pack.f32(at(0x10)?)?;
                // the fourth lane of each end holds the radius again
                if pack.f32(at(0x2C)?)? != radius || pack.f32(at(0x3C)?)? != radius {
                    return Err(pack.err(at(0x2C)?, "a capsule end's w lane is not the radius"));
                }
                Self::Capsule {
                    radius,
                    a: pack.floats::<3>(at(0x20)?)?,
                    b: pack.floats::<3>(at(0x30)?)?,
                }
            }
            "hkpCylinderShape" => Self::Cylinder {
                radius: pack.f32(at(0x10)?)?,
                cylinder_radius: pack.f32(at(0x14)?)?,
                base_radius_factor: pack.f32(at(0x18)?)?,
                a: pack.floats::<3>(at(0x20)?)?,
                b: pack.floats::<3>(at(0x30)?)?,
            },
            "hkpConvexTranslateShape" => Self::ConvexTranslate {
                child: pack.target(at(0x18)?, "convex translate child")?,
                translation: pack.floats::<3>(at(0x20)?)?,
            },
            "hkpListShape" => {
                let info = pack.array_header(at(0x18)?)?;
                let mut children = Vec::new();
                for i in 0..info.len {
                    let rec = pack.at(info.data.unwrap_or(0), i.saturating_mul(16))?;
                    zero(
                        pack,
                        pack.at(rec, 8)?,
                        8,
                        "the end of a list shape's child record",
                    )?;
                    children.push(ListChild {
                        shape: pack.target(rec, "list child shape")?,
                        collision_filter_info: pack.u32(pack.at(rec, 4)?)?,
                    });
                }
                Self::List { children }
            }
            other => return Err(pack.err(offset, format!("no view for shape class {other}"))),
        };
        shape_user_data(pack, offset)?;
        Ok(shape)
    }
}
