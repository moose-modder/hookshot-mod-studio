//! The cloth following an animation (`LiveCloth::step_posed` with `anim::Clip::pose_by_name`). First, the bones the
//! cloth moves at rest in the model's skeleton against the cloth file's own (they should be the same places). Then
//! the animation played through twice, the cloth stepped with each frame's pose after settling on the first: where
//! each simulated chain starts (its first bone, on a body bone) against where the animation has it. The rest of a
//! chain hangs and swings as the simulation has it (a long cap hangs down from a bent head, away from the
//! animation's own pose for it), so only the starts are measured.
//!
//!     cargo run --release -p modstudio-core --example cloth_anim -- <romfs> <model.bfres.zs> <name filter>

use modstudio_core::anim::{self, Clip};
use modstudio_core::cloth::{self, LiveCloth};
use modstudio_core::math;
use modstudio_formats::romfs::Romfs;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [root, model, filter] = args.as_slice() else {
        eprintln!("usage: cloth_anim <romfs> <model> <name filter>");
        std::process::exit(2);
    };
    let romfs = Romfs::open(root).expect("romfs");
    let (source, file) = cloth::find_cloth(&romfs, None, model).expect("the model has cloth");
    let mut live = LiveCloth::new(&std::fs::read(&file).unwrap(), &source).expect("cloth");
    let list = anim::list(&romfs, None, model).expect("list");
    let entry = list
        .iter()
        .find(|a| a.name.contains(filter.as_str()))
        .expect("an animation of that name");
    let clip = Clip::open(&romfs, model, &entry.file, &entry.name).expect("clip");
    println!(
        "{source}: {} parts, {} bones moved; {} / {}: {} frames",
        live.parts.len(),
        live.bones.len(),
        entry.file,
        entry.name,
        clip.frame_count
    );
    let at = |m: &math::Mat4| [m[12], m[13], m[14]];
    let dist = |a: [f32; 3], b: [f32; 3]| {
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
    };
    // 1. the same rest places?
    let rest = clip.rest_pose_by_name();
    let mut worst = (0f32, String::new());
    let mut missing = 0;
    for b in &live.bones {
        match rest.get(b.name.as_str()) {
            Some(m) => {
                let d = dist(at(m), at(&b.rest));
                if d > worst.0 {
                    worst = (d, b.name.clone());
                }
            }
            None => missing += 1,
        }
    }
    println!(
        "at rest, the cloth's bones against the model's: worst {:.2} mm ({}), {missing} not in the model",
        worst.0 * 1000.0,
        worst.1
    );
    // 2. played through twice; the chains' first bones against the animation
    let starts: Vec<usize> = live
        .bones
        .iter()
        .enumerate()
        .filter(|(_, b)| !clip.parent(&b.name).is_some_and(|p| p.starts_with("Ph_")))
        .map(|(i, _)| i)
        .collect();
    let dt = 1.0 / anim::FPS;
    let first = clip.pose_by_name(0.0);
    for _ in 0..60 {
        live.step_posed(&math::IDENTITY, &|n| first.get(n).copied(), dt)
            .unwrap();
    }
    let mut far = (0f32, String::new(), 0);
    let length = clip.frame_count.max(1) as usize + 1;
    for step in 0..length * 2 {
        let posed = clip.pose_by_name((step % length) as f32);
        live.step_posed(&math::IDENTITY, &|n| posed.get(n).copied(), dt)
            .unwrap_or_else(|e| panic!("step {step}: {e}"));
        let places = live.places();
        for &i in &starts {
            let b = &live.bones[i];
            if let Some(m) = posed.get(b.name.as_str()) {
                let d = dist(at(m), at(&places[i]));
                if d > far.0 {
                    far = (d, b.name.clone(), step);
                }
            }
        }
    }
    println!(
        "played twice: {} chains; the furthest a chain's first bone got from the animation's place: {:.1} cm ({} at step {})",
        starts.len(),
        far.0 * 100.0,
        far.1,
        far.2
    );
}
