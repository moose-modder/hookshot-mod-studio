//! For every character material: how grey its colour texture is (or that the game lacks it), its render mode,
//! and its `ns_param_color0`. Local analysis of which materials the game tints with a colour parameter.
//!
//! `cargo run --release -p modstudio-core --example colour_survey -- <romfs>`

use std::path::Path;

use modstudio_core::look::{Look, look};
use modstudio_core::model::{Loaded, TEXTURE_DIR};
use modstudio_formats::bntx::Bntx;
use modstudio_formats::romfs::{self, Romfs};

fn main() {
    let root = std::env::args().nth(1).expect("romfs");
    let romfs = Romfs::open(&root).expect("romfs");
    let mut d = romfs.decoder().unwrap();
    let mut files: Vec<String> = std::fs::read_dir(Path::new(&root).join("Model"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|f| {
            f.ends_with(".bfres.zs")
                && !f.contains(".anim.")
                && !f.contains("_Animation.")
                && ["Npc_", "Enemy_", "Player_", "Animal_"]
                    .iter()
                    .any(|p| f.starts_with(p))
        })
        .collect();
    files.sort();
    for f in files {
        let Ok(loaded) = Loaded::read(&romfs, &mut d, Path::new(&format!("Model/{f}"))) else {
            continue;
        };
        let Ok(bfres) = loaded.bfres() else { continue };
        for m in &bfres.models[0].materials {
            let Look::Color { texture } = look(m) else {
                continue;
            };
            let color0 = m
                .shader
                .as_ref()
                .and_then(|s| s.param("ns_param_color0"))
                .map(|p| p.floats())
                .unwrap_or_default();
            let mode = m
                .shader
                .as_ref()
                .and_then(|s| s.render_string("gsys_render_state_mode"))
                .unwrap_or("-")
                .to_owned();
            let path = romfs.resolve(Path::new(TEXTURE_DIR).join(format!("{texture}.bntx.zs")));
            let grey = if !path.exists() {
                "missing".to_owned()
            } else {
                let data = romfs::read_maybe_zs(&mut d, &path).unwrap();
                let bntx = Bntx::parse(&data).unwrap();
                let t = &bntx.textures[0];
                let level = t.mip_count.saturating_sub(5);
                match t.decode(level, 0) {
                    Ok(mut img) => {
                        t.apply_channel_sources(&mut img);
                        let px = img.rgba.as_chunks::<4>().0;
                        let sat: f32 = px
                            .iter()
                            .map(|p| {
                                (*p[..3].iter().max().unwrap() - *p[..3].iter().min().unwrap())
                                    as f32
                            })
                            .sum::<f32>()
                            / px.len() as f32;
                        format!("sat {sat:5.1}")
                    }
                    Err(_) => "undecodable".into(),
                }
            };
            println!(
                "{:40} {:22} {:11} {:12} color0 {:?}",
                f.split('.').nth(1).unwrap_or(&f),
                m.name,
                mode,
                grey,
                color0
                    .iter()
                    .map(|v| (v * 100.0).round() / 100.0)
                    .collect::<Vec<_>>()
            );
        }
    }
}
