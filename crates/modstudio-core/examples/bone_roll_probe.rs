//! Research probe (a tester's report: new cloth bones twist): where the game's cloth parts put each bone they set from
//! a triangle (`hclSimpleMeshBoneDeformOperator`), against the new parts' strips (`cloth_new`), and how much the bones
//! twist about their length in the test motion.
//!
//! 1. Every cloth file: per bone set from a triangle, the bone's origin in barycentric coordinates of its triangle
//!    (from the stored matrix alone: its translation is the origin in the triangle frame's axes), and where the
//!    particle files' rest pose gives it, whether the triangle lies on both sides of the bone's length.
//! 2. Adult Link: a new chain of four `Ph_` bones laid exactly along his cap's bones (`Ph_Cap_1` .. `_4`, from
//!    `Head_2`), made into a part by `cloth_new` as the build makes it; both simulated in the test motion. Each bone's
//!    twist about its length (relative to its parent, against the parent's at rest) and its bend, per second; and how
//!    the game's feather, hood string and cap and the new part are simulated (their settings and constraint sets).
//!
//! `--show <part>` instead prints a part of adult Link's file: each bone's triangle and every particle in the bone's
//! frame at rest.
//!
//! ```text
//! cargo run --release -p modstudio-core --example bone_roll_probe -- <romfs> <cache dir> [--stiffness S]
//!     [--colliders auto|none] [--file <part of a cloth file's name>] [--show <part>]
//! ```

use std::collections::BTreeMap;
use std::path::Path;

use havok_formats::Tagfile;
use havok_formats::cloth::{self, BufferKind, OperatorKind};
use modstudio_core::cloth::{LiveCloth, Motion};
use modstudio_core::cloth_new::{self, RestBone, SceneClothSettings};
use modstudio_core::math;
use modstudio_core::{actor_links, model};
use modstudio_formats::bfres::Bfres;
use modstudio_formats::phive::Phive;
use modstudio_formats::romfs::Romfs;

type V = [f32; 3];
fn sub(a: V, b: V) -> V {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: V, b: V) -> V {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale(a: V, k: f32) -> V {
    [a[0] * k, a[1] * k, a[2] * k]
}
fn dot(a: V, b: V) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V, b: V) -> V {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn len(a: V) -> f32 {
    dot(a, a).sqrt()
}
fn norm(a: V) -> V {
    let l = len(a);
    if l > 0.0 { scale(a, 1.0 / l) } else { a }
}

#[derive(Default)]
struct Tally {
    corner: usize,
    edge: usize,
    inside: usize,
    outside: usize,
    straddles: usize,
    one_side: usize,
    no_geometry: usize,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [root, cache, rest @ ..] = args.as_slice() else {
        eprintln!(
            "usage: bone_roll_probe <romfs> <cache dir> [--stiffness S] [--colliders C] [--file F]"
        );
        std::process::exit(2);
    };
    let opt = |name: &str| {
        rest.iter()
            .position(|a| a == name)
            .and_then(|i| rest.get(i + 1))
            .cloned()
    };
    let romfs = Romfs::open(root).expect("romfs");
    let mut d = romfs.decoder().unwrap();

    // ---- 1. the game's parts: where a bone sits on its triangle
    let filter = opt("--file").unwrap_or_default();
    let mut files: Vec<String> = std::fs::read_dir(Path::new(root).join("Phive/Cloth"))
        .unwrap()
        .flatten()
        .map(|e| format!("Phive/Cloth/{}", e.file_name().to_string_lossy()))
        .filter(|f| f.contains(filter.as_str()))
        .collect();
    files.sort();
    let mut t = Tally::default();
    let mut parts = 0;
    let mut offcentre: Vec<f32> = Vec::new();
    for f in &files {
        let Ok(bytes) = romfs.read(&mut d, f) else {
            continue;
        };
        let Ok(phive) = Phive::parse(&bytes) else {
            continue;
        };
        let Ok(tag) = Tagfile::read(phive.tagfile, None) else {
            continue;
        };
        let Ok(physics) = cloth::read(&tag) else {
            continue;
        };
        for c in &physics.cloths {
            parts += 1;
            for op in &c.operators {
                let OperatorKind::SimpleMeshBoneDeform {
                    input_buffer,
                    triangle_bone_pairs,
                    local_bone_transforms,
                    ..
                } = &op.kind
                else {
                    continue;
                };
                let def = &c.buffers[*input_buffer as usize];
                let (tris, positions): (Vec<u16>, Option<Vec<V>>) = match &def.kind {
                    BufferKind::Scratch {
                        triangle_indices, ..
                    }
                    | BufferKind::Shadow {
                        triangle_indices, ..
                    } => (triangle_indices.clone(), None),
                    _ => match usize::try_from(def.sub_type)
                        .ok()
                        .and_then(|s| c.sim_cloths.get(s))
                    {
                        Some(sim) if def.buffer_type == 1 || def.buffer_type == 2 => (
                            sim.triangle_indices.clone(),
                            sim.poses
                                .first()
                                .map(|p| p.positions.iter().map(|q| [q[0], q[1], q[2]]).collect()),
                        ),
                        _ => (Vec::new(), None),
                    },
                };
                for (&(_, tri_off), local) in triangle_bone_pairs.iter().zip(local_bone_transforms)
                {
                    let (u, v) = (local[12], local[13]);
                    let g = (1.0 - u - v) / 3.0;
                    let bary = [u + g, v + g, g];
                    let lo = bary.iter().copied().fold(f32::MAX, f32::min);
                    let hi = bary.iter().copied().fold(f32::MIN, f32::max);
                    if lo < -0.02 {
                        t.outside += 1;
                    } else if hi > 0.98 {
                        t.corner += 1;
                    } else if lo < 0.02 {
                        t.edge += 1;
                    } else {
                        t.inside += 1;
                    }
                    // geometry: the bone's length (its x axis, as the deform makes it orthonormal) and whether
                    // the triangle's corners lie on both sides of it in the triangle's plane
                    let k = usize::from(tri_off) / 2;
                    let (Some(p), Some(idx)) = (positions.as_ref(), tris.get(k..k + 3)) else {
                        t.no_geometry += 1;
                        continue;
                    };
                    let corners: Vec<V> = idx.iter().map(|&i| p[usize::from(i)]).collect();
                    let cen = scale(add(add(corners[0], corners[1]), corners[2]), 1.0 / 3.0);
                    let s = sub(corners[0], cen);
                    let tt = sub(corners[1], cen);
                    let n = cross(s, tt);
                    let col = |c: usize| -> V {
                        add(
                            add(scale(s, local[c * 4]), scale(tt, local[c * 4 + 1])),
                            scale(n, local[c * 4 + 2]),
                        )
                    };
                    let origin = add(cen, col(3));
                    let (x0, z) = (col(0), col(2));
                    let y = cross(z, x0);
                    let x = norm(cross(y, z));
                    let size = corners
                        .iter()
                        .map(|&q| len(sub(q, cen)))
                        .fold(0.0, f32::max);
                    let across = norm(cross(norm(n), x));
                    let ds: Vec<f32> = corners
                        .iter()
                        .map(|&q| dot(sub(q, origin), across))
                        .collect();
                    let (mn, mx) = (
                        ds.iter().copied().fold(f32::MAX, f32::min),
                        ds.iter().copied().fold(f32::MIN, f32::max),
                    );
                    if mn < -0.05 * size && mx > 0.05 * size {
                        t.straddles += 1;
                    } else {
                        t.one_side += 1;
                    }
                    if size > 0.0 {
                        offcentre.push(dot(sub(cen, origin), across).abs() / size);
                    }
                }
            }
        }
    }
    offcentre.sort_by(f32::total_cmp);
    let q = |f: f32| {
        offcentre
            .get(((offcentre.len() as f32 - 1.0) * f) as usize)
            .copied()
            .unwrap_or(0.0)
    };
    println!(
        "{} files, {parts} parts; bones set from a triangle: origin on a corner {}, on an edge {}, inside {}, outside {}",
        files.len(),
        t.corner,
        t.edge,
        t.inside,
        t.outside
    );
    println!(
        "  in the rest pose: the triangle on both sides of the bone {}, on one side {} ({} without a rest pose)",
        t.straddles, t.one_side, t.no_geometry
    );
    println!(
        "  the triangle's middle off the bone, across it, in its size: median {:.2}, 90% {:.2}, most {:.2}",
        q(0.5),
        q(0.9),
        q(1.0)
    );

    // --show <part>: a part of adult Link's file, each bone's triangle and every particle in the bone's frame at rest
    if let Some(show) = opt("--show") {
        let bytes = romfs
            .read(&mut d, "Phive/Cloth/Player_Adult.Nin_Ounce_NVN2.bphcl")
            .unwrap();
        let phive = Phive::parse(&bytes).unwrap();
        let tag = Tagfile::read(phive.tagfile, None).unwrap();
        let physics = cloth::read(&tag).unwrap();
        for c in physics
            .cloths
            .iter()
            .filter(|c| c.name.contains(show.as_str()))
        {
            let sim = &c.sim_cloths[0];
            let p: Vec<V> = sim.poses[0]
                .positions
                .iter()
                .map(|q| [q[0], q[1], q[2]])
                .collect();
            // flatness: the smallest spread of the particles against the largest (covariance's eigenvalues by power
            // iteration would do; the box of the particles in their own best plane is enough here)
            println!(
                "{}: {} particles (fixed {:?}), triangles {:?}",
                c.name,
                p.len(),
                sim.fixed_particles,
                sim.triangle_indices.chunks(3).collect::<Vec<_>>()
            );
            for op in &c.operators {
                let OperatorKind::SimpleMeshBoneDeform {
                    triangle_bone_pairs,
                    local_bone_transforms,
                    ..
                } = &op.kind
                else {
                    continue;
                };
                for (&(bo, to), l) in triangle_bone_pairs.iter().zip(local_bone_transforms) {
                    let k = usize::from(to) / 2;
                    let idx = &sim.triangle_indices[k..k + 3];
                    let c3: Vec<V> = idx.iter().map(|&i| p[usize::from(i)]).collect();
                    let cen = scale(add(add(c3[0], c3[1]), c3[2]), 1.0 / 3.0);
                    let s = sub(c3[0], cen);
                    let tt = sub(c3[1], cen);
                    let n = cross(s, tt);
                    let col = |c: usize| -> V {
                        add(
                            add(scale(s, l[c * 4]), scale(tt, l[c * 4 + 1])),
                            scale(n, l[c * 4 + 2]),
                        )
                    };
                    let origin = add(cen, col(3));
                    let (x0, z0) = (col(0), col(2));
                    let y = norm(cross(z0, x0));
                    let x = norm(cross(y, z0));
                    let z = norm(z0);
                    let local = |q: V| {
                        let r = sub(q, origin);
                        format!("({:+.3} {:+.3} {:+.3})", dot(r, x), dot(r, y), dot(r, z))
                    };
                    println!(
                        "  bone {:2}: triangle {:?} at {}",
                        bo / 64,
                        idx,
                        c3.iter().map(|&q| local(q)).collect::<Vec<_>>().join(" ")
                    );
                    println!(
                        "      all particles: {}",
                        p.iter().map(|&q| local(q)).collect::<Vec<_>>().join(" ")
                    );
                }
            }
        }
        return;
    }
    if !filter.is_empty() {
        return;
    }

    // ---- 2. adult Link: a new chain along his cap, against the cap
    let file = "Phive/Cloth/Player_Adult.Nin_Ounce_NVN2.bphcl";
    let strings = model::load_external_strings(&romfs, &mut d).unwrap();
    let links = actor_links::links(Path::new(root), Some(Path::new(cache))).unwrap();
    let model_path = links
        .models_with(file)
        .first()
        .map(|s| s.to_string())
        .expect("a model");
    let data = romfs.read(&mut d, &model_path).unwrap();
    let b = Bfres::parse(&data, Some(&strings)).unwrap();
    let mut skeleton: Vec<RestBone> = cloth_new::rest_bones(&b.models[0].skeleton);
    let world = cloth_new::rest_world(&skeleton);
    let find = |name: &str| skeleton.iter().position(|b| b.name == name).expect(name);
    let head2 = find("Head_2");
    let cap: Vec<usize> = (1..=5).map(|k| find(&format!("Ph_Cap_{k}"))).collect();
    let mut added = Vec::new();
    let mut parent = head2;
    let mut parent_world = world[head2];
    for k in 0..4 {
        let local = math::mul(
            &math::inverse_affine(&parent_world).unwrap(),
            &world[cap[k]],
        );
        parent_world = world[cap[k]];
        let (tr, rot, sc) = math::decompose(&local);
        skeleton.push(RestBone {
            name: format!("Ph_Probe_{}", k + 1),
            parent: Some(parent),
            translation: tr,
            rotation: rot,
            scale: sc,
        });
        parent = skeleton.len() - 1;
        added.push(skeleton[parent].name.clone());
    }
    let tip = [world[cap[4]][12], world[cap[4]][13], world[cap[4]][14]];
    let tips = BTreeMap::from([("Ph_Probe_4".to_string(), tip)]);
    let stiffness: Option<f32> = opt("--stiffness").and_then(|s| s.parse().ok());
    let settings = BTreeMap::from([(
        "Ph_Probe_1".to_string(),
        SceneClothSettings {
            swing: None,
            stiffness,
            gravity: None,
            colliders: opt("--colliders"),
        },
    )]);
    let (chains, notes) = cloth_new::chains_of(&skeleton, &added, &[], &tips, &settings);
    for n in notes {
        println!("note: {n}");
    }
    let part = &chains[0];
    println!(
        "new part {}: anchor {}, {} bones, across {:?} (the strip's second column; the first is on the bones)",
        part.name,
        skeleton[part.anchor].name,
        part.bones.len(),
        part.across[0]
    );
    let bytes = romfs.read(&mut d, file).unwrap();
    let (written, met) =
        cloth_new::add_parts(&bytes, &skeleton, std::slice::from_ref(part)).unwrap();
    for m in met {
        println!("  {m}");
    }
    // how each part is simulated: the game's feather, hood string and cap, and the new part
    {
        let phive = Phive::parse(&written).unwrap();
        let tag = Tagfile::read(phive.tagfile, None).unwrap();
        let physics = cloth::read(&tag).unwrap();
        for name in [
            "SimulationMesh_Feather",
            "SimulationMesh_HoodString",
            "SimulationMesh_Hood",
            "SimulationMesh_Probe_1",
        ] {
            let Some(c) = physics.cloths.iter().find(|c| c.name == name) else {
                continue;
            };
            let sim = &c.sim_cloths[0];
            let masses: Vec<f32> = sim.particles.iter().map(|p| p.mass).collect();
            println!(
                "{name}: {} particles, masses {:?}, radius {:?}, gravity {:?}, damping {}/s",
                sim.particles.len(),
                masses,
                sim.particles.first().map(|p| p.radius),
                sim.gravity,
                sim.global_damping_per_second
            );
            for op in &c.operators {
                if let OperatorKind::Simulate { configs, .. } = &op.kind {
                    for cf in configs {
                        println!(
                            "  simulate config {}: sub-steps {}, iterations {}, adapt stiffness {}, order {:?}",
                            cf.name,
                            cf.sub_steps,
                            cf.solve_iterations,
                            cf.adapt_constraint_stiffness,
                            cf.constraint_execution
                        );
                    }
                }
            }
            for set in &sim.constraint_sets {
                use havok_formats::cloth::ConstraintKind as K;
                let line = match &set.kind {
                    K::StandardLink(links) | K::StretchLink(links) => {
                        let shares: Vec<f32> = links
                            .iter()
                            .map(|l| {
                                l.stiffness
                                    * (sim.particles[l.particle_a as usize].inv_mass
                                        + sim.particles[l.particle_b as usize].inv_mass)
                            })
                            .collect();
                        format!(
                            "{} links, stiffness {:.3}..{:.3}, share {:.3}..{:.3}",
                            links.len(),
                            links.iter().map(|l| l.stiffness).fold(f32::MAX, f32::min),
                            links.iter().map(|l| l.stiffness).fold(f32::MIN, f32::max),
                            shares.iter().copied().fold(f32::MAX, f32::min),
                            shares.iter().copied().fold(f32::MIN, f32::max)
                        )
                    }
                    K::BendStiffness { links, .. } => {
                        format!("{} bend-stiffness links", links.len())
                    }
                    K::BendLink(links) => format!("{} bend links", links.len()),
                    K::LocalRange {
                        stiffness,
                        constraints,
                        ..
                    } => {
                        format!("{} ranges, stiffness {stiffness}", constraints.len())
                    }
                    K::Transition { particles, .. } => {
                        format!("transition, {} particles", particles.len())
                    }
                    other => format!("{:?}", std::mem::discriminant(other)),
                };
                println!("  set {}: {line}", set.name);
            }
        }
    }
    // the cap's particles around its bones (in each bone's frame at rest: x along it)
    {
        let phive = Phive::parse(&bytes).unwrap();
        let tag = Tagfile::read(phive.tagfile, None).unwrap();
        let physics = cloth::read(&tag).unwrap();
        let hood = physics
            .cloths
            .iter()
            .find(|c| c.name == "SimulationMesh_Hood")
            .unwrap();
        let sim = &hood.sim_cloths[0];
        println!(
            "cap (SimulationMesh_Hood): {} particles ({} fixed), {} triangles",
            sim.particles.len(),
            sim.fixed_particles.len(),
            sim.triangle_indices.len() / 3
        );
        for op in &hood.operators {
            if let OperatorKind::SimpleMeshBoneDeform {
                triangle_bone_pairs,
                local_bone_transforms,
                ..
            } = &op.kind
            {
                for (&(bo, to), l) in triangle_bone_pairs.iter().zip(local_bone_transforms) {
                    let k = usize::from(to) / 2;
                    let idx = &sim.triangle_indices[k..k + 3];
                    let (u, v) = (l[12], l[13]);
                    let g = (1.0 - u - v) / 3.0;
                    println!(
                        "  bone {:2}: triangle {:?}, origin barycentric [{:.2}, {:.2}, {:.2}], out of plane {:.3}",
                        bo / 64,
                        idx,
                        u + g,
                        v + g,
                        g,
                        l[14]
                    );
                }
            }
        }
    }

    let run = |label: &str, written: &[u8]| {
        let live = LiveCloth::new(written, file).unwrap();
        let names: Vec<String> = live.bones.iter().map(|b| b.name.clone()).collect();
        let at = |n: &str| names.iter().position(|x| x == n);
        let rest_of = |i: usize| live.bones[i].rest;
        // (bone, parent: Some(index in live.bones) or None for Head_2, the anchor; child head at rest)
        let chain = |prefix: &str| -> Vec<(usize, Option<usize>, V)> {
            (1..=4)
                .map(|k| {
                    let me = at(&format!("{prefix}{k}")).unwrap();
                    let parent = if k == 1 {
                        None
                    } else {
                        at(&format!("{prefix}{}", k - 1))
                    };
                    let child = if k < 4 {
                        let c = rest_of(at(&format!("{prefix}{}", k + 1)).unwrap());
                        [c[12], c[13], c[14]]
                    } else {
                        tip
                    };
                    (me, parent, child)
                })
                .collect()
        };
        let head_rest = world[head2];
        let motion = Motion::for_size(live.size);
        let frames = (motion.seconds() * 60.0) as usize;
        for (title, prefix) in [("cap", "Ph_Cap_"), ("new", "Ph_Probe_")] {
            let ch = chain(prefix);
            // per bone: (max |twist|, twist settled, max bend)
            let mut stats = vec![(0f32, 0f32, 0f32); ch.len()];
            let mut per_second: Vec<Vec<f32>> = vec![Vec::new(); ch.len()];
            let mut live2 = LiveCloth::new(written, file).unwrap();
            for fr in 0..frames {
                let m = motion.at(fr as f32 / 60.0);
                live2.step(&m, 1.0 / 60.0).unwrap();
                let places = live2.places();
                for (k, &(me, parent, child)) in ch.iter().enumerate() {
                    let (p_now, p_rest) = match parent {
                        Some(p) => (places[p], rest_of(p)),
                        None => (math::mul(&m, &head_rest), head_rest),
                    };
                    let rel_now = math::mul(&math::inverse_affine(&p_now).unwrap(), &places[me]);
                    let rel_rest = math::mul(&math::inverse_affine(&p_rest).unwrap(), &rest_of(me));
                    let delta = math::mul(&math::inverse_affine(&rel_rest).unwrap(), &rel_now);
                    let (_, q, _) = math::decompose(&delta);
                    // the bone's length in its own frame at rest
                    let inv = math::inverse_affine(&rest_of(me)).unwrap();
                    let axis = norm(math::transform_point(&inv, child));
                    let along = dot([q[0], q[1], q[2]], axis);
                    let mut twist = 2.0 * along.atan2(q[3]).to_degrees();
                    if twist > 180.0 {
                        twist -= 360.0;
                    }
                    if twist < -180.0 {
                        twist += 360.0;
                    }
                    let moved = math::transform_normal(&delta, axis);
                    let bend = dot(norm(moved), axis).clamp(-1.0, 1.0).acos().to_degrees();
                    let s = &mut stats[k];
                    s.0 = s.0.max(twist.abs());
                    s.2 = s.2.max(bend);
                    if (fr as f32) < 60.0 * motion.time {
                        s.1 = twist;
                    }
                    if fr % 60 == 59 {
                        per_second[k].push(twist);
                    }
                }
            }
            println!(
                "{label} {title}: per bone, twist about its length (degrees, against its parent):"
            );
            for (k, s) in stats.iter().enumerate() {
                println!(
                    "  {}{}: settled {:6.1}, most {:6.1}; bend most {:5.1}; each second {:?}",
                    prefix,
                    k + 1,
                    s.1,
                    s.0,
                    s.2,
                    per_second[k]
                        .iter()
                        .map(|x| (x * 10.0).round() / 10.0)
                        .collect::<Vec<_>>()
                );
            }
        }
        drop(live);
    };
    run("as built", &written);
}
