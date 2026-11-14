//! How the game's resource size table sizes the files inside its packs (`Pack/**/*.pack.zs`): for each kind of
//! file (its extension), how `table - size` and `table / size` are spread, and how
//! many entries a rule `a + b * size` would leave short (local analysis, for `sizetable.rs`).
//!
//! `cargo run --release --example restbl_packs -- <romfs> [a b]`

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use modstudio_formats::restbl::Restbl;
use modstudio_formats::romfs::Romfs;
use modstudio_formats::sarc::Sarc;

fn packs(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            packs(&p, out);
        } else if p.to_string_lossy().ends_with(".pack.zs") {
            out.push(p);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [root, rest @ ..] = args.as_slice() else {
        eprintln!("usage: restbl_packs <romfs> [a b]");
        std::process::exit(2);
    };
    let (a, b): (i64, i64) = match rest {
        [a, b] => (a.parse().unwrap(), b.parse().unwrap()),
        _ => (5000, 8),
    };
    let romfs = Romfs::open(root).unwrap();
    let mut d = romfs.decoder().unwrap();
    let res = Path::new(root).join("System").join("Resource");
    let table_path = std::fs::read_dir(&res)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.to_string_lossy().contains(".rsizetable"))
        .unwrap();
    let table = Restbl::parse(&romfs.read(&mut d, &table_path).unwrap()).unwrap();

    let mut all = Vec::new();
    packs(&Path::new(root).join("Pack"), &mut all);
    all.sort();
    // inner path -> (size, entry); a path in several packs is counted once (largest size)
    let mut inner: HashMap<String, (u64, Option<u32>)> = HashMap::new();
    let mut unreadable = 0;
    for p in &all {
        let Ok(data) = romfs.read(&mut d, p) else {
            unreadable += 1;
            continue;
        };
        let Ok(sarc) = Sarc::parse(&data) else {
            unreadable += 1;
            continue;
        };
        for f in sarc.files() {
            let Some(name) = f.name else { continue };
            let key = name.trim_end_matches(".zs").to_owned();
            let size = f.data.len() as u64;
            let e = inner.entry(key.clone()).or_insert((0, table.get(&key)));
            e.0 = e.0.max(size);
        }
    }
    println!(
        "{} packs ({unreadable} unreadable), {} distinct inner files",
        all.len(),
        inner.len()
    );

    #[derive(Default)]
    struct Kind {
        n: usize,
        missing: usize,
        short: usize,
        diff: (i64, i64),
        ratio: (f64, f64),
        worst: Option<(String, u64, u32)>,
        worst_over: i64,
    }
    let mut kinds: BTreeMap<String, Kind> = BTreeMap::new();
    for (path, (size, entry)) in &inner {
        let file = path.rsplit('/').next().unwrap();
        let kind = file
            .rsplit_once('.')
            .map(|(_, k)| k)
            .unwrap_or("")
            .to_owned();
        let k = kinds.entry(kind).or_insert_with(|| Kind {
            diff: (i64::MAX, i64::MIN),
            ratio: (f64::MAX, 0.0),
            ..Kind::default()
        });
        k.n += 1;
        let Some(entry) = *entry else {
            k.missing += 1;
            continue;
        };
        let diff = entry as i64 - *size as i64;
        k.diff = (k.diff.0.min(diff), k.diff.1.max(diff));
        if *size > 0 {
            let r = entry as f64 / *size as f64;
            k.ratio = (k.ratio.0.min(r), k.ratio.1.max(r));
        }
        let rule = a + b * *size as i64;
        if (entry as i64) > rule {
            k.short += 1;
            if entry as i64 - rule > k.worst_over {
                k.worst_over = entry as i64 - rule;
                k.worst = Some((path.clone(), *size, entry));
            }
        }
    }
    println!("rule: {a} + {b} * size");
    for (kind, k) in &kinds {
        println!(
            "{kind:<40} {:>6} files, {:>5} not in the table; table-size {}..{}; table/size {:.2}..{:.2}; {} above the rule{}",
            k.n,
            k.missing,
            k.diff.0,
            k.diff.1,
            k.ratio.0,
            k.ratio.1,
            k.short,
            k.worst
                .as_ref()
                .map(|(p, s, e)| format!(" (most: {p} size {s} table {e}, {} over)", k.worst_over))
                .unwrap_or_default()
        );
    }
}
