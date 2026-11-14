//! Every cloth of one cloth file with its bones at rest (the reference pose of the skeleton its transform set names,
//! from the same file) and gravity off, stepped `N` times: how far the particles moved from where the first step put
//! them, and how far the bones the simulation sets (`hclSimpleMeshBoneDeformOperator`) came out from their rest pose.
//! Also per cloth: rest pose times the skin's `boneFromSkinMeshTransforms`, which is one frame for all its bones when
//! the rest pose is the bind pose (the identity where the cloth's mesh is in model space).
//!
//!     cargo run --release -p modstudio-core --example cloth_rest -- <file.bphcl> [N]
//!
//! Environment switches, to narrow down what moves the particles: `CLOTH_DROP=<kind>` leaves out the constraint sets
//! of that kind (`StandardLink`, `LocalRange`, ...), `CLOTH_NOCOLLIDE` clears every particle's collision mask,
//! `CLOTH_COLLIDERS` lists each collidable's stored place against its bone times its offset, `CLOTH_INFO` prints each
//! simulated mesh's settings.
//!
//! Child Link (2026-11-13): the rest pose is the bind pose of every cloth; the belts, skirt and hood stay in place and
//! their bones on their rest pose, and every collidable sits on its bone (0 apart). Hair and strands touching the head's
//! collidables keep moving (a few centimetres, bounded); without collisions they keep still but for one strand that
//! creeps.

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
    for data in &physics.cloths {
        let set = &data.transform_sets[0];
        let Some(skeleton) = skeletons.iter().find(|s| s.name == set.name) else {
            println!("{}: no skeleton {}", data.name, set.name);
            continue;
        };
        let rest = world_pose(&skeleton.parents, &skeleton.reference_pose);
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
            for sim in &data.sim_cloths {
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
        let mut instance = ClothInstance::new(data).expect("instance");
        instance.transform_sets[0] = rest.clone();
        for sim in &mut instance.sims {
            sim.gravity = havok_cloth::simd::V4([0.0; 4]);
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
        for _ in 1..steps {
            instance.transform_sets[0] = rest.clone();
            instance.step(1.0 / 30.0, &mut []).expect("step");
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
