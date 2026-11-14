//! The animation writer on the game's files: in each file, every animation's bones (those with all three base
//! values) written again from their own curves, sampled at every frame (`bfres_anim_write`): every other curve new,
//! the rest kept. Then: the written file's relocation table points inside it, the file reads back, every curve gives
//! the same values at every frame and between frames, and the counts (each bone's first curve, the animation's
//! curves and its baked size) are right.
//!
//!     cargo run --release -p modstudio-formats --example anim_rewrite -- <romfs> [name filter] [--max N]

use std::path::Path;
use std::time::Instant;

use modstudio_formats::bfres::ExternalStrings;
use modstudio_formats::bfres_anim::{self, BoneAnim, Curve, curve_flags};
use modstudio_formats::bfres_anim_write::{self, AnimWriter, BoneEdit, CurveEdit};
use modstudio_formats::romfs::Romfs;

fn angle(a: [f32; 4], b: [f32; 4]) -> f32 {
    let dot = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3])
        .abs()
        .min(1.0);
    2.0 * dot.acos().to_degrees()
}

fn edit_of(b: &BoneAnim, frames: i32, keep_odd: bool) -> Option<BoneEdit> {
    let (s, r, t) = (b.base_scale?, b.base_rotation?, b.base_translation?);
    let curves = b
        .curves
        .iter()
        .enumerate()
        .map(|(i, c)| {
            if keep_odd && i % 2 == 1 {
                return CurveEdit::Keep(i);
            }
            if c.flags & curve_flags::QUATERNION != 0 {
                CurveEdit::Quaternion {
                    values: (0..=frames.max(0))
                        .map(|f| c.quat_at(f as f32).unwrap())
                        .collect(),
                }
            } else {
                CurveEdit::Scalar {
                    target: c.target,
                    values: (0..=frames.max(0)).map(|f| c.value_at(f as f32)).collect(),
                }
            }
        })
        .collect();
    Some(BoneEdit {
        base_scale: s,
        base_rotation: r,
        base_translation: t,
        curves,
    })
}

/// The worst difference between two curves over every quarter frame: for a scalar, against the curve's range.
fn difference(a: &Curve, b: &Curve, frames: i32) -> f32 {
    let mut worst = 0f32;
    if a.flags & curve_flags::QUATERNION != 0 {
        for q in 0..=(frames.max(0) * 4) {
            let f = q as f32 / 4.0;
            worst = worst.max(angle(a.quat_at(f).unwrap(), b.quat_at(f).unwrap()));
        }
        return worst;
    }
    let values: Vec<f32> = (0..=frames.max(0)).map(|f| a.value_at(f as f32)).collect();
    let lo = values.iter().copied().fold(f32::INFINITY, f32::min);
    let hi = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let range = (hi - lo).max(1e-3);
    for q in 0..=(frames.max(0) * 4) {
        let f = q as f32 / 4.0;
        // between frames the two curves are different shapes through the same values: measured at frames only
        if q % 4 != 0 {
            continue;
        }
        worst = worst.max((a.value_at(f) - b.value_at(f)).abs() / range);
    }
    worst
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(root) = args.first() else {
        eprintln!("usage: anim_rewrite <romfs> [name filter] [--max N]");
        std::process::exit(2);
    };
    let filter = args
        .get(1)
        .filter(|a| !a.starts_with("--"))
        .cloned()
        .unwrap_or_default();
    let max: usize = args
        .iter()
        .position(|a| a == "--max")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(usize::MAX);
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
    files.truncate(max);
    let t = Instant::now();
    let (mut ok, mut bad) = (0, Vec::new());
    let (mut worst_scalar, mut worst_quat) = ((0f32, String::new()), (0f32, String::new()));
    let (mut size_in, mut size_out, mut bones_edited, mut pointers) =
        (0usize, 0usize, 0usize, 0usize);
    for f in &files {
        let data = romfs.read(&mut d, format!("Model/{f}")).unwrap();
        let original = bfres_anim::parse(&data, Some(&strings)).unwrap();
        if let Err(e) = bfres_anim_write::check_relocations(&data) {
            bad.push(format!("{f}: the game's own table: {e}"));
            continue;
        }
        let mut w = AnimWriter::new(&data).unwrap();
        let mut edited = Vec::new();
        for a in &original.anims {
            for (i, b) in a.bones.iter().enumerate() {
                if let Some(e) = edit_of(b, a.frame_count, i % 2 == 0) {
                    w.edit_bone(&data, a, i, &e).unwrap();
                    edited.push((a.name.clone(), i));
                }
            }
        }
        bones_edited += edited.len();
        let out = match w.finish(&original.anims) {
            Ok(o) => o,
            Err(e) => {
                bad.push(format!("{f}: writing: {e}"));
                continue;
            }
        };
        size_in += data.len();
        size_out += out.len();
        match bfres_anim_write::check_relocations(&out) {
            Ok(n) => pointers += n,
            Err(e) => {
                bad.push(format!("{f}: the written table: {e}"));
                continue;
            }
        }
        let written = match bfres_anim::parse(&out, Some(&strings)) {
            Ok(x) => x,
            Err(e) => {
                bad.push(format!("{f}: reading it back: {e}"));
                continue;
            }
        };
        let mut fine = true;
        for (a, b) in original.anims.iter().zip(&written.anims) {
            let curves: usize = b.bones.iter().map(|x| x.curves.len()).sum();
            if b.curve_count as usize != curves
                || b.baked_size != bfres_anim_write::baked_size(curves, b.frame_count)
            {
                bad.push(format!(
                    "{f} / {}: counts {} / {} for {curves} curves",
                    a.name, b.curve_count, b.baked_size
                ));
                fine = false;
            }
            let mut first = 0;
            for (x, y) in a.bones.iter().zip(&b.bones) {
                if y.begin_curve != first {
                    bad.push(format!(
                        "{f} / {} / {}: first curve {} where {first}",
                        a.name, y.name, y.begin_curve
                    ));
                    fine = false;
                }
                first += y.curves.len() as i32;
                if x.curves.len() != y.curves.len() || x.name != y.name {
                    bad.push(format!(
                        "{f} / {} / {}: {} curves became {}",
                        a.name,
                        x.name,
                        x.curves.len(),
                        y.curves.len()
                    ));
                    fine = false;
                    continue;
                }
                for (cx, cy) in x.curves.iter().zip(&y.curves) {
                    let diff = difference(cx, cy, a.frame_count);
                    let slot = if cx.flags & curve_flags::QUATERNION != 0 {
                        &mut worst_quat
                    } else {
                        &mut worst_scalar
                    };
                    if diff > slot.0 {
                        *slot = (
                            diff,
                            format!("{f} / {} / {} 0x{:x}", a.name, x.name, cx.target),
                        );
                    }
                }
            }
        }
        if fine {
            ok += 1;
        }
    }
    println!(
        "{} file(s) in {:.1} s: {ok} written, read back and matching; {} bone animations written again; {:.1} MB -> {:.1} MB; {pointers} pointers relocated",
        files.len(),
        t.elapsed().as_secs_f32(),
        bones_edited,
        size_in as f32 / 1e6,
        size_out as f32 / 1e6
    );
    println!(
        "worst scalar curve difference at frames (share of its range): {:.6} ({})",
        worst_scalar.0, worst_scalar.1
    );
    println!(
        "worst quaternion curve difference (degrees, every quarter frame): {:.4} ({})",
        worst_quat.0, worst_quat.1
    );
    for b in bad.iter().take(12) {
        println!("    {b}");
    }
    if !bad.is_empty() {
        println!("    ({} problems)", bad.len());
    }
}
