//! A hand-made miniature Havok 7.1.0-r1 packfile with a root container, physics data, a system, two rigid bodies (named
//! objects only), three constraint instances and the data they point to (a limited hinge, a breakable constraint around
//! a second hinge, a ragdoll joint): every value is written here by hand (no game data, no second reader), and read
//! back by `havok_formats::constraint::packfile`; then the refusals. The 490 Tony Hawk's SHRED files are read by
//! tests/packfile_constraints_corpus.rs.
#![allow(clippy::unwrap_used)]
use havok_formats::constraint::packfile::{ConstraintData, ConstraintInstance, PhysicsData, PhysicsSystem};
use havok_formats::packfile::physics::FilterInfo;
use havok_formats::packfile::{CLASS_SIGNATURES, Packfile};

/// Builds a packfile: class names (each with the signature [`CLASS_SIGNATURES`] lists, or `0x12345678` for a name it
/// does not), data bytes, local/global/virtual fixups.
struct Builder {
    classes: Vec<&'static str>,
    data: Vec<u8>,
    local: Vec<(u32, u32)>,
    global: Vec<(u32, u32)>,
    objects: Vec<(u32, usize)>,
}

impl Builder {
    fn new(size: usize) -> Self {
        Self {
            classes: Vec::new(),
            data: vec![0; size],
            local: Vec::new(),
            global: Vec::new(),
            objects: Vec::new(),
        }
    }
    fn object(&mut self, offset: u32, class: &'static str) {
        let c = self.classes.iter().position(|c| *c == class).unwrap_or_else(|| {
            self.classes.push(class);
            self.classes.len() - 1
        });
        self.objects.push((offset, c));
    }
    fn u16(&mut self, at: usize, v: u16) {
        self.data[at..at + 2].copy_from_slice(&v.to_be_bytes());
    }
    fn f32(&mut self, at: usize, v: f32) {
        self.data[at..at + 4].copy_from_slice(&v.to_be_bytes());
    }
    fn floats(&mut self, at: usize, v: &[f32]) {
        for (i, x) in v.iter().enumerate() {
            self.f32(at + 4 * i, *x);
        }
    }
    fn text(&mut self, at: usize, s: &str) {
        self.data[at..at + s.len()].copy_from_slice(s.as_bytes());
    }
    /// A pointer slot with a local fixup (to an array or string inside the same object).
    fn local(&mut self, at: usize, to: usize) {
        self.local.push((at as u32, to as u32));
    }
    /// A pointer slot with a global fixup (to another object).
    fn global(&mut self, at: usize, to: usize) {
        self.global.push((at as u32, to as u32));
    }
    /// Point the fixup of the pointer slot at `at` somewhere else.
    fn retarget(&mut self, at: usize, to: usize) {
        for (src, dst) in self.local.iter_mut().chain(&mut self.global) {
            if *src as usize == at {
                *dst = to as u32;
            }
        }
    }
    /// An hkArray header at `at`, its data at `to` (local fixup), `len` elements.
    fn array(&mut self, at: usize, to: Option<usize>, len: u32) {
        if let Some(to) = to {
            self.local(at, to);
        }
        self.data[at + 4..at + 8].copy_from_slice(&len.to_be_bytes());
        self.data[at + 8..at + 12].copy_from_slice(&(0x8000_0000 | len).to_be_bytes());
    }
    fn build(&self) -> Vec<u8> {
        let mut names = Vec::new();
        let mut name_at = Vec::new();
        for c in &self.classes {
            let signature = CLASS_SIGNATURES
                .iter()
                .find(|(name, _)| name == c)
                .map_or(0x1234_5678, |(_, s)| *s);
            names.extend(signature.to_be_bytes());
            names.push(9);
            name_at.push(names.len() as u32);
            names.extend(c.as_bytes());
            names.push(0);
        }
        names.extend([0xFF; 3]);
        let data_len = self.data.len() as u32;
        let mut section = self.data.clone();
        let local_at = data_len;
        for (s, d) in &self.local {
            section.extend(s.to_be_bytes());
            section.extend(d.to_be_bytes());
        }
        section.extend([0xFF; 8]);
        let global_at = section.len() as u32;
        for (s, d) in &self.global {
            section.extend(s.to_be_bytes());
            section.extend(2u32.to_be_bytes());
            section.extend(d.to_be_bytes());
        }
        section.extend([0xFF; 12]);
        let virtual_at = section.len() as u32;
        for (o, c) in &self.objects {
            section.extend(o.to_be_bytes());
            section.extend(0u32.to_be_bytes());
            section.extend(name_at[*c].to_be_bytes());
        }
        section.extend([0xFF; 12]);
        let end = section.len() as u32;
        let mut out = Vec::new();
        out.extend([0x57, 0xE0, 0xE0, 0x57, 0x10, 0xC0, 0xC0, 0x10]);
        out.extend(0u32.to_be_bytes());
        out.extend(7u32.to_be_bytes());
        out.extend([4, 0, 1, 1]);
        out.extend(3u32.to_be_bytes());
        out.extend(2u32.to_be_bytes());
        out.extend(0u32.to_be_bytes());
        out.extend(0u32.to_be_bytes());
        out.extend(name_at[self.objects[0].1].to_be_bytes());
        let mut version = b"Havok-7.1.0-r1".to_vec();
        version.resize(16, 0);
        out.extend(version);
        out.extend([0xFF; 8]);
        assert_eq!(out.len(), 0x40);
        let classnames_start = 0x40 + 3 * 48;
        let data_start = classnames_start + names.len();
        for (tag, start, vals) in [
            ("__classnames__", classnames_start, [names.len() as u32; 6]),
            ("__types__", data_start, [0; 6]),
            (
                "__data__",
                data_start,
                [local_at, global_at, virtual_at, end, end, end],
            ),
        ] {
            let mut t = tag.as_bytes().to_vec();
            t.resize(19, 0);
            t.push(0xFF);
            out.extend(t);
            out.extend((start as u32).to_be_bytes());
            for v in vals {
                out.extend(v.to_be_bytes());
            }
        }
        out.extend(names);
        out.extend(section);
        out
    }
}

const ROOT: usize = 0x000;
const DATA: usize = 0x050;
const SYSTEM: usize = 0x080;
const BODY_A: usize = 0x100;
const BODY_B: usize = 0x110;
const INSTANCE_HINGE: usize = 0x120;
const INSTANCE_BREAKABLE: usize = 0x170;
const HINGE: usize = 0x1C0;
const BREAKABLE: usize = 0x2A0;
const WRAPPED_HINGE: usize = 0x2D0;
const RAGDOLL: usize = 0x3B0;
const INSTANCE_RAGDOLL: usize = 0x4F0;

/// An `hkTransform` at `at`: the three columns (with a fourth lane each) and the translation (and its fourth lane).
fn put_transform(b: &mut Builder, at: usize, columns: [[f32; 3]; 3], lanes: [f32; 4], translation: [f32; 3]) {
    for (i, c) in columns.iter().enumerate() {
        b.floats(at + 16 * i, c);
        b.f32(at + 16 * i + 12, lanes[i]);
    }
    b.floats(at + 48, &translation);
    b.f32(at + 60, lanes[3]);
}

const TURN: [[f32; 3]; 3] = [[0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];
const IDENTITY: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// The set-local-transforms atom at `at` (type 2): frame A is the identity at (0.5, 0.25, -0.125) with a stray fourth
/// lane in each column; frame B turns a quarter about z at (1, 2, 3).
fn put_transforms(b: &mut Builder, at: usize) {
    b.u16(at, 2);
    put_transform(
        b,
        at + 0x10,
        IDENTITY,
        [0.75, -0.5, 0.0, 9.0],
        [0.5, 0.25, -0.125],
    );
    put_transform(b, at + 0x50, TURN, [0.0; 4], [1.0, 2.0, 3.0]);
}

fn put_ball_socket(b: &mut Builder, at: usize) {
    b.u16(at, 5);
    b.data[at + 3] = 0x30;
    b.f32(at + 4, f32::from_bits(0x7F7F_FFEE));
}

fn put_ang_friction(b: &mut Builder, at: usize, num_axes: u8, torque: f32) {
    b.u16(at, 17);
    b.data[at + 2] = 1;
    b.data[at + 4] = num_axes;
    b.f32(at + 8, torque);
}

/// A limited hinge at `at` with the angle limit `[min, max]`.
fn put_hinge(b: &mut Builder, at: usize, min: f32, max: f32) {
    b.object(at as u32, "hkpLimitedHingeConstraintData");
    put_transforms(b, at + 0x10);
    // angMotor: disabled, runtime offsets 0x40, 0x44, 0x10
    b.u16(at + 0xA0, 18);
    b.u16(at + 0xA4, 0x40);
    b.u16(at + 0xA6, 0x44);
    b.u16(at + 0xA8, 0x10);
    put_ang_friction(b, at + 0xB4, 1, 5.0);
    // angLimit
    b.u16(at + 0xC0, 14);
    b.data[at + 0xC2] = 1;
    b.f32(at + 0xC4, min);
    b.f32(at + 0xC8, max);
    b.f32(at + 0xCC, 1.0);
    // 2dAng
    b.u16(at + 0xD0, 12);
    put_ball_socket(b, at + 0xD4);
}

/// A ragdoll joint at `at`.
fn put_ragdoll(b: &mut Builder, at: usize) {
    b.object(at as u32, "hkpRagdollConstraintData");
    put_transforms(b, at + 0x10);
    // ragdollMotors: disabled, runtime offsets 0x60 and 0x64, target = frame B's rotation
    b.u16(at + 0xA0, 19);
    b.u16(at + 0xA4, 0x60);
    b.u16(at + 0xA6, 0x64);
    for (i, c) in TURN.iter().enumerate() {
        b.floats(at + 0xB0 + 16 * i, c);
    }
    put_ang_friction(b, at + 0xF0, 3, 0.7);
    // twistLimit
    b.u16(at + 0xFC, 15);
    b.data[at + 0xFE] = 1;
    b.data[at + 0x100] = 1;
    b.f32(at + 0x104, -0.5);
    b.f32(at + 0x108, 0.25);
    b.f32(at + 0x10C, 0.8);
    // coneLimit: measured from aligned vectors, 0x38 into the runtime
    b.u16(at + 0x110, 16);
    b.data[at + 0x112] = 1;
    b.data[at + 0x116] = 0x38;
    b.f32(at + 0x118, -100.0);
    b.f32(at + 0x11C, 1.0);
    b.f32(at + 0x120, 0.8);
    // planesLimit: reference axis 1, measured from perpendicular vectors
    b.u16(at + 0x124, 16);
    b.data[at + 0x126] = 1;
    b.data[at + 0x128] = 1;
    b.data[at + 0x129] = 1;
    b.f32(at + 0x12C, -0.75);
    b.f32(at + 0x130, 0.5);
    b.f32(at + 0x134, 0.8);
    put_ball_socket(b, at + 0x138);
}

/// A constraint instance at `at` joining the two bodies through the data at `data`.
fn put_instance(b: &mut Builder, at: usize, data: usize, name: &str) {
    b.object(at as u32, "hkpConstraintInstance");
    b.global(at + 0x0C, data);
    b.global(at + 0x14, BODY_A);
    b.global(at + 0x18, BODY_B);
    b.data[at + 0x1C] = 1;
    b.data[at + 0x1D] = 1;
    b.local(at + 0x28, at + 0x38);
    b.text(at + 0x38, name);
}

fn scene_builder() -> Builder {
    let mut b = Builder::new(0x540);
    b.object(ROOT as u32, "hkRootLevelContainer");
    b.array(ROOT, Some(0x10), 1);
    b.local(0x10, 0x20);
    b.text(0x20, "Physics Data");
    b.local(0x14, 0x30);
    b.text(0x30, "hkpPhysicsData");
    b.global(0x18, DATA);

    b.object(DATA as u32, "hkpPhysicsData");
    b.array(DATA + 0x0C, Some(0x70), 1);
    b.global(0x70, SYSTEM);

    b.object(SYSTEM as u32, "hkpPhysicsSystem");
    b.array(SYSTEM + 0x08, Some(0xD0), 2);
    b.array(SYSTEM + 0x14, Some(0xD8), 3);
    b.array(SYSTEM + 0x20, None, 0);
    b.array(SYSTEM + 0x2C, None, 0);
    b.local(SYSTEM + 0x38, 0xE4);
    b.data[SYSTEM + 0x40] = 1;
    b.global(0xD0, BODY_A);
    b.global(0xD4, BODY_B);
    b.global(0xD8, INSTANCE_HINGE);
    b.global(0xDC, INSTANCE_BREAKABLE);
    b.global(0xE0, INSTANCE_RAGDOLL);
    b.text(0xE4, "Default Physics System");

    b.object(BODY_A as u32, "hkpRigidBody");
    b.object(BODY_B as u32, "hkpRigidBody");

    put_instance(&mut b, INSTANCE_HINGE, HINGE, "Hinge1");
    put_instance(&mut b, INSTANCE_BREAKABLE, BREAKABLE, "Gate1");
    put_instance(&mut b, INSTANCE_RAGDOLL, RAGDOLL, "Ragdoll1");
    put_hinge(&mut b, HINGE, -1.5, 1.5);

    b.object(BREAKABLE as u32, "hkpBreakableConstraintData");
    b.data[BREAKABLE + 0x0D] = 1;
    b.global(BREAKABLE + 0x14, BREAKABLE);
    b.global(BREAKABLE + 0x18, WRAPPED_HINGE);
    b.u16(BREAKABLE + 0x1C, 0x50);
    b.u16(BREAKABLE + 0x1E, 8);
    b.f32(BREAKABLE + 0x20, 25.0);
    put_hinge(&mut b, WRAPPED_HINGE, -0.0523, 0.0523);

    put_ragdoll(&mut b, RAGDOLL);
    b
}

fn scene() -> Vec<u8> {
    scene_builder().build()
}

#[test]
fn the_root_leads_to_the_physics_data_and_its_system() {
    let bytes = scene();
    let p = Packfile::read(&bytes).unwrap();
    let data = PhysicsData::from_root(&p).unwrap();
    assert_eq!(data.offset, DATA);
    assert_eq!(data.variant_name.as_deref(), Some("Physics Data"));
    assert_eq!(data.systems, vec![SYSTEM]);
    let systems = data.physics_systems(&p).unwrap();
    assert_eq!(systems, vec![PhysicsSystem::read(&p, SYSTEM).unwrap()]);
    let system = &systems[0];
    assert_eq!(system.rigid_bodies, vec![BODY_A, BODY_B]);
    assert_eq!(
        system.constraints,
        vec![INSTANCE_HINGE, INSTANCE_BREAKABLE, INSTANCE_RAGDOLL]
    );
    assert_eq!(system.name.as_deref(), Some("Default Physics System"));
    assert!(system.active);
    assert_eq!(system.user_data, 0);
    // read directly, a physics data has no variant name
    assert_eq!(PhysicsData::read(&p, DATA).unwrap().variant_name, None);
}

#[test]
fn an_instance_names_its_bodies_and_its_data() {
    let bytes = scene();
    let p = Packfile::read(&bytes).unwrap();
    let system = PhysicsSystem::read(&p, SYSTEM).unwrap();
    let instances = system.instances(&p).unwrap();
    assert_eq!(instances.len(), 3);
    let first = &instances[0];
    assert_eq!(first, &ConstraintInstance::read(&p, INSTANCE_HINGE).unwrap());
    assert_eq!(first.data, HINGE);
    assert_eq!(first.entities, [BODY_A, BODY_B]);
    assert_eq!(
        (first.priority, first.want_runtime, first.destruction_remap_info),
        (1, true, 0)
    );
    assert_eq!(
        instances.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(),
        ["Hinge1", "Gate1", "Ragdoll1"]
    );
    assert_eq!(
        instances.iter().map(|i| i.data).collect::<Vec<_>>(),
        [HINGE, BREAKABLE, RAGDOLL]
    );
}

#[test]
fn a_limited_hinge_reads_with_its_atoms() {
    let bytes = scene();
    let p = Packfile::read(&bytes).unwrap();
    let ConstraintData::LimitedHinge(h) = ConstraintInstance::read(&p, INSTANCE_HINGE)
        .unwrap()
        .constraint(&p)
        .unwrap()
    else {
        panic!("not a limited hinge");
    };
    assert_eq!(h.user_data, 0);
    // the stray fourth lanes are not read
    assert_eq!(h.transforms.transform_a.columns, IDENTITY);
    assert_eq!(h.transforms.transform_a.translation, [0.5, 0.25, -0.125]);
    assert_eq!(h.transforms.transform_b.columns, TURN);
    assert_eq!(h.transforms.transform_b.translation, [1.0, 2.0, 3.0]);
    assert!(!h.motor.enabled);
    assert_eq!(
        (
            h.motor.initialized_offset,
            h.motor.previous_target_angle_offset,
            h.motor.corresponding_ang_limit_solver_result_offset,
            h.motor.target_angle
        ),
        (0x40, 0x44, 0x10, 0.0)
    );
    assert!(h.friction.enabled);
    assert_eq!(
        (h.friction.first_friction_axis, h.friction.num_friction_axes),
        (0, 1)
    );
    assert_eq!(h.friction.max_friction_torque, 5.0);
    assert!(h.limit.enabled);
    assert_eq!(
        (
            h.limit.limit_axis,
            h.limit.min_angle,
            h.limit.max_angle,
            h.limit.tau_factor
        ),
        (0, -1.5, 1.5, 1.0)
    );
    assert_eq!(h.free_rotation_axis, 0);
    assert_eq!(h.ball_socket.bodies_to_notify, 0);
    assert_eq!(h.ball_socket.stabilization_factor, 0x30);
    assert_eq!(h.ball_socket.max_impulse.to_bits(), 0x7F7F_FFEE);
}

#[test]
fn a_breakable_constraint_reads_the_hinge_it_wraps() {
    let bytes = scene();
    let p = Packfile::read(&bytes).unwrap();
    let ConstraintData::Breakable(b) = ConstraintData::read(&p, BREAKABLE).unwrap() else {
        panic!("not a breakable constraint");
    };
    assert_eq!((b.child_runtime_size, b.child_num_solver_results), (0x50, 8));
    assert_eq!(b.solver_result_limit, 25.0);
    assert!(!b.remove_when_broken && !b.revert_back_velocity_on_break);
    let ConstraintData::LimitedHinge(h) = *b.constraint else {
        panic!("the breakable constraint wraps no hinge");
    };
    assert_eq!((h.limit.min_angle, h.limit.max_angle), (-0.0523, 0.0523));
}

#[test]
fn a_ragdoll_joint_reads_with_its_atoms() {
    let bytes = scene();
    let p = Packfile::read(&bytes).unwrap();
    let ConstraintData::Ragdoll(r) = ConstraintData::read(&p, RAGDOLL).unwrap() else {
        panic!("not a ragdoll joint");
    };
    assert_eq!(r.transforms.transform_b.columns, TURN);
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
        (r.friction.num_friction_axes, r.friction.max_friction_torque),
        (3, 0.7)
    );
    assert_eq!(
        (
            r.twist.enabled,
            r.twist.twist_axis,
            r.twist.ref_axis,
            r.twist.min_angle,
            r.twist.max_angle,
            r.twist.tau_factor
        ),
        (true, 0, 1, -0.5, 0.25, 0.8)
    );
    assert_eq!(
        (
            r.cone.ref_axis_in_b,
            r.cone.angle_measurement_mode,
            r.cone.mem_offset_to_angle_offset,
            r.cone.min_angle,
            r.cone.max_angle
        ),
        (0, 0, 0x38, -100.0, 1.0)
    );
    assert_eq!(
        (
            r.planes.ref_axis_in_b,
            r.planes.angle_measurement_mode,
            r.planes.mem_offset_to_angle_offset,
            r.planes.min_angle,
            r.planes.max_angle
        ),
        (1, 1, 0, -0.75, 0.5)
    );
    assert!(r.cone.enabled && r.planes.enabled);
    assert_eq!((r.cone.tau_factor, r.planes.tau_factor), (0.8, 0.8));
    assert_eq!(r.ball_socket.stabilization_factor, 0x30);
}

#[test]
fn the_group_filter_split_follows_the_bit_fields() {
    // layer 18, sub-system id 3, ignores sub-system 5, system group 0x1234; bit 15 set and not part of the split
    let word = 18 | (3 << 5) | (5 << 10) | (1 << 15) | (0x1234 << 16);
    let f = FilterInfo(word);
    assert_eq!(
        (
            f.layer(),
            f.sub_system_id(),
            f.sub_system_dont_collide_with(),
            f.system_group()
        ),
        (18, 3, 5, 0x1234)
    );
    assert_eq!(f.word(), word);
    // the words of the SHRED bodies set the layer alone
    for layer in [0u32, 2, 7, 11, 15, 18] {
        let f = FilterInfo(layer);
        assert_eq!(u32::from(f.layer()), layer);
        assert_eq!(
            (
                f.sub_system_id(),
                f.sub_system_dont_collide_with(),
                f.system_group()
            ),
            (0, 0, 0)
        );
    }
}

/// The error the read of the instance at `at` (and its data) gives, or the data's own, for a scene `b` mutated.
fn refusal(b: Builder, read: impl Fn(&Packfile<'_>) -> Result<(), havok_formats::Error>) -> String {
    let bytes = b.build();
    let p = Packfile::read(&bytes).unwrap();
    read(&p).unwrap_err().to_string()
}

#[test]
fn a_type_word_that_is_not_the_rule_of_its_slot_is_refused() {
    let mut b = scene_builder();
    b.u16(HINGE + 0xC0, 15);
    let e = refusal(b, |p| ConstraintData::read(p, HINGE).map(drop));
    assert!(e.contains("angLimit") && e.contains("type 15"), "{e}");
    let mut b = scene_builder();
    b.u16(RAGDOLL + 0x138, 6);
    let e = refusal(b, |p| ConstraintData::read(p, RAGDOLL).map(drop));
    assert!(e.contains("ballSocket"), "{e}");
}

#[test]
fn a_flag_that_is_not_zero_or_one_and_a_stray_padding_byte_are_refused() {
    let mut b = scene_builder();
    b.data[HINGE + 0xB6] = 2;
    let e = refusal(b, |p| ConstraintData::read(p, HINGE).map(drop));
    assert!(e.contains("not a flag"), "{e}");
    let mut b = scene_builder();
    b.data[RAGDOLL + 0x12] = 1;
    let e = refusal(b, |p| ConstraintData::read(p, RAGDOLL).map(drop));
    assert!(e.contains("padding"), "{e}");
}

#[test]
fn a_motor_object_a_world_and_actions_are_refused() {
    let mut b = scene_builder();
    b.global(RAGDOLL + 0xE0, BODY_A);
    let e = refusal(b, |p| ConstraintData::read(p, RAGDOLL).map(drop));
    assert!(e.contains("ragdoll motor"), "{e}");
    let mut b = scene_builder();
    b.global(DATA + 0x08, SYSTEM);
    let e = refusal(b, |p| PhysicsData::read(p, DATA).map(drop));
    assert!(e.contains("worldCinfo"), "{e}");
    let mut b = scene_builder();
    b.array(SYSTEM + 0x20, Some(0xD0), 1);
    let e = refusal(b, |p| PhysicsSystem::read(p, SYSTEM).map(drop));
    assert!(e.contains("actions"), "{e}");
}

#[test]
fn a_breakable_constraint_inside_another_and_a_wrong_class_are_refused() {
    let mut b = scene_builder();
    b.retarget(BREAKABLE + 0x18, BREAKABLE);
    let e = refusal(b, |p| ConstraintData::read(p, BREAKABLE).map(drop));
    assert!(e.contains("inside a breakable"), "{e}");
    let mut b = scene_builder();
    b.retarget(INSTANCE_HINGE + 0x14, HINGE);
    let e = refusal(b, |p| ConstraintInstance::read(p, INSTANCE_HINGE).map(drop));
    assert!(e.contains("hkpRigidBody"), "{e}");
    let mut b = scene_builder();
    b.retarget(INSTANCE_HINGE + 0x0C, BODY_A);
    let e = refusal(b, |p| ConstraintInstance::read(p, INSTANCE_HINGE).map(drop));
    assert!(e.contains("no view"), "{e}");
    // a root container whose one variant is of another class
    let mut b = scene_builder();
    b.text(0x30, "hkpPhysicsDatX");
    let e = refusal(b, |p| PhysicsData::from_root(p).map(drop));
    assert!(e.contains("no hkpPhysicsData"), "{e}");
}
