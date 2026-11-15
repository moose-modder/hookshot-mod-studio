//! The new-part generator (`modstudio_core::cloth_new`) on the game's cloth files: each file's model (the first its
//! actors give it) gets a chain of two new bones hanging from its highest bone, a part is made for the chain, and the
//! written file is checked: it reads back, it simulates (`LiveCloth`), the new bones start where the skeleton has
//! them (settled at rest, within 5% of the chain's length: it sags a little), and they swing in the test motion and come back.
//!
//!     cargo run --release -p modstudio-core --example cloth_new_check -- <romfs> <cache dir> [part of a name] [--out <dir>]

use std::collections::BTreeMap;
use std::path::Path;

use modstudio_core::cloth::{LiveCloth, Motion};
use modstudio_core::cloth_new::{self, NewPart, RestBone};
use modstudio_core::{actor_links, math, model};
use modstudio_formats::bfres::Bfres;
use modstudio_formats::romfs::Romfs;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [root, cache, rest @ ..] = args.as_slice() else {
        eprintln!("usage: cloth_new_check <romfs> <cache dir> [part of a name] [--out <dir>]");
        std::process::exit(2);
    };
    let filter = rest
        .first()
        .filter(|a| !a.starts_with("--"))
        .cloned()
        .unwrap_or_default();
    let out_dir = rest
        .iter()
        .position(|a| a == "--out")
        .and_then(|i| rest.get(i + 1));
    let romfs = Romfs::open(root).expect("romfs");
    let mut d = romfs.decoder().unwrap();
    let strings = model::load_external_strings(&romfs, &mut d).unwrap();
    let links = actor_links::links(Path::new(root), Some(Path::new(cache))).unwrap();
    let mut files: Vec<String> = std::fs::read_dir(Path::new(root).join("Phive/Cloth"))
        .unwrap()
        .flatten()
        .map(|e| format!("Phive/Cloth/{}", e.file_name().to_string_lossy()))
        .filter(|f| f.contains(filter.as_str()))
        .collect();
    files.sort();
    let mut results: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut ok = 0;
    for f in &files {
        let Some(model_path) = links.models_with(f).first().map(|s| s.to_string()) else {
            results
                .entry("no model".into())
                .or_default()
                .push(f.clone());
            continue;
        };
        let outcome = (|| -> Result<String, String> {
            let data = romfs.read(&mut d, &model_path).map_err(|e| e.to_string())?;
            let b = Bfres::parse(&data, Some(&strings)).map_err(|e| e.to_string())?;
            let s = &b.models[0].skeleton;
            let euler = s.euler_rotations();
            let mut skeleton: Vec<RestBone> = s
                .bones
                .iter()
                .map(|x| RestBone {
                    name: x.name.clone(),
                    parent: x.parent,
                    translation: x.translation,
                    rotation: if euler {
                        math::quat_from_euler_xyz([x.rotation[0], x.rotation[1], x.rotation[2]])
                    } else {
                        x.rotation
                    },
                    scale: x.scale,
                })
                .collect();
            let world = cloth_new::rest_world(&skeleton);
            // the highest bone; the chain hangs from it, two bones of a tenth of the model's height each
            let (anchor, top) = world
                .iter()
                .enumerate()
                .map(|(i, m)| (i, m[13]))
                .fold((0, f32::MIN), |a, b| if b.1 > a.1 { b } else { a });
            let low = world.iter().map(|m| m[13]).fold(f32::MAX, f32::min);
            let step = ((top - low) * 0.1).max(0.02);
            let a = &world[anchor];
            let head = [a[12], a[13], a[14]];
            let joints: Vec<[f32; 3]> = (0..3)
                .map(|k| [head[0], head[1] - step * k as f32, head[2]])
                .collect();
            let mut parent_world = *a;
            let mut bones = Vec::new();
            for (k, joint) in joints.iter().take(2).enumerate() {
                let place = math::from_trs(*joint, [0.0, 0.0, 0.0, 1.0], [1.0; 3]);
                let local = math::mul(
                    &math::inverse_affine(&parent_world).ok_or("a flat bone")?,
                    &place,
                );
                let (t, q, sc) = math::decompose(&local);
                let parent = if k == 0 { anchor } else { skeleton.len() - 1 };
                skeleton.push(RestBone {
                    name: format!("Ph_ModStudio_Test_{}", k + 1),
                    parent: Some(parent),
                    translation: t,
                    rotation: q,
                    scale: sc,
                });
                bones.push(skeleton.len() - 1);
                parent_world = place;
            }
            let part = NewPart {
                name: "SimulationMesh_ModStudio_Test".into(),
                anchor,
                bones: bones.clone(),
                joints: joints.clone(),
                across: vec![[step * 0.3, 0.0, 0.0]; 3],
                settings: cloth_new::PartSettings::default(),
            };
            let bytes = romfs.read(&mut d, f).map_err(|e| e.to_string())?;
            let (written, met) =
                cloth_new::add_parts(&bytes, &skeleton, &[part]).map_err(|e| e.0)?;
            if let Some(dir) = out_dir {
                let p = Path::new(dir).join(f);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(&p, &written).unwrap();
            }
            let mut live = LiveCloth::new(&written, f).map_err(|e| e.0)?;
            let names: Vec<&str> = live.bones.iter().map(|b| b.name.as_str()).collect();
            let idx: Vec<usize> = (1..=2)
                .map(|k| {
                    names
                        .iter()
                        .position(|n| *n == format!("Ph_ModStudio_Test_{k}"))
                        .ok_or(format!("the simulation doesn't move Ph_ModStudio_Test_{k}"))
                })
                .collect::<Result<_, _>>()?;
            let world2 = cloth_new::rest_world(&skeleton);
            // at rest (settled): where the skeleton has them
            let mut worst_rest = 0f32;
            for (k, &i) in idx.iter().enumerate() {
                let p = live.places()[i];
                let want = world2[bones[k]];
                let dist = ((p[12] - want[12]).powi(2)
                    + (p[13] - want[13]).powi(2)
                    + (p[14] - want[14]).powi(2))
                .sqrt();
                worst_rest = worst_rest.max(dist);
            }
            // the test motion: how far the tip bone strays from where its rest place would carry it, at most
            let motion = Motion::for_size(live.size);
            let frames = (motion.seconds() * 60.0) as usize;
            let mut swing = 0f32;
            let mut end = 0f32;
            for fr in 0..frames {
                let m = motion.at(fr as f32 / 60.0);
                live.step(&m, 1.0 / 60.0).map_err(|e| e.0)?;
                let p = live.places()[idx[1]];
                let want = math::mul(&m, &world2[bones[1]]);
                let dist = ((p[12] - want[12]).powi(2)
                    + (p[13] - want[13]).powi(2)
                    + (p[14] - want[14]).powi(2))
                .sqrt();
                swing = swing.max(dist);
                end = dist;
            }
            if worst_rest > 0.05 * 2.0 * step {
                return Err(format!("at rest the new bones are {worst_rest:.4} m off"));
            }
            if !swing.is_finite() || swing > 4.0 * 2.0 * step {
                return Err(format!("it runs off ({swing} m)"));
            }
            let colliders = met
                .first()
                .and_then(|l| l.split(" meets ").nth(1))
                .map_or(0, |l| l.split(", ").count());
            Ok(format!(
                "{colliders} colliders; anchor {}, chain {:.3} m: at rest off {:.4} m; swings {:.3} m; ends {:.3} m off",
                skeleton[anchor].name,
                2.0 * step,
                worst_rest,
                swing,
                end
            ))
        })();
        match outcome {
            Ok(line) => {
                ok += 1;
                if files.len() <= 12 {
                    println!("{f}: {line}");
                }
                results.entry("ok".into()).or_default().push(f.clone());
            }
            Err(e) => {
                let key: String = e
                    .chars()
                    .filter(|c| !c.is_ascii_digit())
                    .take(120)
                    .collect();
                results.entry(key).or_default().push(format!("{f}: {e}"));
            }
        }
    }
    println!(
        "{} files: {ok} with a new part that reads back, simulates and swings",
        files.len()
    );
    for (k, v) in &results {
        if k == "ok" {
            continue;
        }
        println!("  {} x {k}\n      e.g. {}", v.len(), v[0]);
    }
}
