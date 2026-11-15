//! What a cloth file's simulated parts are made of, for adding one: per cloth data, its skeletons against the model's
//! (bone for bone, by name and order), its buffers and transform sets, its states' operator lists, and per sim cloth:
//! particles (fixed ones), triangles, constraint sets, collidables, poses, and the operators that work on it (what each
//! reads and writes, and the bones it uses, by name).
//!
//!     cargo run --release -p modstudio-core --example cloth_parts_probe -- <romfs> <cloth file> [<model>] [<part filter>]

use havok_formats::Tagfile;
use havok_formats::animation;
use havok_formats::cloth::{self, OperatorKind};
use modstudio_core::model;
use modstudio_formats::bfres::Bfres;
use modstudio_formats::phive::Phive;
use modstudio_formats::romfs::Romfs;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [root, file, rest @ ..] = args.as_slice() else {
        eprintln!("usage: cloth_parts_probe <romfs> <cloth file> [<model>] [<part filter>]");
        std::process::exit(2);
    };
    let romfs = Romfs::open(root).expect("romfs");
    let mut d = romfs.decoder().unwrap();
    let bytes = romfs.read(&mut d, file).unwrap();
    let phive = Phive::parse(&bytes).unwrap();
    let tag = Tagfile::read(phive.tagfile, None).unwrap();
    let physics = cloth::read(&tag).unwrap();
    let skeletons = animation::container(&tag).unwrap().skeletons;
    let filter = rest.get(1).cloned().unwrap_or_default();
    if let Some(model_path) = rest.first() {
        let strings = model::load_external_strings(&romfs, &mut d).unwrap();
        let data = romfs.read(&mut d, model_path).unwrap();
        let b = Bfres::parse(&data, Some(&strings)).unwrap();
        let mb: Vec<&str> = b.models[0]
            .skeleton
            .bones
            .iter()
            .map(|x| x.name.as_str())
            .collect();
        for s in &skeletons {
            let same_order = s
                .bones
                .iter()
                .zip(&mb)
                .filter(|(a, b)| a.name == **b)
                .count();
            let missing: Vec<&str> = s
                .bones
                .iter()
                .map(|x| x.name.as_str())
                .filter(|n| !mb.contains(n))
                .take(5)
                .collect();
            println!(
                "skeleton {}: {} bones; model {} bones; same name at the same index: {same_order}; not in the model: {missing:?}",
                s.name,
                s.bones.len(),
                mb.len()
            );
        }
    }
    if let Ok(a) = modstudio_formats::aamp::Aamp::read(phive.params) {
        fn show(l: &modstudio_formats::aamp::List, depth: usize) {
            println!(
                "{}list 0x{:08x}: {} lists, {} objects",
                "  ".repeat(depth),
                l.hash,
                l.lists.len(),
                l.objects.len()
            );
            for o in l.objects.iter().take(40) {
                let texts: Vec<String> = o
                    .params
                    .iter()
                    .map(|p| {
                        p.text().map_or(
                            format!(
                                "0x{:08x}:{}={:?}",
                                p.hash,
                                p.kind,
                                &p.value[..p.value.len().min(8)]
                            ),
                            |s| format!("0x{:08x}={s}", p.hash),
                        )
                    })
                    .collect();
                println!(
                    "{}  object 0x{:08x}: {}",
                    "  ".repeat(depth),
                    o.hash,
                    texts.join(", ")
                );
            }
            for c in &l.lists {
                show(c, depth + 1);
            }
        }
        show(&a.root, 0);
    }
    println!(
        "tagfile {} bytes; parameter archive {} bytes",
        phive.tagfile.len(),
        bytes.len() - phive.tagfile.len()
    );
    for c in &physics.cloths {
        if !c.name.contains(filter.as_str()) {
            continue;
        }
        for t in &c.transform_sets {
            if let Some(s) = skeletons.iter().find(|s| s.name == t.name) {
                for (i, b) in s.bones.iter().enumerate() {
                    let p = s.parents[i];
                    let parent = if p >= 0 {
                        s.bones[p as usize].name.as_str()
                    } else {
                        "-"
                    };
                    let r = &s.reference_pose[i];
                    println!(
                        "  skeleton bone {i} {} (parent {parent}) rest {:?}",
                        b.name, r
                    );
                }
            }
        }
        println!(
            "cloth data {}: {} sim cloths, {} buffers, {} transform sets, {} operators, {} states",
            c.name,
            c.sim_cloths.len(),
            c.buffers.len(),
            c.transform_sets.len(),
            c.operators.len(),
            c.states.len()
        );
        for (i, t) in c.transform_sets.iter().enumerate() {
            println!(
                "  transform set {i}: {} type {} transforms {}",
                t.name, t.set_type, t.transforms
            );
        }
        for (i, b) in c.buffers.iter().enumerate() {
            println!("  buffer {i}: {:?}", b);
        }
        for s in &c.states {
            println!(
                "  state {}: operators {:?}; sim cloths {:?}; {} buffers, {} transform sets used",
                s.name,
                s.operators,
                s.used_sim_cloths,
                s.used_buffers.len(),
                s.used_transform_sets.len()
            );
        }
        let bone = |set: u32, i: u16| -> String {
            let name = &c.transform_sets[set as usize].name;
            skeletons
                .iter()
                .find(|s| &s.name == name)
                .and_then(|s| s.bones.get(i as usize))
                .map_or(format!("#{i}"), |b| b.name.clone())
        };
        if std::env::var("CLOTH_FULL").is_ok() {
            println!("FULL SIM CLOTHS {:#?}", c.sim_cloths);
            println!("FULL OPERATORS {:#?}", c.operators);
            println!("FULL STATES {:#?}", c.states);
        }
        for (si, sc) in c.sim_cloths.iter().enumerate() {
            println!(
                "  sim cloth {si} {}: {} particles ({} fixed), {} triangles, {} poses, {} collidables, masks {}, sim ops {:?}, mass {}",
                sc.name,
                sc.particles.len(),
                sc.fixed_particles.len(),
                sc.triangle_indices.len() / 3,
                sc.poses.len(),
                sc.collidables.len(),
                sc.static_collision_masks.len(),
                sc.sim_op_ids,
                sc.total_mass
            );
            for cs in &sc.constraint_sets {
                println!(
                    "      constraint set {}: {}",
                    cs.name,
                    format!("{:?}", cs.kind)
                        .chars()
                        .take(90)
                        .collect::<String>()
                );
            }
            for (oi, op) in c.operators.iter().enumerate() {
                let line = match &op.kind {
                    OperatorKind::Simulate { sim_cloth, configs } if *sim_cloth as usize == si => {
                        format!("simulate, {} config(s)", configs.len())
                    }
                    OperatorKind::MoveParticles {
                        pairs,
                        sim_cloth,
                        reference_buffer,
                    } if *sim_cloth as usize == si => {
                        format!(
                            "move particles: {} pairs from buffer {reference_buffer}",
                            pairs.len()
                        )
                    }
                    _ => String::new(),
                };
                if !line.is_empty() {
                    println!("      operator {oi} {}: {line}", op.name);
                }
            }
        }
        for (oi, op) in c.operators.iter().enumerate() {
            let line = match &op.kind {
                OperatorKind::ObjectSpaceSkin {
                    components,
                    transform_subset,
                    output_buffer,
                    transform_set,
                    local,
                    local_unpacked,
                    ..
                } => format!(
                    "object space skin {components:?} -> buffer {output_buffer}: bones {:?}, {} blocks, {} unpacked",
                    transform_subset
                        .iter()
                        .take(8)
                        .map(|&b| bone(*transform_set, b))
                        .collect::<Vec<_>>(),
                    local.len(),
                    local_unpacked.len()
                ),
                OperatorKind::SimpleMeshBoneDeform { .. } => {
                    format!("{:?}", op.kind).chars().take(1500).collect()
                }
                OperatorKind::Simulate { sim_cloth, .. } => {
                    format!("simulate sim cloth {sim_cloth}")
                }
                OperatorKind::MoveParticles {
                    sim_cloth,
                    reference_buffer,
                    pairs,
                } => {
                    format!(
                        "move particles {} -> sim cloth {sim_cloth} from buffer {reference_buffer}",
                        pairs.len()
                    )
                }
                other => format!("{other:?}").chars().take(700).collect(),
            };
            println!(
                "  operator {oi} {} (type {}, buffers {:?}, sets {:?}): {line}",
                op.name, op.operator_type, op.used_buffers, op.used_transform_sets
            );
        }
    }
}
