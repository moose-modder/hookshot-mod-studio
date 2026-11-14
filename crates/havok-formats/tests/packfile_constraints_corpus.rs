//! The physics data, systems, constraint instances and constraint data of the 490 Tony Hawk's SHRED (PS3) packfiles
//! (version 7, `Havok-7.1.0-r1`): every file is read from its root container, every system, instance and data object
//! reads (none refused), and the counts and tables below hold. The expected values are a census of the same 490 files
//! made by an independent Python reader (offsets taken from the ragdoll files' own class metadata, not from this crate;
//! its output: 972 systems, 161 instances, 72 ragdoll, 89 limited hinge and 16 breakable data objects, 12,789 rigid
//! bodies). Skips what is not configured (see tests/inputs/README.md); no game data is in the repository.
mod inputs;

use std::collections::{BTreeMap, BTreeSet};

use havok_formats::constraint::Transform;
use havok_formats::constraint::packfile::{ConstraintData, LimitedHinge, PRIORITY_PSI, PhysicsData, Ragdoll};
use havok_formats::packfile::Packfile;
use havok_formats::packfile::physics::FilterInfo;
use inputs::{Game, OracleSet, Query, expect_found, resolve};

const QUERY: Query = Query::Oracle(OracleSet::Corpus, Game::Shred, "havok-packfiles");

fn bits(x: f32) -> u32 {
    x.to_bits()
}

/// An angle (or any float) in ten-thousandths, rounded.
fn k(x: f32) -> i64 {
    (f64::from(x) * 10000.0).round() as i64
}

/// The largest deviation of a transform's columns from orthonormal.
fn orthonormality(t: &Transform) -> f64 {
    let dot = |a: &[f32; 3], b: &[f32; 3]| -> f64 {
        a.iter().zip(b).map(|(x, y)| f64::from(*x) * f64::from(*y)).sum()
    };
    let mut worst = 0.0f64;
    for (i, a) in t.columns.iter().enumerate() {
        for (j, b) in t.columns.iter().enumerate() {
            let want = if i == j { 1.0 } else { 0.0 };
            worst = worst.max((dot(a, b) - want).abs());
        }
    }
    worst
}

#[derive(Default)]
struct Tally {
    files: usize,
    variants: BTreeMap<String, usize>,
    system_counts: BTreeMap<usize, usize>,
    systems: usize,
    active: usize,
    systems_with_bodies: usize,
    systems_with_constraints: usize,
    named: BTreeMap<String, usize>,
    bodies: usize,
    layers: BTreeMap<u8, usize>,
    other_filter_bits: usize,
    instances: usize,
    instance_systems: BTreeMap<(usize, (usize, usize)), usize>,
    instance_constraint_counts: BTreeMap<usize, usize>,
    classes: BTreeMap<&'static str, usize>,
    hinge_limits: BTreeMap<(i64, i64), usize>,
    hinge_torque: BTreeMap<u32, usize>,
    ragdoll_torque: BTreeMap<u32, usize>,
    twist: BTreeMap<(i64, i64), usize>,
    cone_max: BTreeMap<i64, usize>,
    planes: BTreeMap<(i64, i64), usize>,
    solver_limits: BTreeMap<u32, usize>,
    worst_orthonormality: f64,
    transforms: usize,
}

fn hinge(t: &mut Tally, h: &LimitedHinge) {
    *t.classes.entry("hinge").or_default() += 1;
    assert_eq!(h.user_data, 0);
    for tr in [&h.transforms.transform_a, &h.transforms.transform_b] {
        t.worst_orthonormality = t.worst_orthonormality.max(orthonormality(tr));
        t.transforms += 1;
    }
    assert!(!h.motor.enabled && h.motor.motor_axis == 0 && h.motor.target_angle == 0.0);
    assert_eq!(
        (
            h.motor.initialized_offset,
            h.motor.previous_target_angle_offset,
            h.motor.corresponding_ang_limit_solver_result_offset
        ),
        (0x40, 0x44, 0x10)
    );
    assert!(h.friction.enabled);
    assert_eq!(
        (h.friction.first_friction_axis, h.friction.num_friction_axes),
        (0, 1)
    );
    *t.hinge_torque
        .entry(bits(h.friction.max_friction_torque))
        .or_default() += 1;
    assert!(h.limit.enabled && h.limit.limit_axis == 0 && h.limit.tau_factor == 1.0);
    assert!(h.limit.min_angle <= h.limit.max_angle);
    *t.hinge_limits
        .entry((k(h.limit.min_angle), k(h.limit.max_angle)))
        .or_default() += 1;
    assert_eq!(h.free_rotation_axis, 0);
    assert_eq!(
        (h.ball_socket.bodies_to_notify, h.ball_socket.stabilization_factor),
        (0, 0x30)
    );
    assert_eq!(bits(h.ball_socket.max_impulse), 0x7F7F_FFEE);
}

fn ragdoll(t: &mut Tally, r: &Ragdoll) {
    *t.classes.entry("ragdoll").or_default() += 1;
    assert_eq!(r.user_data, 0);
    for tr in [&r.transforms.transform_a, &r.transforms.transform_b] {
        t.worst_orthonormality = t.worst_orthonormality.max(orthonormality(tr));
        t.transforms += 1;
    }
    assert!(!r.motors.enabled);
    assert_eq!(
        (
            r.motors.initialized_offset,
            r.motors.previous_target_angles_offset
        ),
        (0x60, 0x64)
    );
    assert_eq!(r.motors.target_b_rca, r.transforms.transform_b.columns);
    assert!(r.friction.enabled);
    assert_eq!(
        (r.friction.first_friction_axis, r.friction.num_friction_axes),
        (0, 3)
    );
    *t.ragdoll_torque
        .entry(bits(r.friction.max_friction_torque))
        .or_default() += 1;
    assert!(r.twist.enabled);
    assert_eq!((r.twist.twist_axis, r.twist.ref_axis), (0, 1));
    assert!(r.twist.min_angle <= 0.0 && 0.0 <= r.twist.max_angle);
    assert_eq!(r.twist.tau_factor, 0.8);
    *t.twist
        .entry((k(r.twist.min_angle), k(r.twist.max_angle)))
        .or_default() += 1;
    assert!(r.cone.enabled && r.planes.enabled);
    assert_eq!(
        (
            r.cone.twist_axis_in_a,
            r.cone.ref_axis_in_b,
            r.cone.angle_measurement_mode,
            r.cone.mem_offset_to_angle_offset
        ),
        (0, 0, 0, 0x38)
    );
    assert_eq!(r.cone.min_angle, -100.0);
    assert!(0.0 <= r.cone.max_angle);
    assert_eq!(r.cone.tau_factor, 0.8);
    *t.cone_max.entry(k(r.cone.max_angle)).or_default() += 1;
    assert_eq!(
        (
            r.planes.twist_axis_in_a,
            r.planes.ref_axis_in_b,
            r.planes.angle_measurement_mode,
            r.planes.mem_offset_to_angle_offset
        ),
        (0, 1, 1, 0)
    );
    assert!(r.planes.min_angle <= 0.0 && 0.0 <= r.planes.max_angle);
    assert_eq!(r.planes.tau_factor, 0.8);
    *t.planes
        .entry((k(r.planes.min_angle), k(r.planes.max_angle)))
        .or_default() += 1;
    assert_eq!(
        (r.ball_socket.bodies_to_notify, r.ball_socket.stabilization_factor),
        (0, 0x30)
    );
    assert_eq!(bits(r.ball_socket.max_impulse), 0x7F7F_FFEE);
}

fn tally_all() -> Result<Option<Tally>, Box<dyn std::error::Error>> {
    let Some(dir) = resolve(QUERY) else {
        return Ok(None);
    };
    let mut paths = std::fs::read_dir(dir)?
        .map(|f| f.map(|f| f.path()))
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort();
    let mut t = Tally::default();
    for path in &paths {
        let bytes = std::fs::read(path)?;
        let name = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let pack = Packfile::read(&bytes).map_err(|e| format!("{name}: {e}"))?;
        let data = PhysicsData::from_root(&pack).map_err(|e| format!("{name}: {e}"))?;
        t.files += 1;
        *t.variants
            .entry(data.variant_name.clone().unwrap_or_default())
            .or_default() += 1;
        *t.system_counts.entry(data.systems.len()).or_default() += 1;
        let systems = data.physics_systems(&pack).map_err(|e| format!("{name}: {e}"))?;
        // which system holds each body
        let mut home: BTreeMap<usize, usize> = BTreeMap::new();
        for (si, system) in systems.iter().enumerate() {
            t.systems += 1;
            t.active += usize::from(system.active);
            assert_eq!(system.user_data, 0);
            if let Some(n) = &system.name {
                *t.named.entry(n.clone()).or_default() += 1;
            }
            if !system.rigid_bodies.is_empty() {
                t.systems_with_bodies += 1;
            }
            if !system.constraints.is_empty() {
                t.systems_with_constraints += 1;
                *t.instance_constraint_counts
                    .entry(system.constraints.len())
                    .or_default() += 1;
            }
            for &b in &system.rigid_bodies {
                assert!(home.insert(b, si).is_none(), "{name}: a body is in two systems");
            }
            for body in system.bodies(&pack).map_err(|e| format!("{name}: {e}"))? {
                t.bodies += 1;
                let f = body.filter();
                *t.layers.entry(f.layer()).or_default() += 1;
                if f.word() > FilterInfo::LAYER_MASK {
                    t.other_filter_bits += 1;
                }
            }
        }
        let all: BTreeSet<usize> = pack.objects_of("hkpRigidBody").collect();
        assert_eq!(
            home.keys().copied().collect::<BTreeSet<_>>(),
            all,
            "{name}: the systems' bodies are not the file's bodies"
        );
        for (si, system) in systems.iter().enumerate() {
            for instance in system.instances(&pack).map_err(|e| format!("{name}: {e}"))? {
                t.instances += 1;
                assert_eq!(instance.priority, PRIORITY_PSI);
                assert!(instance.want_runtime);
                assert_eq!((instance.destruction_remap_info, instance.user_data), (0, 0));
                assert_ne!(instance.entities[0], instance.entities[1]);
                let of = |b: usize| home.get(&b).copied().unwrap_or(usize::MAX);
                *t.instance_systems
                    .entry((si, (of(instance.entities[0]), of(instance.entities[1]))))
                    .or_default() += 1;
                match instance.constraint(&pack).map_err(|e| format!("{name}: {e}"))? {
                    ConstraintData::Ragdoll(r) => ragdoll(&mut t, &r),
                    ConstraintData::LimitedHinge(h) => hinge(&mut t, &h),
                    ConstraintData::Breakable(b) => {
                        *t.classes.entry("breakable").or_default() += 1;
                        assert_eq!(b.user_data, 0);
                        assert_eq!((b.child_runtime_size, b.child_num_solver_results), (0x50, 8));
                        assert!(!b.remove_when_broken && !b.revert_back_velocity_on_break);
                        *t.solver_limits.entry(bits(b.solver_result_limit)).or_default() += 1;
                        let ConstraintData::LimitedHinge(h) = *b.constraint else {
                            panic!("{name}: a breakable constraint wraps no limited hinge");
                        };
                        hinge(&mut t, &h);
                    }
                }
            }
        }
    }
    expect_found(QUERY, "packfiles", t.files);
    Ok(Some(t))
}

#[test]
fn the_physics_data_of_the_490_shred_files_reads_and_matches_the_census()
-> Result<(), Box<dyn std::error::Error>> {
    let Some(t) = tally_all()? else {
        return Ok(());
    };
    eprintln!(
        "worst orthonormality {:e} over {} transforms",
        t.worst_orthonormality, t.transforms
    );
    assert_eq!(t.files, 490);
    assert_eq!(
        t.variants,
        BTreeMap::from([
            ("Physics Data".to_string(), 8),
            ("hkpPhysicsData".to_string(), 482)
        ])
    );
    assert_eq!(t.system_counts, BTreeMap::from([(1, 8), (2, 482)]));
    assert_eq!((t.systems, t.active), (972, 490));
    assert_eq!((t.systems_with_bodies, t.systems_with_constraints), (100, 10));
    assert_eq!(
        t.named,
        BTreeMap::from([("Default Physics System".to_string(), 8)])
    );
    // 4 ragdoll files of 17 constraints, 4 of 19, FOXTROT's base file 1, JULIET's 16
    assert_eq!(
        t.instance_constraint_counts,
        BTreeMap::from([(1, 1), (16, 1), (17, 4), (19, 4)])
    );
    // every rigid body is in exactly one system (checked per file above); their filter words set the layer alone
    assert_eq!(t.bodies, 12_789);
    assert_eq!(
        t.layers,
        BTreeMap::from([(0, 148), (2, 1_933), (7, 8_741), (11, 1_953), (15, 1), (18, 13)])
    );
    assert_eq!(t.other_filter_bits, 0);
    // 161 instances: 144 in the ragdolls' own system with both bodies; 17 in a system with no bodies (FOXTROT, JULIET),
    // joining bodies of the file's first system
    assert_eq!(t.instances, 161);
    assert_eq!(
        t.instance_systems,
        BTreeMap::from([((0, (0, 0)), 144), ((1, (0, 0)), 17)])
    );
    // 72 ragdoll joints, 73 hinges and 16 breakable constraints around 16 more hinges (89 hinges)
    assert_eq!(
        t.classes,
        BTreeMap::from([("breakable", 16), ("hinge", 89), ("ragdoll", 72)])
    );
    assert_eq!(t.transforms, 2 * (72 + 89));
    assert!(t.worst_orthonormality < 1e-5, "{}", t.worst_orthonormality);
    assert_eq!(
        t.solver_limits,
        BTreeMap::from([(bits(25.0), 12), (bits(30.0), 4)])
    );
    assert_eq!(t.hinge_torque, BTreeMap::from([(bits(0.0), 80), (bits(5.0), 9)]));
    assert_eq!(
        t.ragdoll_torque,
        BTreeMap::from([(bits(0.7), 56), (bits(2.0), 8), (bits(3.0), 8)])
    );
    assert_eq!(
        t.hinge_limits,
        BTreeMap::from([
            ((-13087, 13087), 16),
            ((-8725, 3490), 12),
            ((-8725, 5235), 12),
            ((-5235, 5235), 8),
            ((-3490, 1745), 8),
            ((-523, 523), 8),
            ((-10470, 1745), 4),
            ((-3490, 3490), 4),
            ((-3490, 6980), 4),
            ((-1570, 0), 4),
            ((-174, 174), 4),
            ((-5235, 2617), 2),
            ((-3490, 5235), 2),
            ((-41880, 8725), 1),
        ])
    );
    assert_eq!(
        t.twist,
        BTreeMap::from([
            ((-872, 872), 34),
            ((-3490, 3490), 10),
            ((-1745, 1745), 10),
            ((-3839, 3839), 8),
            ((-214, 214), 8),
            ((-5235, 872), 2),
        ])
    );
    assert_eq!(
        t.cone_max,
        BTreeMap::from([
            (5235, 18),
            (4362, 14),
            (9597, 12),
            (4274, 8),
            (7852, 8),
            (8725, 8),
            (12215, 4)
        ])
    );
    assert_eq!(
        t.planes,
        BTreeMap::from([
            ((-15705, 12500), 8),
            ((-15278, 14423), 8),
            ((-8725, 0), 8),
            ((-3490, 3490), 8),
            ((-3490, 6980), 8),
            ((-12215, 0), 4),
            ((-7852, 0), 4),
            ((-6980, 9597), 4),
            ((-5235, 5235), 4),
            ((-4362, 4362), 4),
            ((-2617, 7852), 4),
            ((-1745, 4362), 2),
            ((-1396, 5235), 2),
            ((-872, 4362), 2),
            ((-872, 5235), 2),
        ])
    );
    Ok(())
}
