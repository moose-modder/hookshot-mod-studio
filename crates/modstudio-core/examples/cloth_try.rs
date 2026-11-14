//! A cloth file with a scene's edits (`cloth_scene_dump`'s JSON, edited), against the file as it is: both settled at
//! rest (`LiveCloth`), how far each bone the cloth moves ends up from where the game's own file has it, and how much
//! the edited cloth still moves in the second after (it should keep still).
//!
//!     cargo run --release -p modstudio-core --example cloth_try -- <file.bphcl> <edited.json>

use modstudio_core::cloth::LiveCloth;
use modstudio_core::cloth_edit::{self, ClothScene};
use modstudio_core::math;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [file, json] = args.as_slice() else {
        eprintln!("usage: cloth_try <file.bphcl> <edited.json>");
        std::process::exit(2);
    };
    let bytes = std::fs::read(file).unwrap_or_else(|e| panic!("{file}: {e}"));
    let text = std::fs::read_to_string(json).unwrap_or_else(|e| panic!("{json}: {e}"));
    let edited: ClothScene = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{json}: {e}"));
    let applied = cloth_edit::apply(&bytes, &edited).unwrap_or_else(|e| panic!("{e}"));
    for c in &applied.changes {
        println!("change: {c}");
    }
    for w in &applied.warnings {
        println!("note: {w}");
    }
    let Some(written) = applied.bytes else {
        println!("nothing changed");
        return;
    };
    let game = LiveCloth::new(&bytes, file).unwrap_or_else(|e| panic!("{e}"));
    let mut edit = LiveCloth::new(&written, file).unwrap_or_else(|e| panic!("{e}"));
    let dist = |a: &math::Mat4, b: &math::Mat4| {
        ((a[12] - b[12]).powi(2) + (a[13] - b[13]).powi(2) + (a[14] - b[14]).powi(2)).sqrt()
    };
    let before = game.places();
    let after = edit.places();
    let mut rows: Vec<(f32, &str)> = edit
        .bones
        .iter()
        .zip(&after)
        .filter_map(|(b, a)| {
            let i = game.bones.iter().position(|g| g.name == b.name)?;
            Some((dist(a, &before[i]), b.name.as_str()))
        })
        .collect();
    rows.sort_by(|a, b| b.0.total_cmp(&a.0));
    println!("settled at rest, the bones furthest from the game's:");
    for (d, n) in rows.iter().take(8) {
        println!("  {n:32} {:.1} cm", d * 100.0);
    }
    // then a few seconds more at rest, each second's largest movement: the edited cloth's and the game's
    let mut game = game;
    let mut seconds = Vec::new();
    for _ in 0..4 {
        let (e0, g0) = (edit.places(), game.places());
        for _ in 0..30 {
            edit.step(&math::IDENTITY, 1.0 / 30.0)
                .unwrap_or_else(|e| panic!("{e}"));
            game.step(&math::IDENTITY, 1.0 / 30.0)
                .unwrap_or_else(|e| panic!("{e}"));
        }
        let most = |now: Vec<math::Mat4>, then: &[math::Mat4]| {
            now.iter()
                .zip(then)
                .map(|(a, b)| dist(a, b))
                .fold(0f32, f32::max)
                * 100.0
        };
        seconds.push(format!(
            "{:.1}/{:.1}",
            most(edit.places(), &e0),
            most(game.places(), &g0)
        ));
    }
    println!(
        "then, each second at rest, the most a bone moves (cm, edited/game's): {}",
        seconds.join(" ")
    );
}
