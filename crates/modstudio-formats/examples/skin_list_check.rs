//! A shape's list of skin bones against the bones its vertices are weighted to: for every smooth and rigid shape of
//! the game's models, whether its list is exactly the bones its vertices' slots reference (with weight; or any),
//! more, or fewer.
//!
//!     cargo run --release -p modstudio-formats --example skin_list_check -- <romfs> [name filter]

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use modstudio_formats::bfres::{Bfres, ExternalStrings};
use modstudio_formats::bfres_write::ShapeGeometry;
use modstudio_formats::romfs::Romfs;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(root) = args.first() else {
        eprintln!("usage: skin_list_check <romfs> [name filter]");
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
    let mut examples: Vec<String> = Vec::new();
    for f in &files {
        let Ok(data) = romfs.read(&mut d, format!("Model/{f}")) else {
            continue;
        };
        let Ok(b) = Bfres::parse(&data, Some(&strings)) else {
            continue;
        };
        for m in &b.models {
            for s in m
                .shapes
                .iter()
                .filter(|s| s.skin_count >= 1 && !s.is_meshlet())
            {
                let Ok(g) = ShapeGeometry::from_shape(m, s) else {
                    continue;
                };
                let (mut weighted, mut any) = (BTreeSet::new(), BTreeSet::new());
                let sets = [
                    (g.attributes.get("_i0"), g.attributes.get("_w0")),
                    (g.attributes.get("_i1"), g.attributes.get("_w1")),
                ];
                for v in 0..g.vertex_count as usize {
                    for k in 0..s.skin_count as usize {
                        let (Some(i), w) = sets[k / 4] else { continue };
                        let Some(&bone) = m.skeleton.matrix_to_bone.get(i[v][k % 4] as usize)
                        else {
                            continue;
                        };
                        any.insert(bone);
                        let wt = if s.skin_count == 1 {
                            1.0
                        } else {
                            w.map_or(0.0, |w| w[v][k % 4])
                        };
                        if wt > 0.0 {
                            weighted.insert(bone);
                        }
                    }
                }
                let list: BTreeSet<u16> = s.skin_bone_indices.iter().copied().collect();
                let sorted = s.skin_bone_indices.windows(2).all(|w| w[0] < w[1]);
                let kind = format!(
                    "skin {}: {}{}",
                    if s.skin_count == 1 { "rigid" } else { "smooth" },
                    if list == weighted {
                        "the weighted bones"
                    } else if list == any {
                        "the bones of any slot"
                    } else if list.is_superset(&weighted) {
                        "more than the weighted bones"
                    } else {
                        "missing some weighted bones"
                    },
                    if sorted { "" } else { ", not ascending" }
                );
                if !kind.contains("the weighted bones") && examples.len() < 6 {
                    examples.push(format!(
                        "{f} {}: list {:?} weighted {:?}",
                        s.name,
                        &s.skin_bone_indices[..s.skin_bone_indices.len().min(8)],
                        weighted.iter().take(8).collect::<Vec<_>>()
                    ));
                }
                *kinds.entry(kind).or_default() += 1;
            }
        }
    }
    for (k, v) in &kinds {
        println!("{v:7} {k}");
    }
    for e in &examples {
        println!("    {e}");
    }
}
