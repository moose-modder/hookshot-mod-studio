//! A model's material shaders and parameter values (local analysis of how materials are drawn, e.g. where the
//! eye shader puts the iris).
//!
//! `cargo run --release --example material_params -- <romfs> <model.bfres.zs> [material name part]`
//! `cargo run --release --example material_params -- <romfs> --all <material name part> <param>,<param>,...`:
//! the named parameters of that material in every model (one line each).

use modstudio_formats::bfres::{Bfres, ExternalStrings};
use modstudio_formats::romfs::Romfs;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [root, all, filter, names] = args.as_slice()
        && all == "--all"
    {
        return scan(root, filter, names);
    }
    let (root, model, filter) = match args.as_slice() {
        [r, m] => (r, m, ""),
        [r, m, f] => (r, m, f.as_str()),
        _ => {
            eprintln!("usage: material_params <romfs> <model> [material]");
            std::process::exit(2);
        }
    };
    let romfs = Romfs::open(root).expect("romfs");
    let mut d = romfs.decoder().unwrap();
    let strings = ExternalStrings::parse(
        &romfs
            .read(&mut d, "Shader/ExternalBinaryString.bfres.zs")
            .unwrap(),
    )
    .unwrap();
    let data = romfs.read(&mut d, model).expect("model");
    let bfres = Bfres::parse(&data, Some(&strings)).expect("bfres");
    for m in &bfres.models[0].materials {
        if !m.name.contains(filter) {
            continue;
        }
        let Some(s) = &m.shader else {
            println!("{}: no shader info", m.name);
            continue;
        };
        println!("{} ({} / {})", m.name, s.archive, s.model);
        for (t, smp) in m.textures.iter().zip(&m.samplers) {
            println!("    texture {smp:6} {t}");
        }
        for i in &s.render_info {
            println!("    render {:33} {:?}", i.name, i.value);
        }
        for p in &s.params {
            println!("    {:40} type {:2} {:?}", p.name, p.kind, p.floats());
        }
    }
}

fn scan(root: &str, filter: &str, names: &str) {
    let romfs = Romfs::open(root).expect("romfs");
    let mut d = romfs.decoder().unwrap();
    let strings = ExternalStrings::parse(
        &romfs
            .read(&mut d, "Shader/ExternalBinaryString.bfres.zs")
            .unwrap(),
    )
    .unwrap();
    let names: Vec<&str> = names.split(',').collect();
    let mut files: Vec<_> = std::fs::read_dir(std::path::Path::new(root).join("Model"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|f| f.ends_with(".bfres.zs") && !f.contains(".anim.") && !f.starts_with("Land_"))
        .collect();
    files.sort();
    for f in files {
        let Ok(data) = romfs.read(&mut d, format!("Model/{f}")) else {
            continue;
        };
        let Ok(bfres) = Bfres::parse(&data, Some(&strings)) else {
            continue;
        };
        for model in &bfres.models {
            for m in model.materials.iter().filter(|m| m.name.contains(filter)) {
                let Some(s) = &m.shader else { continue };
                let values: Vec<String> = names
                    .iter()
                    .map(|n| match s.param(n) {
                        Some(p) => format!("{n}={:?}", p.floats()),
                        None => format!("{n}=-"),
                    })
                    .collect();
                let first = m.textures.first().map(String::as_str).unwrap_or("");
                println!(
                    "{:40} {:18} {} | {}",
                    f.split('.').nth(1).unwrap_or(&f),
                    m.name,
                    values.join(" "),
                    first
                );
            }
        }
    }
}
