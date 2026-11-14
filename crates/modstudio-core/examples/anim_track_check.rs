//! One bone's track in an animation, against its rest place in a model: at every `step`-th frame, its turn from rest
//! (degrees, and about which of its own axes) and its move from rest (cm). For checking an added or edited track.
//!
//!     cargo run --release -p modstudio-core --example anim_track_check -- <romfs> <model> <anim file> <anim> <bone> [step]
//!
//! (`<model>` and `<anim file>`: paths in the romfs, or a built mod's files by their own path.)

use modstudio_core::math;
use modstudio_core::model;
use modstudio_formats::bfres::Bfres;
use modstudio_formats::bfres_anim::{self, target};
use modstudio_formats::romfs::Romfs;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [root, model_path, file, name, bone, rest @ ..] = args.as_slice() else {
        eprintln!("usage: anim_track_check <romfs> <model> <anim file> <anim> <bone> [step]");
        std::process::exit(2);
    };
    let step: usize = rest.first().and_then(|s| s.parse().ok()).unwrap_or(15);
    let romfs = Romfs::open(root).expect("romfs");
    let mut d = romfs.decoder().unwrap();
    let strings = model::load_external_strings(&romfs, &mut d).unwrap();
    let model_data = romfs.read(&mut d, model_path).unwrap();
    let bfres = Bfres::parse(&model_data, Some(&strings)).unwrap();
    let skel = &bfres.models[0].skeleton;
    let b = skel
        .bones
        .iter()
        .find(|b| &b.name == bone)
        .expect("the bone in the model");
    let euler_skel = skel.euler_rotations();
    let rest_q = if euler_skel {
        math::quat_from_euler_xyz([b.rotation[0], b.rotation[1], b.rotation[2]])
    } else {
        b.rotation
    };
    let anim_data = romfs.read(&mut d, file).unwrap();
    let anim = bfres_anim::find(&anim_data, Some(&strings), name).unwrap();
    let Some(t) = anim.bones.iter().find(|t| &t.name == bone) else {
        println!("{name}: no track for {bone} ({} tracks)", anim.bones.len());
        return;
    };
    println!(
        "{name}: {} frames; track {bone}: flags 0x{:08x}, {} curve(s) {:?}, first curve {}, base S {:?} R {:?} T {:?}",
        anim.frame_count + 1,
        t.flags,
        t.curves.len(),
        t.curves
            .iter()
            .map(|c| format!("0x{:x}", c.target))
            .collect::<Vec<_>>(),
        t.begin_curve,
        t.base_scale,
        t.base_rotation,
        t.base_translation
    );
    println!(
        "rest: T {:?} R {:?} (Euler skeleton {euler_skel})",
        b.translation, b.rotation
    );
    let value = |target: u32, base: f32, f: f32| t.curve(target).map_or(base, |c| c.value_at(f));
    for f in (0..=anim.frame_count.max(0) as usize).step_by(step) {
        let f = f as f32;
        let br = t.base_rotation.unwrap_or([0.0, 0.0, 0.0, 1.0]);
        let q = if t.quaternion() {
            t.curve(target::ROTATE_X)
                .and_then(|c| c.quat_at(f))
                .unwrap_or(br)
        } else {
            math::quat_from_euler_xyz([
                value(target::ROTATE_X, br[0], f),
                value(target::ROTATE_Y, br[1], f),
                value(target::ROTATE_Z, br[2], f),
            ])
        };
        // the turn from rest, in the bone's own axes: rest^-1 * q
        let inv = [-rest_q[0], -rest_q[1], -rest_q[2], rest_q[3]];
        let d = math::quat_mul(inv, q);
        let s = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let angle = 2.0 * s.atan2(d[3]).to_degrees();
        let axis = if s > 1e-6 {
            [d[0] / s, d[1] / s, d[2] / s]
        } else {
            [0.0; 3]
        };
        let bt = t.base_translation.unwrap_or(b.translation);
        let tr = [
            value(target::TRANSLATE_X, bt[0], f),
            value(target::TRANSLATE_Y, bt[1], f),
            value(target::TRANSLATE_Z, bt[2], f),
        ];
        let moved = ((tr[0] - b.translation[0]).powi(2)
            + (tr[1] - b.translation[1]).powi(2)
            + (tr[2] - b.translation[2]).powi(2))
        .sqrt();
        println!(
            "  frame {f:5}: turned {angle:7.2} deg about ({:5.2}, {:5.2}, {:5.2}), moved {:.3} cm",
            axis[0],
            axis[1],
            axis[2],
            moved * 100.0
        );
    }
}
