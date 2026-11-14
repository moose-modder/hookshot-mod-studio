//! The animation round trip without Blender: an animation written for the scene (`anim_edit::write_for_blender`),
//! read back as Blender would export it unchanged (each bone's change from rest), then with one bone turned 30
//! degrees about its own x axis at every frame (its descendants carried along, as Blender's are), and each applied
//! (`anim_edit::apply`). Unchanged, nothing is written; turned, only that bone changes, and the written file has
//! the turn.
//!
//!     cargo run --release -p modstudio-core --example anim_roundtrip -- <romfs> <model.bfres.zs> <anim file> <name> <bone>

use std::path::Path;

use modstudio_core::anim::Clip;
use modstudio_core::anim_edit::{self, SceneAnim, SceneEdits};
use modstudio_core::math::{self, Mat4};
use modstudio_formats::bfres_anim;
use modstudio_formats::romfs::Romfs;

fn floats(path: &Path) -> Vec<f32> {
    let raw = std::fs::read(path).unwrap();
    raw.as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect()
}

fn write_edits(dir: &Path, header: &anim_edit::BlenderAnim, data: &[f32]) -> std::path::PathBuf {
    let bin: Vec<u8> = data.iter().flat_map(|v| v.to_le_bytes()).collect();
    std::fs::write(dir.join(anim_edit::EDITS_DATA), bin).unwrap();
    let edits = SceneEdits {
        data: anim_edit::EDITS_DATA.to_owned(),
        anims: vec![SceneAnim {
            file: header.file.clone(),
            name: header.name.clone(),
            frame_count: header.frame_count,
            action: header.name.clone(),
            bones: header.bones.iter().map(|b| b.name.clone()).collect(),
            offset: 0,
        }],
    };
    let path = dir.join(anim_edit::EDITS_FILE);
    std::fs::write(&path, serde_json::to_string(&edits).unwrap()).unwrap();
    path
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [root, model, file, name, bone] = args.as_slice() else {
        eprintln!("usage: anim_roundtrip <romfs> <model> <anim file> <name> <bone>");
        std::process::exit(2);
    };
    let romfs = Romfs::open(root).expect("romfs");
    let dir = std::env::temp_dir().join(format!("modstudio-anim-{}", std::process::id()));
    let (header_path, header) =
        anim_edit::write_for_blender(&romfs, model, file, name, &dir).unwrap();
    let g = floats(&header_path.with_file_name(&header.data));
    let nb = header.bones.len();
    let frames = header.frame_count as usize + 1;
    println!("{name}: {frames} frames, {nb} animated bones written for the scene");
    // as Blender exports it: each bone's change from rest
    let delta = |g: &[f32]| -> Vec<f32> {
        let mut d = Vec::with_capacity(g.len());
        for f in 0..frames {
            for (k, b) in header.bones.iter().enumerate() {
                let at = (f * nb + k) * 16;
                let m: Mat4 = g[at..at + 16].try_into().unwrap();
                d.extend(math::mul(&m, &math::inverse_affine(&b.rest).unwrap()));
            }
        }
        d
    };
    let edits = write_edits(&dir, &header, &delta(&g));
    let unchanged = anim_edit::apply(&romfs, model, &edits).unwrap();
    println!(
        "unchanged: {} file(s) written, changes {:?}, warnings {:?}",
        unchanged.files.len(),
        unchanged.changes,
        unchanged.warnings
    );
    // one bone turned about its own x axis, its descendants carried along
    let clip = Clip::open(&romfs, model, file, name).unwrap();
    let k_bone = header
        .bones
        .iter()
        .position(|b| &b.name == bone)
        .expect("an animated bone");
    let mut descendants: Vec<usize> = Vec::new();
    for (k, b) in header.bones.iter().enumerate() {
        let mut p = Some(b.name.as_str());
        while let Some(n) = p {
            if n == bone {
                descendants.push(k);
                break;
            }
            p = clip.parent(n);
        }
    }
    let turn = math::from_trs(
        [0.0; 3],
        math::quat_from_euler_xyz([30f32.to_radians(), 0.0, 0.0]),
        [1.0; 3],
    );
    let mut g2 = g.clone();
    for f in 0..frames {
        let at = (f * nb + k_bone) * 16;
        let head: Mat4 = g[at..at + 16].try_into().unwrap();
        let world_turn = math::mul(
            &math::mul(&head, &turn),
            &math::inverse_affine(&head).unwrap(),
        );
        for &k in &descendants {
            let at = (f * nb + k) * 16;
            let m: Mat4 = g[at..at + 16].try_into().unwrap();
            g2[at..at + 16].copy_from_slice(&math::mul(&world_turn, &m));
        }
    }
    let edits = write_edits(&dir, &header, &delta(&g2));
    let turned = anim_edit::apply(&romfs, model, &edits).unwrap();
    println!(
        "{bone} turned ({} bones carried): {} file(s) written, changes {:?}, warnings {:?}",
        descendants.len(),
        turned.files.len(),
        turned.changes,
        turned.warnings
    );
    // the written file: the bone's rotation is the game's turned by 30 degrees at every frame
    let (path, bytes) = &turned.files[0];
    let mut d = romfs.decoder().unwrap();
    let raw = d.decompress(bytes).unwrap();
    let strings = modstudio_core::model::load_external_strings(&romfs, &mut d).unwrap();
    let written = bfres_anim::find(&raw, Some(&strings), name).unwrap();
    let original_data = romfs.read(&mut d, path).unwrap();
    let original = bfres_anim::find(&original_data, Some(&strings), name).unwrap();
    let rot = |a: &bfres_anim::SkeletalAnim, f: f32| -> [f32; 4] {
        let b = a.bones.iter().find(|b| &b.name == bone).unwrap();
        let base = b.base_rotation.unwrap();
        if b.quaternion() {
            b.curve(0x20).and_then(|c| c.quat_at(f)).unwrap_or(base)
        } else {
            math::quat_from_euler_xyz([
                b.curve(0x20).map_or(base[0], |c| c.value_at(f)),
                b.curve(0x24).map_or(base[1], |c| c.value_at(f)),
                b.curve(0x28).map_or(base[2], |c| c.value_at(f)),
            ])
        }
    };
    let mut worst = 0f32;
    for f in 0..frames {
        let want = math::quat_mul(
            rot(&original, f as f32),
            math::quat_from_euler_xyz([30f32.to_radians(), 0.0, 0.0]),
        );
        let got = rot(&written, f as f32);
        let dot = (0..4).map(|i| want[i] * got[i]).sum::<f32>().abs().min(1.0);
        worst = worst.max(2.0 * dot.acos().to_degrees());
    }
    let others_same = written
        .bones
        .iter()
        .zip(&original.bones)
        .filter(|(w, o)| &w.name != bone && w.curves.len() == o.curves.len())
        .count();
    println!(
        "written {path}: {bone}'s rotation against the game's turned 30 degrees: worst {worst:.3} degrees over {frames} frames; {others_same} of {} other bones keep their curve count",
        written.bones.len() - 1
    );
    std::fs::remove_dir_all(&dir).ok();
}
