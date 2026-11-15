//! Shapes stored as meshlets (`shape_flags::MESHLET`), which the reader doesn't read: which model files have them
//! (by the first part of their names, and the others by name), and for the first few, the shape record's words and
//! where its pointers lead (the relocation table's), its vertex buffer record and its meshes, to find where the
//! geometry is.
//!
//!     cargo run --release -p modstudio-formats --example meshlet_probe -- <romfs> [name filter] [--dump N]

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use modstudio_formats::bfres::{Bfres, ExternalStrings};
use modstudio_formats::romfs::Romfs;

fn u16_at(d: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(d[at..at + 2].try_into().unwrap())
}
fn u32_at(d: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(d[at..at + 4].try_into().unwrap())
}
fn u64_at(d: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(d[at..at + 8].try_into().unwrap())
}

/// The relocated pointer fields of a file.
fn relocated(d: &[u8]) -> BTreeSet<usize> {
    let rlt = u32_at(d, 0x18) as usize;
    let count = u32_at(d, rlt + 8) as usize;
    let total: usize = (0..count)
        .map(|i| u32_at(d, rlt + 16 + i * 24 + 20) as usize)
        .sum();
    let entries = rlt + 16 + count * 24;
    let mut out = BTreeSet::new();
    for i in 0..total {
        let e = entries + i * 8;
        let pos = u32_at(d, e) as usize;
        let structs = u16_at(d, e + 4) as usize;
        let ptrs = d[e + 6] as usize;
        let pad = d[e + 7] as usize;
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
        eprintln!("usage: meshlet_probe <romfs> [name filter] [--dump N]");
        std::process::exit(2);
    };
    let filter = args
        .get(1)
        .filter(|a| !a.starts_with("--"))
        .cloned()
        .unwrap_or_default();
    let dump: usize = args
        .iter()
        .position(|a| a == "--dump")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
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
    let mut by_prefix: BTreeMap<String, usize> = BTreeMap::new();
    let mut others = Vec::new();
    let mut mixed = 0;
    let mut dumped = 0;
    for f in &files {
        let Ok(data) = romfs.read(&mut d, format!("Model/{f}")) else {
            continue;
        };
        let Ok(b) = Bfres::parse(&data, Some(&strings)) else {
            continue;
        };
        let Some(m) = b.models.first() else { continue };
        let meshlets: Vec<_> = m.shapes.iter().filter(|s| s.is_meshlet()).collect();
        if meshlets.is_empty() {
            continue;
        }
        if meshlets.len() < m.shapes.len() {
            mixed += 1;
        }
        let prefix = f.split('_').next().unwrap_or("").to_owned();
        *by_prefix.entry(prefix.clone()).or_default() += 1;
        if prefix != "Land" && prefix != "Obj" {
            others.push(format!(
                "{f} ({} of {} shapes)",
                meshlets.len(),
                m.shapes.len()
            ));
        }
        if dumped < dump {
            dumped += 1;
            let rel = relocated(&data);
            println!(
                "== {f}: {} bytes, GPU data from 0x{:x}",
                data.len(),
                b.buffer_offset
            );
            for s in meshlets.iter().take(2) {
                println!("  shape {} at 0x{:x}, flags 0x{:x}", s.name, s.at, s.flags);
                for w in 0..0x80 / 8 {
                    let at = s.at + w * 8;
                    let v = u64_at(&data, at);
                    let mark = if rel.contains(&at) { "pointer" } else { "" };
                    print!("    +0x{:02x}: {v:#018x} {mark}", w * 8);
                    if rel.contains(&at) && v != 0 && (v as usize) < data.len() {
                        let t = v as usize;
                        let words: Vec<String> = (0..6)
                            .map(|k| format!("{:08x}", u32_at(&data, t + k * 4)))
                            .collect();
                        print!(" -> {}", words.join(" "));
                    }
                    println!();
                }
                let vb = &m.vertex_buffers[s.vertex_buffer];
                println!(
                    "    vertex buffer {}: {} vertices, {} attributes, buffers {:?}",
                    s.vertex_buffer,
                    vb.vertex_count,
                    vb.attributes.len(),
                    vb.buffers
                        .iter()
                        .map(|x| (x.stride, x.data.len()))
                        .collect::<Vec<_>>()
                );
                println!("    meshes: {}", s.meshes.len());
            }
        }
    }
    println!(
        "{} model files with meshlet shapes ({mixed} with ordinary shapes too): {by_prefix:?}",
        by_prefix.values().sum::<usize>()
    );
    for o in &others {
        println!("    {o}");
    }
}
