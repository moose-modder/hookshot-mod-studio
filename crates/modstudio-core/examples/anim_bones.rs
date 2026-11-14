//! One animation bone by bone: at a few frames, each bone's rotation against its rest rotation (degrees) and its
//! translation against its rest translation (cm), the largest first, with how its curves are stored. For finding a
//! decoding fault: a standing idle should keep most bones near rest.
//!
//!     cargo run --release -p modstudio-core --example anim_bones -- <romfs> <model.bfres.zs> <anim file> <name>

use modstudio_core::math;
use modstudio_core::model;
use modstudio_formats::bfres::Bfres;
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

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [root, model_path, file, name] = args.as_slice() else {
        eprintln!("usage: anim_bones <romfs> <model> <anim file> <name>");
        std::process::exit(2);
    };
    let romfs = Romfs::open(root).expect("romfs");
    let mut d = romfs.decoder().unwrap();
    let strings = model::load_external_strings(&romfs, &mut d).unwrap();
    let data = romfs.read(&mut d, model_path).unwrap();
    let bfres = Bfres::parse(&data, Some(&strings)).unwrap();
    let skel = &bfres.models[0].skeleton;
    let anim_data = romfs.read(&mut d, file).unwrap();
    let anim = bfres_anim::find(&anim_data, Some(&strings), name).unwrap();
    println!(
        "{name}: flags 0x{:x}, {} frames, {} bones; skeleton {} bones, {}",
        anim.flags,
        anim.frame_count,
        anim.bones.len(),
        skel.bones.len(),
        if skel.euler_rotations() {
            "Euler"
        } else {
            "quaternions"
        }
    );
    for frame in [0.0, anim.frame_count as f32 / 2.0] {
        let mut rows = Vec::new();
        for b in &anim.bones {
            let Some(rest) = skel.bones.iter().find(|r| r.name == b.name) else {
                continue;
            };
            let rest_q =
                math::quat_from_euler_xyz([rest.rotation[0], rest.rotation[1], rest.rotation[2]]);
            let base = b.base_rotation.unwrap_or([
                rest.rotation[0],
                rest.rotation[1],
                rest.rotation[2],
                0.0,
            ]);
            let e = [
                b.curve(target::ROTATE_X)
                    .map_or(base[0], |c| c.value_at(frame)),
                b.curve(target::ROTATE_Y)
                    .map_or(base[1], |c| c.value_at(frame)),
                b.curve(target::ROTATE_Z)
                    .map_or(base[2], |c| c.value_at(frame)),
            ];
            let q = if b.quaternion() {
                b.curve(target::ROTATE_X)
                    .and_then(|c| c.quat_at(frame))
                    .unwrap_or(base)
            } else {
                math::quat_from_euler_xyz(e)
            };
            let deg = angle(q, rest_q);
            let enc: Vec<String> = b
                .curves
                .iter()
                .map(|c| format!("{:x}:{:x}", c.target, c.flags))
                .collect();
            let mut enc = enc;
            enc.insert(0, format!("bone flags 0x{:x}", b.flags));
            rows.push((
                deg,
                b.name.clone(),
                e,
                [rest.rotation[0], rest.rotation[1], rest.rotation[2]],
                b.base_rotation,
                enc,
            ));
        }
        rows.sort_by(|a, b| b.0.total_cmp(&a.0));
        let big = rows.iter().filter(|r| r.0 > 20.0).count();
        println!(
            "frame {frame}: {} bones over 20 degrees from rest; the largest:",
            big
        );
        let others: Vec<String> = rows
            .iter()
            .filter(|r| r.0 > 20.0 && !r.1.starts_with("Finger") && !r.1.starts_with("Thumb"))
            .map(|r| format!("{} {:.0}", r.1, r.0))
            .collect();
        println!("    over 20 degrees, not a finger: {others:?}");
        for r in rows.iter().take(12) {
            println!(
                "    {:6.1} deg {:24} anim {:?} rest {:?} base {:?} curves {:?}",
                r.0,
                r.1,
                r.2.map(|v| (v * 1000.0).round() / 1000.0),
                r.3.map(|v| (v * 1000.0).round() / 1000.0),
                r.4.map(|v| v.map(|x| (x * 1000.0).round() / 1000.0)),
                r.5
            );
        }
    }
}
