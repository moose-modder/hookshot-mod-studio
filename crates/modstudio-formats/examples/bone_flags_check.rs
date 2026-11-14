//! A bone's flags worked out from its transform, against every bone of the game's models: the bits besides the
//! transform ones (visible, rotation mode, billboard) as found, and the transform bits (24 to 27: this bone's own
//! scale uniform, scale volume one, rotation zero, translation zero; 28 to 31: the same down from the root) under a
//! few readings, each counted where it gives the game's bits.
//!
//!     cargo run --release -p modstudio-formats --example bone_flags_check -- <romfs> [name filter]

use std::collections::BTreeMap;
use std::path::Path;

use modstudio_formats::bfres::{Bfres, Bone, ExternalStrings};
use modstudio_formats::romfs::Romfs;

fn own(b: &Bone, euler: bool) -> u32 {
    let s = b.scale;
    let uniform = s[0] == s[1] && s[1] == s[2];
    let volume_one = s[0] * s[1] * s[2] == 1.0;
    let rotate_zero = if euler {
        b.rotation[0] == 0.0 && b.rotation[1] == 0.0 && b.rotation[2] == 0.0
    } else {
        b.rotation[0] == 0.0 && b.rotation[1] == 0.0 && b.rotation[2] == 0.0 && b.rotation[3] == 1.0
    };
    let translate_zero = b.translation == [0.0; 3];
    (uniform as u32) << 24
        | (volume_one as u32) << 25
        | (rotate_zero as u32) << 26
        | (translate_zero as u32) << 27
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(root) = args.first() else {
        eprintln!("usage: bone_flags_check <romfs> [name filter]");
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
    let mut bones = 0usize;
    let mut other_bits: BTreeMap<String, usize> = BTreeMap::new();
    let (mut own_ok, mut cum_and, mut cum_strict) = (0usize, 0usize, 0usize);
    let mut misses: Vec<String> = Vec::new();
    for f in &files {
        let Ok(data) = romfs.read(&mut d, format!("Model/{f}")) else {
            continue;
        };
        let Ok(b) = Bfres::parse(&data, Some(&strings)) else {
            continue;
        };
        for m in &b.models {
            let s = &m.skeleton;
            let euler = s.euler_rotations();
            let owns: Vec<u32> = s.bones.iter().map(|x| own(x, euler)).collect();
            // down from the root: each bit set if set for the bone and all its parents
            let mut cum = vec![0u32; s.bones.len()];
            let mut cum2 = vec![0u32; s.bones.len()];
            for i in 0..s.bones.len() {
                let mine = owns[i] >> 24;
                let (up, up2) = match s.bones[i].parent {
                    Some(p) if p < i => (cum[p], cum2[p]),
                    _ => (0xF, 0xF),
                };
                cum[i] = mine & up;
                // a stricter reading: scale bits need the parents' scale one, rotation zero the parents' too
                let parent_scale_one = up2 & 0b11 == 0b11;
                let mut c2 = mine & up2;
                if !parent_scale_one {
                    c2 &= !0b11;
                }
                cum2[i] = c2;
            }
            for (i, bone) in s.bones.iter().enumerate() {
                bones += 1;
                let other = bone.flags & !0xFF00_0000;
                *other_bits
                    .entry(format!("0x{other:x} (skeleton 0x{:x})", s.flags & 0x7000))
                    .or_default() += 1;
                let game_own = bone.flags & 0x0F00_0000;
                let game_cum = (bone.flags >> 28) & 0xF;
                if game_own == owns[i] {
                    own_ok += 1;
                } else if misses.len() < 8 {
                    misses.push(format!(
                        "{f} {}: own 0x{:x} vs 0x{:x}, scale {:?} rotation {:?} translation {:?}",
                        bone.name,
                        game_own >> 24,
                        owns[i] >> 24,
                        bone.scale,
                        bone.rotation,
                        bone.translation
                    ));
                }
                if game_cum == cum[i] {
                    cum_and += 1;
                } else if misses.len() < 16 {
                    misses.push(format!(
                        "{f} {}: cumulative 0x{game_cum:x} vs and 0x{:x} / strict 0x{:x}",
                        bone.name, cum[i], cum2[i]
                    ));
                }
                if game_cum == cum2[i] {
                    cum_strict += 1;
                }
            }
        }
    }
    println!(
        "{bones} bones: own bits as worked out {own_ok}; cumulative as each parent's AND {cum_and}, stricter {cum_strict}"
    );
    println!("the other bits: {other_bits:?}");
    for m in &misses {
        println!("    {m}");
    }
}
