//! A model's animations edited in Blender. Picked ones are written for the scene ([`write_for_blender`]: each
//! animated bone's place in the model at every frame, which `pipeline/anim_scene.py` turns into an action on the
//! armature, marked with the game's file and animation). The build reads the scene's marked actions back (the same
//! script, from `prep_for_game.py`: [`EDITS_FILE`]) and writes them into the game's animation files where they
//! differ ([`apply`]).
//!
//! What comes back is each bone's place in the model at every frame, as a change from its rest place (so the scene
//! may be at another size or orientation than the game's). Each bone's own translation, rotation and scale (relative
//! to its parent) are compared with the game's animation; a bone whose translation, rotation or scale moved gets
//! new curves for that (a key at every frame; Euler angles kept near the frame before's, so they don't jump a turn),
//! and keeps its other curves as they are. A bone the game's animation doesn't move isn't added to it: a warning
//! says so. The animation keeps its length.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use modstudio_formats::bfres_anim::{self, BoneAnim, target};
use modstudio_formats::bfres_anim_write::{AnimWriter, BoneEdit, CurveEdit};
use modstudio_formats::romfs::Romfs;
use serde::{Deserialize, Serialize};

use crate::anim::{Clip, SkelBone, load_skeleton, rest_world};
use crate::math::{self, Mat4, Quat};
use crate::{Context, Error, Result, model};

/// What the build reads back from the scene, in the work folder (with its floats in [`EDITS_DATA`]).
pub const EDITS_FILE: &str = "anim_edits.json";
pub const EDITS_DATA: &str = "anim_edits.bin";
/// Where the animations picked for the scene are written, in the project's folder.
pub const SCENE_DIR: &str = "anims";

/// A bone moved less than this (metres), turned less than this (degrees) or scaled less than this hasn't changed.
const MOVE: f32 = 1e-4;
const TURN: f32 = 0.02;
const STRETCH: f32 = 1e-4;

/// An animation as written for the scene: its bones' places in the model at every frame are in `data`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlenderAnim {
    /// The game's file and the animation in it.
    pub file: String,
    pub name: String,
    pub fps: f32,
    pub frame_count: i32,
    pub looping: bool,
    /// The bones it animates, each with its place in the model at rest (column-major, the game's space).
    pub bones: Vec<BlenderBone>,
    /// The floats beside this file: per frame (0 to `frame_count`), per bone, its place in the model (16 floats).
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlenderBone {
    pub name: String,
    pub rest: Mat4,
}

/// A file name for an animation: its file's stem and its name, letters, digits and `_` only.
fn stem_of(file: &str, name: &str) -> String {
    let base = file
        .rsplit('/')
        .next()
        .unwrap_or(file)
        .trim_end_matches(".anim.bfres.zs");
    format!("{base}__{name}")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Writes animation `name` of `file` (with the skeleton of `model`) for the scene, into `dir`: the header
/// (`<stem>.json`, returned) and its floats (`<stem>.bin`).
pub fn write_for_blender(
    romfs: &Romfs,
    model: &str,
    file: &str,
    name: &str,
    dir: &Path,
) -> Result<(PathBuf, BlenderAnim)> {
    let clip = Clip::open(romfs, model, file, name)?;
    let rest = rest_world(&clip.skeleton);
    let animated: Vec<usize> = (0..clip.skeleton.len())
        .filter(|&i| clip.track_of[i].is_some())
        .collect();
    std::fs::create_dir_all(dir).context(dir.display())?;
    let stem = stem_of(file, name);
    let mut floats = Vec::with_capacity((clip.frame_count as usize + 1) * animated.len() * 16 * 4);
    for f in 0..=clip.frame_count.max(0) {
        let pose = clip.pose(f as f32);
        for &i in &animated {
            for v in pose[i] {
                floats.extend_from_slice(&v.to_le_bytes());
            }
        }
    }
    let data = dir.join(format!("{stem}.bin"));
    std::fs::write(&data, floats).context(data.display())?;
    let header = BlenderAnim {
        file: file.to_owned(),
        name: name.to_owned(),
        fps: clip.fps,
        frame_count: clip.frame_count,
        looping: clip.looping,
        bones: animated
            .iter()
            .map(|&i| BlenderBone {
                name: clip.skeleton[i].name.clone(),
                rest: rest[i],
            })
            .collect(),
        data: format!("{stem}.bin"),
    };
    let path = dir.join(format!("{stem}.json"));
    let text = serde_json::to_string_pretty(&header).map_err(|e| Error(e.to_string()))?;
    std::fs::write(&path, text).context(path.display())?;
    Ok((path, header))
}

/// The scene's game animations as the build read them back ([`EDITS_FILE`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneEdits {
    /// The floats beside the file ([`EDITS_DATA`]).
    pub data: String,
    pub anims: Vec<SceneAnim>,
}

/// One of the scene's actions: the game animation it is, and where its bones' places start in the floats: per frame
/// (0 to `frame_count`), per bone of `bones`, the bone's change from its rest place in the model (16 floats,
/// column-major, the game's space).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneAnim {
    pub file: String,
    pub name: String,
    pub frame_count: i32,
    /// The action's name in the scene.
    #[serde(default)]
    pub action: String,
    pub bones: Vec<String>,
    pub offset: usize,
}

/// What [`apply`] made: the animation files written (compressed, by their path in the romfs), what changed, and
/// what was left out.
#[derive(Debug, Default)]
pub struct Applied {
    pub files: Vec<(String, Vec<u8>)>,
    pub changes: Vec<String>,
    pub warnings: Vec<String>,
}

/// A bone's own translation, rotation and scale at every frame.
struct Locals {
    t: Vec<[f32; 3]>,
    r: Vec<Quat>,
    s: Vec<[f32; 3]>,
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// The angle between two rotations (unit quaternions), in degrees: from the length of their difference (the nearer
/// of `b` and `-b`), which keeps small angles exact where the cosine of their dot product can't in f32.
fn degrees_between(a: Quat, b: Quat) -> f32 {
    let len = |s: f32| {
        (0..4)
            .map(|i| (a[i] - s * b[i]).powi(2))
            .sum::<f32>()
            .sqrt()
    };
    let d = len(1.0).min(len(-1.0));
    4.0 * (d / 2.0).min(1.0).asin().to_degrees()
}

/// The value of a sampled track at frame `f` (empty: the bone's rest value; one: held).
fn at<T: Copy>(v: &[T], f: usize, rest: T) -> T {
    match v.len() {
        0 => rest,
        n => v[f.min(n - 1)],
    }
}

/// The scene's bones' own places at every frame (`skeleton`'s order): from their places in the model (a change
/// from rest for the bones the scene gave; at rest relative to their parent for the others).
/// What placing the scene's bones in one frame reads: the skeleton, its rest places, which bones the scene gave and
/// where their floats are.
struct Frame<'a> {
    skeleton: &'a [SkelBone],
    rest: &'a [Mat4],
    given: &'a HashMap<&'a str, usize>,
    floats: &'a [f32],
    /// Where this frame's floats start.
    base: usize,
    frame: usize,
}

impl Frame<'_> {
    /// Bone `i`'s place in the model this frame (its parents' placed first, into `world`).
    fn place(&self, i: usize, world: &mut [Option<Mat4>], depth: usize) -> Result<Mat4> {
        if let Some(m) = world[i] {
            return Ok(m);
        }
        let b = &self.skeleton[i];
        let m = match self.given.get(b.name.as_str()) {
            Some(&k) => {
                let at = self.base + k * 16;
                let d: Mat4 = self
                    .floats
                    .get(at..at + 16)
                    .ok_or_else(|| {
                        Error(format!(
                            "the scene's animation data ends before frame {}",
                            self.frame
                        ))
                    })?
                    .try_into()
                    .expect("16 floats");
                math::mul(&d, &self.rest[i])
            }
            None => {
                let local = math::from_trs(b.translation, b.rotation, b.scale);
                match b.parent {
                    Some(p) if p != i && depth < 512 => {
                        math::mul(&self.place(p, world, depth + 1)?, &local)
                    }
                    _ => local,
                }
            }
        };
        world[i] = Some(m);
        Ok(m)
    }
}

/// The scene's bones' own places at every frame (`skeleton`'s order): from their places in the model (a change
/// from rest for the bones the scene gave; at rest relative to their parent for the others).
fn locals_from_scene(
    skeleton: &[SkelBone],
    rest: &[Mat4],
    scene: &SceneAnim,
    floats: &[f32],
    frames: usize,
) -> Result<Vec<Locals>> {
    let given: HashMap<&str, usize> = scene
        .bones
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i))
        .collect();
    let mut out: Vec<Locals> = skeleton
        .iter()
        .map(|_| Locals {
            t: Vec::with_capacity(frames),
            r: Vec::with_capacity(frames),
            s: Vec::with_capacity(frames),
        })
        .collect();
    let per_frame = scene.bones.len() * 16;
    for f in 0..frames {
        let frame = Frame {
            skeleton,
            rest,
            given: &given,
            floats,
            base: scene.offset + f * per_frame,
            frame: f,
        };
        let mut world: Vec<Option<Mat4>> = vec![None; skeleton.len()];
        for i in 0..skeleton.len() {
            frame.place(i, &mut world, 0)?;
        }
        for (i, b) in skeleton.iter().enumerate() {
            let me = world[i].expect("placed");
            let local = match b.parent {
                Some(p) if p != i => math::mul(
                    &math::inverse_affine(&world[p].expect("placed")).ok_or_else(|| {
                        Error(format!(
                            "bone {}: its parent can't be undone (scaled to 0)",
                            b.name
                        ))
                    })?,
                    &me,
                ),
                _ => me,
            };
            let (t, r, s) = math::decompose(&local);
            out[i].t.push(t);
            out[i].r.push(r);
            out[i].s.push(s);
        }
    }
    Ok(out)
}

/// Whether a component's values over the frames hold still.
fn constant<const N: usize>(v: &[[f32; N]]) -> bool {
    v.iter()
        .all(|x| (0..N).all(|k| (x[k] - v[0][k]).abs() <= 1e-6))
}

/// The new [`BoneEdit`] for `bone` (skeleton bone `skel`): the parts that changed from `locals`, the rest kept.
fn bone_edit(
    bone: &BoneAnim,
    skel: &SkelBone,
    locals: &Locals,
    changed: [bool; 3],
    frames: usize,
) -> BoneEdit {
    let [moved, turned, stretched] = changed;
    let quaternion = bone.quaternion();
    let mut base_scale = bone.base_scale.unwrap_or(skel.scale);
    let mut base_rotation = bone.base_rotation.unwrap_or(if quaternion {
        skel.rotation
    } else {
        [skel.euler[0], skel.euler[1], skel.euler[2], 1.0]
    });
    let mut base_translation = bone.base_translation.unwrap_or(skel.translation);
    let mut curves = Vec::new();
    for (i, c) in bone.curves.iter().enumerate() {
        let keep = match c.target {
            0x04..=0x0C => !stretched,
            0x10..=0x18 => !moved,
            _ => !turned,
        };
        if keep {
            curves.push(CurveEdit::Keep(i));
        }
    }
    // a vector component per axis: held (its base value) or a curve
    let axes =
        |values: &[[f32; 3]], first: u32, base: &mut [f32; 3], curves: &mut Vec<CurveEdit>| {
            for k in 0..3 {
                base[k] = values[0][k];
                let one: Vec<[f32; 1]> = values.iter().map(|v| [v[k]]).collect();
                if !constant(&one) {
                    curves.push(CurveEdit::Scalar {
                        target: first + 4 * k as u32,
                        values: values.iter().map(|v| v[k]).collect(),
                    });
                }
            }
        };
    if moved {
        axes(
            &locals.t,
            target::TRANSLATE_X,
            &mut base_translation,
            &mut curves,
        );
    }
    if stretched {
        axes(&locals.s, target::SCALE_X, &mut base_scale, &mut curves);
    }
    if turned {
        if quaternion {
            let mut q = locals.r.clone();
            for k in 1..q.len() {
                let (p, c) = (q[k - 1], q[k]);
                if p[0] * c[0] + p[1] * c[1] + p[2] * c[2] + p[3] * c[3] < 0.0 {
                    q[k] = [-c[0], -c[1], -c[2], -c[3]];
                }
            }
            base_rotation = q[0];
            if q.iter().any(|x| degrees_between(*x, q[0]) > 1e-3) {
                curves.push(CurveEdit::Quaternion { values: q });
            }
        } else {
            // the angles near the game's own at the first frame, then near the frame before
            let first = [
                bone.curve(target::ROTATE_X)
                    .map_or(base_rotation[0], |c| c.value_at(0.0)),
                bone.curve(target::ROTATE_Y)
                    .map_or(base_rotation[1], |c| c.value_at(0.0)),
                bone.curve(target::ROTATE_Z)
                    .map_or(base_rotation[2], |c| c.value_at(0.0)),
            ];
            let mut near = first;
            let angles: Vec<[f32; 3]> = locals
                .r
                .iter()
                .take(frames)
                .map(|q| {
                    near = math::euler_xyz_near(*q, near);
                    near
                })
                .collect();
            let mut base3 = [base_rotation[0], base_rotation[1], base_rotation[2]];
            axes(&angles, target::ROTATE_X, &mut base3, &mut curves);
            base_rotation = [base3[0], base3[1], base3[2], 1.0];
        }
    }
    BoneEdit {
        base_scale,
        base_rotation,
        base_translation,
        curves,
    }
}

/// The scene's game animations ([`EDITS_FILE`] at `edits`, with the skeleton of `model`) written into the game's
/// animation files (read through `romfs`) where they differ: each file with at least one change, compressed.
pub fn apply(romfs: &Romfs, model: &str, edits: &Path) -> Result<Applied> {
    let text = std::fs::read_to_string(edits).context(edits.display())?;
    let scene: SceneEdits = serde_json::from_str(&text).context(edits.display())?;
    let data_path = edits.with_file_name(&scene.data);
    let raw = std::fs::read(&data_path).context(data_path.display())?;
    let floats: Vec<f32> = raw
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect();
    let mut decoder = romfs.decoder()?;
    let strings = model::load_external_strings(romfs, &mut decoder)?;
    let skeleton = load_skeleton(romfs, &mut decoder, &strings, model)?;
    let rest = rest_world(&skeleton);
    let index_of: HashMap<&str, usize> = skeleton
        .iter()
        .enumerate()
        .map(|(i, b)| (b.name.as_str(), i))
        .collect();
    let mut by_file: BTreeMap<&str, Vec<&SceneAnim>> = BTreeMap::new();
    for a in &scene.anims {
        by_file.entry(a.file.as_str()).or_default().push(a);
    }
    let mut applied = Applied::default();
    for (file, anims) in by_file {
        let data = romfs.read(&mut decoder, file).context(file)?;
        let parsed = bfres_anim::parse(&data, Some(&strings)).context(file)?;
        let mut writer = AnimWriter::new(&data).context(file)?;
        let mut any = false;
        for sa in anims {
            let label = if sa.action.is_empty() || sa.action == sa.name {
                sa.name.clone()
            } else {
                format!("{} (action {})", sa.name, sa.action)
            };
            let Some(anim) = parsed.anims.iter().find(|a| a.name == sa.name) else {
                applied.warnings.push(format!(
                    "{label}: {file} has no animation of that name; left out"
                ));
                continue;
            };
            if sa.frame_count != anim.frame_count {
                applied.warnings.push(format!(
                    "{label}: the scene has {} frames and the game's animation {}; its first {} are used",
                    sa.frame_count + 1,
                    anim.frame_count + 1,
                    anim.frame_count.min(sa.frame_count) + 1
                ));
            }
            let frames = (anim.frame_count.min(sa.frame_count).max(0) + 1) as usize;
            let clip = Clip::sample(file, anim, skeleton.clone());
            let locals = locals_from_scene(&skeleton, &rest, sa, &floats, frames)?;
            let mut changed_bones = Vec::new();
            for (bi, bone) in anim.bones.iter().enumerate() {
                let Some(&si) = index_of.get(bone.name.as_str()) else {
                    continue;
                };
                let skel = &skeleton[si];
                let track = clip.track_of[si].map(|t| &clip.tracks[t]);
                let empty3: &[[f32; 3]] = &[];
                let empty4: &[Quat] = &[];
                let (ot, or, os) = match track {
                    Some(t) => (
                        t.translation.as_slice(),
                        t.rotation.as_slice(),
                        t.scale.as_slice(),
                    ),
                    None => (empty3, empty4, empty3),
                };
                let l = &locals[si];
                let moved =
                    (0..frames).any(|f| distance(l.t[f], at(ot, f, skel.translation)) > MOVE);
                let turned =
                    (0..frames).any(|f| degrees_between(l.r[f], at(or, f, skel.rotation)) > TURN);
                let stretched =
                    (0..frames).any(|f| distance(l.s[f], at(os, f, skel.scale)) > STRETCH);
                if !(moved || turned || stretched) {
                    continue;
                }
                // the scene's values over this animation's frames
                let trimmed = Locals {
                    t: l.t[..frames].to_vec(),
                    r: l.r[..frames].to_vec(),
                    s: l.s[..frames].to_vec(),
                };
                let mut edit = bone_edit(bone, skel, &trimmed, [moved, turned, stretched], frames);
                // a longer scene than the game's animation is cut to it; a shorter one holds its last frame
                let n = anim.frame_count.max(0) as usize + 1;
                for c in &mut edit.curves {
                    match c {
                        CurveEdit::Scalar { values, .. } => {
                            let last = *values.last().expect("a value");
                            values.resize(n, last);
                        }
                        CurveEdit::Quaternion { values } => {
                            let last = *values.last().expect("a value");
                            values.resize(n, last);
                        }
                        CurveEdit::Keep(_) => {}
                    }
                }
                writer.edit_bone(&data, anim, bi, &edit).context(file)?;
                changed_bones.push(bone.name.clone());
            }
            // bones the scene moves that the game's animation doesn't
            let animated: std::collections::HashSet<&str> =
                anim.bones.iter().map(|b| b.name.as_str()).collect();
            let left: Vec<&str> = skeleton
                .iter()
                .enumerate()
                .filter(|(_, b)| !animated.contains(b.name.as_str()))
                .filter(|(i, b)| {
                    let l = &locals[*i];
                    (0..frames).any(|f| {
                        distance(l.t[f], b.translation) > MOVE
                            || degrees_between(l.r[f], b.rotation) > TURN
                            || distance(l.s[f], b.scale) > STRETCH
                    })
                })
                .map(|(_, b)| b.name.as_str())
                .collect();
            if !left.is_empty() {
                applied.warnings.push(format!(
                    "{label}: {} bone(s) move in the scene that the game's animation doesn't animate, so their motion is left out: {}{}",
                    left.len(),
                    left.iter().take(6).copied().collect::<Vec<_>>().join(", "),
                    if left.len() > 6 { ", ..." } else { "" }
                ));
            }
            if !changed_bones.is_empty() {
                any = true;
                applied.changes.push(format!(
                    "{label}: {} bone(s) changed: {}{}",
                    changed_bones.len(),
                    changed_bones
                        .iter()
                        .take(8)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", "),
                    if changed_bones.len() > 8 { ", ..." } else { "" }
                ));
            }
        }
        if any {
            let bytes = writer.finish(&parsed.anims).context(file)?;
            // what was written reads back
            bfres_anim::parse(&bytes, Some(&strings)).map_err(|e| {
                Error(format!(
                    "{file}: the written animation doesn't read back: {e}"
                ))
            })?;
            modstudio_formats::bfres_anim_write::check_relocations(&bytes)
                .map_err(|e| Error(format!("{file}: the written animation: {e}")))?;
            applied
                .files
                .push((file.to_owned(), modstudio_formats::zs::compress(&bytes)));
        }
    }
    Ok(applied)
}

/// Adds the game animations `picks` (its file and its name, each) of the project's model to its scene, as actions
/// (`anim_scene.py`, Blender in the background; Blender keeps the scene before it as `<scene>.blend1`). The scene
/// mustn't be open in Blender: saving it there would drop them again.
pub fn add_to_scene(
    settings: &crate::settings::Settings,
    project: &mut crate::project::Project,
    picks: &[(String, String)],
    progress: &mut crate::progress::Progress,
) -> Result<Vec<BlenderAnim>> {
    let blender = settings.blender()?;
    let romfs = settings.romfs()?;
    progress.step("The animations");
    let dir = project.dir().join(SCENE_DIR);
    let mut headers = Vec::new();
    let mut written = Vec::new();
    for (file, name) in picks {
        let (path, header) = write_for_blender(&romfs, &project.data.model, file, name, &dir)?;
        progress.line(format!(
            "{name} ({}): {} frames, {} bones",
            file.trim_start_matches("Model/"),
            header.frame_count + 1,
            header.bones.len()
        ));
        headers.push(path);
        written.push(header);
        progress.check()?;
    }
    progress.step("Adding them to the scene");
    let scripts = crate::tools::scripts_dir(&settings.cache_dir())?;
    let armature = project
        .data
        .prep
        .get("armature")
        .and_then(|a| a.as_str())
        .unwrap_or("Armature")
        .to_owned();
    let logs = project.work_dir();
    std::fs::create_dir_all(&logs).context(logs.display())?;
    let mut args = crate::args![
        "--background",
        project.blend(),
        "--python",
        scripts.join("anim_scene.py"),
        "--",
        "--armature",
        &armature,
        "--add"
    ]
    .to_vec();
    args.extend(headers.iter().map(|h| h.as_os_str().to_owned()));
    crate::tools::run(
        blender,
        &args,
        &logs.join("add_anims.log"),
        &|l| l.starts_with("[anim]") || l.contains("Error"),
        progress,
    )?;
    for (file, name) in picks {
        let key = format!("{file}:{name}");
        if !project.data.scene_anims.contains(&key) {
            project.data.scene_anims.push(key);
        }
    }
    project.save()?;
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_for_animations() {
        assert_eq!(
            stem_of("Model/Player_Child_04_Animation.anim.bfres.zs", "Wait"),
            "Player_Child_04_Animation__Wait"
        );
        assert_eq!(
            stem_of("Model/Dm_X.anim.bfres.zs", "C01-Npc_A-A-0 Loop"),
            "Dm_X__C01_Npc_A_A_0_Loop"
        );
    }

    #[test]
    fn angles_between_rotations_small_and_large() {
        let q = math::quat_from_euler_xyz([0.3, 0.2, 0.1]);
        assert_eq!(degrees_between(q, q), 0.0);
        assert_eq!(degrees_between(q, [-q[0], -q[1], -q[2], -q[3]]), 0.0);
        let turned = math::quat_mul(
            q,
            math::quat_from_euler_xyz([0.001f32.to_radians(), 0.0, 0.0]),
        );
        assert!(
            (degrees_between(q, turned) - 0.001).abs() < 2e-4,
            "{}",
            degrees_between(q, turned)
        );
        let half = math::quat_from_euler_xyz([std::f32::consts::PI, 0.0, 0.0]);
        assert!((degrees_between([0.0, 0.0, 0.0, 1.0], half) - 180.0).abs() < 1e-3);
    }

    #[test]
    fn held_values_and_curves() {
        assert!(constant(&[[1.0, 2.0, 3.0], [1.0, 2.0, 3.0]]));
        assert!(!constant(&[[1.0, 2.0, 3.0], [1.0, 2.1, 3.0]]));
        assert_eq!(at(&[[1.0f32]], 5, [0.0]), [1.0]);
        assert_eq!(at::<[f32; 1]>(&[], 5, [9.0]), [9.0]);
    }
}
