//! `bfres_bones::rest_world` and `inverse_rows` against the game: for every smooth bone of every model, its inverse
//! bind matrix worked out from the bones' rest transforms against the one the file keeps (the worst difference).
//!
//!     cargo run --release -p modstudio-formats --example inverse_check -- <romfs> [name filter]

use std::path::Path;

use modstudio_formats::bfres::{Bfres, ExternalStrings};
use modstudio_formats::bfres_bones::{inverse_rows, rest_world};
use modstudio_formats::romfs::Romfs;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(root) = args.first() else {
        eprintln!("usage: inverse_check <romfs> [name filter]");
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
    let (mut checked, mut close) = (0usize, 0usize);
    let mut worst = (0f32, String::new());
    for f in &files {
        let Ok(data) = romfs.read(&mut d, format!("Model/{f}")) else {
            continue;
        };
        let Ok(b) = Bfres::parse(&data, Some(&strings)) else {
            continue;
        };
        for m in &b.models {
            let s = &m.skeleton;
            let world = rest_world(&s.bones, s.euler_rotations());
            for (i, bone) in s.bones.iter().enumerate() {
                let Some(sm) = bone.smooth_matrix else {
                    continue;
                };
                let Some(file) = s.inverse_model_matrices.get(sm) else {
                    continue;
                };
                let Some(mine) = inverse_rows(&world[i]) else {
                    continue;
                };
                checked += 1;
                // relative to the matrix's size (translations in metres)
                let diff = file
                    .iter()
                    .zip(&mine)
                    .map(|(a, b)| (a - b).abs())
                    .fold(0f32, f32::max);
                let scale = file.iter().map(|v| v.abs()).fold(1f32, f32::max);
                let rel = diff / scale;
                if rel < 1e-4 {
                    close += 1;
                }
                if rel > worst.0 {
                    worst = (rel, format!("{f} {}", bone.name));
                }
            }
        }
    }
    println!(
        "{checked} smooth bones: {close} within 1e-4 of the file's inverse bind matrix; worst {:.2e} ({})",
        worst.0, worst.1
    );
}
