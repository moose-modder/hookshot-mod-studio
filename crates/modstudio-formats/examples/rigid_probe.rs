//! Rigid matrices in the game's models: how many models have them, which bones they are, and which shapes are
//! skinned rigidly (one influence, vertices indexing the matrix list past its smooth part), in all models and in
//! the ones whose name has the filter.
//!
//!     cargo run --release -p modstudio-formats --example rigid_probe -- <romfs> [name filter]

use std::path::Path;

use modstudio_formats::bfres::{Bfres, ExternalStrings};
use modstudio_formats::romfs::Romfs;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(root) = args.first() else {
        eprintln!("usage: rigid_probe <romfs> [name filter]");
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
        .filter(|n| n.ends_with(".bfres.zs") && !n.contains(".anim.") && !n.contains("Skeleton"))
        .collect();
    files.sort();
    let (mut models, mut with_rigid, mut with_rigid_shapes) = (0, 0, 0);
    let mut largest: Vec<(usize, usize, String)> = Vec::new();
    for f in &files {
        let Ok(data) = romfs.read(&mut d, format!("Model/{f}")) else {
            continue;
        };
        let Ok(b) = Bfres::parse(&data, Some(&strings)) else {
            continue;
        };
        for m in &b.models {
            models += 1;
            let s = &m.skeleton;
            largest.push((s.smooth_count + s.rigid_count, s.bones.len(), f.clone()));
            let rigid_shapes: Vec<&str> = m
                .shapes
                .iter()
                .filter(|x| x.skin_count == 1)
                .map(|x| x.name.as_str())
                .collect();
            if s.rigid_count > 0 {
                with_rigid += 1;
            }
            if !rigid_shapes.is_empty() {
                with_rigid_shapes += 1;
            }
            if !filter.is_empty() && f.contains(filter.as_str()) {
                let rigid_bones: Vec<&str> = s.matrix_to_bone[s.smooth_count..]
                    .iter()
                    .map(|&i| s.bones[i as usize].name.as_str())
                    .collect();
                println!(
                    "{f}: {} smooth, {} rigid matrices (bones {:?}); rigid shapes {:?}",
                    s.smooth_count, s.rigid_count, rigid_bones, rigid_shapes
                );
            }
        }
    }
    println!(
        "{models} models: {with_rigid} with rigid matrices, {with_rigid_shapes} with rigidly skinned shapes"
    );
    largest.sort();
    for (matrices, bones, f) in largest.iter().rev().take(5) {
        println!("    most matrices: {matrices} ({bones} bones) in {f}");
    }
    let mut by_bones = largest.clone();
    by_bones.sort_by_key(|x| x.1);
    for (matrices, bones, f) in by_bones.iter().rev().take(3) {
        println!("    most bones: {bones} ({matrices} matrices) in {f}");
    }
}
