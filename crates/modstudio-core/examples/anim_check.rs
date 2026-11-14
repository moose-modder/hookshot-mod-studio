//! Checks the animation decoding against a model's skeleton: for every animation of the given files, each bone it
//! shares with the skeleton (by name) at frame 0 against the bone's rest pose, its rotation read each way the
//! animation's rotation mode could mean (Euler XYZ; a quaternion stored without w), and over every frame whether
//! the bone keeps its length (a wrong scale or offset in the decoding would stretch bones).
//!
//!     cargo run --release -p modstudio-core --example anim_check -- <romfs> <model.bfres.zs> <anim file>...

use std::collections::BTreeMap;

use modstudio_core::math;
use modstudio_formats::bfres::{Bfres, ExternalStrings};
use modstudio_formats::bfres_anim::{self, target};
use modstudio_formats::romfs::Romfs;

fn angle(a: [f32; 4], b: [f32; 4]) -> f32 {
    let a = math::normalize_quat(a);
    let b = math::normalize_quat(b);
    let dot = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3])
        .abs()
        .min(1.0);
    2.0 * dot.acos().to_degrees()
}

fn median(mut v: Vec<f32>) -> f32 {
    if v.is_empty() {
        return f32::NAN;
    }
    v.sort_by(f32::total_cmp);
    v[v.len() / 2]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [root, model, anims @ ..] = args.as_slice() else {
        eprintln!("usage: anim_check <romfs> <model> <anim>...");
        std::process::exit(2);
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
    let skel = &bfres.models[0].skeleton;
    let rest: BTreeMap<&str, (math::Quat, [f32; 3], bool)> = skel
        .bones
        .iter()
        .map(|b| {
            let q = if skel.euler_rotations() {
                math::quat_from_euler_xyz([b.rotation[0], b.rotation[1], b.rotation[2]])
            } else {
                b.rotation
            };
            (b.name.as_str(), (q, b.translation, b.parent.is_none()))
        })
        .collect();
    println!(
        "{model}: {} bones, rotations {}",
        skel.bones.len(),
        if skel.euler_rotations() {
            "Euler"
        } else {
            "quaternions"
        }
    );
    // per rotation mode: angles to rest at frame 0 (as Euler, as quaternion), bones whose length holds
    #[derive(Default)]
    struct Tally {
        anims: usize,
        euler: Vec<f32>,
        quat: Vec<f32>,
        lengths: usize,
        held: usize,
        worst: (f32, String),
        unmatched: usize,
        matched: usize,
        frame0: Vec<f32>,
        failing: BTreeMap<String, usize>,
        samples: Vec<String>,
    }
    let mut by_mode: BTreeMap<u32, Tally> = BTreeMap::new();
    for file in anims {
        let data = romfs.read(&mut d, format!("Model/{file}")).expect("anim");
        let parsed = bfres_anim::parse(&data, Some(&strings)).expect("parse");
        for a in &parsed.anims {
            let t = by_mode.entry(a.flags & 0x7000).or_default();
            t.anims += 1;
            for b in &a.bones {
                let Some(&(rest_q, rest_t, is_root)) = rest.get(b.name.as_str()) else {
                    t.unmatched += 1;
                    continue;
                };
                t.matched += 1;
                let base_r = b.base_rotation.unwrap_or([0.0, 0.0, 0.0, 1.0]);
                let rot = |f: f32| -> [f32; 3] {
                    [
                        b.curve(target::ROTATE_X)
                            .map_or(base_r[0], |c| c.value_at(f)),
                        b.curve(target::ROTATE_Y)
                            .map_or(base_r[1], |c| c.value_at(f)),
                        b.curve(target::ROTATE_Z)
                            .map_or(base_r[2], |c| c.value_at(f)),
                    ]
                };
                let r0 = rot(0.0);
                t.euler.push(angle(math::quat_from_euler_xyz(r0), rest_q));
                let w = (1.0 - (r0[0] * r0[0] + r0[1] * r0[1] + r0[2] * r0[2]))
                    .max(0.0)
                    .sqrt();
                t.quat.push(angle([r0[0], r0[1], r0[2], w], rest_q));
                let base_t = b.base_translation.unwrap_or(rest_t);
                let has_t = [
                    target::TRANSLATE_X,
                    target::TRANSLATE_Y,
                    target::TRANSLATE_Z,
                ]
                .iter()
                .any(|&x| b.curve(x).is_some());
                let rest_len =
                    (rest_t[0] * rest_t[0] + rest_t[1] * rest_t[1] + rest_t[2] * rest_t[2]).sqrt();
                // bones that can't stretch: no physics helpers, face, weapons or the root's motion
                let rigid = ![
                    "Ph_", "Eye", "Lip", "Cheek", "Mouth", "Jaw", "Tongue", "Weapon", "Skl_",
                    "Nose", "Ear",
                ]
                .iter()
                .any(|p| b.name.starts_with(p));
                if has_t && !is_root && rest_len > 1e-3 && rigid {
                    t.lengths += 1;
                    let mut worst = 0f32;
                    for q in 0..=(a.frame_count.max(0) * 4) {
                        let f = q as f32 / 4.0;
                        let v = [
                            b.curve(target::TRANSLATE_X)
                                .map_or(base_t[0], |c| c.value_at(f)),
                            b.curve(target::TRANSLATE_Y)
                                .map_or(base_t[1], |c| c.value_at(f)),
                            b.curve(target::TRANSLATE_Z)
                                .map_or(base_t[2], |c| c.value_at(f)),
                        ];
                        let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                        worst = worst.max((len / rest_len - 1.0).abs());
                    }
                    let v0 = [
                        b.curve(target::TRANSLATE_X)
                            .map_or(base_t[0], |c| c.value_at(0.0)),
                        b.curve(target::TRANSLATE_Y)
                            .map_or(base_t[1], |c| c.value_at(0.0)),
                        b.curve(target::TRANSLATE_Z)
                            .map_or(base_t[2], |c| c.value_at(0.0)),
                    ];
                    let off0 = ((v0[0] - rest_t[0]).powi(2)
                        + (v0[1] - rest_t[1]).powi(2)
                        + (v0[2] - rest_t[2]).powi(2))
                    .sqrt();
                    t.frame0.push(off0);
                    if worst < 0.02 {
                        t.held += 1;
                    } else {
                        let key = b.name.split('_').take(2).collect::<Vec<_>>().join("_");
                        *t.failing.entry(key).or_default() += 1;
                        if t.samples.len() < 6 && b.name == "Clavicle_L" {
                            let c = b.curve(target::TRANSLATE_X);
                            t.samples.push(format!(
                                "{} {}: rest {:?}; x curve {:?}",
                                a.name,
                                b.name,
                                rest_t,
                                c.map(|c| (
                                    c.flags,
                                    c.scale,
                                    c.offset,
                                    c.delta,
                                    c.frames.iter().take(4).cloned().collect::<Vec<_>>(),
                                    c.segments.iter().take(3).cloned().collect::<Vec<_>>()
                                ))
                            ));
                        }
                    }
                    if worst > t.worst.0 {
                        t.worst = (worst, format!("{} {}", a.name, b.name));
                    }
                }
            }
        }
    }
    for (mode, t) in &by_mode {
        println!(
            "rotation mode 0x{mode:x}: {} animations, {} bone tracks matched by name ({} not in the skeleton)",
            t.anims, t.matched, t.unmatched
        );
        println!(
            "    frame 0 against rest, median angle: read as Euler {:.2} deg, as a quaternion without w {:.2} deg",
            median(t.euler.clone()),
            median(t.quat.clone())
        );
        println!(
            "    bones keeping their length within 2% over every frame: {} of {} (worst {:.3}: {})",
            t.held, t.lengths, t.worst.0, t.worst.1
        );
        let f0 = &t.frame0;
        println!(
            "    frame 0 translation against rest: median {:.5}, within 1 mm {} of {}",
            median(f0.clone()),
            f0.iter().filter(|&&x| x < 1e-3).count(),
            f0.len()
        );
        let mut failing: Vec<_> = t.failing.iter().collect();
        failing.sort_by(|a, b| b.1.cmp(a.1));
        println!(
            "    failing by name: {:?}",
            &failing[..failing.len().min(14)]
        );
        for s in &t.samples {
            println!("    {s}");
        }
    }
}
