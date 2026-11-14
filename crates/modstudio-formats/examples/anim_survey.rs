//! The game's skeletal animations, read (`bfres_anim`) and surveyed: how many read, which curve encodings they use,
//! whether bones are bound by index in the file, and checks on the decoding: frames in order and inside the curve's
//! range, cubic segments meeting at their keys (a wrong scale or offset breaks that).
//!
//!     cargo run --release -p modstudio-formats --example anim_survey -- <romfs> [name filter]

use std::collections::BTreeMap;
use std::path::Path;

use modstudio_formats::bfres::ExternalStrings;
use modstudio_formats::bfres_anim::{self, curve_flags};
use modstudio_formats::romfs::Romfs;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(root) = args.first() else {
        eprintln!("usage: anim_survey <romfs> [name filter]");
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
        .filter(|n| n.contains(".anim.") && n.contains(filter.as_str()))
        .collect();
    files.sort();
    let (mut ok, mut failed) = (0, Vec::new());
    let mut anims = 0;
    let mut bones = 0;
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut anim_flags: BTreeMap<u32, usize> = BTreeMap::new();
    let mut targets: BTreeMap<u32, usize> = BTreeMap::new();
    let mut base: BTreeMap<u32, usize> = BTreeMap::new();
    let mut bind: BTreeMap<String, usize> = BTreeMap::new();
    let mut frames_bad = 0;
    // per encoding: worst jump at a key, relative to the curve's range, and how many curves
    let mut gaps: BTreeMap<String, (f32, usize, String)> = BTreeMap::new();
    let mut ext_names = 0;
    let mut rot_kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut quat_curves: BTreeMap<String, usize> = BTreeMap::new();
    let mut quat_w = (0usize, 0usize, 0usize);
    for f in &files {
        let data = match romfs.read(&mut d, format!("Model/{f}")) {
            Ok(x) => x,
            Err(e) => {
                failed.push(format!("{f}: {e}"));
                continue;
            }
        };
        let file = match bfres_anim::parse(&data, Some(&strings)) {
            Ok(x) => x,
            Err(e) => {
                failed.push(format!("{f}: {e}"));
                continue;
            }
        };
        ok += 1;
        for a in &file.anims {
            anims += 1;
            *anim_flags.entry(a.flags).or_default() += 1;
            let bound = if a.bind_indices.is_empty() {
                "none".to_owned()
            } else if a.bind_indices.iter().all(|&b| b == 0xFFFF) {
                "all 0xFFFF".to_owned()
            } else {
                "set".to_owned()
            };
            *bind.entry(bound).or_default() += 1;
            for b in &a.bones {
                bones += 1;
                if b.name.starts_with("ExtStr_") {
                    ext_names += 1;
                }
                *base.entry(b.flags & 0x38).or_default() += 1;
                if let Some(r) = b.base_rotation {
                    let len = (r[0] * r[0] + r[1] * r[1] + r[2] * r[2] + r[3] * r[3]).sqrt();
                    let unit = (len - 1.0).abs() < 1e-3;
                    let euler_like =
                        !unit || (r[3] == 1.0 && (r[0] != 0.0 || r[1] != 0.0 || r[2] != 0.0));
                    let identity = r == [0.0, 0.0, 0.0, 1.0];
                    let kind = if identity {
                        "identity"
                    } else if euler_like {
                        "not unit (Euler)"
                    } else {
                        "unit (quaternion)"
                    };
                    let key = format!(
                        "anim rot mode 0x{:x}, bone bit20 {}: base {kind}",
                        a.flags & 0x7000,
                        (b.flags >> 20) & 1
                    );
                    *rot_kinds.entry(key).or_default() += 1;
                }
                if (b.flags >> 20) & 1 == 1 {
                    let mut targets: Vec<String> =
                        b.curves.iter().map(|c| format!("{:x}", c.target)).collect();
                    targets.sort();
                    let key = format!(
                        "curve bits 0x{:03x} -> targets {:?}",
                        (b.flags >> 6) & 0x3FF,
                        targets
                    );
                    *quat_curves.entry(key).or_default() += 1;
                    // W from the curves' x, y, z at frame 0 against the base w
                    if let Some(r) = b.base_rotation {
                        let x = b.curve(0x20).map_or(r[0], |c| c.value_at(0.0));
                        let y = b.curve(0x24).map_or(r[1], |c| c.value_at(0.0));
                        let z = b.curve(0x28).map_or(r[2], |c| c.value_at(0.0));
                        let w2 = 1.0 - x * x - y * y - z * z;
                        if !b.curves.is_empty() {
                            quat_w.0 += 1;
                            if w2 >= -1e-3 && (w2.max(0.0).sqrt() - r[3].abs()).abs() < 0.02 {
                                quat_w.1 += 1;
                            }
                            if r[3] < 0.0 {
                                quat_w.2 += 1;
                            }
                        }
                    }
                }
                for c in &b.curves {
                    *targets.entry(c.target).or_default() += 1;
                    let ct = match c.curve_type() {
                        curve_flags::CUBIC => "cubic",
                        curve_flags::LINEAR => "linear",
                        curve_flags::BAKED_FLOAT => "baked",
                        curve_flags::STEP_FLOAT => "step",
                        curve_flags::STEP_INT => "step int",
                        curve_flags::BAKED_INT => "baked int",
                        _ => "bool",
                    };
                    let kind = format!(
                        "{} {ct} frames:{} keys:{}{}{}",
                        if c.value_keys() {
                            "value-keyed"
                        } else {
                            "older"
                        },
                        ["f32", "10x5", "u8", "?"][(c.flags & 3) as usize],
                        ["f32", "16-bit", "8-bit", "?"][((c.flags >> 2) & 3) as usize],
                        if c.flags & curve_flags::FRAME_SCALED != 0 {
                            " frame-scaled"
                        } else {
                            ""
                        },
                        if c.flags & curve_flags::SLOPE_QUANTISED != 0 {
                            " slope-q"
                        } else {
                            ""
                        },
                    );
                    *kinds.entry(kind.clone()).or_default() += 1;
                    if c.frames.windows(2).any(|w| w[1] < w[0])
                        || c.frames.first().is_some_and(|&x| x < c.start_frame - 0.01)
                        || c.frames.last().is_some_and(|&x| x > c.end_frame + 0.01)
                    {
                        frames_bad += 1;
                    }
                    if c.curve_type() == curve_flags::CUBIC && c.segments.len() > 1 {
                        let lo = c.segments.iter().map(|s| s[0]).fold(f32::MAX, f32::min);
                        let hi = c.segments.iter().map(|s| s[0]).fold(f32::MIN, f32::max);
                        let range = (hi - lo).max(1e-3);
                        let mut worst = 0f32;
                        for w in c.segments.windows(2) {
                            let end = w[0][0] + w[0][1] + w[0][2] + w[0][3];
                            worst = worst.max((end - w[1][0]).abs() / range);
                        }
                        let g = gaps.entry(kind).or_insert((0.0, 0, String::new()));
                        g.1 += 1;
                        if worst > g.0 {
                            *g = (worst, g.1, format!("{f} {} {}", a.name, b.name));
                        }
                    }
                }
            }
        }
    }
    println!("{} files: {ok} read, {} failed", files.len(), failed.len());
    for f in failed.iter().take(10) {
        println!("    {f}");
    }
    println!(
        "{anims} animations, {bones} bone animations ({ext_names} named by an unresolved string ID)"
    );
    println!(
        "animation flags: {:?}",
        anim_flags
            .iter()
            .map(|(k, v)| format!("0x{k:x}:{v}"))
            .collect::<Vec<_>>()
    );
    println!("bind indices: {bind:?}");
    println!(
        "base values (0x08 scale, 0x10 rotate, 0x20 translate): {:?}",
        base.iter()
            .map(|(k, v)| format!("0x{k:x}:{v}"))
            .collect::<Vec<_>>()
    );
    println!(
        "targets: {:?}",
        targets
            .iter()
            .map(|(k, v)| format!("0x{k:x}:{v}"))
            .collect::<Vec<_>>()
    );
    println!("frames out of order or range: {frames_bad}");
    println!("base rotations:");
    for (k, v) in &rot_kinds {
        println!("    {v:8} {k}");
    }
    println!("quaternion bones:");
    for (k, v) in quat_curves.iter().take(20) {
        println!("    {v:8} {k}");
    }
    println!(
        "    with curves: {}; base w equal to sqrt(1 - x2 - y2 - z2) of frame 0: {}; base w negative: {}",
        quat_w.0, quat_w.1, quat_w.2
    );
    println!("curve encodings:");
    for (k, v) in &kinds {
        println!("    {v:7} {k}");
    }
    println!("cubic curves, the worst jump at a key (share of the curve's range):");
    for (k, (g, n, at)) in &gaps {
        println!("    {k}: {n} curves, worst {g:.4} ({at})");
    }
}
