//! How the bones a cloth file moves swing (`LiveCloth`), for checking a built mod's cloth: for each bone the
//! simulation sets whose name has the filter in it, how far it settles from its rest place, and in the test motion
//! (sized to the file's skeletons) how far it gets from where its rest place would carry it, at most and at the end.
//!
//!     cargo run --release -p modstudio-core --example cloth_swing -- <file.bphcl> [part of a bone's name]

use modstudio_core::cloth::{LiveCloth, Motion};
use modstudio_core::math;

fn distance(a: &math::Mat4, b: &math::Mat4) -> f32 {
    ((a[12] - b[12]).powi(2) + (a[13] - b[13]).powi(2) + (a[14] - b[14]).powi(2)).sqrt()
}

/// The angle between two places' rotations, in degrees (their axes normalised first).
fn turn(a: &math::Mat4, b: &math::Mat4) -> f32 {
    let qa = math::quat_from_rotation(a);
    let qb = math::quat_from_rotation(b);
    let dot = (qa[0] * qb[0] + qa[1] * qb[1] + qa[2] * qb[2] + qa[3] * qb[3])
        .abs()
        .min(1.0);
    2.0 * dot.acos().to_degrees()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(file) = args.first() else {
        eprintln!("usage: cloth_swing <file.bphcl> [part of a bone's name]");
        std::process::exit(2);
    };
    let filter = args.get(1).cloned().unwrap_or_default();
    let bytes = std::fs::read(file).expect("the file");
    let mut live = LiveCloth::new(&bytes, file).expect("simulates");
    for w in &live.warnings {
        println!("note: {w}");
    }
    let picked: Vec<usize> = (0..live.bones.len())
        .filter(|&i| live.bones[i].name.contains(filter.as_str()))
        .collect();
    let settled: Vec<f32> = picked
        .iter()
        .map(|&i| distance(&live.places()[i], &live.bones[i].rest))
        .collect();
    let motion = Motion::for_size(live.size);
    let frames = (motion.seconds() * 60.0) as usize;
    let mut most = vec![0f32; picked.len()];
    let mut end = vec![0f32; picked.len()];
    let mut most_turn = vec![0f32; picked.len()];
    let mut end_turn = vec![0f32; picked.len()];
    let settled_turn: Vec<f32> = picked
        .iter()
        .map(|&i| turn(&live.places()[i], &live.bones[i].rest))
        .collect();
    for f in 0..frames {
        let m = motion.at(f as f32 / 60.0);
        live.step(&m, 1.0 / 60.0).expect("a step");
        let places = live.places();
        for (k, &i) in picked.iter().enumerate() {
            let carried = math::mul(&m, &live.bones[i].rest);
            let d = distance(&places[i], &carried);
            most[k] = most[k].max(d);
            end[k] = d;
            let a = turn(&places[i], &carried);
            most_turn[k] = most_turn[k].max(a);
            end_turn[k] = a;
        }
    }
    println!(
        "{} bones moved by the cloth; {} picked; test motion {:.1} s",
        live.bones.len(),
        picked.len(),
        motion.seconds()
    );
    for (k, &i) in picked.iter().enumerate() {
        println!(
            "  {:28} settles {:.4} m and {:.1} deg from rest; in the motion up to {:.3} m and {:.1} deg from where it would be carried, {:.3} m and {:.1} deg at the end",
            live.bones[i].name,
            settled[k],
            settled_turn[k],
            most[k],
            most_turn[k],
            end[k],
            end_turn[k]
        );
    }
}
