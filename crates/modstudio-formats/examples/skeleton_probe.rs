//! What a model's skeleton is made of, for adding bones: in every model file (or those matching a filter), whether
//! its names are IDs into the shared string table or strings in the file (and whether one file mixes them), which
//! name fields the relocation table lists, how its skeleton is laid out (bone array, name dictionary, matrix list,
//! inverse matrices, and what follows each), and the `*_Animation.Skeleton` files against their models.
//!
//!     cargo run --release -p modstudio-formats --example skeleton_probe -- <romfs> [name filter]

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use modstudio_formats::bfres::{Bfres, ExternalStrings, flags};
use modstudio_formats::romfs::Romfs;

fn u16_at(d: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(d[at..at + 2].try_into().unwrap())
}
fn u32_at(d: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(d[at..at + 4].try_into().unwrap())
}
fn u64_at(d: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(d[at..at + 8].try_into().unwrap())
}

/// The pointer fields the relocation table lists.
fn relocated(d: &[u8]) -> HashSet<usize> {
    let rlt = u32_at(d, 0x18) as usize;
    let sections = u32_at(d, rlt + 8) as usize;
    let total: usize = (0..sections)
        .map(|i| u32_at(d, rlt + 16 + i * 24 + 20) as usize)
        .sum();
    let at = rlt + 16 + sections * 24;
    let mut out = HashSet::new();
    for i in 0..total {
        let e = at + i * 8;
        let pos = u32_at(d, e) as usize;
        let structs = u16_at(d, e + 4) as usize;
        let ptrs = d[e + 6] as usize;
        let pad = d[e + 7] as usize;
        for s in 0..structs {
            for p in 0..ptrs {
                out.insert(pos + s * (ptrs + pad) * 8 + p * 8);
            }
        }
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(root) = args.first() else {
        eprintln!("usage: skeleton_probe <romfs> [name filter]");
        std::process::exit(2);
    };
    let filter = args.get(1).cloned().unwrap_or_default();
    let romfs = Romfs::open(root).expect("romfs");
    let mut d = romfs.decoder().unwrap();
    let strings = ExternalStrings::parse(
        &romfs
            .read(&mut d, "Shader/ExternalBinaryString.bfres.zs")
            .unwrap(),
    )
    .unwrap();
    let mut files: Vec<String> = std::fs::read_dir(Path::new(root).join("Model"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| {
            n.ends_with(".bfres.zs") && !n.contains(".anim.") && n.contains(filter.as_str())
        })
        .collect();
    files.sort();
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut shown = 0;
    let mut skeleton_files = Vec::new();
    for f in &files {
        let Ok(data) = romfs.read(&mut d, format!("Model/{f}")) else {
            continue;
        };
        let ext = data[0xEE];
        let Ok(bfres) = Bfres::parse(&data, Some(&strings)) else {
            *kinds.entry("does not parse".into()).or_default() += 1;
            continue;
        };
        if f.contains("_Animation.Skeleton") {
            skeleton_files.push((
                f.clone(),
                bfres
                    .models
                    .iter()
                    .map(|m| {
                        (
                            m.name.clone(),
                            m.skeleton
                                .bones
                                .iter()
                                .map(|b| b.name.clone())
                                .collect::<Vec<_>>(),
                            m.shapes.len(),
                        )
                    })
                    .collect::<Vec<_>>(),
            ));
        }
        let Some(m) = bfres.models.first() else {
            continue;
        };
        let rel = relocated(&data);
        // bone name fields: offsets into the file, or IDs
        let (mut ids, mut offsets, mut listed_ids, mut listed_offsets) = (0, 0, 0, 0);
        for b in &m.skeleton.bones {
            let v = u64_at(&data, b.at) as usize;
            let listed = rel.contains(&b.at);
            if v >= data.len() {
                ids += 1;
                listed_ids += listed as usize;
            } else {
                offsets += 1;
                listed_offsets += listed as usize;
            }
        }
        // other names in the same file: shapes' and materials'
        let (mut other_ids, mut other_offsets) = (0, 0);
        for s in &m.shapes {
            if u64_at(&data, s.at + 0x08) as usize >= data.len() {
                other_ids += 1
            } else {
                other_offsets += 1
            }
        }
        let kind = format!(
            "flags 0x{ext:02x}{}: bone names {} / shape names {}; relocated: ID fields {}, offset fields {}",
            if ext & flags::HAS_EXTERNAL_STRING != 0 {
                " (external strings)"
            } else {
                ""
            },
            match (ids > 0, offsets > 0) {
                (true, true) => "mixed",
                (true, false) => "IDs",
                (false, true) => "in the file",
                _ => "none",
            },
            match (other_ids > 0, other_offsets > 0) {
                (true, true) => "mixed",
                (true, false) => "IDs",
                (false, true) => "in the file",
                _ => "none",
            },
            if ids == 0 {
                "-".to_string()
            } else if listed_ids == ids {
                "all".into()
            } else if listed_ids == 0 {
                "none".into()
            } else {
                "some".into()
            },
            if offsets == 0 {
                "-".to_string()
            } else if listed_offsets == offsets {
                "all".into()
            } else if listed_offsets == 0 {
                "none".into()
            } else {
                "some".into()
            },
        );
        *kinds.entry(kind).or_default() += 1;
        if shown < 3
            && (f.starts_with("Player_Child.")
                || f.starts_with("Player_Adult.")
                || f.starts_with("Npc_U_GreatFairy."))
        {
            shown += 1;
            let s = &m.skeleton;
            let sk = s.at;
            println!(
                "{f}: flags 0x{ext:02x}, {} bones ({} smooth, {} rigid matrices), skeleton flags 0x{:x}",
                s.bones.len(),
                s.smooth_count,
                s.rigid_count,
                s.flags
            );
            for (name, o) in [
                ("dict", 0x08),
                ("bone array", 0x10),
                ("matrix list", 0x18),
                ("inverse matrices", 0x20),
                ("user pointer", 0x28),
                ("0x30", 0x30),
            ] {
                let v = u64_at(&data, sk + o);
                println!(
                    "    skeleton +0x{o:02x} {name}: 0x{v:x}{}",
                    if rel.contains(&(sk + o)) {
                        " (relocated)"
                    } else {
                        ""
                    }
                );
            }
            let b0 = s.bones[0].at;
            println!(
                "    bone 0 raw words: {:x?}",
                (0..11)
                    .map(|i| u64_at(&data, b0 + i * 8))
                    .collect::<Vec<_>>()
            );
            println!(
                "    bone 0 fields relocated: {:?}",
                (0..11)
                    .filter(|i| rel.contains(&(b0 + i * 8)))
                    .collect::<Vec<_>>()
            );
            let last = s.bones.last().unwrap().at + 0x58;
            let dict = u64_at(&data, sk + 0x08) as usize;
            let list = u64_at(&data, sk + 0x18) as usize;
            let inv = u64_at(&data, sk + 0x20) as usize;
            println!(
                "    bone array 0x{:x}..0x{last:x}; dict 0x{dict:x} ({} entries); matrix list 0x{list:x}..0x{:x}; inverse matrices 0x{inv:x}..0x{:x}",
                s.bones[0].at,
                u32_at(&data, dict + 4),
                list + (s.smooth_count + s.rigid_count) * 2,
                inv + s.smooth_count * 48
            );
            println!(
                "    first dict key field: 0x{:x}",
                u64_at(&data, dict + 8 + 16 + 8)
            );
            let rlt = u32_at(&data, 0x18) as usize;
            let sections = u32_at(&data, rlt + 8) as usize;
            for i in 0..sections {
                let x = rlt + 16 + i * 24;
                println!(
                    "    relocation section {i}: pos 0x{:x} size 0x{:x} entries {}",
                    u32_at(&data, x + 8),
                    u32_at(&data, x + 12),
                    u32_at(&data, x + 20)
                );
            }
            println!(
                "    shapes {}: e.g. {} skin bones {:?}, bone {}",
                m.shapes.len(),
                m.shapes[0].name,
                &m.shapes[0].skin_bone_indices[..m.shapes[0].skin_bone_indices.len().min(6)],
                m.shapes[0].bone
            );
        }
    }
    println!("{} model files:", files.len());
    for (k, v) in &kinds {
        println!("    {v:5} {k}");
    }
    println!("{} *_Animation.Skeleton files", skeleton_files.len());
    // each against the model of its name (the file name before `_Animation`, or its `_Demo` and numbered forms)
    for (f, models) in &skeleton_files {
        let stem = f.split("_Animation").next().unwrap_or(f);
        let base = stem.trim_end_matches("_Demo").to_string();
        let model_file = files
            .iter()
            .find(|m| m.starts_with(&format!("{base}.{base}.")))
            .cloned();
        let Some(mf) = model_file else {
            println!("    {f}: no model {base}");
            continue;
        };
        let data = romfs.read(&mut d, format!("Model/{mf}")).unwrap();
        let b = Bfres::parse(&data, Some(&strings)).unwrap();
        let model_bones: Vec<String> = b.models[0]
            .skeleton
            .bones
            .iter()
            .map(|x| x.name.clone())
            .collect();
        let skel = &models[0].1;
        let set: HashSet<&String> = model_bones.iter().collect();
        let in_model = skel.iter().filter(|n| set.contains(n)).count();
        let same_order = skel
            .iter()
            .zip(&model_bones)
            .take_while(|(a, b)| a == b)
            .count();
        let not_in_skel: Vec<&String> = model_bones
            .iter()
            .filter(|n| !skel.contains(n))
            .take(4)
            .collect();
        println!(
            "    {f}: {} bones, {in_model} in {mf} ({} bones); same order for the first {same_order}; model bones not in it: {} e.g. {:?}",
            skel.len(),
            model_bones.len(),
            model_bones.len() - in_model,
            not_in_skel
        );
    }
    for (f, models) in skeleton_files.iter().take(6) {
        for (name, bones, shapes) in models {
            println!(
                "    {f}: model {name}, {} bones, {shapes} shapes; first bones {:?}",
                bones.len(),
                &bones[..bones.len().min(5)]
            );
        }
    }
}
