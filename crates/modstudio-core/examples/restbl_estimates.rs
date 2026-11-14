//! Checks `sizetable::estimate` against the game's own table: for every file of a romfs folder (and below)
//! whose name contains a filter, the estimate for it as if a mod added it, and how many of the game's entries
//! are larger (local analysis; an estimate should never be below the game's own value).
//!
//! `cargo run --release --example restbl_estimates -- <romfs> <folder> [name filter]`

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use modstudio_core::sizetable::{estimate, game_table, kind_of};
use modstudio_formats::romfs::Romfs;

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [root, folder, rest @ ..] = args.as_slice() else {
        eprintln!("usage: restbl_estimates <romfs> <folder> [name filter]");
        std::process::exit(2);
    };
    let filter = rest.first().map(String::as_str).unwrap_or("");
    let romfs = Romfs::open(root).unwrap();
    let (_, table) = game_table(&romfs).unwrap();
    let mut d = romfs.decoder().unwrap();
    let mut files = Vec::new();
    walk(&Path::new(root).join(folder), &mut files);
    files.retain(|p| p.file_name().unwrap().to_string_lossy().contains(filter));
    files.sort();

    // kind -> (files, not in the table, entries above the estimate, the most above, estimate / entry range)
    #[derive(Default)]
    struct Kind {
        n: usize,
        missing: usize,
        above: usize,
        worst: Option<(String, u64, u32, u64)>,
        spare: (f64, f64),
        /// (size, entry) of each file, for the fit below
        sizes: Vec<(u64, u32)>,
    }
    let mut kinds: BTreeMap<String, Kind> = BTreeMap::new();
    for p in &files {
        let rel = p
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let key = rel.trim_end_matches(".zs").to_owned();
        let k = kinds
            .entry(kind_of(&key).to_owned())
            .or_insert_with(|| Kind {
                spare: (f64::MAX, 0.0),
                ..Kind::default()
            });
        k.n += 1;
        let Some(entry) = table.get(&key) else {
            k.missing += 1;
            continue;
        };
        let Ok(data) = romfs.read(&mut d, p) else {
            continue;
        };
        k.sizes.push((data.len() as u64, entry));
        let est = estimate(&key, &data);
        let r = est as f64 / f64::from(entry);
        k.spare = (k.spare.0.min(r), k.spare.1.max(r));
        if u64::from(entry) > est {
            k.above += 1;
            if k.worst
                .as_ref()
                .is_none_or(|w| u64::from(entry) - est > u64::from(w.2) - w.3)
            {
                k.worst = Some((key, data.len() as u64, entry, est));
            }
        }
    }
    println!("{} files in {folder} matching {filter:?}", files.len());
    for (kind, k) in &kinds {
        println!(
            "{kind:<10} {:>6} files, {:>5} not in the table, {:>5} entries above the estimate; estimate / entry {:.3}..{:.3}{}",
            k.n,
            k.missing,
            k.above,
            k.spare.0,
            k.spare.1,
            k.worst
                .as_ref()
                .map(|(p, s, e, est)| format!(" (most: {p} size {s} entry {e} estimate {est})"))
                .unwrap_or_default()
        );
        // a + b * size covering every entry: the least b for some a
        let fits: Vec<String> = [0u64, 1024, 2048, 4096, 8192, 16384, 65536]
            .iter()
            .map(|&a| {
                let b = k
                    .sizes
                    .iter()
                    .filter(|(s, _)| *s > 0)
                    .map(|&(s, e)| (f64::from(e) - a as f64) / s as f64)
                    .fold(0.0f64, f64::max);
                format!("{a}+{b:.4}x")
            })
            .collect();
        println!("           covering all: {}", fits.join(", "));
    }
}
