//! `bfres_write::write_edited` (and `write_skeleton`, for files holding a skeleton alone) with bones added, on the game's models: to every model the writer takes, two bones
//! (one on the root, one on that), and then: the written file's relocation table points inside it, it reads back with
//! the bones, its name dictionary is the one `res_dict` builds and finds every name, the rigidly skinned shapes'
//! vertices are on the same bones as before (their matrix indices moved up), every other vertex and index buffer is
//! byte for byte the game's, and it writes back unchanged.
//!
//!     cargo run --release -p modstudio-formats --example bones_add -- <romfs> [name filter]

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use modstudio_formats::bfres::{Bfres, ExternalStrings, Model};
use modstudio_formats::bfres_anim_write::check_relocations;
use modstudio_formats::bfres_bones::NewBone;
use modstudio_formats::bfres_write::{ShapeGeometry, write, write_edited, write_skeleton};
use modstudio_formats::res_dict::{self, DictNode};
use modstudio_formats::romfs::Romfs;

fn read_u64(d: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(d[at..at + 8].try_into().unwrap())
}

fn dict_at(d: &[u8], at: usize) -> Vec<DictNode> {
    let u32_at = |o: usize| u32::from_le_bytes(d[o..o + 4].try_into().unwrap());
    let count = u32_at(at + 4) as usize;
    (0..=count)
        .map(|i| {
            let n = at + 8 + i * 16;
            DictNode {
                reference: u32_at(n),
                left: u16::from_le_bytes(d[n + 4..n + 6].try_into().unwrap()),
                right: u16::from_le_bytes(d[n + 6..n + 8].try_into().unwrap()),
            }
        })
        .collect()
}

/// Each rigid shape's vertices' bones, through the matrix list.
fn rigid_bones(m: &Model) -> Vec<Vec<u16>> {
    m.shapes
        .iter()
        .filter(|s| s.skin_count == 1)
        .map(|s| {
            let g = ShapeGeometry::from_shape(m, s).unwrap();
            match g.attributes.get("_i0") {
                Some(i0) => i0
                    .iter()
                    .map(|v| m.skeleton.matrix_to_bone[v[0] as usize])
                    .collect(),
                None => vec![],
            }
        })
        .collect()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(root) = args.first() else {
        eprintln!("usage: bones_add <romfs> [name filter]");
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
    let t = Instant::now();
    let (mut ok, mut skipped, mut bad) = (0, BTreeMap::<String, usize>::new(), Vec::new());
    let (mut rigid_models, mut moved_gpu, mut skeletons) = (0, 0, 0);
    for f in &files {
        let Ok(data) = romfs.read(&mut d, format!("Model/{f}")) else {
            continue;
        };
        let Ok(b) = Bfres::parse(&data, Some(&strings)) else {
            continue;
        };
        let Some(m) = b.models.first() else { continue };
        let new = vec![
            NewBone {
                name: "ModStudio_Test_A".into(),
                parent: 0,
                scale: [1.0; 3],
                rotation: [0.0, 0.0, 0.0, 1.0],
                translation: [0.0, 0.1, 0.0],
            },
            NewBone {
                name: "ModStudio_Test_B".into(),
                parent: m.skeleton.bones.len(),
                scale: [1.0; 3],
                rotation: [0.3, 0.0, 0.0, 1.0],
                translation: [0.0, 0.2, 0.05],
            },
        ];
        // skeleton files (the `*_Animation.Skeleton` beside animation files) through `write_skeleton`
        let skeleton_only = m.shapes.is_empty();
        if skeleton_only {
            skeletons += 1;
        }
        let written = if skeleton_only {
            write_skeleton(&b, &new)
        } else {
            write_edited(&b, &BTreeMap::new(), &new, &BTreeMap::new())
        };
        let out = match written {
            Ok(o) => o,
            Err(e) => {
                *skipped.entry(e.to_string()).or_default() += 1;
                continue;
            }
        };
        let mut problems = Vec::new();
        if let Err(e) = check_relocations(&out) {
            problems.push(format!("relocations: {e}"));
        }
        let back = match Bfres::parse(&out, Some(&strings)) {
            Ok(x) => x,
            Err(e) => {
                bad.push(format!("{f}: doesn't read back: {e}"));
                continue;
            }
        };
        let bm = &back.models[0];
        let s = &bm.skeleton;
        let n = m.skeleton.bones.len();
        if s.bones.len() != n + 2
            || s.bones[n].name != "ModStudio_Test_A"
            || s.bones[n + 1].parent != Some(n)
        {
            problems.push("bones".into());
        }
        let names: Vec<&str> = s.bones.iter().map(|x| x.name.as_str()).collect();
        let dict = dict_at(&out, read_u64(&out, s.at + 8) as usize);
        if dict != res_dict::build(&names) {
            problems.push("dictionary".into());
        }
        if names
            .iter()
            .enumerate()
            .any(|(i, k)| res_dict::find(&dict, &names, k) != Some(i))
        {
            problems.push("a name not found".into());
        }
        if m.shapes.iter().any(|x| x.skin_count == 1) {
            rigid_models += 1;
        }
        if rigid_bones(m) != rigid_bones(bm) {
            problems.push("rigid vertices on other bones".into());
        }
        // every other buffer as the game's
        for (vi, (va, vb)) in m.vertex_buffers.iter().zip(&bm.vertex_buffers).enumerate() {
            let rigid = m
                .shapes
                .iter()
                .any(|x| x.vertex_buffer == vi && x.skin_count == 1);
            if !rigid
                && va
                    .buffers
                    .iter()
                    .zip(&vb.buffers)
                    .any(|(x, y)| x.data != y.data)
            {
                problems.push(format!("vertex buffer {vi} changed"));
            }
        }
        for (sa, sb) in m.shapes.iter().zip(&bm.shapes) {
            for (ma, mb) in sa.meshes.iter().zip(&sb.meshes) {
                if ma.indices().ok() != mb.indices().ok() {
                    problems.push(format!("indices of {}", sa.name));
                }
            }
        }
        if back.buffer_offset != b.buffer_offset {
            moved_gpu += 1;
        }
        let again = if skeleton_only {
            write_skeleton(&back, &[])
        } else {
            write(&back, &BTreeMap::new())
        };
        match again {
            Ok(again) if again == out => {}
            Ok(_) => problems.push("written again, not the same".into()),
            Err(e) => problems.push(format!("written again: {e}")),
        }
        if problems.is_empty() {
            ok += 1;
        } else {
            bad.push(format!("{f}: {}", problems.join("; ")));
        }
    }
    println!(
        "{} model files in {:.1} s: {ok} written with two bones and checked ({skeletons} skeleton files); {} with problems; {rigid_models} of them with rigid shapes; the GPU data moved in {moved_gpu}",
        files.len(),
        t.elapsed().as_secs_f32(),
        bad.len()
    );
    for (why, n) in &skipped {
        println!("    not written ({n}): {why}");
    }
    for x in bad.iter().take(10) {
        println!("    {x}");
    }
}
