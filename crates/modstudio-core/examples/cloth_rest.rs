//! Every cloth of one cloth file with its bones at rest (the reference pose of the skeleton its transform set names,
//! from the same file) and gravity off, stepped `N` times: how far the particles moved from where the first step put
//! them, and how far the bones the simulation sets (`hclSimpleMeshBoneDeformOperator`) came out from their rest pose.
//! Also per cloth: rest pose times the skin's `boneFromSkinMeshTransforms`, which is one frame for all its bones when
//! the rest pose is the bind pose (the identity where the cloth's mesh is in model space).
//!
//!     cargo run --release -p modstudio-core --example cloth_rest -- <file.bphcl> [N]
//!
//! Environment switches, to narrow down what moves the particles: `CLOTH_ONLY=<part of a name>` runs only those
//! cloths, `CLOTH_GRAVITY` keeps the file's gravity, `CLOTH_TRACE` prints how far the particles went in each second,
//! `CLOTH_DROP=<kind>` leaves out the constraint sets of that kind (`StandardLink`, `LocalRange`, ...),
//! `CLOTH_NOCOLLIDE` clears every particle's collision mask. And to look at the data: `CLOTH_COLLIDERS` lists each
//! collidable's stored place against its bone times its offset, `CLOTH_DEPTH` how deep the particles start inside each
//! collidable, `CLOTH_SHAPES=<part of a name>` those collidables' shapes, `CLOTH_BEND` (and `CLOTH_BEND_LINKS`) what
//! each bend stiffness link pushes at rest, `CLOTH_CHAIN=<bone>` a bone's parents, `CLOTH_INFO` each simulated mesh's
//! settings, solve order and constraint sets.
//!
//! Child Link (2026-11-13): the rest pose is the bind pose of every cloth, and every collidable sits on its bone (0
//! apart). With gravity, as in the game, every part settles within a second (under 0.07 mm a second after that).
//! Without it, parts that touch a collidable keep moving by millimetres to centimetres a second: nothing presses them
//! onto it, and their damping (0.001 a second) keeps what the contacts and constraints trade. One strand
//! (FrontHair_A_R) also creeps without collisions: some of the hair's bend stiffness links push along their hinge at
//! rest (as stored: the bend formula itself checks out, `CLOTH_BEND_LINKS`), which gravity outweighs.

use havok_cloth::instance::ClothInstance;
use havok_cloth::matrix::Mat4;
use havok_formats::Tagfile;
use havok_formats::animation::{self, QsTransform};
use havok_formats::cloth::{self, OperatorKind};
use modstudio_formats::phive::Phive;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.first() else {
        eprintln!("usage: cloth_rest <file.bphcl> [N]");
        std::process::exit(2);
    };
    let steps: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(30);
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let phive = Phive::parse(&bytes).expect("Phive file");
    let tag = Tagfile::read(phive.tagfile, None).expect("tagfile");
    let physics = cloth::read(&tag).expect("cloth data");
    let skeletons = animation::container(&tag).expect("skeletons").skeletons;
    println!(
        "{} cloth data, {} skeletons",
        physics.cloths.len(),
        skeletons.len()
    );
    let (mut worst_particle, mut worst_bone, mut worst_axis) = (0f32, 0f32, 0f32);
    // CLOTH_ONLY=<part of a name>: only the cloths whose name has it
    let only = std::env::var("CLOTH_ONLY").unwrap_or_default();
    for data in physics.cloths.iter().filter(|d| d.name.contains(&only)) {
        let set = &data.transform_sets[0];
        let Some(skeleton) = skeletons.iter().find(|s| s.name == set.name) else {
            println!("{}: no skeleton {}", data.name, set.name);
            continue;
        };
        let rest = world_pose(&skeleton.parents, &skeleton.reference_pose);
        // CLOTH_CHAIN=<bone>: that bone's parents up to the root
        if let Some(bone) = std::env::var_os("CLOTH_CHAIN") {
            let bone = bone.to_string_lossy().into_owned();
            if let Some(mut i) = skeleton.bones.iter().position(|b| b.name == bone) {
                let mut chain = vec![bone.clone()];
                while let Ok(p) = usize::try_from(skeleton.parents[i]) {
                    chain.push(format!(
                        "{} (at y {:.3})",
                        skeleton.bones[p].name, rest[p].cols[3].0[1]
                    ));
                    i = p;
                }
                println!("    {}: {}", skeleton.name, chain.join(" < "));
            }
        }
        // CLOTH_DROP=<kind>: that kind of constraint set left out, to see which one moves the particles
        let mut data = data.clone();
        if let Some(drop) = std::env::var_os("CLOTH_DROP") {
            let drop = drop.to_string_lossy().into_owned();
            for sim in &mut data.sim_cloths {
                sim.constraint_sets
                    .retain(|c| !format!("{:?}", c.kind).starts_with(drop.as_str()));
            }
        }
        if std::env::var_os("CLOTH_NOCOLLIDE").is_some() {
            for sim in &mut data.sim_cloths {
                sim.static_collision_masks.iter_mut().for_each(|m| *m = 0);
            }
        }
        if std::env::var_os("CLOTH_INFO").is_some() {
            for op in &data.operators {
                if let OperatorKind::Simulate { configs, .. } = &op.kind {
                    for c in configs {
                        println!(
                            "    config {}: substeps {}, iterations {}, all collidables {}, used {:?}, order {:?}, adapt {}",
                            c.name,
                            c.sub_steps,
                            c.solve_iterations,
                            c.use_all_instance_collidables,
                            c.instance_collidables_used
                                .iter()
                                .map(|&u| u8::from(u))
                                .collect::<Vec<_>>(),
                            c.constraint_execution,
                            c.adapt_constraint_stiffness
                        );
                    }
                }
            }
            for sim in &data.sim_cloths {
                println!(
                    "    sets: {}",
                    sim.constraint_sets
                        .iter()
                        .enumerate()
                        .map(|(i, c)| format!(
                            "{i} {}",
                            format!("{:?}", c.kind)
                                .split([' ', '(', '{'])
                                .next()
                                .unwrap_or("")
                        ))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                println!(
                    "    {}: damping/s {}, transfer motion {} ({:?}), collidables {}, masks {:?}, max radius {}, total mass {}",
                    sim.name,
                    sim.global_damping_per_second,
                    sim.transfer_motion_enabled,
                    sim.transfer_motion,
                    sim.collidables.len(),
                    sim.static_collision_masks
                        .iter()
                        .take(4)
                        .collect::<Vec<_>>(),
                    sim.max_particle_radius,
                    sim.total_mass
                );
            }
        }
        let data = &data;
        // CLOTH_BEND: each bend stiffness set's push at the sim cloth's own rest pose (zero where the rest curvature is
        // read right), as written, with the rest curvature's sign flipped, and with no rest curvature
        if std::env::var_os("CLOTH_BEND").is_some() {
            // the file's rest pose of each sim cloth, and where one step at rest without collisions puts the particles
            let mut loose = data.clone();
            for sim in &mut loose.sim_cloths {
                sim.static_collision_masks.iter_mut().for_each(|m| *m = 0);
            }
            let mut probe = ClothInstance::new(&loose).expect("instance");
            probe.transform_sets[0] = rest.clone();
            for sim in &mut probe.sims {
                sim.gravity = havok_cloth::simd::V4([0.0; 4]);
            }
            probe.step(1.0 / 30.0, &mut []).expect("step");
            for sim in &data.sim_cloths {
                println!(
                    "    {}: poses {:?}",
                    sim.name,
                    sim.poses
                        .iter()
                        .map(|p| p.name.as_str())
                        .collect::<Vec<_>>()
                );
            }
            let sources: [(&str, Vec<Vec<[f32; 4]>>); 3] = [
                (
                    "file pose",
                    data.sim_cloths
                        .iter()
                        .map(|s| {
                            s.poses
                                .first()
                                .map(|p| p.positions.clone())
                                .unwrap_or_default()
                        })
                        .collect(),
                ),
                (
                    "file pose 2",
                    data.sim_cloths
                        .iter()
                        .map(|s| {
                            s.poses
                                .get(1)
                                .map(|p| p.positions.clone())
                                .unwrap_or_default()
                        })
                        .collect(),
                ),
                (
                    "one step",
                    probe
                        .sims
                        .iter()
                        .map(|s| s.positions.iter().map(|p| p.0).collect())
                        .collect(),
                ),
            ];
            for (source, all) in &sources {
                for (sim, positions) in data.sim_cloths.iter().zip(all) {
                    if positions.is_empty() {
                        continue;
                    }
                    for set in &sim.constraint_sets {
                        let cloth::ConstraintKind::BendStiffness {
                            links,
                            max_rest_pose_height_sq,
                            clamp_bend_stiffness,
                            use_rest_pose_config,
                        } = &set.kind
                        else {
                            continue;
                        };
                        let mut worst = [0f32; 3];
                        for link in links {
                            let p = link.particles.map(|i| positions[usize::from(i)]);
                            let sub =
                                |a: [f32; 4], b: [f32; 4]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
                            let cross = |a: [f32; 3], b: [f32; 3]| {
                                [
                                    a[1] * b[2] - a[2] * b[1],
                                    a[2] * b[0] - a[0] * b[2],
                                    a[0] * b[1] - a[1] * b[0],
                                ]
                            };
                            let len =
                                |a: [f32; 3]| (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
                            let mut bend = [0f32; 3];
                            for (q, w) in p.iter().zip(link.weights) {
                                for k in 0..3 {
                                    bend[k] += q[k] * w;
                                }
                            }
                            let (ca, cb, cd) = (sub(p[0], p[2]), sub(p[1], p[2]), sub(p[3], p[2]));
                            let (nc, nd) = (cross(cd, ca), cross(cb, cd));
                            let (lc, ld) = (len(nc), len(nd));
                            let mut offset = [0f32; 3];
                            for k in 0..3 {
                                offset[k] = nc[k] / lc.max(1e-12) + nd[k] / ld.max(1e-12);
                            }
                            let lo = len(offset).max(1e-12);
                            let current = lc * ld
                                / (cd[0] * cd[0] + cd[1] * cd[1] + cd[2] * cd[2]).max(1e-12);
                            for (v, sign) in [(0, 1.0), (1, -1.0)] {
                                let mut r = bend;
                                for k in 0..3 {
                                    r[k] += sign * link.rest_curvature * current * offset[k] / lo;
                                }
                                worst[v] = worst[v].max(len(r));
                            }
                            worst[2] = worst[2].max(len(bend));
                            if std::env::var_os("CLOTH_BEND_LINKS").is_some() {
                                let along = (bend[0] * offset[0]
                                    + bend[1] * offset[1]
                                    + bend[2] * offset[2])
                                    / lo;
                                let base = len(cd);
                                let hinge =
                                    (bend[0] * cd[0] + bend[1] * cd[1] + bend[2] * cd[2]) / base;
                                println!(
                                    "      link {:?} w {:?}: |bend| {:.3e}, along offset {:.3e}, along hinge {:.3e}, rest curvature {:.4}, heights {:.4} {:.4}, hinge {:.4}, needed/curvature {:.4}",
                                    link.particles,
                                    link.weights,
                                    len(bend),
                                    along,
                                    hinge,
                                    link.rest_curvature,
                                    lc / base,
                                    ld / base,
                                    base,
                                    -along / link.rest_curvature
                                );
                            }
                        }
                        println!(
                            "    bend stiffness ({source}): {} links, rest pose config {use_rest_pose_config}, clamp {clamp_bend_stiffness} ({max_rest_pose_height_sq}); largest push at rest: as written {:.2e}, sign flipped {:.2e}, flat {:.2e}",
                            links.len(),
                            worst[0],
                            worst[1],
                            worst[2]
                        );
                    }
                }
            }
        }
        // CLOTH_SHAPES=<part of a collidable's name>: those collidables' shapes and places, and the particles' extent
        if let Some(which) = std::env::var_os("CLOTH_SHAPES") {
            let which = which.to_string_lossy().into_owned();
            for sim in &data.sim_cloths {
                for c in sim.collidables.iter().filter(|c| c.name.contains(&which)) {
                    println!(
                        "    {}: at {:?}, x axis {:?}, y axis {:?}: {:?}",
                        c.name,
                        &c.transform[12..15],
                        &c.transform[0..3],
                        &c.transform[4..7],
                        c.shape
                    );
                }
                if let Some(pose) = sim.poses.first() {
                    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
                    for p in &pose.positions {
                        for k in 0..3 {
                            lo[k] = lo[k].min(p[k]);
                            hi[k] = hi[k].max(p[k]);
                        }
                    }
                    println!("    {} particles from {lo:?} to {hi:?}", sim.name);
                }
            }
        }
        if std::env::var_os("CLOTH_COLLIDERS").is_some() {
            for sim in &data.sim_cloths {
                let Some(map) = &sim.collidable_transform_map else {
                    continue;
                };
                for ((c, &bone), offset) in sim
                    .collidables
                    .iter()
                    .zip(&map.transform_indices)
                    .zip(&map.offsets)
                {
                    let Some(b) = usize::try_from(bone).ok().and_then(|b| rest.get(b)) else {
                        continue;
                    };
                    let target = b.mul(&Mat4::from_columns(*offset)).to_columns();
                    let stored = c.transform;
                    let d = ((target[12] - stored[12]).powi(2)
                        + (target[13] - stored[13]).powi(2)
                        + (target[14] - stored[14]).powi(2))
                    .sqrt();
                    println!(
                        "    {:34} on {:18} stored ({:.3} {:.3} {:.3}) bone x offset ({:.3} {:.3} {:.3}): {:.4} apart",
                        c.name,
                        skeleton
                            .bones
                            .get(bone as usize)
                            .map_or("?", |b| b.name.as_str()),
                        stored[12],
                        stored[13],
                        stored[14],
                        target[12],
                        target[13],
                        target[14],
                        d
                    );
                }
            }
        }
        // rest * boneFromSkinMesh is the identity when the rest pose is the skin's bind pose
        let mut bind_err = 0f32;
        let mut worst_bind = String::new();
        for op in &data.operators {
            if let OperatorKind::ObjectSpaceSkin {
                bone_from_skin_mesh,
                transform_subset,
                ..
            } = &op.kind
            {
                for (&bone, m) in transform_subset.iter().zip(bone_from_skin_mesh) {
                    let b = usize::from(bone);
                    let product = rest[b].mul(&Mat4::from_columns(*m)).to_columns();
                    let off = product
                        .iter()
                        .zip(Mat4::IDENTITY.to_columns())
                        .map(|(a, b)| (a - b).abs())
                        .fold(0f32, f32::max);
                    if off > bind_err {
                        bind_err = off;
                        worst_bind = format!("{} ({:?})", skeleton.bones[b].name, &product[12..15]);
                    }
                }
            }
        }
        println!(
            "    rest x boneFromSkinMesh off identity by {bind_err:.2e}, worst bone {worst_bind}"
        );
        // the products of every skinned bone: one frame for the whole cloth? which bone's rest transform is it?
        let mut products: Vec<[f32; 16]> = Vec::new();
        for op in &data.operators {
            if let OperatorKind::ObjectSpaceSkin {
                bone_from_skin_mesh,
                transform_subset,
                ..
            } = &op.kind
            {
                for (&bone, m) in transform_subset.iter().zip(bone_from_skin_mesh) {
                    products.push(
                        rest[usize::from(bone)]
                            .mul(&Mat4::from_columns(*m))
                            .to_columns(),
                    );
                }
            }
        }
        if let Some(first) = products.first() {
            let spread = products
                .iter()
                .map(|p| {
                    p.iter()
                        .zip(first)
                        .map(|(a, b)| (a - b).abs())
                        .fold(0f32, f32::max)
                })
                .fold(0f32, f32::max);
            let frame = Mat4::from_columns(*first);
            let (best, off) = rest
                .iter()
                .enumerate()
                .map(|(i, r)| {
                    let d = r
                        .to_columns()
                        .iter()
                        .zip(frame.to_columns())
                        .map(|(a, b)| (a - b).abs())
                        .fold(0f32, f32::max);
                    (i, d)
                })
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .unwrap_or((0, f32::MAX));
            println!(
                "    one frame for all {} skinned bones (spread {spread:.2e}); nearest bone at rest: {} (off by {off:.2e})",
                products.len(),
                skeleton.bones[best].name
            );
            let c = frame.to_columns();
            println!(
                "    frame: translation ({:.4} {:.4} {:.4}), x axis ({:.3} {:.3} {:.3}), y axis ({:.3} {:.3} {:.3})",
                c[12], c[13], c[14], c[0], c[1], c[2], c[4], c[5], c[6]
            );
        }
        // CLOTH_DEPTH: per collidable, how deep the particles that meet it start inside it (where one step at rest
        // without collisions puts them, the collidable at its stored place), less the particle's radius
        if std::env::var_os("CLOTH_DEPTH").is_some() {
            let mut loose = data.clone();
            for sim in &mut loose.sim_cloths {
                sim.static_collision_masks.iter_mut().for_each(|m| *m = 0);
            }
            let mut probe = ClothInstance::new(&loose).expect("instance");
            probe.transform_sets[0] = rest.clone();
            for sim in &mut probe.sims {
                sim.gravity = havok_cloth::simd::V4([0.0; 4]);
            }
            probe.step(1.0 / 30.0, &mut []).expect("step");
            for (sim, state) in data.sim_cloths.iter().zip(&probe.sims) {
                for (index, c) in sim.collidables.iter().enumerate() {
                    let shape = havok_cloth::collide::WorldShape::new(
                        &c.shape,
                        &Mat4::from_columns(c.transform),
                    );
                    let mask = 1u32 << index.min(30);
                    let (mut met, mut inside, mut deepest) = (0, 0, 0f32);
                    for (i, p) in sim.particles.iter().enumerate() {
                        if sim
                            .static_collision_masks
                            .get(i)
                            .is_none_or(|m| m & mask == 0)
                        {
                            continue;
                        }
                        met += 1;
                        let at = state.positions[i];
                        let depth = shape.closest(at, p.radius).distance - p.radius;
                        if depth < 0.0 {
                            inside += 1;
                            deepest = deepest.min(depth);
                        }
                    }
                    if met > 0 {
                        println!(
                            "    {:30} {:14} enabled {:5}: {met:3} particles meet it, {inside:3} start inside, deepest {deepest:.4}",
                            c.name,
                            format!("{:?}", c.shape)
                                .split([' ', '{'])
                                .next()
                                .unwrap_or(""),
                            c.enabled
                        );
                    }
                }
            }
        }
        let mut instance = ClothInstance::new(data).expect("instance");
        instance.transform_sets[0] = rest.clone();
        // CLOTH_GRAVITY: the file's gravity kept (the cloth hangs and settles instead of keeping its rest shape)
        if std::env::var_os("CLOTH_GRAVITY").is_none() {
            for sim in &mut instance.sims {
                sim.gravity = havok_cloth::simd::V4([0.0; 4]);
            }
        }
        let start: Vec<Vec<[f32; 4]>> = instance
            .sims
            .iter()
            .map(|s| s.positions.iter().map(|p| p.0).collect())
            .collect();
        instance.step(1.0 / 30.0, &mut []).expect("step");
        let mut jump = 0f32;
        for (state, s0) in instance.sims.iter().zip(&start) {
            for (p, q) in state.positions.iter().zip(s0) {
                jump = jump.max(dist(p.0, *q));
            }
        }
        let gravity = data
            .sim_cloths
            .first()
            .map(|s| s.gravity)
            .unwrap_or_default();
        let first: Vec<Vec<[f32; 4]>> = instance
            .sims
            .iter()
            .map(|s| s.positions.iter().map(|p| p.0).collect())
            .collect();
        // CLOTH_TRACE: how far the particles went in each second (30 steps), to tell settling from moving on
        let trace = std::env::var_os("CLOTH_TRACE").is_some();
        let mut window = first.clone();
        let mut seconds = Vec::new();
        for i in 1..steps {
            instance.transform_sets[0] = rest.clone();
            instance.step(1.0 / 30.0, &mut []).expect("step");
            if trace && i % 30 == 0 {
                let mut most = 0f32;
                for (state, w) in instance.sims.iter().zip(&mut window) {
                    for (p, q) in state.positions.iter().zip(w.iter_mut()) {
                        most = most.max(dist(p.0, *q));
                        *q = p.0;
                    }
                }
                seconds.push(format!("{most:.1e}"));
            }
        }
        if trace {
            println!("    per second: {}", seconds.join(" "));
        }
        let mut moved = 0f32;
        for (state, start) in instance.sims.iter().zip(&first) {
            for (p, q) in state.positions.iter().zip(start) {
                moved = moved.max(dist(p.0, *q));
            }
        }
        // the bones the bone deformers set
        let mut bone_err = 0f32;
        let mut axis_err = 0f32;
        let mut set_bones = 0;
        for op in &data.operators {
            if let OperatorKind::SimpleMeshBoneDeform {
                triangle_bone_pairs,
                ..
            } = &op.kind
            {
                for &(bone, _) in triangle_bone_pairs {
                    // a byte offset into the transform set: 64 bytes a matrix
                    let b = usize::from(bone) / 64;
                    let (got, want) = (instance.transform_sets[0][b], rest[b]);
                    bone_err = bone_err.max(dist(got.cols[3].0, want.cols[3].0));
                    for c in 0..3 {
                        axis_err = axis_err.max(dist(got.cols[c].0, want.cols[c].0));
                    }
                    set_bones += 1;
                }
            }
        }
        println!(
            "{:28} {:3} particles, {:3} bones set: first step from the file's pose {:.2e}, then moved {:.2e}, bones off by {:.2e} (axes {:.2e}), file gravity {:?}",
            data.name,
            instance
                .sims
                .iter()
                .map(|s| s.positions.len())
                .sum::<usize>(),
            set_bones,
            jump,
            moved,
            bone_err,
            axis_err,
            gravity
        );
        worst_particle = worst_particle.max(moved);
        worst_bone = worst_bone.max(bone_err);
        worst_axis = worst_axis.max(axis_err);
    }
    println!(
        "largest: particles moved {worst_particle:.2e}, bone positions {worst_bone:.2e}, bone axes {worst_axis:.2e}"
    );
}

/// The world transform of every bone at rest: each parent's times the bone's local translation, rotation and scale.
fn world_pose(parents: &[i16], pose: &[QsTransform]) -> Vec<Mat4> {
    let mut world: Vec<Mat4> = Vec::with_capacity(pose.len());
    for (i, q) in pose.iter().enumerate() {
        let local = Mat4::from_columns(trs(q));
        let w = match usize::try_from(parents[i]).ok().and_then(|p| world.get(p)) {
            Some(parent) => parent.mul(&local),
            None => local,
        };
        world.push(w);
    }
    world
}

fn trs(q: &QsTransform) -> [f32; 16] {
    let [x, y, z, w] = q.rotation;
    let s = q.scale;
    let t = q.translation;
    [
        (1. - 2. * (y * y + z * z)) * s[0],
        (2. * (x * y + w * z)) * s[0],
        (2. * (x * z - w * y)) * s[0],
        0.,
        (2. * (x * y - w * z)) * s[1],
        (1. - 2. * (x * x + z * z)) * s[1],
        (2. * (y * z + w * x)) * s[1],
        0.,
        (2. * (x * z + w * y)) * s[2],
        (2. * (y * z - w * x)) * s[2],
        (1. - 2. * (x * x + y * y)) * s[2],
        0.,
        t[0],
        t[1],
        t[2],
        1.,
    ]
}

fn dist(a: [f32; 4], b: [f32; 4]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}
