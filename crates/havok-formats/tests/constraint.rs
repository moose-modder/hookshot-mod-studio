//! A synthetic `hkpRagdollConstraintData` (the class layouts follow the member names and kinds of the Sekiro and Elden
//! Ring type tables; no game payload is embedded) read by `havok_formats::constraint`: the typed atoms, the floats that
//! are not finite, and the refusals.
#![allow(
    clippy::unwrap_used,
    reason = "a test fixture that does not parse is the test's failure"
)]
mod tagbuilder;

use havok_formats::Tagfile;
use havok_formats::constraint::{ConstraintData, read_constraint_data};
use tagbuilder::{Built, Definition, Layout, fixed, primitive, record, tagfile};

const SDK: &[u8; 8] = b"20180100";

/// Where each atom of `hkpRagdollConstraintData::Atoms` sits in the object (the atoms start 16 bytes in).
const TRANSFORMS: usize = 16;
const SETUP: usize = TRANSFORMS + 144;
const MOTORS: usize = SETUP + 16;
const FRICTION: usize = MOTORS + 88;
const TWIST: usize = FRICTION + 12;
const CONE: usize = TWIST + 24;
const PLANES: usize = CONE + 24;
const BALL: usize = PLANES + 24;
const SIZE: usize = BALL + 16;

fn definitions(class: &'static str) -> Vec<Definition> {
    vec![
        primitive("hkUint8", 0x2004, 1, None),         // 1
        primitive("hkUint16", 0x4004, 2, None),        // 2
        primitive("hkReal", 0x174605, 4, None),        // 3
        fixed("T[N]<hkReal,16>", 16, 3, 4),            // 4 hkTransform
        fixed("T[N]<hkReal,12>", 12, 3, 4),            // 5 hkMatrix3
        fixed("T[N]<hkReal,4>", 4, 3, 4),              // 6 hkVector4
        record("hkUFloat8", 1, vec![("value", 0, 1)]), // 7
        record(
            "hkpPositionConstraintMotor",
            28,
            vec![
                ("type", 0, 1),
                ("minForce", 4, 3),
                ("maxForce", 8, 3),
                ("tau", 12, 3),
                ("damping", 16, 3),
                ("proportionalRecoveryVelocity", 20, 3),
                ("constantRecoveryVelocity", 24, 3),
            ],
        ), // 8
        primitive("T*", 6, 8, Some(8)),                // 9
        fixed("T[N]<T*,3>", 3, 9, 8),                  // 10
        record(
            "hkpSetLocalTransformsConstraintAtom",
            144,
            vec![("type", 0, 2), ("transformA", 16, 4), ("transformB", 80, 4)],
        ), // 11
        record(
            "hkpSetupStabilizationAtom",
            16,
            vec![
                ("type", 0, 2),
                ("enabled", 2, 1),
                ("maxLinImpulse", 4, 3),
                ("maxAngImpulse", 8, 3),
                ("maxAngle", 12, 3),
            ],
        ), // 12
        record(
            "hkpRagdollMotorConstraintAtom",
            88,
            vec![
                ("type", 0, 2),
                ("isEnabled", 2, 1),
                ("initializedOffset", 4, 2),
                ("previousTargetAnglesOffset", 6, 2),
                ("target_bRca", 16, 5),
                ("motors", 64, 10),
            ],
        ), // 13
        record(
            "hkpAngFrictionConstraintAtom",
            12,
            vec![
                ("type", 0, 2),
                ("isEnabled", 2, 1),
                ("firstFrictionAxis", 3, 1),
                ("numFrictionAxes", 4, 1),
                ("maxFrictionTorque", 8, 3),
            ],
        ), // 14
        record(
            "hkpTwistLimitConstraintAtom",
            24,
            vec![
                ("type", 0, 2),
                ("isEnabled", 2, 1),
                ("twistAxis", 3, 1),
                ("refAxis", 4, 1),
                ("minAngle", 8, 3),
                ("maxAngle", 12, 3),
                ("angularLimitsTauFactor", 16, 3),
                ("angularLimitsDampFactor", 20, 3),
            ],
        ), // 15
        record(
            "hkpConeLimitConstraintAtom",
            24,
            vec![
                ("type", 0, 2),
                ("isEnabled", 2, 1),
                ("twistAxisInA", 3, 1),
                ("refAxisInB", 4, 1),
                ("angleMeasurementMode", 5, 1),
                ("memOffsetToAngleOffset", 6, 2),
                ("minAngle", 8, 3),
                ("maxAngle", 12, 3),
                ("angularLimitsTauFactor", 16, 3),
                ("angularLimitsDampFactor", 20, 3),
            ],
        ), // 16
        record(
            "hkpBallSocketConstraintAtom",
            16,
            vec![
                ("type", 0, 2),
                ("solvingMethod", 2, 1),
                ("bodiesToNotify", 3, 1),
                ("velocityStabilizationFactor", 4, 7),
                ("enableLinearImpulseLimit", 5, 1),
                ("breachImpulse", 8, 3),
                ("inertiaStabilizationFactor", 12, 3),
            ],
        ), // 17
        record(
            "hkpRagdollConstraintData::Atoms",
            u32::try_from(SIZE - TRANSFORMS).unwrap(),
            vec![
                ("transforms", 0, 11),
                (
                    "setupStabilization",
                    u32::try_from(SETUP - TRANSFORMS).unwrap(),
                    12,
                ),
                ("ragdollMotors", u32::try_from(MOTORS - TRANSFORMS).unwrap(), 13),
                ("angFriction", u32::try_from(FRICTION - TRANSFORMS).unwrap(), 14),
                ("twistLimit", u32::try_from(TWIST - TRANSFORMS).unwrap(), 15),
                ("coneLimit", u32::try_from(CONE - TRANSFORMS).unwrap(), 16),
                ("planesLimit", u32::try_from(PLANES - TRANSFORMS).unwrap(), 16),
                ("ballSocket", u32::try_from(BALL - TRANSFORMS).unwrap(), 17),
            ],
        ), // 18
        record(class, u32::try_from(SIZE).unwrap(), vec![("atoms", 16, 18)]), // 19
    ]
}

/// The atoms' `type` words, in the order of the offsets above.
const TYPES: [(usize, u16); 8] = [
    (TRANSFORMS, 2),
    (SETUP, 23),
    (MOTORS, 19),
    (FRICTION, 17),
    (TWIST, 15),
    (CONE, 16),
    (PLANES, 16),
    (BALL, 5),
];

/// A ragdoll constraint data tagfile: `edit` changes its bytes before the file is written; `motor` puts a motor object
/// behind the second of the three motor pointers.
fn fixture(class: &'static str, motor: bool, edit: impl FnOnce(&mut Layout, usize)) -> Built<Vec<u8>> {
    let mut layout = Layout::default();
    let (_, root) = layout.alloc(19, SIZE, 1, false)?;
    for (at, kind) in TYPES {
        layout.put(root + at, &kind.to_le_bytes());
    }
    // the flags the stored atoms have: stabilization, friction and the limits on
    for at in [SETUP + 2, FRICTION + 2, TWIST + 2, CONE + 2, PLANES + 2] {
        layout.put(root + at, &[1]);
    }
    layout.put(root + FRICTION + 4, &[3]);
    layout.put(root + TWIST + 4, &[1]);
    layout.put(root + PLANES + 4, &[1]);
    layout.put(root + PLANES + 5, &[1]);
    layout.put(root + BALL + 4, &[48]);
    for (at, x) in [
        (TWIST + 8, -0.5f32),
        (TWIST + 12, 0.25),
        (CONE + 8, -100.0),
        (CONE + 12, 1.5),
    ] {
        layout.put(root + at, &x.to_le_bytes());
    }
    // the first frame: the identity rotation and the translation (1, 2, 3), the fourth lane of each column arbitrary
    let frame: [f32; 16] = [1., 0., 0., 7., 0., 1., 0., 8., 0., 0., 1., 9., 1., 2., 3., 4.];
    for (i, x) in frame.iter().enumerate() {
        layout.put(root + TRANSFORMS + 16 + 4 * i, &x.to_le_bytes());
    }
    if motor {
        let (item, at) = layout.alloc(8, 28, 1, false)?;
        layout.put(at, &[1]);
        layout.put(at + 4, &(-10f32).to_le_bytes());
        layout.put(at + 8, &20f32.to_le_bytes());
        layout.put(at + 12, &0.8f32.to_le_bytes());
        layout.put(root + MOTORS + 64 + 8, &item.to_le_bytes());
    }
    edit(&mut layout, root);
    tagfile(SDK, &definitions(class), layout.data, &layout.items)
}

#[test]
fn a_ragdoll_constraint_reads_its_atoms() -> Built<()> {
    let bytes = fixture("hkpRagdollConstraintData", true, |_, _| {})?;
    let tag = Tagfile::read(&bytes, None).unwrap();
    let ConstraintData::Ragdoll(r) = read_constraint_data(tag.root().unwrap()).unwrap() else {
        panic!("not a ragdoll");
    };
    // the first frame: the rotation columns without their fourth lane, then the translation
    assert_eq!(
        r.transforms.transform_a.columns,
        [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]]
    );
    assert_eq!(r.transforms.transform_a.translation, [1., 2., 3.]);
    assert!(r.setup_stabilization.enabled);
    assert!(!r.motors.enabled);
    assert_eq!(r.friction.num_friction_axes, 3);
    assert!(r.friction.enabled);
    assert_eq!((r.twist.twist_axis, r.twist.ref_axis), (0, 1));
    assert_eq!((r.twist.min_angle, r.twist.max_angle), (-0.5, 0.25));
    assert_eq!(
        (r.cone.min_angle, r.cone.max_angle, r.cone.angle_measurement_mode),
        (-100.0, 1.5, 0)
    );
    assert_eq!((r.planes.ref_axis_in_b, r.planes.angle_measurement_mode), (1, 1));
    assert_eq!(r.ball_socket.velocity_stabilization_factor, 48);
    // the motors: a null pointer, a motor, a null pointer
    assert!(r.motors.motors[0].is_none() && r.motors.motors[2].is_none());
    let motor = r.motors.motors[1].unwrap();
    assert_eq!((motor.kind, motor.min_force, motor.max_force), (1, -10.0, 20.0));
    assert_eq!(motor.tau, 0.8);
    Ok(())
}

#[test]
fn floats_that_are_not_finite_are_kept_and_named() -> Built<()> {
    // a finite constraint names nothing
    let bytes = fixture("hkpRagdollConstraintData", true, |_, _| {})?;
    let tag = Tagfile::read(&bytes, None).unwrap();
    assert_eq!(
        read_constraint_data(tag.root().unwrap()).unwrap().non_finite(),
        []
    );
    // the second frame's rotation NaN in every lane (the fourth lanes too, which are not read), as stored, a cone angle
    // of minus infinity and a motor's tau of plus infinity
    let nan = f32::from_bits(0xffc0_0000);
    let bytes = fixture("hkpRagdollConstraintData", true, |layout, root| {
        for lane in 0..12 {
            layout.put(root + TRANSFORMS + 80 + 4 * lane, &nan.to_le_bytes());
        }
        layout.put(root + CONE + 8, &f32::NEG_INFINITY.to_le_bytes());
        // the motor object follows the root, 16-aligned; its tau is 12 bytes in
        layout.put(
            root + SIZE.next_multiple_of(16) + 12,
            &f32::INFINITY.to_le_bytes(),
        );
    })?;
    let tag = Tagfile::read(&bytes, None).unwrap();
    let data = read_constraint_data(tag.root().unwrap()).unwrap();
    let ConstraintData::Ragdoll(r) = &data else {
        panic!("not a ragdoll");
    };
    assert!(
        r.transforms
            .transform_b
            .columns
            .iter()
            .flatten()
            .all(|x| x.to_bits() == 0xffc0_0000)
    );
    assert_eq!(r.transforms.transform_b.translation, [0.0; 3]);
    assert_eq!(r.cone.min_angle, f32::NEG_INFINITY);
    let found = data.non_finite();
    let named: Vec<_> = found
        .iter()
        .map(|n| (n.member.as_str(), n.lanes.iter().map(|l| l.0).collect::<Vec<_>>()))
        .collect();
    assert_eq!(
        named,
        [
            ("transforms.transformB", vec![0, 1, 2, 4, 5, 6, 8, 9, 10]),
            ("ragdollMotors.motors[1].tau", vec![0]),
            ("coneLimit.minAngle", vec![0]),
        ]
    );
    assert_eq!(
        found[2].to_string(),
        "coneLimit.minAngle: lane 0 -inf (0xff800000)"
    );
    assert!(
        found[0]
            .to_string()
            .starts_with("transforms.transformB: lane 0 NaN (0xffc00000) lane 1 NaN"),
        "{}",
        found[0]
    );
    Ok(())
}

#[test]
fn an_atom_with_the_wrong_type_a_bad_flag_or_an_unknown_class_is_refused() -> Built<()> {
    // setupStabilization carries the type of a twist limit
    let bytes = fixture("hkpRagdollConstraintData", false, |layout, root| {
        layout.put(root + SETUP, &15u16.to_le_bytes());
    })?;
    let tag = Tagfile::read(&bytes, None).unwrap();
    let e = read_constraint_data(tag.root().unwrap()).unwrap_err().to_string();
    assert!(e.contains("atom setupStabilization has type 15, not 23"), "{e}");
    // a flag of 2
    let bytes = fixture("hkpRagdollConstraintData", false, |layout, root| {
        layout.put(root + TWIST + 2, &[2]);
    })?;
    let tag = Tagfile::read(&bytes, None).unwrap();
    let e = read_constraint_data(tag.root().unwrap()).unwrap_err().to_string();
    assert!(e.contains("isEnabled is 2, not a flag"), "{e}");
    // a class with no reader
    let bytes = fixture("hkpPrismaticConstraintData", false, |_, _| {})?;
    let tag = Tagfile::read(&bytes, None).unwrap();
    let e = read_constraint_data(tag.root().unwrap()).unwrap_err().to_string();
    assert!(
        e.contains("no reader for the constraint data class hkpPrismaticConstraintData"),
        "{e}"
    );
    Ok(())
}
