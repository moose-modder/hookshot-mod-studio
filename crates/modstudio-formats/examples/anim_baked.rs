//! What an animation's "baked size" counts: for every animation of the game, its baked size against sums over its
//! curves (keys times the bytes a key takes in a few layouts), and which of them matches it exactly in how many.
//!
//!     cargo run --release -p modstudio-formats --example anim_baked -- <romfs> [name filter]

use std::collections::BTreeMap;
use std::path::Path;

use modstudio_formats::bfres::ExternalStrings;
use modstudio_formats::bfres_anim::{self, curve_flags};
use modstudio_formats::romfs::Romfs;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(root) = args.first() else {
        eprintln!("usage: anim_baked <romfs> [name filter]");
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
        .filter(|n| n.contains(".anim.") && n.contains(filter.as_str()))
        .collect();
    files.sort();
    let mut matches: BTreeMap<&str, usize> = BTreeMap::new();
    let mut anims = 0;
    let mut shown = 0;
    let mut misses: Vec<String> = Vec::new();
    for f in &files {
        let Ok(data) = romfs.read(&mut d, format!("Model/{f}")) else {
            continue;
        };
        let Ok(file) = bfres_anim::parse(&data, Some(&strings)) else {
            continue;
        };
        for a in &file.anims {
            anims += 1;
            let curves: Vec<_> = a.bones.iter().flat_map(|b| b.curves.iter()).collect();
            let keys: usize = curves.iter().map(|c| c.frames.len()).sum();
            // per curve: keys x (a frame f32 + its coefficients), in a few layouts
            let cand: Vec<(&str, usize)> = vec![
                ("keys x 20 (frame + 4 coefficients)", keys * 20),
                (
                    "per curve: keys x (4 + 16), quaternion keys x (4 + 16)",
                    curves.iter().map(|c| c.frames.len() * 20).sum(),
                ),
                (
                    "per curve: keys x 16 (4 coefficients), quaternion keys x 16",
                    curves.iter().map(|c| c.frames.len() * 16).sum(),
                ),
                (
                    "per curve: (keys x 4) + (keys x 16), each 8-aligned",
                    curves
                        .iter()
                        .map(|c| {
                            (c.frames.len() * 4).next_multiple_of(8)
                                + (c.frames.len() * 16).next_multiple_of(8)
                        })
                        .sum(),
                ),
                (
                    "per curve: keys x (4 + 16), a quaternion curve's keys x (4 + 32)",
                    curves
                        .iter()
                        .map(|c| {
                            c.frames.len()
                                * if c.flags & curve_flags::QUATERNION != 0 {
                                    36
                                } else {
                                    20
                                }
                        })
                        .sum(),
                ),
                (
                    "(frames + 1) x curves x 4",
                    (a.frame_count as usize + 1) * curves.len() * 4,
                ),
                (
                    "curves x (4 x frames + 32)",
                    curves.len() * (4 * a.frame_count.max(0) as usize + 32),
                ),
                (
                    "curves x (4 x frames + 32, rounded up to 8)",
                    curves.len() * (4 * a.frame_count.max(0) as usize + 32).next_multiple_of(8),
                ),
                (
                    "curves x (4 x frames + 32), a quaternion curve as 4",
                    curves
                        .iter()
                        .map(|c| {
                            if c.flags & curve_flags::QUATERNION != 0 {
                                4
                            } else {
                                1
                            }
                        })
                        .sum::<usize>()
                        * (4 * a.frame_count.max(0) as usize + 32),
                ),
            ];
            let simple =
                curves.len() * (4 * a.frame_count.max(0) as usize + 32).next_multiple_of(8);
            if simple != a.baked_size as usize && misses.len() < 6 {
                misses.push(format!(
                    "{f} / {}: baked {}, {} curves ({} quaternion), {} frames, flags 0x{:x}, simple {simple}",
                    a.name,
                    a.baked_size,
                    curves.len(),
                    curves.iter().filter(|c| c.flags & curve_flags::QUATERNION != 0).count(),
                    a.frame_count,
                    a.flags
                ));
            }
            for (name, v) in &cand {
                if *v == a.baked_size as usize {
                    *matches.entry(name).or_default() += 1;
                }
            }
            if shown < 4 {
                shown += 1;
                println!(
                    "{f} / {}: baked {}, {} curves, {} keys: {:?}",
                    a.name,
                    a.baked_size,
                    curves.len(),
                    keys,
                    cand.iter().map(|(_, v)| *v).collect::<Vec<_>>()
                );
            }
        }
    }
    println!("{anims} animations; exact matches: {matches:#?}");
    for m in &misses {
        println!("    not matching: {m}");
    }
}
