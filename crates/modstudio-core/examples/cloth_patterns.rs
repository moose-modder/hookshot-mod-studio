//! How the game's cloth files are put together, for making new parts: per file, its cloth datas (parts), each by its
//! operators' classes in order and its states, counted across all files; and the smallest part of each file (by
//! particles) with its counts.
//!
//!     cargo run --release -p modstudio-core --example cloth_patterns -- <romfs>

use std::collections::BTreeMap;
use std::path::Path;

use havok_formats::Tagfile;
use havok_formats::cloth::{self, OperatorKind};
use modstudio_formats::phive::Phive;
use modstudio_formats::romfs::Romfs;

fn class(k: &OperatorKind) -> String {
    let s = format!("{k:?}");
    s.split([' ', '{', '(']).next().unwrap_or("?").to_owned()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(root) = args.first() else {
        eprintln!("usage: cloth_patterns <romfs>");
        std::process::exit(2);
    };
    let romfs = Romfs::open(root).expect("romfs");
    let mut d = romfs.decoder().unwrap();
    let mut files: Vec<String> = std::fs::read_dir(Path::new(root).join("Phive/Cloth"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    files.sort();
    let mut patterns: BTreeMap<String, usize> = BTreeMap::new();
    let mut parts = 0usize;
    let mut one_sim = 0usize;
    for f in &files {
        let Ok(bytes) = romfs.read(&mut d, format!("Phive/Cloth/{f}")) else {
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
        let mut smallest: Option<(usize, String)> = None;
        for c in &physics.cloths {
            parts += 1;
            if c.sim_cloths.len() == 1 {
                one_sim += 1;
            }
            let ops: Vec<String> = c.operators.iter().map(|o| class(&o.kind)).collect();
            let states: Vec<String> = c
                .states
                .iter()
                .map(|s| format!("{}{:?}", s.name, s.operators))
                .collect();
            *patterns
                .entry(format!(
                    "{} sims; ops {}; states {}",
                    c.sim_cloths.len(),
                    ops.join(","),
                    states.join(" ")
                ))
                .or_default() += 1;
            let n: usize = c.sim_cloths.iter().map(|s| s.particles.len()).sum();
            if smallest.as_ref().is_none_or(|(m, _)| n < *m) {
                smallest = Some((n, c.name.clone()));
            }
        }
        if let Some((n, name)) = smallest
            && args.iter().any(|a| a == "--smallest")
        {
            println!(
                "{f}: {} parts; smallest {name} ({n} particles)",
                physics.cloths.len()
            );
        }
    }
    println!(
        "{} files, {parts} parts ({one_sim} with one sim cloth)",
        files.len()
    );
    let mut p: Vec<_> = patterns.into_iter().collect();
    p.sort_by_key(|x| std::cmp::Reverse(x.1));
    for (k, n) in p.iter().take(12) {
        println!("{n:6}  {k}");
    }
}
