//! Hand-written miniature Havok 7.1.0-r1 packfiles: every value is worked out here by hand (no game data, no second
//! reader), so these check the packfile and physics views against fixtures. The 490-file Tony Hawk's SHRED check the layout
//! table in `src/packfile/physics.rs` cites runs in primase, outside this repository:
//! <https://github.com/Kalmarv/primase/tree/main/worlds/shred/tests/collision_disc.rs>.
#![allow(clippy::unwrap_used)]
use havok_formats::enums::hkp::{BroadPhaseType, MotionType, ResponseType, WeldingType};
use havok_formats::packfile::physics::{
    self, ConvexVertices, ExtendedMesh, ListChild, MOTION_BOX_INERTIA, MOTION_FIXED, MoppCode, RigidBody,
    Shape,
};
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
    fn class(&mut self, name: &'static str) -> usize {
        self.classes.iter().position(|c| *c == name).unwrap_or_else(|| {
            self.classes.push(name);
            self.classes.len() - 1
        })
    }
    fn object(&mut self, offset: u32, class: &'static str) {
        let c = self.class(class);
        self.objects.push((offset, c));
    }
    fn u32(&mut self, at: usize, v: u32) {
        self.data[at..at + 4].copy_from_slice(&v.to_be_bytes());
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
    /// A pointer slot with a local fixup.
    fn local(&mut self, at: usize, to: usize) {
        self.local.push((at as u32, to as u32));
    }
    /// A pointer slot with a global fixup.
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
        self.u32(at + 4, len);
        self.u32(at + 8, 0x8000_0000 | len);
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

/// Root + a fixed rigid body (rotation 90 degrees about +Y, translation (10,20,30)) + its box shape.
fn scene() -> Vec<u8> {
    scene_builder().build()
}

fn scene_builder() -> Builder {
    let mut b = Builder::new(0x340);
    b.object(0, "hkRootLevelContainer");
    b.object(0x40, "hkpRigidBody");
    b.object(0x300, "hkpBoxShape");
    b.global(0x40 + 0x10, 0x300);
    b.u32(0x40 + 0x14, 0xFFFF_FFFF);
    b.local(0x40 + 0x74, 0x2C0);
    b.data[0x2C0..0x2C5].copy_from_slice(b"Box1\0");
    b.data[0x40 + 0x28] = 1; // the broad-phase handle's type: BROAD_PHASE_ENTITY
    b.data[0x40 + 0x8C] = 1;
    b.f32(0x40 + 0x90, 0.5);
    b.f32(0x40 + 0x94, 0.25);
    b.u32(0x40 + 0x2C, 7);
    b.u32(0x40 + 0xA8, 0x0003_FFFF);
    b.data[0x40 + 0xE8] = 5;
    // columns: c0 = (0,0,-1), c1 = (0,1,0), c2 = (1,0,0)
    b.floats(0x40 + 0xF0, &[0.0, 0.0, -1.0]);
    b.floats(0x40 + 0x100, &[0.0, 1.0, 0.0]);
    b.floats(0x40 + 0x110, &[1.0, 0.0, 0.0]);
    b.floats(0x40 + 0x120, &[10.0, 20.0, 30.0]);
    b.f32(0x300 + 0x10, 0.05);
    b.floats(0x300 + 0x20, &[1.0, 2.0, 3.0]);
    b
}

#[test]
fn rigid_body_and_box() {
    let bytes = scene();
    let p = Packfile::read(&bytes).unwrap();
    assert_eq!(p.objects().len(), 3);
    assert_eq!(p.class_at(0x40).unwrap(), "hkpRigidBody");
    assert_eq!(p.root(), 0);
    let c = physics::census(&p);
    assert_eq!(c.decoded.get("hkpRigidBody"), Some(&1));
    assert_eq!(c.decoded.get("hkpBoxShape"), Some(&1));
    assert_eq!(c.decoded.get("hkRootLevelContainer"), Some(&1));
    assert!(c.unknown.is_empty());
    assert_eq!(physics::rigid_bodies(&p), vec![0x40]);
    let body = RigidBody::read(&p, 0x40).unwrap();
    assert_eq!(body.name, "Box1");
    assert_eq!(body.shape, 0x300);
    assert_eq!(body.motion_type, MOTION_FIXED);
    assert_eq!((body.friction, body.restitution), (0.5, 0.25));
    assert_eq!((body.collision_filter_info, body.storage_index), (7, 3));
    // the enumerated members, named
    assert_eq!(body.motion(), Some(MotionType::Fixed));
    assert_eq!(body.response(), Some(ResponseType::SimpleContact));
    assert_eq!(body.broad_phase_type, BroadPhaseType::Entity);
    let filter = body.filter();
    assert_eq!(
        (filter.layer(), filter.sub_system_id(), filter.system_group()),
        (7, 0, 0)
    );
    // world = 1*c0 + 2*c1 + 3*c2 + t = (0,0,-1) + (0,2,0) + (3,0,0) + (10,20,30)
    assert_eq!(body.to_world([1.0, 2.0, 3.0]), [13.0, 22.0, 29.0]);
    assert_eq!(
        Shape::read(&p, 0x300).unwrap(),
        Shape::Box {
            radius: 0.05,
            half_extents: [1.0, 2.0, 3.0]
        }
    );
}

/// A shape-only fixture: an extended mesh with one subpart (the unit quad as two triangles).
fn mesh() -> Vec<u8> {
    mesh_builder().build()
}

fn mesh_builder() -> Builder {
    let mut b = Builder::new(0x400);
    b.object(0, "hkRootLevelContainer");
    b.object(0x40, "hkpStorageExtendedMeshShape");
    b.object(0x200, "hkpStorageExtendedMeshShapeMeshSubpartStorage");
    let sub = 0x150; // one 96-byte subpart (the shape's trailing data, after its 0x110-byte body)
    b.floats(0x40 + 0x90, &[0.5, 0.0, 0.5]);
    b.floats(0x40 + 0xA0, &[0.5, 0.0, 0.5]);
    b.u32(0x40 + 0xB0, 12);
    b.array(0x40 + 0xB4, Some(sub), 1);
    b.array(0x40 + 0xC0, None, 0);
    b.array(0x40 + 0xFC, None, 0);
    b.array(0x40 + 0xCC, Some(0x1C0), 2);
    b.array(0x40 + 0xF0, Some(0x1D0), 1);
    b.global(0x1D0, 0x200);
    b.u16(0x1C0, 0x1111);
    b.u16(0x1C2, 0x2222);
    b.u32(sub, 0x0002_000C);
    b.u32(sub + 0x08, 0x0002_0001);
    b.u32(sub + 0x14, 2);
    b.u32(sub + 0x1C, 16);
    b.u32(sub + 0x20, 4);
    b.u32(sub + 0x44, 16);
    b.u32(sub + 0x48, 0x0300_0000);
    // the embedded subpart: a copy of subpart 0; the scaling (1, 1, 1, 1)
    b.data.copy_within(sub..sub + 96, 0x40 + 0x20);
    b.floats(0x40 + 0x80, &[1.0; 4]);
    let s = 0x200;
    b.array(s + 0x08, Some(0x280), 4);
    b.array(s + 0x14, None, 0);
    b.array(s + 0x20, None, 0);
    b.array(s + 0x2C, Some(0x2C0), 8);
    b.array(s + 0x38, None, 0);
    b.array(s + 0x44, Some(0x2F0), 1);
    b.array(s + 0x50, None, 0);
    b.array(s + 0x5C, Some(0x320), 2);
    for (i, v) in [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 0.0, 1.0], [0.0, 0.0, 1.0]]
        .iter()
        .enumerate()
    {
        b.floats(0x280 + 16 * i, v);
    }
    for (i, w) in [0u32, 1, 2, 0, 0, 2, 3, 0].iter().enumerate() {
        b.u32(0x2C0 + 4 * i, *w);
    }
    b.u32(0x2F0, 0x58); // surface (0x58 >> 3) & 0x3F = 11
    b.u32(0x2F4, 0x3F80); // restitution 0, friction 1 (halves)
    b.u32(0x2F8, 0xDEAD_BEEF); // userData, kept as stored
    b.u16(0x320, 0);
    b.u16(0x322, 0);
    b
}

#[test]
fn extended_mesh() {
    let bytes = mesh();
    let p = Packfile::read(&bytes).unwrap();
    let m = ExtendedMesh::read(&p, 0x40).unwrap();
    assert_eq!(m.subparts.len(), 1);
    let s = &m.subparts[0];
    assert_eq!(
        s.vertices,
        vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 0.0, 1.0], [0.0, 0.0, 1.0]]
    );
    assert_eq!(s.triangles, vec![[0, 1, 2], [0, 2, 3]]);
    assert_eq!(s.material_indices, vec![0, 0]);
    assert_eq!(s.materials.len(), 1);
    let material = s.materials[0];
    assert_eq!(material.filter_info, 0x58);
    assert_eq!((material.filter_info >> 3) & 0x3F, 11);
    assert_eq!((material.restitution, material.friction), (0.0, 1.0));
    assert_eq!(material.user_data, 0xDEAD_BEEF);
    assert_eq!(m.welding, vec![0x1111, 0x2222]);
    assert_eq!(m.triangle_count(), 2);
    assert_eq!(m.aabb_center, [0.5, 0.0, 0.5]);
}

fn convex() -> Vec<u8> {
    convex_builder().build()
}

fn convex_builder() -> Builder {
    let mut b = Builder::new(0x300);
    b.object(0, "hkRootLevelContainer");
    b.object(0x40, "hkpConvexVerticesShape");
    b.f32(0x40 + 0x10, 0.05);
    b.floats(0x40 + 0x20, &[2.0, 2.0, 2.0]);
    b.floats(0x40 + 0x30, &[2.0, 12.0, 22.0]);
    b.array(0x40 + 0x40, Some(0x100), 2);
    b.u32(0x40 + 0x4C, 5);
    b.array(0x40 + 0x54, Some(0x180), 1);
    // vertex j = (j, 10 + j, 20 + j); group g holds x[4], y[4], z[4] for vertices 4g..4g+4 (missing lanes 0)
    for g in 0..2 {
        for axis in 0..3 {
            for lane in 0..4 {
                let j = 4 * g + lane;
                let v = if j < 5 { (j + 10 * axis) as f32 } else { 0.0 };
                b.f32(0x100 + 48 * g + 16 * axis + 4 * lane, v);
            }
        }
    }
    b.floats(0x180, &[0.0, 1.0, 0.0, -14.0]);
    b
}

#[test]
fn convex_vertices_four_vectors() {
    let bytes = convex();
    let p = Packfile::read(&bytes).unwrap();
    let c = ConvexVertices::read(&p, 0x40).unwrap();
    let want: Vec<[f32; 3]> = (0..5)
        .map(|j| [j as f32, 10.0 + j as f32, 20.0 + j as f32])
        .collect();
    assert_eq!(c.vertices, want);
    assert_eq!(c.planes, vec![[0.0, 1.0, 0.0, -14.0]]);
    assert_eq!(c.radius, 0.05);
}

fn refuse(bytes: &[u8]) -> String {
    Packfile::read(bytes).unwrap_err().to_string()
}

#[test]
fn refusals_name_the_offset() {
    let good = scene();
    assert!(Packfile::read(&good).is_ok());
    let patch = |at: usize, v: &[u8]| {
        let mut b = good.clone();
        b[at..at + v.len()].copy_from_slice(v);
        b
    };
    assert!(refuse(&patch(0, &[0])).contains("magic"));
    let e = refuse(&patch(0x0C, &[0, 0, 0, 8]));
    assert!(e.contains("version 8") && e.contains("0xc"), "{e}");
    assert!(refuse(&patch(0x10, &[8])).contains("pointer size 8"));
    // a little-endian file stores its header words little endian too: version 7, and the flag
    let mut little = good.clone();
    little[0x0C..0x10].copy_from_slice(&7u32.to_le_bytes());
    little[0x11] = 1;
    let e = refuse(&little);
    assert!(e.contains("little endian") && e.contains("0x11"), "{e}");
    assert!(refuse(&patch(0x12, &[0])).contains("reuse padding 0"));
    assert!(refuse(&patch(0x28, b"Havok-5.5.0-r1\0")).contains("contents version"));
    // the class-name section is the first at 0x40 + 144; break its separator byte
    assert!(refuse(&patch(0x40 + 3 * 48 + 4, &[7])).contains("0xFF padding"));
    // section start inside the header table
    assert!(refuse(&patch(0x40 + 20, &[0, 0, 0, 0x10])).contains("outside the file"));
    // truncated file
    assert!(refuse(&good[..good.len() - 40]).contains("outside the file"));
    // hkRootLevelContainer's signature (the first class-name entry) is not the 7.1 class's
    let e = refuse(&patch(0x40 + 3 * 48, &[0, 0, 0, 0]));
    assert!(
        e.contains("hkRootLevelContainer") && e.contains("0x2772c11e") && e.contains("0xd0"),
        "{e}"
    );
}

#[test]
fn header_words_and_class_signatures_are_kept() {
    let mut b = Builder::new(0x80);
    b.object(0, "hkRootLevelContainer");
    b.object(0x40, "hkpSomeClassNotInTheCorpus");
    let bytes = b.build();
    let p = Packfile::read(&bytes).unwrap();
    assert_eq!((p.user_tag, p.filler), (0, [0xFFFF_FFFF; 2]));
    let names: Vec<_> = p
        .class_names()
        .iter()
        .map(|c| (c.name.as_str(), c.signature))
        .collect();
    // a name the table does not list is kept with its signature, unchecked
    assert_eq!(
        names,
        [
            ("hkRootLevelContainer", 0x2772_C11E),
            ("hkpSomeClassNotInTheCorpus", 0x1234_5678)
        ]
    );
    assert_eq!(
        physics::census(&p).unknown.get("hkpSomeClassNotInTheCorpus"),
        Some(&1)
    );
}

#[test]
fn a_broad_phase_handle_that_is_not_an_entity_is_refused() {
    for handle_type in [0, 2, 3, 4, 0xFF] {
        let mut b = scene_builder();
        b.data[0x40 + 0x28] = handle_type;
        let e = refusal(&b.build(), |p| RigidBody::read(p, 0x40));
        assert!(e.contains("BROAD_PHASE_ENTITY"), "{handle_type}: {e}");
    }
}

#[test]
fn rigid_body_contact_delay_is_checked() {
    let mut bytes = scene();
    // the data section's start, from its header (the third, at 0x40 + 96)
    let data_start = u32::from_be_bytes(bytes[0x40 + 96 + 20..0x40 + 96 + 24].try_into().unwrap()) as usize;
    bytes[data_start + 0x40 + 0xAA] = 0;
    let p = Packfile::read(&bytes).unwrap();
    let e = RigidBody::read(&p, 0x40).unwrap_err().to_string();
    assert!(e.contains("contactPointCallbackDelay"), "{e}");
}

#[test]
fn fixup_refusals() {
    let base = scene();
    // locate the data section's fixup tables from the header (data section header at 0x40 + 96)
    let hdr = 0x40 + 96 + 20;
    let rd = |o: usize| u32::from_be_bytes(base[o..o + 4].try_into().unwrap()) as usize;
    let (start, local, global, virt) = (rd(hdr), rd(hdr + 4), rd(hdr + 8), rd(hdr + 12));
    let patch = |at: usize, v: u32| {
        let mut b = base.clone();
        b[at..at + 4].copy_from_slice(&v.to_be_bytes());
        b
    };
    // a local fixup whose target is beyond the data
    let e = refuse(&patch(start + local + 4, 0x10_0000));
    assert!(e.contains("outside the data"), "{e}");
    // a local fixup whose source slot is beyond the data
    assert!(refuse(&patch(start + local, 0x10_0000)).contains("outside the data"));
    // a global fixup into another section
    assert!(refuse(&patch(start + global + 4, 1)).contains("global fixup into section 1"));
    // a virtual fixup naming something that is not a class name
    let e = refuse(&patch(start + virt + 8, 3));
    assert!(e.contains("not a name"), "{e}");
    // a virtual fixup naming the wrong section
    assert!(refuse(&patch(start + virt + 4, 1)).contains("not the class names"));
    // two virtual fixups for one object
    let e = refuse(&patch(start + virt + 12, 0));
    assert!(e.contains("two virtual fixups") || e.contains("root"), "{e}");
}

#[test]
fn pointer_and_array_checks() {
    // a pointer slot with no fixup must hold zero
    let mut b = Builder::new(0x230);
    b.object(0, "hkRootLevelContainer");
    b.object(0x40, "hkpRigidBody");
    b.object(0x200, "hkpBoxShape");
    b.u32(0x40 + 0x10, 0x200); // no fixup
    let bytes = b.build();
    let p = Packfile::read(&bytes).unwrap();
    let e = RigidBody::read(&p, 0x40).unwrap_err().to_string();
    assert!(e.contains("no fixup"), "{e}");
    // an array whose capacity word is not its size or 0x80000000
    let mut b = Builder::new(0x400);
    b.object(0, "hkRootLevelContainer");
    b.object(0x40, "hkpStorageExtendedMeshShape");
    b.u32(0x40 + 0xB0, 12);
    b.local(0x40 + 0xB4, 0x140);
    b.u32(0x40 + 0xB4 + 4, 5);
    b.u32(0x40 + 0xB4 + 8, 0x8000_0001);
    let bytes = b.build();
    let p = Packfile::read(&bytes).unwrap();
    let e = ExtendedMesh::read(&p, 0x40).unwrap_err().to_string();
    assert!(e.contains("array of 5 has capacity word 0x80000001"), "{e}");
}

#[test]
fn mesh_refusals() {
    let bytes = mesh();
    // the data section ends the file: its start is the file length minus the header's `end`
    let rd = |o: usize| u32::from_be_bytes(bytes[o..o + 4].try_into().unwrap()) as usize;
    let data_start = bytes.len() - rd(0x40 + 96 + 20 + 24);
    let patch = |at: usize, v: u32| {
        let mut x = bytes.clone();
        x[data_start + at..data_start + at + 4].copy_from_slice(&v.to_be_bytes());
        x
    };
    let e = {
        let x = patch(0x2C0, 9);
        let p = Packfile::read(&x).unwrap();
        ExtendedMesh::read(&p, 0x40).unwrap_err().to_string()
    };
    assert!(e.contains("outside the vertices"), "{e}");
    let e = {
        let x = patch(0x2C0 + 12, 1);
        let p = Packfile::read(&x).unwrap();
        ExtendedMesh::read(&p, 0x40).unwrap_err().to_string()
    };
    assert!(e.contains("fourth index word"), "{e}");
    let refusal = |at: usize, v: u32| {
        let x = patch(at, v);
        let p = Packfile::read(&x).unwrap();
        ExtendedMesh::read(&p, 0x40).unwrap_err().to_string()
    };
    // friction 1.0078 is a finite half and is read; an infinite one is not
    {
        let x = patch(0x2F4, 0x3F81);
        let p = Packfile::read(&x).unwrap();
        let m = ExtendedMesh::read(&p, 0x40).unwrap();
        assert_eq!(m.subparts[0].materials[0].friction, f32::from_bits(0x3F81_0000));
    }
    let e = refusal(0x2F4, 0x7F80);
    assert!(e.contains("non-finite half"), "{e}");
    let e = refusal(0x40 + 0x8C, 0x3F00_0000); // scaling w 0.5
    assert!(e.contains("scaling"), "{e}");
    let e = refusal(0x40 + 0x20 + 0x14, 3); // embedded triangle count differs from subpart 0's
    assert!(e.contains("embedded subpart"), "{e}");
    // a subpart word changed in subpart 0 and in its embedded copy (at shape + 0x20), so the copy still holds
    let subpart_refusal = |at: usize, v: u32| {
        let mut x = patch(0x150 + at, v);
        x[data_start + 0x60 + at..data_start + 0x60 + at + 4].copy_from_slice(&v.to_be_bytes());
        let p = Packfile::read(&x).unwrap();
        ExtendedMesh::read(&p, 0x40).unwrap_err().to_string()
    };
    for (at, v, want) in [
        (0x50, 1, "userData"),
        (0x30, 0x3F80_0000, "extrusion"), // extrusion x
        (0x18, 0x10, "vertexBase"),
        (0x48, 0x0301_0000, "flipAlternateTriangles"),
        (0x00, 0x0102_000C, "type is not triangles"), // type 1
        (0x00, 0x0001_000C, "16-bit"),                // 8-bit material indices
        (0x00, 0x0002_0010, "material striding"),     // 16-byte material records
    ] {
        let e = subpart_refusal(at, v);
        assert!(e.contains(want), "{want}: {e}");
    }
    let e = {
        let x = patch(0x320, 0x0001_0000); // material index 1 of 1
        let p = Packfile::read(&x).unwrap();
        ExtendedMesh::read(&p, 0x40).unwrap_err().to_string()
    };
    assert!(e.contains("outside the materials"), "{e}");
}

#[test]
fn non_finite_floats_are_errors() {
    let bytes = {
        let mut b = Builder::new(0x230);
        b.object(0, "hkRootLevelContainer");
        b.object(0x40, "hkpRigidBody");
        b.object(0x200, "hkpBoxShape");
        b.f32(0x200 + 0x10, f32::NAN);
        b.build()
    };
    let p = Packfile::read(&bytes).unwrap();
    assert!(
        Shape::read(&p, 0x200)
            .unwrap_err()
            .to_string()
            .contains("non-finite")
    );
}

/// The error text of reading `read` from the packfile `bytes` (empty when it reads).
fn refusal<T>(bytes: &[u8], read: impl FnOnce(&Packfile<'_>) -> havok_formats::Result<T>) -> String {
    let p = Packfile::read(bytes).unwrap();
    match read(&p) {
        Ok(_) => String::new(),
        Err(e) => e.to_string(),
    }
}

#[test]
fn section_header_byte_19_and_the_bytes_after_the_tag() {
    let good = scene();
    // the first section header is at 0x40: tag `__classnames__` (14 bytes), its NUL, then zeros up to byte 19 = 0xFF
    assert_eq!(&good[0x40..0x40 + 14], b"__classnames__");
    assert_eq!(good[0x40 + 19], 0xFF);
    let patch = |at: usize, v: u8| {
        let mut b = good.clone();
        b[at] = v;
        b
    };
    let e = refuse(&patch(0x40 + 19, 0));
    assert!(
        e.contains("byte 19 is 0x00, not 0xFF") && e.contains("0x53"),
        "{e}"
    );
    let e = refuse(&patch(0x40 + 16, 1));
    assert!(
        e.contains("followed by bytes that are not zero") && e.contains("0x4e"),
        "{e}"
    );
    // a tag that fills the 19 bytes has no NUL and no bytes after it
    let mut b = good.clone();
    b[0x40 + 96..0x40 + 115].copy_from_slice(b"__datadatadatadata_");
    assert!(refuse(&b).contains("exactly one __data__"));
}

#[test]
fn array_capacity_word_is_the_size_with_the_flag() {
    let read = |tweak: &dyn Fn(&mut Builder)| {
        let mut b = mesh_builder();
        tweak(&mut b);
        let bytes = b.build();
        let p = Packfile::read(&bytes).unwrap();
        p.array_header(0x40 + 0xCC)
            .map(|a| (a.data, a.len))
            .map_err(|e| e.to_string())
    };
    assert_eq!(read(&|_| {}), Ok((Some(0x1C0), 2)));
    // the capacity word of the welding array is 0x80000002: each other value is refused
    for word in [2, 0x8000_0003, 0x8000_0001, 0xC000_0002, 0x4000_0002, 0x8000_0000] {
        let e = read(&|b| b.u32(0x40 + 0xCC + 8, word)).unwrap_err();
        assert!(
            e.contains(&format!("capacity word {word:#010x}")) && e.contains("array of 2"),
            "{word:#x}: {e}"
        );
    }
    // an empty array stores 0x80000000 too, and has no fixup
    let empty = |tweak: &dyn Fn(&mut Builder)| {
        let mut b = mesh_builder();
        tweak(&mut b);
        let bytes = b.build();
        let p = Packfile::read(&bytes).unwrap();
        p.array_header(0x40 + 0xC0)
            .map(|a| (a.data, a.len))
            .map_err(|e| e.to_string())
    };
    assert_eq!(empty(&|_| {}), Ok((None, 0)));
    let e = empty(&|b| b.u32(0x40 + 0xC0 + 8, 0)).unwrap_err();
    assert!(e.contains("capacity word 0x00000000"), "{e}");
    let e = empty(&|b| b.array(0x40 + 0xC0, Some(0x1E0), 0)).unwrap_err();
    assert!(e.contains("empty array has a fixup"), "{e}");
    // a non-empty array needs its fixup
    let e = read(&|b| b.local.retain(|(src, _)| *src != 0x40 + 0xCC)).unwrap_err();
    assert!(e.contains("non-empty array has no data"), "{e}");
}

#[test]
fn a_pointer_slot_with_a_fixup_holds_zero() {
    let mut b = scene_builder();
    b.u32(0x40 + 0x10, 0x300);
    let e = refusal(&b.build(), |p| RigidBody::read(p, 0x40));
    assert!(e.contains("with a fixup holds 0x300, not zero"), "{e}");
    // without the fixup it is the older refusal
    let mut b = scene_builder();
    b.global.clear();
    b.u32(0x40 + 0x10, 0x300);
    assert!(refusal(&b.build(), |p| RigidBody::read(p, 0x40)).contains("has no fixup"));
}

#[test]
fn fixups_are_aligned() {
    let base = || {
        let mut b = Builder::new(0x200);
        b.object(0, "hkRootLevelContainer");
        b
    };
    let read = |b: &Builder| Packfile::read(&b.build()).err().map(|e| e.to_string());
    assert_eq!(read(&base()), None);
    let mut b = base();
    b.local(0x80, 0x100);
    b.global(0x84, 0x110);
    b.object(0x140, "hkpBoxShape");
    assert_eq!(read(&b), None);
    // a local source, a global source: not on a 4-byte boundary
    let mut b = base();
    b.local(0x82, 0x100);
    let e = read(&b).unwrap();
    assert!(e.contains("local fixup source 0x82 is not 4-byte aligned"), "{e}");
    let mut b = base();
    b.global(0x81, 0x110);
    let e = read(&b).unwrap();
    assert!(
        e.contains("global fixup source 0x81 is not 4-byte aligned"),
        "{e}"
    );
    // a local target is a word; a global target is a 16-byte vector
    let mut b = base();
    b.local(0x80, 0x102);
    let e = read(&b).unwrap();
    assert!(
        e.contains("local fixup target 0x102 is not 4-byte aligned"),
        "{e}"
    );
    let mut b = base();
    b.global(0x84, 0x118);
    let e = read(&b).unwrap();
    assert!(
        e.contains("global fixup target 0x118 is not 16-byte aligned"),
        "{e}"
    );
    // an object starts on a 16-byte boundary
    let mut b = base();
    b.object(0x148, "hkpBoxShape");
    let e = read(&b).unwrap();
    assert!(e.contains("object is not 16-byte aligned"), "{e}");
}

/// The fixed body of [`scene`], made dynamic: box inertia, mass 2, and `tweak` applied to its bytes.
fn dynamic_body(tweak: impl FnOnce(&mut Builder)) -> Vec<u8> {
    let mut b = scene_builder();
    b.data[0x40 + 0xE8] = MOTION_BOX_INERTIA;
    b.data[0x40 + 0x2A] = 4;
    b.f32(0x40 + 0x5C, 0.25);
    b.f32(0x40 + 0x190, 3.5);
    b.f32(0x40 + 0x194, 0.0);
    b.f32(0x40 + 0x198, 0.05);
    b.floats(0x40 + 0x1A0, &[0.5, 0.25, 0.125]);
    b.f32(0x40 + 0x1AC, 0.5);
    b.floats(0x40 + 0x170, &[0.1, 0.2, 0.3]);
    b.u16(0x40 + 0x1FE, 0x3F80);
    tweak(&mut b);
    b.build()
}

#[test]
fn a_body_reads_its_entity_and_motion_members() {
    let bytes = dynamic_body(|_| {});
    let p = Packfile::read(&bytes).unwrap();
    let body = RigidBody::read(&p, 0x40).unwrap();
    assert_eq!(body.motion_type, MOTION_BOX_INERTIA);
    assert_eq!(body.quality_type, 4);
    assert_eq!(body.allowed_penetration_depth, 0.25);
    assert_eq!(body.object_radius, 3.5);
    assert_eq!((body.linear_damping, body.angular_damping), (0.0, 0.05));
    assert_eq!(body.inverse_inertia, [0.5, 0.25, 0.125]);
    assert_eq!(body.inverse_mass, 0.5);
    assert_eq!(body.center_of_mass_local, [0.1, 0.2, 0.3]);
    assert_eq!(body.gravity_factor, 1.0);
    // the fixed body of the scene stores zeros for all of the mass members, and the quality type 0
    let fixed = scene();
    let p = Packfile::read(&fixed).unwrap();
    let body = RigidBody::read(&p, 0x40).unwrap();
    assert_eq!(
        (body.inverse_inertia, body.inverse_mass, body.quality_type),
        ([0.0; 3], 0.0, 0)
    );
}

#[test]
fn a_body_that_is_not_at_rest_or_whose_mass_disagrees_with_its_motion_is_refused() {
    let refused = |bytes: Vec<u8>| refusal(&bytes, |p| RigidBody::read(p, 0x40));
    // a fixed body with a mass, or with one inverse inertia lane
    let mut b = scene_builder();
    b.f32(0x40 + 0x1AC, 1.0);
    assert!(refused(b.build()).contains("a fixed body has inverse inertia or inverse mass"));
    let mut b = scene_builder();
    b.f32(0x40 + 0x1A4, 1.0);
    assert!(refused(b.build()).contains("a fixed body has inverse inertia or inverse mass"));
    // a dynamic body needs every one of the four positive
    for at in [0x1A0, 0x1A4, 0x1A8, 0x1AC] {
        let e = refused(dynamic_body(|b| b.f32(0x40 + at, 0.0)));
        assert!(
            e.contains("a dynamic body has no positive inverse inertia and inverse mass"),
            "{at:#x}: {e}"
        );
    }
    // velocities: any lane of either
    for at in [0x1B0, 0x1B8, 0x1BC, 0x1C0, 0x1C4, 0x1CC] {
        let e = refused(dynamic_body(|b| b.f32(0x40 + at, 1.0)));
        assert!(e.contains("velocity is not zero"), "{at:#x}: {e}");
    }
    // the swept transform of a body at rest: both centres and both rotations equal
    for at in [0x130, 0x138] {
        let e = refused(dynamic_body(|b| b.f32(0x40 + at, 1.0)));
        assert!(e.contains("two centres of mass differ"), "{at:#x}: {e}");
    }
    for at in [0x150, 0x15C, 0x16C] {
        let e = refused(dynamic_body(|b| b.f32(0x40 + at, 1.0)));
        assert!(e.contains("two rotations differ"), "{at:#x}: {e}");
    }
    // the time lanes of the centres (the fourth lane) are not compared
    let bytes = dynamic_body(|b| b.f32(0x40 + 0x13C, 1.0));
    assert!(refused(bytes).is_empty());
    let e = refused(dynamic_body(|b| b.u16(0x40 + 0x1FE, 0x7F80)));
    assert!(e.contains("non-finite half"), "{e}");
}

#[test]
fn an_extended_mesh_reads_its_trailing_members() {
    let mut b = mesh_builder();
    b.data[0x40 + 0xD8] = 1;
    b.u32(0x40 + 0xDC, 0x30);
    b.u32(0x40 + 0xE0, 1);
    b.f32(0x40 + 0xE4, 0.001);
    b.u32(0x40 + 0x08, 1);
    let bytes = b.build();
    let p = Packfile::read(&bytes).unwrap();
    let m = ExtendedMesh::read(&p, 0x40).unwrap();
    assert_eq!(
        (
            m.welding_type,
            m.default_collision_filter_info,
            m.cached_num_child_shapes,
            m.triangle_radius,
            m.user_data
        ),
        (1, 0x30, 1, 0.001, 1)
    );
    // 1 is no value of the welding type (0, 4, 5 and 6 are)
    assert_eq!(m.welding(), None);
    assert_eq!(WeldingType::from_value(6), Some(WeldingType::None));
    // a cached count equal to the triangle count is allowed, one above it is not
    let mut b = mesh_builder();
    b.u32(0x40 + 0xE0, 2);
    assert!(refusal(&b.build(), |p| ExtendedMesh::read(p, 0x40)).is_empty());
    let mut b = mesh_builder();
    b.u32(0x40 + 0xE0, 3);
    let e = refusal(&b.build(), |p| ExtendedMesh::read(p, 0x40));
    assert!(
        e.contains("cachedNumChildShapes 3 exceeds the 2 triangles"),
        "{e}"
    );
    // the padding bytes of the two gaps
    for (at, want) in [
        (0xD9, "padding after weldingType"),
        (0xDB, "padding after weldingType"),
        (0xE8, "padding after triangleRadius"),
        (0xEF, "padding after triangleRadius"),
    ] {
        let mut b = mesh_builder();
        b.data[0x40 + at] = 1;
        let e = refusal(&b.build(), |p| ExtendedMesh::read(p, 0x40));
        assert!(e.contains(want), "{at:#x}: {e}");
    }
    // the shape storage array
    let mut b = mesh_builder();
    b.array(0x40 + 0xFC, Some(0x1E0), 1);
    let e = refusal(&b.build(), |p| ExtendedMesh::read(p, 0x40));
    assert!(e.contains("shape storage is not supported"), "{e}");
    // the shape type word
    let mut b = mesh_builder();
    b.u32(0x40 + 0x0C, 1);
    let e = refusal(&b.build(), |p| ExtendedMesh::read(p, 0x40));
    assert!(e.contains("shape type word is not zero"), "{e}");
}

/// A convex shape of the five vertices of [`convex`] with a connectivity object holding `indices` cut by `counts`.
fn connected(indices: &[u16], counts: &[u8]) -> Builder {
    let mut b = convex_builder();
    b.object(0x1C0, "hkpConvexVerticesConnectivity");
    b.local(0x40 + 0x60, 0x1C0);
    b.array(0x1C0 + 0x08, Some(0x200), u32::try_from(indices.len()).unwrap());
    for (i, v) in indices.iter().enumerate() {
        b.u16(0x200 + 2 * i, *v);
    }
    b.array(0x1C0 + 0x14, Some(0x220), u32::try_from(counts.len()).unwrap());
    b.data[0x220..0x220 + counts.len()].copy_from_slice(counts);
    b
}

#[test]
fn a_convex_shape_reads_its_faces() {
    let bytes = connected(&[0, 1, 2, 3, 4], &[5]).build();
    let p = Packfile::read(&bytes).unwrap();
    let c = ConvexVertices::read(&p, 0x40).unwrap();
    assert_eq!(c.faces, vec![vec![0, 1, 2, 3, 4]]);
    assert_eq!(c.planes.len(), 1);
    assert_eq!(c.user_data, 0);
    assert_eq!(
        physics::census(&p).decoded.get("hkpConvexVerticesConnectivity"),
        Some(&1)
    );
    // no connectivity pointer: no faces
    let bytes = convex();
    let p = Packfile::read(&bytes).unwrap();
    assert!(ConvexVertices::read(&p, 0x40).unwrap().faces.is_empty());
    // faces of different sizes keep their runs; two planes for two faces
    let mut b = connected(&[0, 1, 2, 2, 3, 4], &[3, 3]);
    b.array(0x40 + 0x54, Some(0x180), 2);
    b.local
        .remove(b.local.iter().position(|&(s, _)| s == 0x40 + 0x54).unwrap());
    b.floats(0x190, &[1.0, 0.0, 0.0, -4.0]);
    let bytes = b.build();
    let p = Packfile::read(&bytes).unwrap();
    let c = ConvexVertices::read(&p, 0x40).unwrap();
    assert_eq!(c.faces, vec![vec![0, 1, 2], vec![2, 3, 4]]);
    assert_eq!(c.planes.len(), 2);
    // the user data word is kept
    let mut b = convex_builder();
    b.u32(0x40 + 0x08, 1);
    let bytes = b.build();
    let p = Packfile::read(&bytes).unwrap();
    assert_eq!(ConvexVertices::read(&p, 0x40).unwrap().user_data, 1);
}

#[test]
fn a_convex_shape_that_disagrees_with_its_connectivity_is_refused() {
    let refused = |b: Builder| refusal(&b.build(), |p| ConvexVertices::read(p, 0x40));
    // fewer planes than faces (one plane, two faces)
    let e = refused(connected(&[0, 1, 2, 2, 3, 4], &[3, 3]));
    assert!(e.contains("1 planes for 2 faces"), "{e}");
    // an index past the five vertices
    let e = refused(connected(&[0, 1, 2, 3, 5], &[5]));
    assert!(e.contains("names a vertex the shape does not have"), "{e}");
    // counts that use more or fewer indices than stored
    let e = refused(connected(&[0, 1, 2, 3, 4], &[6]));
    assert!(e.contains("use more vertex indices than are stored"), "{e}");
    let e = refused(connected(&[0, 1, 2, 3, 4], &[4]));
    assert!(e.contains("leave stored vertex indices unused"), "{e}");
    // the connectivity pointer names another class
    let mut b = connected(&[0, 1, 2, 3, 4], &[5]);
    b.retarget(0x40 + 0x60, 0);
    let e = refused(b);
    assert!(e.contains("not a hkpConvexVerticesConnectivity"), "{e}");
    // the slot at +0x50 has no fixup and holds zero; the bytes after the connectivity pointer are zero
    let mut b = convex_builder();
    b.local(0x40 + 0x50, 0x180);
    assert!(refused(b).contains("slot at +0x50 has a fixup"));
    for at in [0x64, 0x6F] {
        let mut b = convex_builder();
        b.data[0x40 + at] = 1;
        assert!(refused(b).contains("the end of the shape is not zero"), "{at:#x}");
    }
    let mut b = convex_builder();
    b.u32(0x40 + 0x0C, 4);
    assert!(refused(b).contains("shape type word is not zero"));
}

/// A MOPP tree shape at 0x40 over a box (0x100), its code at 0x140 holding `[1, 2, 3, 4]` at `code_at`.
fn mopp_builder(code_at: usize) -> Builder {
    let mut b = Builder::new(0x200);
    b.object(0, "hkRootLevelContainer");
    b.object(0x40, "hkpMoppBvTreeShape");
    b.object(0x100, "hkpBoxShape");
    b.object(0x140, "hkpMoppCode");
    b.global(0x40 + 0x14, 0x140);
    b.global(0x40 + 0x34, 0x100);
    b.floats(0x140 + 0x10, &[1.0, 2.0, 3.0]);
    b.f32(0x140 + 0x1C, 4.0);
    b.array(0x140 + 0x20, Some(code_at), 4);
    b.data[code_at..code_at + 4].copy_from_slice(&[1, 2, 3, 4]);
    b
}

#[test]
fn a_mopp_shape_names_its_code() {
    let bytes = mopp_builder(0x170).build();
    let p = Packfile::read(&bytes).unwrap();
    assert_eq!(
        Shape::read(&p, 0x40).unwrap(),
        Shape::Mopp {
            child: 0x100,
            code: 0x140
        }
    );
    assert_eq!(
        MoppCode::read(&p, 0x140).unwrap(),
        MoppCode {
            offset: [1.0, 2.0, 3.0],
            scale: 4.0,
            build_type: 0,
            code: vec![1, 2, 3, 4]
        }
    );
    assert_eq!(physics::census(&p).decoded.get("hkpMoppCode"), Some(&1));
    // the build type is signed
    let mut b = mopp_builder(0x170);
    b.data[0x140 + 0x2C] = 0xFF;
    let bytes = b.build();
    let p = Packfile::read(&bytes).unwrap();
    assert_eq!(MoppCode::read(&p, 0x140).unwrap().build_type, -1);
}

#[test]
fn a_mopp_code_that_is_not_laid_out_as_stored_is_refused() {
    let code = |b: Builder| refusal(&b.build(), |p| MoppCode::read(p, 0x140));
    // the code does not follow the body
    let e = code(mopp_builder(0x180));
    assert!(e.contains("does not follow the 0x30-byte body"), "{e}");
    for (at, want) in [
        (0x00, "start of a MOPP code"),
        (0x0F, "start of a MOPP code"),
        (0x2D, "padding after buildType"),
        (0x2F, "padding after buildType"),
    ] {
        let mut b = mopp_builder(0x170);
        b.data[0x140 + at] = 1;
        let e = code(b);
        assert!(e.contains(want) && e.contains("is not zero"), "{at:#x}: {e}");
    }
    let mut b = mopp_builder(0x170);
    b.f32(0x140 + 0x10, f32::INFINITY);
    assert!(code(b).contains("non-finite"));
    // the tree shape's code pointer names another class, or is null
    let shape = |b: Builder| refusal(&b.build(), |p| Shape::read(p, 0x40));
    let mut b = mopp_builder(0x170);
    b.retarget(0x40 + 0x14, 0x100);
    assert!(shape(b).contains("MOPP code is not a hkpMoppCode"));
    let mut b = mopp_builder(0x170);
    b.global.retain(|(src, _)| *src != 0x40 + 0x14);
    assert!(shape(b).contains("MOPP code is null"));
}

fn capsule(tweak: &dyn Fn(&mut Builder)) -> Vec<u8> {
    let mut b = Builder::new(0x100);
    b.object(0, "hkRootLevelContainer");
    b.object(0x40, "hkpCapsuleShape");
    b.f32(0x40 + 0x10, 0.5);
    b.floats(0x40 + 0x20, &[1.0, 2.0, 3.0, 0.5]);
    b.floats(0x40 + 0x30, &[4.0, 5.0, 6.0, 0.5]);
    tweak(&mut b);
    b.build()
}

#[test]
fn a_capsule_repeats_its_radius_in_the_w_lanes() {
    let bytes = capsule(&|_| {});
    let p = Packfile::read(&bytes).unwrap();
    assert_eq!(
        Shape::read(&p, 0x40).unwrap(),
        Shape::Capsule {
            radius: 0.5,
            a: [1.0, 2.0, 3.0],
            b: [4.0, 5.0, 6.0]
        }
    );
    for at in [0x2C, 0x3C] {
        let bytes = capsule(&|b| b.f32(0x40 + at, 0.25));
        let e = refusal(&bytes, |p| Shape::read(p, 0x40));
        assert!(e.contains("w lane is not the radius"), "{at:#x}: {e}");
    }
}

fn list(tweak: &dyn Fn(&mut Builder)) -> Vec<u8> {
    let mut b = Builder::new(0x300);
    b.object(0, "hkRootLevelContainer");
    b.object(0x40, "hkpListShape");
    b.object(0x200, "hkpBoxShape");
    b.object(0x240, "hkpSphereShape");
    b.array(0x40 + 0x18, Some(0x100), 2);
    b.global(0x100, 0x200);
    b.u32(0x104, 0x0007_0007);
    b.global(0x110, 0x240);
    tweak(&mut b);
    b.build()
}

#[test]
fn a_list_shape_reads_its_child_records() {
    let bytes = list(&|_| {});
    let p = Packfile::read(&bytes).unwrap();
    assert_eq!(
        Shape::read(&p, 0x40).unwrap(),
        Shape::List {
            children: vec![
                ListChild {
                    shape: 0x200,
                    collision_filter_info: 0x0007_0007
                },
                ListChild {
                    shape: 0x240,
                    collision_filter_info: 0
                }
            ]
        }
    );
    for at in [0x108, 0x10C, 0x118, 0x11C] {
        let bytes = list(&|b| b.u32(at, 1));
        let e = refusal(&bytes, |p| Shape::read(p, 0x40));
        assert!(
            e.contains("list shape's child record is not zero"),
            "{at:#x}: {e}"
        );
    }
}

#[test]
fn every_shape_class_has_a_zero_type_word_and_a_user_data_word() {
    for class in ["hkpBoxShape", "hkpSphereShape", "hkpCapsuleShape"] {
        let make = |tweak: &dyn Fn(&mut Builder)| {
            let mut b = Builder::new(0x100);
            b.object(0, "hkRootLevelContainer");
            b.object(0x40, class);
            b.floats(0x40 + 0x20, &[1.0, 1.0, 1.0, 0.0]);
            tweak(&mut b);
            b.build()
        };
        let bytes = make(&|b| b.u32(0x40 + 0x08, 7));
        let p = Packfile::read(&bytes).unwrap();
        assert_eq!(physics::shape_user_data(&p, 0x40).unwrap(), 7, "{class}");
        let bytes = make(&|b| b.u32(0x40 + 0x0C, 3));
        let p = Packfile::read(&bytes).unwrap();
        let e = physics::shape_user_data(&p, 0x40).unwrap_err().to_string();
        assert!(e.contains("shape type word is not zero"), "{class}: {e}");
        let e = Shape::read(&p, 0x40).unwrap_err().to_string();
        assert!(e.contains("shape type word"), "{class}: {e}");
    }
}

/// The head of a packfile of another SDK: the magic, the file version and layout rules in the file's byte order, the
/// contents version, and (after a 16-byte header extension, as later versions have) a first section header whose tag is
/// `__classnames__`, then the class names `hkClass` and `hkClassMember` with the given signatures. Nothing else of the
/// file: the reader refuses it before it reads further.
fn foreign(version: u32, rules: [u8; 4], contents: &[u8], signatures: [u32; 2]) -> Vec<u8> {
    let little = rules[1] != 0;
    let word = |v: u32| if little { v.to_le_bytes() } else { v.to_be_bytes() };
    let mut out = vec![0x57, 0xE0, 0xE0, 0x57, 0x10, 0xC0, 0xC0, 0x10];
    out.extend(word(0));
    out.extend(word(version));
    out.extend(rules);
    out.extend(word(3));
    out.resize(0x28, 0);
    let mut text = contents.to_vec();
    text.resize(16, 0);
    out.extend(text);
    out.resize(0x50, 0);
    let mut tag = b"__classnames__".to_vec();
    tag.resize(19, 0);
    tag.push(0xFF);
    out.extend(tag);
    out.extend(word(0x80));
    out.resize(0x80, 0);
    for (signature, name) in signatures.into_iter().zip([&b"hkClass"[..], b"hkClassMember"]) {
        out.extend(word(signature));
        out.push(9);
        out.extend(name);
        out.push(0);
    }
    out
}

#[test]
fn a_packfile_of_another_sdk_is_named_by_its_first_class_names() {
    // files of the 2013.1 to 2014.2 generation in versions the reader does not know (9 and 11 it reads): little endian,
    // 8-byte pointers
    for (version, contents) in [
        (10, &b"hk_2014.1.0-r1"[..]),
        (12, b"hk_2013.1.0-r1"),
        (8, b"hk_2014.2.0-r1"),
    ] {
        let bytes = foreign(version, [8, 1, 0, 1], contents, [0x33D4_2383, 0xB0EF_A719]);
        let e = refuse(&bytes);
        let want = format!(
            "unsupported packfile version {version} (7, 9 and 11 only): contents version \"{}\", class names of Havok 2013.1 to 2014.2 (hkClass 0x33d42383, hkClassMember 0xb0efa719)",
            String::from_utf8_lossy(contents)
        );
        assert!(e.contains(&want), "{e}");
        assert!(e.contains("0xc"), "{e}");
    }
    // the same signatures in another order are the signatures of no SDK it knows, and are given as stored
    let e = refuse(&foreign(
        10,
        [8, 1, 0, 1],
        b"hk_2014.1.0-r1",
        [0xB0EF_A719, 0x33D4_2383],
    ));
    assert!(
        e.contains("hkClass 0xb0efa719 and hkClassMember 0x33d42383 are those of no SDK this reader knows"),
        "{e}"
    );
    // a big endian file of 4-byte pointers that is not version 7: the 7.1 class names, and the byte order is honoured
    let e = refuse(&foreign(
        8,
        [4, 0, 1, 1],
        b"Havok-7.1.0-r1",
        [0x1442_5E51, 0x4A98_6551],
    ));
    assert!(
        e.contains("unsupported packfile version 8 (7, 9 and 11 only)"),
        "{e}"
    );
    assert!(
        e.contains("class names of Havok 7.1.0-r1 (hkClass 0x14425e51, hkClassMember 0x4a986551)"),
        "{e}"
    );
}

#[test]
fn the_other_refusals_of_a_foreign_file_name_it_too() {
    // version 7 with 8-byte pointers: the pointer size is refused, and the class names say whose file it is
    let e = refuse(&foreign(
        7,
        [8, 0, 1, 1],
        b"Havok-7.1.0-r1",
        [0x1442_5E51, 0x4A98_6551],
    ));
    assert!(e.contains("unsupported pointer size 8 (4 only in version 7): contents version \"Havok-7.1.0-r1\", class names of Havok 7.1.0-r1"), "{e}");
    let e = refuse(&foreign(
        7,
        [4, 1, 1, 1],
        b"hk_2014.1.0-r1",
        [0x33D4_2383, 0xB0EF_A719],
    ));
    assert!(e.contains("little endian (big endian only in version 7): contents version \"hk_2014.1.0-r1\", class names of Havok 2013.1 to 2014.2"), "{e}");
    let e = refuse(&foreign(
        7,
        [4, 0, 0, 1],
        b"hk_2014.1.0-r1",
        [0x33D4_2383, 0xB0EF_A719],
    ));
    assert!(
        e.contains("reuse padding 0") && e.contains("class names of Havok 2013.1 to 2014.2"),
        "{e}"
    );
    // the contents version is refused with its text once, and the class names after it
    let e = refuse(&foreign(
        7,
        [4, 0, 1, 1],
        b"Havok-6.6.0-b1",
        [0x1442_5E51, 0x4A98_6551],
    ));
    assert!(e.contains("unsupported contents version \"Havok-6.6.0-b1\" (Havok-7.1.0-r1 only in version 7): class names of Havok 7.1.0-r1"), "{e}");
    assert_eq!(e.matches("Havok-6.6.0-b1").count(), 1, "{e}");
    // the fixture's first class is the root container, not hkClass, so only the contents version is named
    let mut good = scene();
    good[0x0C..0x10].copy_from_slice(&6u32.to_be_bytes());
    let e = refuse(&good);
    assert!(
        e.ends_with("unsupported packfile version 6 (7, 9 and 11 only): contents version \"Havok-7.1.0-r1\""),
        "{e}"
    );
}

#[test]
fn a_foreign_file_without_readable_class_names_gets_the_plain_refusal() {
    // the header only; the version of a file too short to say its byte order is read big endian
    let mut head = vec![0x57, 0xE0, 0xE0, 0x57, 0x10, 0xC0, 0xC0, 0x10, 0, 0, 0, 0];
    head.extend(8u32.to_be_bytes());
    assert_eq!(
        refuse(&head),
        "Havok at 0xc: unsupported packfile version 8 (7, 9 and 11 only)"
    );
    // a section table whose class names are not hkClass, hkClassMember, or that run off the file
    let good = foreign(10, [8, 1, 0, 1], b"hk_2014.1.0-r1", [0x33D4_2383, 0xB0EF_A719]);
    let plain = |bytes: &[u8]| {
        refuse(bytes)
            .split_once("(7, 9 and 11 only)")
            .map(|(_, rest)| rest.to_owned())
    };
    let mut renamed = good.clone();
    renamed[0x85..0x8C].copy_from_slice(b"hkClasz");
    assert_eq!(
        plain(&renamed).as_deref(),
        Some(": contents version \"hk_2014.1.0-r1\"")
    );
    for cut in [0x30, 0x60, 0x86, 0x90] {
        let rest = plain(&good[..cut]).unwrap();
        assert!(!rest.contains("class names"), "cut at {cut:#x}: {rest}");
    }
    // no tag at all
    let mut untagged = good.clone();
    untagged[0x50..0x5E].copy_from_slice(b"__classnamez__");
    assert_eq!(
        plain(&untagged).as_deref(),
        Some(": contents version \"hk_2014.1.0-r1\"")
    );
}
