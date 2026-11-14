//! The formats skin index attributes (`_i0`, `_i1`) have in the game's models: per format, how many attributes, the
//! largest matrix index stored in one, and the buffers they share (with which attributes, at what stride), in all
//! models or those matching the filter.
//!
//!     cargo run --release -p modstudio-formats --example index_formats -- <romfs> [name filter]

use std::collections::BTreeMap;
use std::path::Path;

use modstudio_formats::bfres::{Bfres, ExternalStrings};
use modstudio_formats::romfs::Romfs;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(root) = args.first() else {
        eprintln!("usage: index_formats <romfs> [name filter]");
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
    // format -> (attributes, largest index, layouts of their buffer)
    let mut seen: BTreeMap<String, (usize, f32, BTreeMap<String, usize>)> = BTreeMap::new();
    let mut models_over_255 = 0;
    for f in &files {
        let Ok(data) = romfs.read(&mut d, format!("Model/{f}")) else {
            continue;
        };
        let Ok(b) = Bfres::parse(&data, Some(&strings)) else {
            continue;
        };
        for m in &b.models {
            if m.skeleton.smooth_count + m.skeleton.rigid_count > 256 {
                models_over_255 += 1;
            }
            for vb in &m.vertex_buffers {
                for a in vb
                    .attributes
                    .iter()
                    .filter(|a| a.name == "_i0" || a.name == "_i1")
                {
                    let largest = vb.read(a).ok().map_or(0.0, |v| {
                        v.iter()
                            .flat_map(|x| x.iter().copied())
                            .fold(0f32, f32::max)
                    });
                    let layout = vb
                        .attributes
                        .iter()
                        .filter(|x| x.buffer == a.buffer)
                        .map(|x| format!("{}@{}:{}", x.name, x.offset, x.format.name()))
                        .collect::<Vec<_>>()
                        .join(" ");
                    let layout =
                        format!("stride {}: {layout}", vb.buffers[a.buffer as usize].stride);
                    let e = seen
                        .entry(format!("{} 0x{:04x}", a.format.name(), a.format.0))
                        .or_default();
                    e.0 += 1;
                    e.1 = e.1.max(largest);
                    *e.2.entry(layout).or_default() += 1;
                }
            }
        }
    }
    println!(
        "{} files; {models_over_255} models with more than 256 matrices",
        files.len()
    );
    for (format, (n, largest, layouts)) in &seen {
        println!("{format}: {n} attributes, largest index {largest}");
        let mut l: Vec<_> = layouts.iter().collect();
        l.sort_by(|a, b| b.1.cmp(a.1));
        for (layout, k) in l.iter().take(4) {
            println!("    {k:6} {layout}");
        }
    }
}
