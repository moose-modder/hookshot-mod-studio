//! How the game's resource size table relates to file sizes: for every file of a folder, its decompressed size
//! and the table's size for it (entries keyed by the CRC-32 of the path without `.zs`). Prints how
//! `table - size` is spread, per file kind, to find the formula the sizes follow (local analysis).
//!
//! `cargo run --release --example restbl_probe -- <romfs> <decompressed table> <folder> [limit]`

use std::collections::{BTreeMap, HashMap};

use modstudio_formats::restbl::crc32;
use modstudio_formats::romfs::Romfs;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [root, table, folder, rest @ ..] = args.as_slice() else {
        eprintln!("usage: restbl_probe <romfs> <table> <folder> [limit]");
        std::process::exit(2);
    };
    let limit: usize = rest
        .first()
        .and_then(|n| n.parse().ok())
        .unwrap_or(usize::MAX);
    let t = std::fs::read(table).unwrap();
    let u32_at = |at: usize| u32::from_le_bytes(t[at..at + 4].try_into().unwrap());
    let (strsize, ncrc, nname) = (
        u32_at(10) as usize,
        u32_at(14) as usize,
        u32_at(18) as usize,
    );
    let mut by_crc = HashMap::new();
    for i in 0..ncrc {
        by_crc.insert(u32_at(22 + i * 8), u32_at(26 + i * 8));
    }
    let mut by_name = HashMap::new();
    let names_at = 22 + ncrc * 8;
    for i in 0..nname {
        let at = names_at + i * (strsize + 4);
        let raw = &t[at..at + strsize];
        let name =
            String::from_utf8_lossy(&raw[..raw.iter().position(|&b| b == 0).unwrap_or(strsize)])
                .into_owned();
        by_name.insert(name, u32_at(at + strsize));
    }

    let romfs = Romfs::open(root).unwrap();
    let mut d = romfs.decoder().unwrap();
    let mut files: Vec<String> = std::fs::read_dir(std::path::Path::new(root).join(folder))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|f| f.ends_with(".zs"))
        .collect();
    files.sort();
    // kind -> (diff -> count), and a few examples
    let mut diffs: BTreeMap<String, BTreeMap<i64, usize>> = BTreeMap::new();
    let mut examples: Vec<String> = Vec::new();
    let mut missing = 0;
    for f in files.iter().take(limit) {
        let path = format!("{folder}/{}", f.trim_end_matches(".zs"));
        let Some(size) = by_name
            .get(&path)
            .copied()
            .or_else(|| by_crc.get(&crc32(path.as_bytes())).copied())
        else {
            missing += 1;
            continue;
        };
        let Ok(data) = romfs.read(&mut d, format!("{folder}/{f}")) else {
            continue;
        };
        let kind = path.rsplit('.').next().unwrap_or("").to_owned();
        let diff = size as i64 - data.len() as i64;
        *diffs
            .entry(kind.clone())
            .or_default()
            .entry(diff)
            .or_default() += 1;
        if examples.len() < 12 {
            examples.push(format!(
                "{path}: file {} table {} diff {diff}",
                data.len(),
                size
            ));
        }
    }
    for e in &examples {
        println!("{e}");
    }
    println!("not in the table: {missing}");
    for (kind, ds) in &diffs {
        let n: usize = ds.values().sum();
        let mut top: Vec<(&i64, &usize)> = ds.iter().collect();
        top.sort_by_key(|(_, c)| std::cmp::Reverse(**c));
        println!(
            "{kind}: {n} files, {} distinct differences; most common {:?}; range {}..{}",
            ds.len(),
            &top[..top.len().min(8)],
            ds.keys().next().unwrap(),
            ds.keys().last().unwrap()
        );
    }
}
