//! The constraint side of Havok 7.1.0-r1 packfiles (Tony Hawk's SHRED, PS3): the physics data a file's root container
//! holds, its systems, the constraint instances of a system and the constraint data they point to, with the atoms
//! typed. The classes are the ones the 490 files hold: `hkpPhysicsData` (one per file), `hkpPhysicsSystem` (972),
//! `hkpConstraintInstance` (161) and the data classes `hkpRagdollConstraintData` (72), `hkpLimitedHingeConstraintData`
//! (89) and `hkpBreakableConstraintData` (16, each wrapping a limited hinge).
//!
//! The offsets are those of the 7.1 member lists and are confirmed twice: the eight ragdoll files carry the classes
//! themselves ([`crate::packfile::reflection`]), and every member offset below that they cover equals the stored one
//! (a unit test in this module); and every one of the 161 instances and their data reads here with each `type` word the
//! rule of its slot and each padding byte zero. The atom numbering is the file's own (`AtomType` in the metadata of the
//! ragdoll files): `1` bridge, `2` set local transforms, `5` ball socket, `12` 2d angle, `14` angle limit, `15` twist
//! limit, `16` cone limit, `17` angular friction, `18` angular motor, `19` ragdoll motor. It is not the numbering of the
//! Sekiro and Elden Ring tagfiles ([`super`]), where `23` is a setup-stabilization atom and `24` the fixed joint's
//! `ang`; the atoms that share a name there do not share their members either (7.1 has no damp factors, no `cosineAxis`
//! and no solving method), so this module has its own types for them and reuses the ones whose members are the same
//! ([`Transform`], [`LocalTransforms`], [`AngFriction`]).
//!
//! | class | member | offset | what the 161 instances hold |
//! |---|---|---|---|
//! | `hkRootLevelContainer` | namedVariants (12-byte records: name, class name, object) | `+0x00` | one variant, of class `hkpPhysicsData`, named `hkpPhysicsData` (482 files) or `Physics Data` (the 8 ragdolls) |
//! | `hkpPhysicsData` | worldCinfo (null), systems (array of pointers) | `+0x08`, `+0x0C` | null in 490; two systems (482 files) or one (the ragdolls) |
//! | `hkpPhysicsSystem` | rigidBodies, constraints, actions, phantoms (arrays of pointers), name | `+0x08`, `+0x14`, `+0x20`, `+0x2C`, `+0x38` | actions and phantoms empty in 972; named `Default Physics System` in the 8 ragdolls' systems only |
//! | | userData, active, padding | `+0x3C`, `+0x40`, `+0x41..+0x44` | 0; 1 in 490 systems, 0 in 482; zero |
//! | `hkpConstraintInstance` | data, constraintModifiers (null), entities (two rigid bodies) | `+0x0C`, `+0x10`, `+0x14`, `+0x18` | the three data classes (73 + 72 + 16); null; two distinct bodies in all |
//! | | priority, wantRuntime, destructionRemapInfo | `+0x1C`, `+0x1D`, `+0x1E` | `1`, `1`, `0` in all 161 |
//! | | listeners, name, userData, internal, uid | `+0x20`, `+0x28`, `+0x2C`, `+0x30`, `+0x34` | zero; a string; zero; zero; zero |
//! | every data class | userData | `+0x08` | 0 |
//! | `hkpRagdollConstraintData` | atoms (transforms, ragdollMotors, angFriction, twistLimit, coneLimit, planesLimit, ballSocket) | `+0x10`, `+0xA0`, `+0xF0`, `+0xFC`, `+0x110`, `+0x124`, `+0x138` | type words `2, 19, 17, 15, 16, 16, 5` in 72 |
//! | `hkpLimitedHingeConstraintData` | atoms (transforms, angMotor, angFriction, angLimit, 2dAng, ballSocket) | `+0x10`, `+0xA0`, `+0xB4`, `+0xC0`, `+0xD0`, `+0xD4` | type words `2, 18, 17, 14, 12, 5` in 89 |
//! | `hkpBreakableConstraintData` | bridge atom type, buildJacobianFunc (null), the data itself (a pointer to the object), the wrapped data | `+0x0C`, `+0x10`, `+0x14`, `+0x18` | `1`; null; the object's own address; a limited hinge, in 16 |
//! | | childRuntimeSize, childNumSolverResults, solverResultLimit, removeWhenBroken, revertBackVelocityOnBreak | `+0x1C`, `+0x1E`, `+0x20`, `+0x24`, `+0x25` | 80; 8; 25.0 (12) or 30.0 (4); 0; 0 |
//!
//! Each atom is a `u16` type word, then its members; the next atom starts at the end of the previous one, as the class
//! sizes in the ragdoll files' metadata say. The `hkpSetLocalTransformsConstraintAtom` is 144 bytes (a transform A at
//! `+0x10`, a transform B at `+0x50`; each is three 16-byte columns and a translation, and the fourth lane of every
//! column and of the translation is not read: the three column lanes of transform A are zero in the 17 hinges of the
//! two level files that have constraints and non-zero in the 144 data objects of the ragdoll files). The
//! ragdoll motor atom's `target_bRca` equals the rotation of transform B in all 72 (its fourth lanes too).
//!
//! What is *not* in the files: no `hkpWorldCinfo` (the physics data's world is null in all 490, so no world collision
//! filter object and no layer table exists to decode a filter word against; see
//! [`FilterInfo`](crate::packfile::physics::FilterInfo)), no constraint motor object, no action and no phantom. A file
//! that has one is refused, not skipped.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use super::{AngFriction, LocalTransforms, Transform};
use crate::Result;
use crate::packfile::Packfile;
use crate::packfile::physics::RigidBody;

/// `AtomType` values of the 7.1 files (the enum the ragdoll files' metadata stores).
pub mod atom_type {
    /// `TYPE_BRIDGE`: the atom a breakable constraint's body starts with.
    pub const BRIDGE: u16 = 1;
    /// `TYPE_SET_LOCAL_TRANSFORMS`.
    pub const SET_LOCAL_TRANSFORMS: u16 = 2;
    /// `TYPE_BALL_SOCKET`.
    pub const BALL_SOCKET: u16 = 5;
    /// `TYPE_2D_ANG`.
    pub const ANG_2D: u16 = 12;
    /// `TYPE_ANG_LIMIT`.
    pub const ANG_LIMIT: u16 = 14;
    /// `TYPE_TWIST_LIMIT`.
    pub const TWIST_LIMIT: u16 = 15;
    /// `TYPE_CONE_LIMIT`.
    pub const CONE_LIMIT: u16 = 16;
    /// `TYPE_ANG_FRICTION`.
    pub const ANG_FRICTION: u16 = 17;
    /// `TYPE_ANG_MOTOR`.
    pub const ANG_MOTOR: u16 = 18;
    /// `TYPE_RAGDOLL_MOTOR`.
    pub const RAGDOLL_MOTOR: u16 = 19;
}

/// `hkpConstraintInstance::ConstraintPriority` 1, `PRIORITY_PSI`: the priority of all 161 instances.
pub const PRIORITY_PSI: u8 = 1;

/// Fail unless the `len` bytes at `offset` are zero.
fn zero(pack: &Packfile<'_>, offset: usize, len: usize, what: &str) -> Result<()> {
    if pack.bytes(offset, len)?.iter().any(|&b| b != 0) {
        Err(pack.err(offset, format!("{what} is not zero")))
    } else {
        Ok(())
    }
}

/// Fail unless the pointer slot at `offset` is null.
fn null(pack: &Packfile<'_>, offset: usize, what: &str) -> Result<()> {
    if pack.pointer(offset)?.is_some() {
        Err(pack.err(offset, format!("{what} is set; no file of the corpus has one")))
    } else {
        Ok(())
    }
}

/// A boolean byte: 0 or 1.
fn flag(pack: &Packfile<'_>, offset: usize, what: &str) -> Result<bool> {
    match pack.u8(offset)? {
        0 => Ok(false),
        1 => Ok(true),
        other => Err(pack.err(offset, format!("{what} is {other}, not a flag"))),
    }
}

/// The signed 16-bit word at `offset`.
fn int16(pack: &Packfile<'_>, offset: usize) -> Result<i16> {
    Ok(i16::from_be_bytes(pack.u16(offset)?.to_be_bytes()))
}

/// Fail unless the atom at `offset` has type `want`.
fn atom_type_is(pack: &Packfile<'_>, offset: usize, want: u16, what: &str) -> Result<()> {
    let found = pack.u16(offset)?;
    if found == want {
        Ok(())
    } else {
        Err(pack.err(offset, format!("atom {what} has type {found}, not {want}")))
    }
}

/// The rotation columns and translation of an `hkTransform` at `offset` (the fourth lanes are not read).
fn transform(pack: &Packfile<'_>, offset: usize) -> Result<Transform> {
    let row = |i: usize| -> Result<[f32; 3]> { pack.floats::<3>(pack.at(offset, i.saturating_mul(16))?) };
    Ok(Transform {
        columns: [row(0)?, row(1)?, row(2)?],
        translation: row(3)?,
    })
}

/// `hkpSetLocalTransformsConstraintAtom` at `atom`: type 2, zero up to the first transform at `+0x10`, the two
/// transforms.
fn local_transforms(pack: &Packfile<'_>, atom: usize) -> Result<LocalTransforms> {
    atom_type_is(pack, atom, atom_type::SET_LOCAL_TRANSFORMS, "transforms")?;
    zero(pack, pack.at(atom, 2)?, 14, "the padding of the transforms atom")?;
    Ok(LocalTransforms {
        transform_a: transform(pack, pack.at(atom, 0x10)?)?,
        transform_b: transform(pack, pack.at(atom, 0x50)?)?,
    })
}

/// `hkpAngFrictionConstraintAtom` at `atom`.
fn ang_friction(pack: &Packfile<'_>, atom: usize) -> Result<AngFriction> {
    atom_type_is(pack, atom, atom_type::ANG_FRICTION, "angFriction")?;
    zero(pack, pack.at(atom, 5)?, 3, "the padding of the angFriction atom")?;
    Ok(AngFriction {
        enabled: flag(pack, pack.at(atom, 2)?, "angFriction.isEnabled")?,
        first_friction_axis: pack.u8(pack.at(atom, 3)?)?,
        num_friction_axes: pack.u8(pack.at(atom, 4)?)?,
        max_friction_torque: pack.f32(pack.at(atom, 8)?)?,
    })
}

/// `hkpBallSocketConstraintAtom` (7.1): which bodies to notify, the stabilization factor and the impulse limit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BallSocket {
    /// `bodiesToNotify`: 0 in all 161.
    pub bodies_to_notify: u8,
    /// `stabilizationFactor`, the byte stored: 48 (`0x30`) in all 161.
    pub stabilization_factor: u8,
    /// `maxImpulse`: `3.40282e38` (`0x7F7FFFEE`, just short of `f32::MAX`) in all 161.
    pub max_impulse: f32,
}

/// `hkpTwistLimitConstraintAtom` (7.1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TwistLimit {
    /// `isEnabled`: 1 in all 72.
    pub enabled: bool,
    /// `twistAxis`: 0 in all 72.
    pub twist_axis: u8,
    /// `refAxis`: 1 in all 72.
    pub ref_axis: u8,
    /// `minAngle` in radians; at most 0 and at most `max_angle` in all 72.
    pub min_angle: f32,
    /// `maxAngle` in radians.
    pub max_angle: f32,
    /// `angularLimitsTauFactor`: 0.8 (`0x3F4CCCCD`) in all 72.
    pub tau_factor: f32,
}

/// `hkpConeLimitConstraintAtom` (7.1). A ragdoll holds two: the cone itself and the planes limit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConeLimit {
    /// `isEnabled`: 1 in all 144.
    pub enabled: bool,
    /// `twistAxisInA`: 0 in all 144.
    pub twist_axis_in_a: u8,
    /// `refAxisInB`: 0 for the cone and 1 for the planes limit, in all 72 ragdolls.
    pub ref_axis_in_b: u8,
    /// `angleMeasurementMode`: 0 (`ZERO_WHEN_VECTORS_ALIGNED`) for the cone, 1 (`ZERO_WHEN_VECTORS_PERPENDICULAR`) for
    /// the planes limit.
    pub angle_measurement_mode: u8,
    /// `memOffsetToAngleOffset`, a byte offset into the solver's runtime: 56 (`0x38`) for the cone, 0 for the planes
    /// limit.
    pub mem_offset_to_angle_offset: u8,
    /// `minAngle` in radians: -100.0 for the cone in all 72, so effectively unbounded below.
    pub min_angle: f32,
    /// `maxAngle` in radians.
    pub max_angle: f32,
    /// `angularLimitsTauFactor`: 0.8 in all 144.
    pub tau_factor: f32,
}

/// `hkpAngLimitConstraintAtom` (7.1): a hinge's angle limit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AngLimit {
    /// `isEnabled`: 1 in all 89.
    pub enabled: bool,
    /// `limitAxis`: 0 in all 89.
    pub limit_axis: u8,
    /// `minAngle` in radians; at most `max_angle` in all 89.
    pub min_angle: f32,
    /// `maxAngle` in radians.
    pub max_angle: f32,
    /// `angularLimitsTauFactor`: 1.0 in all 89.
    pub tau_factor: f32,
}

/// `hkpAngMotorConstraintAtom` (7.1): a hinge's motor, which the files leave disabled and without a motor object.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AngMotor {
    /// `isEnabled`: 0 in all 89.
    pub enabled: bool,
    /// `motorAxis`: 0 in all 89.
    pub motor_axis: u8,
    /// `initializedOffset`, a byte offset into the solver's runtime: 64 (`0x40`) in all 89.
    pub initialized_offset: i16,
    /// `previousTargetAngleOffset`: 68 (`0x44`) in all 89.
    pub previous_target_angle_offset: i16,
    /// `correspondingAngLimitSolverResultOffset`: 16 (`0x10`) in all 89.
    pub corresponding_ang_limit_solver_result_offset: i16,
    /// `targetAngle`: 0 in all 89.
    pub target_angle: f32,
}

/// `hkpRagdollMotorConstraintAtom` (7.1): the target orientation of a ragdoll joint's three motors, which the files
/// leave disabled and without motor objects.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RagdollMotors {
    /// `isEnabled`: 0 in all 72.
    pub enabled: bool,
    /// `initializedOffset`, a byte offset into the solver's runtime: 96 (`0x60`) in all 72.
    pub initialized_offset: i16,
    /// `previousTargetAnglesOffset`: 100 (`0x64`) in all 72.
    pub previous_target_angles_offset: i16,
    /// `target_bRca`: the target orientation, three columns (the fourth lane of each is not read). Equal to the
    /// rotation of transform B in all 72.
    pub target_b_rca: [[f32; 3]; 3],
}

/// `hkpRagdollConstraintData` (7.1): a ragdoll joint.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ragdoll {
    /// `userData`: 0 in all 72.
    pub user_data: u32,
    /// `transforms`.
    pub transforms: LocalTransforms,
    /// `ragdollMotors`.
    pub motors: RagdollMotors,
    /// `angFriction`: a torque of 0.7 (56), 2.0 (8) or 3.0 (8), one axis count of 3.
    pub friction: AngFriction,
    /// `twistLimit`.
    pub twist: TwistLimit,
    /// `coneLimit`.
    pub cone: ConeLimit,
    /// `planesLimit`.
    pub planes: ConeLimit,
    /// `ballSocket`.
    pub ball_socket: BallSocket,
}

/// `hkpLimitedHingeConstraintData` (7.1): a hinge with an angle limit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LimitedHinge {
    /// `userData`: 0 in all 89.
    pub user_data: u32,
    /// `transforms`.
    pub transforms: LocalTransforms,
    /// `angMotor`.
    pub motor: AngMotor,
    /// `angFriction`: a torque of 0 (80) or 5.0 (9), one axis.
    pub friction: AngFriction,
    /// `angLimit`.
    pub limit: AngLimit,
    /// `2dAng`'s `freeRotationAxis`: 0 in all 89.
    pub free_rotation_axis: u8,
    /// `ballSocket`.
    pub ball_socket: BallSocket,
}

/// `hkpBreakableConstraintData` (7.1): a constraint that gives way under load, wrapping another.
#[derive(Debug, Clone, PartialEq)]
pub struct Breakable {
    /// `userData`: 0 in all 16.
    pub user_data: u32,
    /// The wrapped constraint (`constraintData`): a limited hinge in all 16.
    pub constraint: Box<ConstraintData>,
    /// `childRuntimeSize`: 80 in all 16 (the wrapped hinge's runtime size).
    pub child_runtime_size: u16,
    /// `childNumSolverResults`: 8 in all 16.
    pub child_num_solver_results: u16,
    /// `solverResultLimit`: 25.0 in 12 and 30.0 in 4.
    pub solver_result_limit: f32,
    /// `removeWhenBroken`: 0 in all 16.
    pub remove_when_broken: bool,
    /// `revertBackVelocityOnBreak`: 0 in all 16.
    pub revert_back_velocity_on_break: bool,
}

/// The constraint data an `hkpConstraintInstance` points to.
#[derive(Debug, Clone, PartialEq)]
pub enum ConstraintData {
    /// `hkpRagdollConstraintData`.
    Ragdoll(Ragdoll),
    /// `hkpLimitedHingeConstraintData`.
    LimitedHinge(LimitedHinge),
    /// `hkpBreakableConstraintData`.
    Breakable(Breakable),
}

/// The ball socket atom at `atom`.
fn ball_socket(pack: &Packfile<'_>, atom: usize) -> Result<BallSocket> {
    atom_type_is(pack, atom, atom_type::BALL_SOCKET, "ballSocket")?;
    Ok(BallSocket {
        bodies_to_notify: pack.u8(pack.at(atom, 2)?)?,
        stabilization_factor: pack.u8(pack.at(atom, 3)?)?,
        max_impulse: pack.f32(pack.at(atom, 4)?)?,
    })
}

fn twist_limit(pack: &Packfile<'_>, atom: usize) -> Result<TwistLimit> {
    atom_type_is(pack, atom, atom_type::TWIST_LIMIT, "twistLimit")?;
    zero(pack, pack.at(atom, 5)?, 3, "the padding of the twistLimit atom")?;
    Ok(TwistLimit {
        enabled: flag(pack, pack.at(atom, 2)?, "twistLimit.isEnabled")?,
        twist_axis: pack.u8(pack.at(atom, 3)?)?,
        ref_axis: pack.u8(pack.at(atom, 4)?)?,
        min_angle: pack.f32(pack.at(atom, 8)?)?,
        max_angle: pack.f32(pack.at(atom, 0xC)?)?,
        tau_factor: pack.f32(pack.at(atom, 0x10)?)?,
    })
}

fn cone_limit(pack: &Packfile<'_>, atom: usize, what: &str) -> Result<ConeLimit> {
    atom_type_is(pack, atom, atom_type::CONE_LIMIT, what)?;
    zero(pack, pack.at(atom, 7)?, 1, "the padding of a cone limit atom")?;
    Ok(ConeLimit {
        enabled: flag(pack, pack.at(atom, 2)?, "coneLimit.isEnabled")?,
        twist_axis_in_a: pack.u8(pack.at(atom, 3)?)?,
        ref_axis_in_b: pack.u8(pack.at(atom, 4)?)?,
        angle_measurement_mode: pack.u8(pack.at(atom, 5)?)?,
        mem_offset_to_angle_offset: pack.u8(pack.at(atom, 6)?)?,
        min_angle: pack.f32(pack.at(atom, 8)?)?,
        max_angle: pack.f32(pack.at(atom, 0xC)?)?,
        tau_factor: pack.f32(pack.at(atom, 0x10)?)?,
    })
}

fn ang_limit(pack: &Packfile<'_>, atom: usize) -> Result<AngLimit> {
    atom_type_is(pack, atom, atom_type::ANG_LIMIT, "angLimit")?;
    Ok(AngLimit {
        enabled: flag(pack, pack.at(atom, 2)?, "angLimit.isEnabled")?,
        limit_axis: pack.u8(pack.at(atom, 3)?)?,
        min_angle: pack.f32(pack.at(atom, 4)?)?,
        max_angle: pack.f32(pack.at(atom, 8)?)?,
        tau_factor: pack.f32(pack.at(atom, 0xC)?)?,
    })
}

fn ang_motor(pack: &Packfile<'_>, atom: usize) -> Result<AngMotor> {
    atom_type_is(pack, atom, atom_type::ANG_MOTOR, "angMotor")?;
    zero(pack, pack.at(atom, 0xA)?, 2, "the padding of the angMotor atom")?;
    null(pack, pack.at(atom, 0x10)?, "the angMotor's motor")?;
    Ok(AngMotor {
        enabled: flag(pack, pack.at(atom, 2)?, "angMotor.isEnabled")?,
        motor_axis: pack.u8(pack.at(atom, 3)?)?,
        initialized_offset: int16(pack, pack.at(atom, 4)?)?,
        previous_target_angle_offset: int16(pack, pack.at(atom, 6)?)?,
        corresponding_ang_limit_solver_result_offset: int16(pack, pack.at(atom, 8)?)?,
        target_angle: pack.f32(pack.at(atom, 0xC)?)?,
    })
}

fn ragdoll_motors(pack: &Packfile<'_>, atom: usize) -> Result<RagdollMotors> {
    atom_type_is(pack, atom, atom_type::RAGDOLL_MOTOR, "ragdollMotors")?;
    zero(
        pack,
        pack.at(atom, 8)?,
        8,
        "the padding of the ragdollMotors atom",
    )?;
    let column = |i: usize| -> Result<[f32; 3]> {
        pack.floats::<3>(pack.at(atom, 0x10usize.saturating_add(i.saturating_mul(16)))?)
    };
    for i in 0..3usize {
        null(
            pack,
            pack.at(atom, 0x40usize.saturating_add(i.saturating_mul(4)))?,
            "a ragdoll motor",
        )?;
    }
    zero(pack, pack.at(atom, 0x4C)?, 4, "the end of the ragdollMotors atom")?;
    Ok(RagdollMotors {
        enabled: flag(pack, pack.at(atom, 2)?, "ragdollMotors.isEnabled")?,
        initialized_offset: int16(pack, pack.at(atom, 4)?)?,
        previous_target_angles_offset: int16(pack, pack.at(atom, 6)?)?,
        target_b_rca: [column(0)?, column(1)?, column(2)?],
    })
}

/// The class of the object at `offset`, which must be `want`.
fn expect_class(pack: &Packfile<'_>, offset: usize, want: &str) -> Result<()> {
    let found = pack.class_at(offset)?;
    if found == want {
        Ok(())
    } else {
        Err(pack.err(offset, format!("a {found}, not a {want}")))
    }
}

impl Ragdoll {
    /// The `hkpRagdollConstraintData` at data-section `offset`.
    pub fn read(pack: &Packfile<'_>, offset: usize) -> Result<Self> {
        pack.require_710()?;
        expect_class(pack, offset, "hkpRagdollConstraintData")?;
        let at = |add| pack.at(offset, add);
        zero(pack, at(0x0C)?, 4, "the start of the atoms")?;
        Ok(Self {
            user_data: pack.u32(at(0x08)?)?,
            transforms: local_transforms(pack, at(0x10)?)?,
            motors: ragdoll_motors(pack, at(0xA0)?)?,
            friction: ang_friction(pack, at(0xF0)?)?,
            twist: twist_limit(pack, at(0xFC)?)?,
            cone: cone_limit(pack, at(0x110)?, "coneLimit")?,
            planes: cone_limit(pack, at(0x124)?, "planesLimit")?,
            ball_socket: ball_socket(pack, at(0x138)?)?,
        })
    }
}

impl LimitedHinge {
    /// The `hkpLimitedHingeConstraintData` at data-section `offset`.
    pub fn read(pack: &Packfile<'_>, offset: usize) -> Result<Self> {
        pack.require_710()?;
        expect_class(pack, offset, "hkpLimitedHingeConstraintData")?;
        let at = |add| pack.at(offset, add);
        zero(pack, at(0x0C)?, 4, "the start of the atoms")?;
        let two_d = at(0xD0)?;
        atom_type_is(pack, two_d, atom_type::ANG_2D, "2dAng")?;
        zero(pack, pack.at(two_d, 3)?, 1, "the padding of the 2dAng atom")?;
        zero(pack, at(0xDC)?, 4, "the end of the hinge")?;
        Ok(Self {
            user_data: pack.u32(at(0x08)?)?,
            transforms: local_transforms(pack, at(0x10)?)?,
            motor: ang_motor(pack, at(0xA0)?)?,
            friction: ang_friction(pack, at(0xB4)?)?,
            limit: ang_limit(pack, at(0xC0)?)?,
            free_rotation_axis: pack.u8(pack.at(two_d, 2)?)?,
            ball_socket: ball_socket(pack, at(0xD4)?)?,
        })
    }
}

impl Breakable {
    /// The `hkpBreakableConstraintData` at data-section `offset`, with the constraint it wraps.
    pub fn read(pack: &Packfile<'_>, offset: usize) -> Result<Self> {
        pack.require_710()?;
        expect_class(pack, offset, "hkpBreakableConstraintData")?;
        let at = |add| pack.at(offset, add);
        atom_type_is(pack, at(0x0C)?, atom_type::BRIDGE, "bridge")?;
        zero(pack, at(0x0E)?, 2, "the padding of the bridge atom")?;
        null(pack, at(0x10)?, "the bridge's buildJacobianFunc")?;
        if pack.target(at(0x14)?, "the bridge's constraint data")? != offset {
            return Err(pack.err(
                at(0x14)?,
                "the bridge atom does not point back at its own constraint data",
            ));
        }
        let child = pack.target(at(0x18)?, "the wrapped constraint data")?;
        if pack.class_at(child)? == "hkpBreakableConstraintData" {
            return Err(pack.err(at(0x18)?, "a breakable constraint inside a breakable constraint"));
        }
        zero(pack, at(0x26)?, 10, "the end of the breakable constraint data")?;
        Ok(Self {
            user_data: pack.u32(at(0x08)?)?,
            constraint: Box::new(ConstraintData::read(pack, child)?),
            child_runtime_size: pack.u16(at(0x1C)?)?,
            child_num_solver_results: pack.u16(at(0x1E)?)?,
            solver_result_limit: pack.f32(at(0x20)?)?,
            remove_when_broken: flag(pack, at(0x24)?, "removeWhenBroken")?,
            revert_back_velocity_on_break: flag(pack, at(0x25)?, "revertBackVelocityOnBreak")?,
        })
    }
}

impl ConstraintData {
    /// The constraint data object at data-section `offset`: a ragdoll, a limited hinge, or a breakable constraint around
    /// a limited hinge (a class this module has no view for is an error naming it).
    pub fn read(pack: &Packfile<'_>, offset: usize) -> Result<Self> {
        pack.require_710()?;
        match pack.class_at(offset)? {
            "hkpRagdollConstraintData" => Ok(Self::Ragdoll(Ragdoll::read(pack, offset)?)),
            "hkpLimitedHingeConstraintData" => Ok(Self::LimitedHinge(LimitedHinge::read(pack, offset)?)),
            "hkpBreakableConstraintData" => Ok(Self::Breakable(Breakable::read(pack, offset)?)),
            other => Err(pack.err(offset, format!("no view for constraint data class {other}"))),
        }
    }
}

/// An `hkpConstraintInstance`: two rigid bodies, the constraint data that joins them and a name.
#[derive(Debug, Clone, PartialEq)]
pub struct ConstraintInstance {
    /// The instance's data-section offset.
    pub offset: usize,
    /// The constraint data object's offset (`data`, `+0x0C`); read it with [`ConstraintInstance::constraint`].
    pub data: usize,
    /// The two bodies (`entities`, `+0x14` and `+0x18`), as `hkpRigidBody` offsets; distinct in all 161.
    pub entities: [usize; 2],
    /// `priority`: [`PRIORITY_PSI`] in all 161.
    pub priority: u8,
    /// `wantRuntime`: true in all 161.
    pub want_runtime: bool,
    /// `destructionRemapInfo`: 0 (`ON_DESTRUCTION_REMAP`) in all 161.
    pub destruction_remap_info: u8,
    /// The instance's name (`name`, `+0x28`).
    pub name: String,
    /// `userData`: 0 in all 161.
    pub user_data: u32,
}

impl ConstraintInstance {
    /// The `hkpConstraintInstance` at data-section `offset`. The constraint data is not read (see
    /// [`constraint`](Self::constraint)) but its class must be one this module has a view for.
    pub fn read(pack: &Packfile<'_>, offset: usize) -> Result<Self> {
        pack.require_710()?;
        expect_class(pack, offset, "hkpConstraintInstance")?;
        let at = |add| pack.at(offset, add);
        zero(pack, offset, 8, "the start of a constraint instance")?;
        null(pack, at(0x08)?, "the owner island")?;
        null(pack, at(0x10)?, "the constraint modifiers")?;
        zero(pack, at(0x1F)?, 1, "the padding after destructionRemapInfo")?;
        zero(pack, at(0x20)?, 8, "the listeners")?;
        zero(pack, at(0x30)?, 8, "the runtime pointer and uid")?;
        let data = pack.target(at(0x0C)?, "the constraint data")?;
        if !matches!(
            pack.class_at(data)?,
            "hkpRagdollConstraintData" | "hkpLimitedHingeConstraintData" | "hkpBreakableConstraintData"
        ) {
            return Err(pack.err(
                at(0x0C)?,
                "the constraint data is of a class this module has no view for",
            ));
        }
        let entity = |add| -> Result<usize> {
            let slot = at(add)?;
            let body = pack.target(slot, "a constraint entity")?;
            expect_class(pack, body, "hkpRigidBody")?;
            Ok(body)
        };
        Ok(Self {
            offset,
            data,
            entities: [entity(0x14)?, entity(0x18)?],
            priority: pack.u8(at(0x1C)?)?,
            want_runtime: flag(pack, at(0x1D)?, "wantRuntime")?,
            destruction_remap_info: pack.u8(at(0x1E)?)?,
            name: pack.c_string(pack.target(at(0x28)?, "the constraint name")?)?,
            user_data: pack.u32(at(0x2C)?)?,
        })
    }

    /// The constraint data this instance points to, with its atoms.
    pub fn constraint(&self, pack: &Packfile<'_>) -> Result<ConstraintData> {
        ConstraintData::read(pack, self.data)
    }
}

/// The objects of `class` an array of pointers at `header` names, in order; any other class is an error.
fn pointer_array(pack: &Packfile<'_>, header: usize, class: &str, what: &str) -> Result<Vec<usize>> {
    let array = pack.array_header(header)?;
    let mut out = Vec::new();
    for i in 0..array.len {
        let slot = pack.at(array.data.unwrap_or(0), i.saturating_mul(4))?;
        let target = pack.target(slot, what)?;
        expect_class(pack, target, class)?;
        out.push(target);
    }
    Ok(out)
}

/// An `hkpPhysicsSystem`: rigid bodies and the constraints between them.
#[derive(Debug, Clone, PartialEq)]
pub struct PhysicsSystem {
    /// The system's data-section offset.
    pub offset: usize,
    /// The rigid bodies (`rigidBodies`, `+0x08`), as `hkpRigidBody` offsets in data order. 100 of the 972 systems have
    /// some; every rigid body of a file is in exactly one system (12,789 bodies in 490 files).
    pub rigid_bodies: Vec<usize>,
    /// The constraint instances (`constraints`, `+0x14`). Only 10 systems have any: the 8 ragdolls' (17 or 19 each),
    /// FOXTROT's base file's (1) and JULIET's (16). The 17 of those two files sit in a system with no bodies, whose
    /// instances join bodies of the file's other (inactive) system.
    pub constraints: Vec<usize>,
    /// `name`: `Default Physics System` in the 8 ragdolls' systems, none in the other 964.
    pub name: Option<String>,
    /// `userData`: 0 in all 972.
    pub user_data: u32,
    /// `active`: true in 490 systems (one per file), false in 482.
    pub active: bool,
}

impl PhysicsSystem {
    /// The `hkpPhysicsSystem` at data-section `offset`. Refused: a system with actions or phantoms (none of the 972
    /// has one), a body or constraint of another class, a non-zero padding byte after `active`.
    pub fn read(pack: &Packfile<'_>, offset: usize) -> Result<Self> {
        pack.require_710()?;
        expect_class(pack, offset, "hkpPhysicsSystem")?;
        let at = |add| pack.at(offset, add);
        zero(pack, offset, 8, "the start of a physics system")?;
        for (add, what) in [(0x20, "actions"), (0x2C, "phantoms")] {
            if pack.array_header(at(add)?)?.len != 0 {
                return Err(pack.err(at(add)?, format!("a physics system with {what}")));
            }
        }
        zero(pack, at(0x41)?, 3, "the padding after active")?;
        Ok(Self {
            offset,
            rigid_bodies: pointer_array(pack, at(0x08)?, "hkpRigidBody", "a system's rigid body")?,
            constraints: pointer_array(
                pack,
                at(0x14)?,
                "hkpConstraintInstance",
                "a system's constraint instance",
            )?,
            name: pack
                .pointer(at(0x38)?)?
                .map(|name| pack.c_string(name))
                .transpose()?,
            user_data: pack.u32(at(0x3C)?)?,
            active: flag(pack, at(0x40)?, "active")?,
        })
    }

    /// The system's rigid bodies, read.
    pub fn bodies(&self, pack: &Packfile<'_>) -> Result<Vec<RigidBody>> {
        self.rigid_bodies
            .iter()
            .map(|&b| RigidBody::read(pack, b))
            .collect()
    }

    /// The system's constraint instances, read.
    pub fn instances(&self, pack: &Packfile<'_>) -> Result<Vec<ConstraintInstance>> {
        self.constraints
            .iter()
            .map(|&c| ConstraintInstance::read(pack, c))
            .collect()
    }
}

/// An `hkpPhysicsData`: the systems of a file.
#[derive(Debug, Clone, PartialEq)]
pub struct PhysicsData {
    /// The object's data-section offset.
    pub offset: usize,
    /// The name of the root container's variant that holds it, when it was found through the root
    /// ([`from_root`](Self::from_root)): `hkpPhysicsData` in 482 files, `Physics Data` in the 8 ragdolls.
    pub variant_name: Option<String>,
    /// The systems (`systems`, `+0x0C`), as `hkpPhysicsSystem` offsets: two in 482 files, one in the 8 ragdolls.
    pub systems: Vec<usize>,
}

impl PhysicsData {
    /// The `hkpPhysicsData` at data-section `offset`. Its `worldCinfo` must be null: it is in all 490 files, so no
    /// `hkpWorldCinfo` (and no world collision filter) is read here.
    pub fn read(pack: &Packfile<'_>, offset: usize) -> Result<Self> {
        pack.require_710()?;
        expect_class(pack, offset, "hkpPhysicsData")?;
        let at = |add| pack.at(offset, add);
        zero(pack, offset, 8, "the start of the physics data")?;
        null(pack, at(0x08)?, "worldCinfo")?;
        Ok(Self {
            offset,
            variant_name: None,
            systems: pointer_array(pack, at(0x0C)?, "hkpPhysicsSystem", "a physics system")?,
        })
    }

    /// The physics data the root container (`hkRootLevelContainer`, whose `namedVariants` array is at `+0x00`) holds:
    /// the one variant of class `hkpPhysicsData`. Refused: no such variant, or two.
    pub fn from_root(pack: &Packfile<'_>) -> Result<Self> {
        pack.require_710()?;
        let root = pack.root();
        expect_class(pack, root, "hkRootLevelContainer")?;
        let variants = pack.array_header(root)?;
        let mut found: Option<(String, usize)> = None;
        for i in 0..variants.len {
            let record = pack.at(variants.data.unwrap_or(0), i.saturating_mul(12))?;
            let name = pack.c_string(pack.target(record, "a variant name")?)?;
            let class = pack.c_string(pack.target(pack.at(record, 4)?, "a variant class name")?)?;
            let object = pack.target(pack.at(record, 8)?, "a variant object")?;
            if class == "hkpPhysicsData" {
                if found.is_some() {
                    return Err(pack.err(record, "the root container holds two hkpPhysicsData variants"));
                }
                found = Some((name, object));
            }
        }
        let (name, object) =
            found.ok_or_else(|| pack.err(root, "the root container holds no hkpPhysicsData variant"))?;
        let mut data = Self::read(pack, object)?;
        data.variant_name = Some(name);
        Ok(data)
    }

    /// The systems, read.
    pub fn physics_systems(&self, pack: &Packfile<'_>) -> Result<Vec<PhysicsSystem>> {
        self.systems
            .iter()
            .map(|&s| PhysicsSystem::read(pack, s))
            .collect()
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_methods,
    reason = "a test reads the corpus's files; the library itself takes bytes"
)]
mod tests {
    use super::*;
    use crate::inputs::{Game, OracleSet, Query, expect_found, resolve};
    use crate::packfile::reflection::ClassTable;

    /// The offset a member is stored at, in the declaring class of the table.
    fn stored(table: &ClassTable, class: &str, member: &str) -> usize {
        table
            .class(class)
            .unwrap_or_else(|| panic!("no class {class}"))
            .members
            .iter()
            .find(|m| m.name == member)
            .unwrap_or_else(|| panic!("no member {class}.{member}"))
            .offset
            .unwrap()
    }

    /// The sizes the metadata states for the classes this module reads.
    fn class_size(table: &ClassTable, class: &str) -> usize {
        table.class(class).unwrap().size.unwrap()
    }

    /// Every offset this module reads that the eight ragdoll files' own class metadata covers equals the stored one:
    /// the instance, the system, the physics data, the atoms of the ragdoll and limited hinge data (where each atom
    /// starts, and the members of each atom). Skips without the SHRED corpus.
    #[test]
    fn the_offsets_are_the_ones_the_ragdoll_files_metadata_states() {
        let query = Query::Oracle(OracleSet::Corpus, Game::Shred, "havok-packfiles");
        let Some(dir) = resolve(query) else {
            return;
        };
        let mut checked = 0usize;
        let mut names: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("hkx")))
            .collect();
        names.sort();
        expect_found(query, "ragdoll .HKX files", names.len());
        assert_eq!(
            names.len(),
            8,
            "the SHRED disc has 8 ragdoll files with class metadata"
        );
        for path in names {
            let bytes = std::fs::read(&path).unwrap();
            let pack = Packfile::read(&bytes).unwrap();
            let t = ClassTable::from_packfile(&pack).unwrap().unwrap();
            let want: &[(&str, &str, usize)] = &[
                ("hkpPhysicsData", "worldCinfo", 0x08),
                ("hkpPhysicsData", "systems", 0x0C),
                ("hkpPhysicsSystem", "rigidBodies", 0x08),
                ("hkpPhysicsSystem", "constraints", 0x14),
                ("hkpPhysicsSystem", "actions", 0x20),
                ("hkpPhysicsSystem", "phantoms", 0x2C),
                ("hkpPhysicsSystem", "name", 0x38),
                ("hkpPhysicsSystem", "userData", 0x3C),
                ("hkpPhysicsSystem", "active", 0x40),
                ("hkpConstraintInstance", "data", 0x0C),
                ("hkpConstraintInstance", "constraintModifiers", 0x10),
                ("hkpConstraintInstance", "entities", 0x14),
                ("hkpConstraintInstance", "priority", 0x1C),
                ("hkpConstraintInstance", "wantRuntime", 0x1D),
                ("hkpConstraintInstance", "destructionRemapInfo", 0x1E),
                ("hkpConstraintInstance", "listeners", 0x20),
                ("hkpConstraintInstance", "name", 0x28),
                ("hkpConstraintInstance", "userData", 0x2C),
                ("hkpConstraintInstance", "internal", 0x30),
                ("hkpConstraintInstance", "uid", 0x34),
                ("hkpConstraintData", "userData", 0x08),
                ("hkpRagdollConstraintData", "atoms", 0x10),
                ("hkpLimitedHingeConstraintData", "atoms", 0x10),
            ];
            for &(class, member, at) in want {
                assert_eq!(
                    stored(&t, class, member),
                    at,
                    "{class}.{member} in {}",
                    path.display()
                );
                checked += 1;
            }
            // where each atom starts, from the atoms struct at +0x10
            let atoms: &[(&str, &[(&str, usize)])] = &[
                (
                    "hkpRagdollConstraintDataAtoms",
                    &[
                        ("transforms", 0x10),
                        ("ragdollMotors", 0xA0),
                        ("angFriction", 0xF0),
                        ("twistLimit", 0xFC),
                        ("coneLimit", 0x110),
                        ("planesLimit", 0x124),
                        ("ballSocket", 0x138),
                    ],
                ),
                (
                    "hkpLimitedHingeConstraintDataAtoms",
                    &[
                        ("transforms", 0x10),
                        ("angMotor", 0xA0),
                        ("angFriction", 0xB4),
                        ("angLimit", 0xC0),
                        ("2dAng", 0xD0),
                        ("ballSocket", 0xD4),
                    ],
                ),
            ];
            for &(class, members) in atoms {
                for &(member, at) in members {
                    assert_eq!(0x10 + stored(&t, class, member), at, "{class}.{member}");
                    checked += 1;
                }
            }
            // the members of each atom, relative to its start
            let inside: &[(&str, &str, usize)] = &[
                ("hkpSetLocalTransformsConstraintAtom", "transformA", 0x10),
                ("hkpSetLocalTransformsConstraintAtom", "transformB", 0x50),
                ("hkpRagdollMotorConstraintAtom", "isEnabled", 0x02),
                ("hkpRagdollMotorConstraintAtom", "initializedOffset", 0x04),
                (
                    "hkpRagdollMotorConstraintAtom",
                    "previousTargetAnglesOffset",
                    0x06,
                ),
                ("hkpRagdollMotorConstraintAtom", "target_bRca", 0x10),
                ("hkpRagdollMotorConstraintAtom", "motors", 0x40),
                ("hkpAngFrictionConstraintAtom", "isEnabled", 0x02),
                ("hkpAngFrictionConstraintAtom", "firstFrictionAxis", 0x03),
                ("hkpAngFrictionConstraintAtom", "numFrictionAxes", 0x04),
                ("hkpAngFrictionConstraintAtom", "maxFrictionTorque", 0x08),
                ("hkpTwistLimitConstraintAtom", "isEnabled", 0x02),
                ("hkpTwistLimitConstraintAtom", "twistAxis", 0x03),
                ("hkpTwistLimitConstraintAtom", "refAxis", 0x04),
                ("hkpTwistLimitConstraintAtom", "minAngle", 0x08),
                ("hkpTwistLimitConstraintAtom", "maxAngle", 0x0C),
                ("hkpTwistLimitConstraintAtom", "angularLimitsTauFactor", 0x10),
                ("hkpConeLimitConstraintAtom", "isEnabled", 0x02),
                ("hkpConeLimitConstraintAtom", "twistAxisInA", 0x03),
                ("hkpConeLimitConstraintAtom", "refAxisInB", 0x04),
                ("hkpConeLimitConstraintAtom", "angleMeasurementMode", 0x05),
                ("hkpConeLimitConstraintAtom", "memOffsetToAngleOffset", 0x06),
                ("hkpConeLimitConstraintAtom", "minAngle", 0x08),
                ("hkpConeLimitConstraintAtom", "maxAngle", 0x0C),
                ("hkpConeLimitConstraintAtom", "angularLimitsTauFactor", 0x10),
                ("hkpBallSocketConstraintAtom", "bodiesToNotify", 0x02),
                ("hkpBallSocketConstraintAtom", "stabilizationFactor", 0x03),
                ("hkpBallSocketConstraintAtom", "maxImpulse", 0x04),
                ("hkpAngMotorConstraintAtom", "isEnabled", 0x02),
                ("hkpAngMotorConstraintAtom", "motorAxis", 0x03),
                ("hkpAngMotorConstraintAtom", "initializedOffset", 0x04),
                ("hkpAngMotorConstraintAtom", "previousTargetAngleOffset", 0x06),
                (
                    "hkpAngMotorConstraintAtom",
                    "correspondingAngLimitSolverResultOffset",
                    0x08,
                ),
                ("hkpAngMotorConstraintAtom", "targetAngle", 0x0C),
                ("hkpAngMotorConstraintAtom", "motor", 0x10),
                ("hkpAngLimitConstraintAtom", "isEnabled", 0x02),
                ("hkpAngLimitConstraintAtom", "limitAxis", 0x03),
                ("hkpAngLimitConstraintAtom", "minAngle", 0x04),
                ("hkpAngLimitConstraintAtom", "maxAngle", 0x08),
                ("hkpAngLimitConstraintAtom", "angularLimitsTauFactor", 0x0C),
                ("hkp2dAngConstraintAtom", "freeRotationAxis", 0x02),
            ];
            for &(class, member, at) in inside {
                assert_eq!(stored(&t, class, member), at, "{class}.{member}");
                checked += 1;
            }
            // the sizes that fix where each object and atom ends
            for (class, size) in [
                ("hkpRagdollConstraintData", 0x140),
                ("hkpLimitedHingeConstraintData", 0xE0),
                ("hkpRagdollConstraintDataAtoms", 0x130),
                ("hkpLimitedHingeConstraintDataAtoms", 0xD0),
                ("hkpSetLocalTransformsConstraintAtom", 0x90),
                ("hkpRagdollMotorConstraintAtom", 0x50),
                ("hkpAngFrictionConstraintAtom", 0xC),
                ("hkpTwistLimitConstraintAtom", 0x14),
                ("hkpConeLimitConstraintAtom", 0x14),
                ("hkpBallSocketConstraintAtom", 0x8),
                ("hkpAngMotorConstraintAtom", 0x14),
                ("hkpAngLimitConstraintAtom", 0x10),
                ("hkp2dAngConstraintAtom", 0x4),
                ("hkpPhysicsSystem", 0x44),
                ("hkpConstraintInstance", 0x38),
            ] {
                assert_eq!(class_size(&t, class), size, "size of {class}");
                checked += 1;
            }
        }
        assert!(checked > 0, "nothing was compared");
    }
}
