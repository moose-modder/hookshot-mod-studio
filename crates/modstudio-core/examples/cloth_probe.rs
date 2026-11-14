//! How far the vendored havok-formats and havok-cloth get with the game's cloth files: each `Phive/Cloth/*.bphcl`
//! is split (Phive header), its tagfile read, its cloth data read, and every cloth stepped for a while with the
//! bones at rest. Files are grouped by where they stop (numbers in messages left out), with the first message of
//! each group in full.
//!
//!     cargo run --release -p modstudio-core --example cloth_probe -- <romfs> [<part of a name>] [--steps N]

use std::collections::BTreeMap;
use std::path::Path;

use havok_cloth::instance::ClothInstance;
use havok_formats::{Tagfile, cloth};
use modstudio_formats::phive::Phive;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(romfs) = args.first() else {
        eprintln!("usage: cloth_probe <romfs> [<part of a name>] [--steps N]");
        std::process::exit(2);
    };
    let steps: usize = args
        .iter()
        .position(|a| a == "--steps")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(90);
    let filter = args.get(1).filter(|a| !a.starts_with("--")).cloned();
    let dir = Path::new(romfs).join("Phive").join("Cloth");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|x| x == "bphcl")
                && filter
                    .as_ref()
                    .is_none_or(|f| p.to_string_lossy().contains(f.as_str()))
        })
        .collect();
    files.sort();
    let mut groups: BTreeMap<String, (String, Vec<String>)> = BTreeMap::new();
    for path in &files {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let name = name.trim_end_matches(".Nin_Ounce_NVN2.bphcl").to_owned();
        let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let outcome = probe(&bytes, steps);
        let entry = groups
            .entry(without_numbers(&outcome))
            .or_insert_with(|| (outcome.clone(), Vec::new()));
        entry.1.push(name);
    }
    let mut sorted: Vec<_> = groups.into_values().collect();
    sorted.sort_by_key(|(_, names)| std::cmp::Reverse(names.len()));
    for (first, names) in sorted {
        println!(
            "{:4}  {first}\n      e.g. {}",
            names.len(),
            names.iter().take(4).cloned().collect::<Vec<_>>().join(", ")
        );
    }
}

/// Where a file stops, or what it ran.
fn probe(bytes: &[u8], steps: usize) -> String {
    let phive = match Phive::parse(bytes) {
        Ok(p) => p,
        Err(e) => return format!("phive: {e}"),
    };
    let tag = match Tagfile::read(phive.tagfile, None) {
        Ok(t) => t,
        Err(e) => return format!("tagfile: {e}"),
    };
    let physics = match cloth::read(&tag) {
        Ok(p) => p,
        Err(e) => return format!("cloth data: {e}"),
    };
    let mut sims = 0;
    let mut particles = 0;
    for data in &physics.cloths {
        let mut instance = match ClothInstance::new(data) {
            Ok(i) => i,
            Err(e) => return format!("instance: {e}"),
        };
        for frame in 0..steps {
            if let Err(e) = instance.step(1.0 / 30.0, &mut []) {
                return format!("step {frame}: {e}");
            }
        }
        for (sim, state) in data.sim_cloths.iter().zip(&instance.sims) {
            if let Some(i) = state.positions.iter().position(|p| !p.is_finite3()) {
                return format!("simulated: {} particle {i} not finite", sim.name);
            }
            sims += 1;
            particles += state.positions.len();
        }
    }
    format!(
        "ran {steps} steps: {} cloth data, {sims} simulated meshes, {particles} particles",
        physics.cloths.len()
    )
}

/// The message with every number (decimal, or hexadecimal after `0x`) as `#`.
fn without_numbers(s: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '0' && chars.get(i + 1) == Some(&'x') {
            i += 2;
            while chars.get(i).is_some_and(char::is_ascii_hexdigit) {
                i += 1;
            }
            out.push('#');
        } else if chars[i].is_ascii_digit() {
            while chars.get(i).is_some_and(char::is_ascii_digit) {
                i += 1;
            }
            out.push('#');
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}
