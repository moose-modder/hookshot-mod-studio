//! For adding a bone to an animation: how the game's animation files hold their bones' names (strings in the file
//! or IDs into the shared string table), which words of a bone animation record the relocation table lists, and
//! whether the game's own animations name bones their models don't have (models given animation files by their
//! actors, `actor_links`): if they do, the game skips names it doesn't find, and an animation file shared by
//! several models can take a bone only one of them has. Also which models share the player's animation files.
//!
//!     cargo run --release -p modstudio-core --example anim_names_probe -- <romfs> [cache dir]

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

use modstudio_core::{actor_links, model};
use modstudio_formats::bfres::Bfres;
use modstudio_formats::bfres_anim;
use modstudio_formats::romfs::Romfs;

fn u8_at(d: &[u8], at: usize) -> u8 {
    d[at]
}
fn u16_at(d: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(d[at..at + 2].try_into().unwrap())
}
fn u32_at(d: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(d[at..at + 4].try_into().unwrap())
}
fn u64_at(d: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(d[at..at + 8].try_into().unwrap())
}

/// The pointer fields the relocation table of `data` lists.
fn relocated(data: &[u8]) -> HashSet<usize> {
    let mut out = HashSet::new();
    let rlt = u32_at(data, 0x18) as usize;
    let count = u32_at(data, rlt + 8) as usize;
    let entries_at = rlt + 16 + count * 24;
    let total: usize = (0..count)
        .map(|i| u32_at(data, rlt + 16 + i * 24 + 20) as usize)
        .sum();
    for i in 0..total {
        let e = entries_at + i * 8;
        let pos = u32_at(data, e) as usize;
        let structs = u16_at(data, e + 4) as usize;
        let ptrs = u8_at(data, e + 6) as usize;
        let pad = u8_at(data, e + 7) as usize;
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
        eprintln!("usage: anim_names_probe <romfs> [cache dir]");
        std::process::exit(2);
    };
    let romfs = Romfs::open(root).expect("romfs");
    let mut d = romfs.decoder().unwrap();
    let strings = model::load_external_strings(&romfs, &mut d).unwrap();
    let mut files: Vec<String> = std::fs::read_dir(Path::new(root).join("Model"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".anim.bfres.zs"))
        .collect();
    files.sort();
    // per file: the names its animations move
    let mut anim_bones: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let (mut in_file, mut external, mut records) = (0usize, 0usize, 0usize);
    let mut words: BTreeMap<String, usize> = BTreeMap::new();
    let mut bind_listed = 0usize;
    let mut name_in_string_table = 0usize;
    let mut other_flags: BTreeMap<String, usize> = BTreeMap::new();
    let mut transform_bits: BTreeMap<String, usize> = BTreeMap::new();
    let (mut words_18_20_set, mut curveless_with_pointer) = (0usize, 0usize);
    for f in &files {
        let Ok(data) = romfs.read(&mut d, format!("Model/{f}")) else {
            continue;
        };
        let Ok(file) = bfres_anim::parse(&data, Some(&strings)) else {
            continue;
        };
        let rel = relocated(&data);
        // the string table: "_STR" block offset at 0x30? found by its magic
        let str_at = data.windows(4).position(|w| w == b"_STR");
        let set = anim_bones.entry(format!("Model/{f}")).or_default();
        for a in &file.anims {
            let bind = u64_at(&data, a.at + 0x20) as usize;
            if bind != 0 && rel.contains(&(a.at + 0x20)) {
                bind_listed += 1;
            }
            for b in &a.bones {
                set.insert(b.name.clone());
                records += 1;
                let v = u64_at(&data, b.at);
                if v as usize >= data.len() {
                    external += 1;
                } else {
                    in_file += 1;
                    if str_at.is_some_and(|s| v as usize > s) {
                        name_in_string_table += 1;
                    }
                }
                let other = b.flags & !(0x38 | 0x3FF << 6 | 1 << 20);
                *other_flags
                    .entry(format!("0x{other:08x} curves {}", !b.curves.is_empty()))
                    .or_default() += 1;
                {
                    // bits 24 to 27 from the base values, as a skeleton's bones have them
                    let s = b.base_scale.unwrap_or([1.0; 3]);
                    let r = b.base_rotation.unwrap_or([0.0, 0.0, 0.0, 1.0]);
                    let tr = b.base_translation.unwrap_or([0.0; 3]);
                    let rotate_zero = if b.quaternion() {
                        r[0] == 0.0 && r[1] == 0.0 && r[2] == 0.0 && r[3] == 1.0
                    } else {
                        r[0] == 0.0 && r[1] == 0.0 && r[2] == 0.0
                    };
                    let mine = ((s[0] == s[1] && s[1] == s[2]) as u32) << 24
                        | ((s[0] * s[1] * s[2] == 1.0) as u32) << 25
                        | (rotate_zero as u32) << 26
                        | ((tr == [0.0; 3]) as u32) << 27;
                    // ... and a component a curve moves isn't uniform, one or zero
                    let moved =
                        |lo: u32, hi: u32| b.curves.iter().any(|c| (lo..=hi).contains(&c.target));
                    let mut mine = mine;
                    if moved(0x04, 0x0C) {
                        mine &= !(0b11 << 24);
                    }
                    if moved(0x20, 0x2C) {
                        mine &= !(1 << 26);
                    }
                    if moved(0x10, 0x18) {
                        mine &= !(1 << 27);
                    }
                    let key = if mine == b.flags & 0x0F00_0000 {
                        "match".to_string()
                    } else {
                        format!(
                            "differ curves {} game 0x{:x} base 0x{:x}",
                            b.curves.len(),
                            (b.flags >> 24) & 0xF,
                            mine >> 24
                        )
                    };
                    *transform_bits.entry(key).or_default() += 1;
                }
                if u64_at(&data, b.at + 0x18) != 0 || u64_at(&data, b.at + 0x20) != 0 {
                    words_18_20_set += 1;
                }
                if b.curves.is_empty() && u64_at(&data, b.at + 0x08) != 0 {
                    curveless_with_pointer += 1;
                }
                let listed: Vec<String> = (0..7)
                    .filter(|w| rel.contains(&(b.at + w * 8)))
                    .map(|w| format!("0x{:02x}", w * 8))
                    .collect();
                *words.entry(listed.join(" ")).or_default() += 1;
            }
        }
    }
    println!(
        "{} animation files, {records} bone tracks: names in the file {in_file} ({name_in_string_table} in its string table), external IDs {external}",
        files.len()
    );
    println!("words of a bone track the table lists: {words:?}");
    println!("animations whose bind index array the table lists: {bind_listed}");
    println!("a track's other flag bits: {other_flags:?}");
    let mut tb: Vec<_> = transform_bits.iter().collect();
    tb.sort_by(|a, b| b.1.cmp(a.1));
    println!(
        "bits 24 to 27 against the base values: {:?}",
        &tb[..tb.len().min(12)]
    );
    println!(
        "tracks with words 0x18/0x20 set: {words_18_20_set}; tracks without curves but a curve pointer: {curveless_with_pointer}"
    );

    let cache = args.get(1).map(Path::new);
    let links = actor_links::links(Path::new(root), cache).expect("actor links");
    let mut models_missing = 0usize;
    let mut models_checked = 0usize;
    let mut examples = Vec::new();
    let mut sharing: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (model_path, l) in &links.models {
        if l.anims.is_empty() {
            continue;
        }
        for a in &l.anims {
            sharing
                .entry(a.file.clone())
                .or_default()
                .push(model_path.clone());
        }
        let Ok(data) = romfs.read(&mut d, model_path) else {
            continue;
        };
        let Ok(b) = Bfres::parse(&data, Some(&strings)) else {
            continue;
        };
        let Some(m) = b.models.first() else { continue };
        let have: HashSet<&str> = m.skeleton.bones.iter().map(|x| x.name.as_str()).collect();
        models_checked += 1;
        let mut missing = BTreeSet::new();
        for a in &l.anims {
            if let Some(names) = anim_bones.get(&a.file) {
                for n in names {
                    if !have.contains(n.as_str()) {
                        missing.insert(n.clone());
                    }
                }
            }
        }
        if !missing.is_empty() {
            models_missing += 1;
            if examples.len() < 10 {
                let first: Vec<&String> = missing.iter().take(4).collect();
                examples.push(format!(
                    "{model_path}: {} names it lacks, e.g. {first:?}",
                    missing.len()
                ));
            }
        }
    }
    println!(
        "{models_checked} models with animation files: {models_missing} given animations naming bones they lack"
    );
    for e in &examples {
        println!("    {e}");
    }
    for (file, models) in &sharing {
        if file.contains("Player_") {
            println!("{file}: {models:?}");
        }
    }
}
