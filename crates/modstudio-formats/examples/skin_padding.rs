//! What the game stores in the bone index and weight components past a shape's skin count (e.g. the 4th
//! component of `_i0`/`_w0` in a shape with skin count 3), over every model whose name starts with a prefix
//! (local analysis for the writer).
//!
//! `cargo run --release --example skin_padding -- <romfs> [prefix]`

use std::collections::BTreeMap;

use modstudio_formats::bfres::{Bfres, ExternalStrings};
use modstudio_formats::romfs::Romfs;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(root) = args.first() else {
        eprintln!("usage: skin_padding <romfs> [prefix]");
        std::process::exit(2);
    };
    let prefix = args.get(1).map_or("", String::as_str);
    let romfs = Romfs::open(root).expect("romfs");
    let mut d = romfs.decoder().unwrap();
    let strings = ExternalStrings::parse(
        &romfs
            .read(&mut d, "Shader/ExternalBinaryString.bfres.zs")
            .unwrap(),
    )
    .unwrap();
    // (skin count, component) -> (index, weight step) -> vertices
    let mut seen: BTreeMap<(u8, usize), BTreeMap<(i64, i64), u64>> = BTreeMap::new();
    let mut shapes: BTreeMap<u8, u64> = BTreeMap::new();
    let mut entries: Vec<_> = std::fs::read_dir(romfs.root().join("Model"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().unwrap().to_string_lossy();
            n.starts_with(prefix) && n.ends_with(".bfres.zs")
        })
        .collect();
    entries.sort();
    for path in entries {
        let Ok(data) = romfs.read(&mut d, &path) else {
            continue;
        };
        let Ok(bfres) = Bfres::parse(&data, Some(&strings)) else {
            continue;
        };
        for m in &bfres.models {
            for s in &m.shapes {
                if s.is_meshlet() || s.skin_count == 0 {
                    continue;
                }
                let vb = &m.vertex_buffers[s.vertex_buffer];
                for (set, (iname, wname)) in
                    [("_i0", "_w0"), ("_i1", "_w1")].into_iter().enumerate()
                {
                    let (Some(ia), Some(wa)) = (vb.attribute(iname), vb.attribute(wname)) else {
                        continue;
                    };
                    let comps = ia.format.components().unwrap_or(0);
                    let used = (s.skin_count as usize).saturating_sub(set * 4).min(comps);
                    if used >= comps {
                        continue;
                    }
                    *shapes.entry(s.skin_count).or_default() += 1;
                    let (Ok(i), Ok(w)) = (vb.read(ia), vb.read(wa)) else {
                        continue;
                    };
                    for (iv, wv) in i.iter().zip(&w) {
                        for c in used..comps {
                            *seen
                                .entry((s.skin_count, set * 4 + c))
                                .or_default()
                                .entry((iv[c] as i64, (wv[c] * 255.0).round() as i64))
                                .or_default() += 1;
                        }
                    }
                }
            }
        }
    }
    println!("shapes with padding, by skin count: {shapes:?}");
    for ((skin, c), values) in seen {
        let mut v: Vec<_> = values.into_iter().collect();
        v.sort_by_key(|x| std::cmp::Reverse(x.1));
        println!("skin {skin} component {c}: {:?}", &v[..v.len().min(8)]);
    }
}
