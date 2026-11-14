//! A model's animations, for the app's 3D view: which files hold them ([`anim_files`]: the ones its actors give it,
//! its own, its cutscenes), what's in them ([`list`]), and one read with the model's skeleton and sampled frame by
//! frame ([`Clip`]), to play, and for the cloth to follow ([`Clip::pose`]).
//!
//! Animations bind to bones by name (the files leave every bind index unset), and hold each bone's own place
//! relative to its parent: translation, rotation (Euler angles XYZ, or a quaternion where the bone's flags say so)
//! and scale, per frame at [`FPS`]. A bone an animation leaves out keeps its rest place.

use std::collections::HashMap;
use std::path::Path;

use modstudio_formats::bfres::Bfres;
use modstudio_formats::bfres_anim::{self, BoneAnim, SkeletalAnim, target};
use modstudio_formats::romfs::Romfs;
use serde::Serialize;

use crate::math::{self, Mat4, Quat};
use crate::{Context, Error, Result, actor_links, catalog, model};

/// Frames a second the game's animations are made at.
pub const FPS: f32 = 30.0;

/// A file of a model's animations, and why it's the model's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AnimSource {
    /// `Model/<name>.anim.bfres.zs`.
    pub file: String,
    /// `actor` (an actor using the model gives it), `own` (named after the model), `cutscene` (`Dm_<scene>_<model>`).
    pub kind: &'static str,
}

/// Whether `stem` (a file name without `.anim.bfres.zs`) is a cutscene of `model`: `Dm_<scene>_<model>`, or a
/// variant of it (`_A`, `_01`, `_Anim` after the model).
fn is_cutscene_of(stem: &str, model: &str) -> bool {
    if !stem.starts_with("Dm_") {
        return false;
    }
    let tail = format!("_{model}");
    if stem.ends_with(&tail) {
        return true;
    }
    let Some(i) = stem.rfind(&format!("{tail}_")) else {
        return false;
    };
    let rest = &stem[i + tail.len() + 1..];
    rest == "Anim"
        || (rest.len() == 1 && rest.chars().all(|c| c.is_ascii_uppercase()))
        || (rest.len() == 2 && rest.chars().all(|c| c.is_ascii_digit()))
}

/// The files holding `model`'s animations (`Model/<group>.<model>.bfres.zs`): the ones its actors give it (the
/// player's bundles), its own (named after its group or itself), and its cutscenes, in that order.
pub fn anim_files(game_romfs: &Path, cache_dir: Option<&Path>, model: &str) -> Vec<AnimSource> {
    let mut out: Vec<AnimSource> = Vec::new();
    let mut push = |file: String, kind| {
        if !out.iter().any(|s| s.file == file) {
            out.push(AnimSource { file, kind });
        }
    };
    if let Ok(links) = actor_links::links(game_romfs, cache_dir) {
        for f in links.anims_of(model) {
            push(f.to_owned(), "actor");
        }
    }
    let (group, name) = catalog::split_name(model);
    for n in [group, name] {
        let own = format!("Model/{n}.anim.bfres.zs");
        if game_romfs.join(&own).is_file() {
            push(own, "own");
        }
    }
    let mut cutscenes: Vec<String> = std::fs::read_dir(game_romfs.join("Model"))
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| {
                    let file = e.file_name().to_string_lossy().into_owned();
                    let stem = file.strip_suffix(".anim.bfres.zs")?;
                    [group, name]
                        .iter()
                        .any(|n| is_cutscene_of(stem, n))
                        .then(|| format!("Model/{file}"))
                })
                .collect()
        })
        .unwrap_or_default();
    cutscenes.sort();
    for c in cutscenes {
        push(c, "cutscene");
    }
    out
}

/// One animation of a model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AnimEntry {
    pub file: String,
    /// As [`AnimSource::kind`].
    pub kind: &'static str,
    pub name: String,
    /// Its length in frames (at [`FPS`]).
    pub frames: i32,
    pub looping: bool,
}

/// Every animation of `model` (a model's romfs path), file by file as [`anim_files`] orders them, read through
/// `romfs` (a mod's own file where an overlay has one). A file that doesn't read is left out.
pub fn list(romfs: &Romfs, cache_dir: Option<&Path>, model: &str) -> Result<Vec<AnimEntry>> {
    let sources = anim_files(romfs.root(), cache_dir, model);
    if sources.is_empty() {
        return Ok(Vec::new());
    }
    let mut decoder = romfs.decoder()?;
    let strings = model::load_external_strings(romfs, &mut decoder)?;
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 8);
    let chunk = sources.len().div_ceil(threads).max(1);
    let found: Vec<Vec<AnimEntry>> = std::thread::scope(|s| {
        let handles: Vec<_> = sources
            .chunks(chunk)
            .map(|part| {
                let strings = &strings;
                s.spawn(move || {
                    let Ok(mut decoder) = romfs.decoder() else {
                        return Vec::new();
                    };
                    let mut out = Vec::new();
                    for src in part {
                        let Ok(data) = romfs.read(&mut decoder, &src.file) else {
                            continue;
                        };
                        let Ok(anims) = bfres_anim::list(&data, Some(strings)) else {
                            continue;
                        };
                        out.extend(anims.into_iter().map(|a| AnimEntry {
                            file: src.file.clone(),
                            kind: src.kind,
                            name: a.name,
                            frames: a.frame_count,
                            looping: a.looping,
                        }));
                    }
                    out
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_default())
            .collect()
    });
    Ok(found.into_iter().flatten().collect())
}

/// A bone of the model's skeleton, at rest relative to its parent.
#[derive(Debug, Clone)]
struct SkelBone {
    name: String,
    parent: Option<usize>,
    translation: [f32; 3],
    rotation: Quat,
    /// The rotation as Euler angles (XYZ), where the skeleton keeps them so.
    euler: [f32; 3],
    scale: [f32; 3],
}

/// What an animation does to one bone, sampled at every frame: each component is empty (the bone's rest value),
/// one value (held) or one per frame.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Track {
    pub bone: String,
    pub translation: Vec<[f32; 3]>,
    /// Quaternions (x, y, z, w), each in the same hemisphere as the one before.
    pub rotation: Vec<Quat>,
    pub scale: Vec<[f32; 3]>,
}

/// An animation read with the model's skeleton, sampled at every frame.
#[derive(Debug, Clone)]
pub struct Clip {
    pub file: String,
    pub name: String,
    pub fps: f32,
    /// Its length in frames: samples are taken at 0, 1, ... `frame_count`.
    pub frame_count: i32,
    pub looping: bool,
    pub tracks: Vec<Track>,
    /// Bones the animation moves that the model doesn't have.
    pub unmatched: Vec<String>,
    /// Where the animation carries the whole model (a run moving forward), if it does.
    pub motion: Option<Motion>,
    skeleton: Vec<SkelBone>,
    /// Per bone of the skeleton, its track.
    track_of: Vec<Option<usize>>,
}

/// How an animation carries the whole model: the bone that moves it (a root of the skeleton, or a root's child,
/// whose place over the ground changes from the first frame to the last) and how far (x and z, in the model's
/// space; y is left to the animation). A viewer shows the animation in place by taking the share of the travel up
/// to each frame off that bone.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Motion {
    pub bone: String,
    pub travel: [f32; 3],
}

/// The bone carrying the model, if one goes more than 5 cm over the ground from the first frame to the last.
fn motion_of(skeleton: &[SkelBone], tracks: &[Track]) -> Option<Motion> {
    let is_root = |name: &str| {
        skeleton
            .iter()
            .find(|b| b.name == name)
            .is_some_and(|b| match b.parent {
                None => true,
                Some(p) => skeleton[p].parent.is_none(),
            })
    };
    tracks
        .iter()
        .filter(|t| t.translation.len() > 1 && is_root(&t.bone))
        .map(|t| {
            let (a, b) = (t.translation[0], t.translation[t.translation.len() - 1]);
            (t, [b[0] - a[0], 0.0, b[2] - a[2]])
        })
        .filter(|(_, d)| (d[0] * d[0] + d[2] * d[2]).sqrt() > 0.05)
        .max_by(|x, y| {
            let len = |d: &[f32; 3]| d[0] * d[0] + d[2] * d[2];
            len(&x.1).total_cmp(&len(&y.1))
        })
        .map(|(t, travel)| Motion {
            bone: t.bone.clone(),
            travel,
        })
}

/// The header of [`Clip::to_bytes`].
#[derive(Serialize)]
struct ClipHeader<'a> {
    id: u64,
    file: &'a str,
    name: &'a str,
    fps: f32,
    frame_count: i32,
    looping: bool,
    samples: usize,
    unmatched: &'a [String],
    motion: &'a Option<Motion>,
    tracks: Vec<TrackHeader<'a>>,
}

/// Where a track's components are in the floats of [`Clip::to_bytes`]: an offset and a count of values.
#[derive(Serialize)]
struct TrackHeader<'a> {
    bone: &'a str,
    t: [usize; 2],
    r: [usize; 2],
    s: [usize; 2],
}

/// The value of one component of `bone` at `frame`: its curve's, else its base value, else `rest`.
fn component(bone: &BoneAnim, target: u32, base: Option<f32>, rest: f32, frame: f32) -> f32 {
    match bone.curve(target) {
        Some(c) => c.value_at(frame),
        None => base.unwrap_or(rest),
    }
}

/// Samples of `n` components of `bone` (targets `targets`, base `base`, rest `rest`) at every frame: none if the
/// animation has neither curves nor a base value for them, one if it has no curve.
fn sampled<const N: usize>(
    bone: &BoneAnim,
    targets: [u32; N],
    base: Option<[f32; N]>,
    rest: [f32; N],
    frames: i32,
) -> Vec<[f32; N]> {
    let curved = targets.iter().any(|&t| bone.curve(t).is_some());
    if !curved {
        return base.map(|b| vec![b]).unwrap_or_default();
    }
    (0..=frames.max(0))
        .map(|f| {
            std::array::from_fn(|i| {
                component(bone, targets[i], base.map(|b| b[i]), rest[i], f as f32)
            })
        })
        .collect()
}

impl Clip {
    /// Reads animation `name` of `file` (a romfs path) and the skeleton of `model`, through `romfs`.
    pub fn open(romfs: &Romfs, model: &str, file: &str, name: &str) -> Result<Clip> {
        let mut decoder = romfs.decoder()?;
        let strings = model::load_external_strings(romfs, &mut decoder)?;
        let model_data = romfs.read(&mut decoder, model).context(model)?;
        let bfres = Bfres::parse(&model_data, Some(&strings)).context(model)?;
        let skel = &bfres
            .models
            .first()
            .ok_or_else(|| Error(format!("{model}: no model in the file")))?
            .skeleton;
        let skeleton: Vec<SkelBone> = skel
            .bones
            .iter()
            .map(|b| SkelBone {
                name: b.name.clone(),
                parent: b.parent,
                translation: b.translation,
                rotation: if skel.euler_rotations() {
                    math::quat_from_euler_xyz([b.rotation[0], b.rotation[1], b.rotation[2]])
                } else {
                    b.rotation
                },
                euler: if skel.euler_rotations() {
                    [b.rotation[0], b.rotation[1], b.rotation[2]]
                } else {
                    [0.0; 3]
                },
                scale: b.scale,
            })
            .collect();
        let anim_data = romfs.read(&mut decoder, file).context(file)?;
        let anim = bfres_anim::find(&anim_data, Some(&strings), name).context(file)?;
        Ok(Self::sample(file, &anim, skeleton))
    }

    fn sample(file: &str, anim: &SkeletalAnim, skeleton: Vec<SkelBone>) -> Clip {
        let by_name: HashMap<&str, usize> = skeleton
            .iter()
            .enumerate()
            .map(|(i, b)| (b.name.as_str(), i))
            .collect();
        let frames = anim.frame_count.max(0);
        let mut tracks = Vec::new();
        let mut track_of = vec![None; skeleton.len()];
        let mut unmatched = Vec::new();
        for b in &anim.bones {
            let Some(&i) = by_name.get(b.name.as_str()) else {
                unmatched.push(b.name.clone());
                continue;
            };
            let rest = &skeleton[i];
            let translation = sampled(
                b,
                [
                    target::TRANSLATE_X,
                    target::TRANSLATE_Y,
                    target::TRANSLATE_Z,
                ],
                b.base_translation,
                rest.translation,
                frames,
            );
            let scale = sampled(
                b,
                [target::SCALE_X, target::SCALE_Y, target::SCALE_Z],
                b.base_scale,
                rest.scale,
                frames,
            );
            // a bone's rotation is a quaternion (its base, and one curve of whole quaternions) where its flags say
            // so, else Euler angles (a curve per angle)
            let mut rotation: Vec<Quat> = if b.quaternion() {
                match b
                    .curve(target::ROTATE_X)
                    .filter(|c| !c.quaternions.is_empty())
                {
                    Some(c) => (0..=frames)
                        .filter_map(|f| c.quat_at(f as f32))
                        .map(math::normalize_quat)
                        .collect(),
                    None => b
                        .base_rotation
                        .map(|q| vec![math::normalize_quat(q)])
                        .unwrap_or_default(),
                }
            } else {
                // an angle without a curve or a base value is the skeleton's own
                let base = b.base_rotation.map(|r| [r[0], r[1], r[2]]);
                sampled(
                    b,
                    [target::ROTATE_X, target::ROTATE_Y, target::ROTATE_Z],
                    base,
                    rest.euler,
                    frames,
                )
                .into_iter()
                .map(math::quat_from_euler_xyz)
                .collect()
            };
            // each in the same hemisphere as the one before, so that blending between two takes the short way
            for k in 1..rotation.len() {
                let (p, q) = (rotation[k - 1], rotation[k]);
                if p[0] * q[0] + p[1] * q[1] + p[2] * q[2] + p[3] * q[3] < 0.0 {
                    rotation[k] = [-q[0], -q[1], -q[2], -q[3]];
                }
            }
            if translation.is_empty() && rotation.is_empty() && scale.is_empty() {
                continue;
            }
            track_of[i] = Some(tracks.len());
            tracks.push(Track {
                bone: b.name.clone(),
                translation,
                rotation,
                scale,
            });
        }
        let motion = motion_of(&skeleton, &tracks);
        Clip {
            file: file.to_owned(),
            name: anim.name.clone(),
            fps: FPS,
            frame_count: frames,
            looping: anim.looping(),
            tracks,
            unmatched,
            motion,
            skeleton,
            track_of,
        }
    }

    /// Each bone of the model's skeleton (in its order) at `frame` (clamped to the animation; between two frames,
    /// blended), in the model's space.
    pub fn pose(&self, frame: f32) -> Vec<Mat4> {
        let f = frame.clamp(0.0, self.frame_count as f32);
        let (k, u) = (f.floor() as usize, f.fract());
        fn at<const N: usize>(v: &[[f32; N]], k: usize, u: f32) -> Option<[f32; N]> {
            let a = *v.get(k.min(v.len().checked_sub(1)?))?;
            let b = *v.get((k + 1).min(v.len() - 1))?;
            Some(std::array::from_fn(|i| a[i] + (b[i] - a[i]) * u))
        }
        let mut world: Vec<Option<Mat4>> = vec![None; self.skeleton.len()];
        fn place(
            clip: &Clip,
            i: usize,
            k: usize,
            u: f32,
            world: &mut Vec<Option<Mat4>>,
            depth: usize,
        ) -> Mat4 {
            if let Some(m) = world[i] {
                return m;
            }
            let b = &clip.skeleton[i];
            let track = clip.track_of[i].map(|t| &clip.tracks[t]);
            let t = track
                .and_then(|t| at(&t.translation, k, u))
                .unwrap_or(b.translation);
            let r = track
                .and_then(|t| at(&t.rotation, k, u))
                .map(math::normalize_quat)
                .unwrap_or(b.rotation);
            let s = track.and_then(|t| at(&t.scale, k, u)).unwrap_or(b.scale);
            let local = math::from_trs(t, r, s);
            let m = match b.parent {
                Some(p) if p != i && depth < 512 => {
                    math::mul(&place(clip, p, k, u, world, depth + 1), &local)
                }
                _ => local,
            };
            world[i] = Some(m);
            m
        }
        (0..self.skeleton.len())
            .map(|i| place(self, i, k, u, &mut world, 0))
            .collect()
    }

    /// The parent of the model's bone `name`, by name.
    pub fn parent(&self, name: &str) -> Option<&str> {
        let b = self.skeleton.iter().find(|b| b.name == name)?;
        b.parent.map(|p| self.skeleton[p].name.as_str())
    }

    /// The model's skeleton at rest, in the model's space, by bone name.
    pub fn rest_pose_by_name(&self) -> HashMap<&str, Mat4> {
        let still = Clip {
            tracks: Vec::new(),
            track_of: vec![None; self.skeleton.len()],
            ..self.clone()
        };
        let pose = still.pose(0.0);
        self.skeleton
            .iter()
            .map(|b| b.name.as_str())
            .zip(pose)
            .collect()
    }

    /// [`Clip::pose`] by bone name.
    pub fn pose_by_name(&self, frame: f32) -> HashMap<&str, Mat4> {
        self.skeleton
            .iter()
            .map(|b| b.name.as_str())
            .zip(self.pose(frame))
            .collect()
    }

    /// For the app (`id`: what the app calls the clip): a little-endian u32 (the header's length), the header (JSON:
    /// the clip and, per track, where its components are), zeros to a multiple of 4 bytes, then the samples as f32.
    pub fn to_bytes(&self, id: u64) -> Vec<u8> {
        let mut floats: Vec<f32> = Vec::new();
        let mut put = |values: &[f32], n: usize| -> [usize; 2] {
            let at = floats.len();
            floats.extend_from_slice(values);
            [at, values.len() / n.max(1)]
        };
        let tracks: Vec<TrackHeader> = self
            .tracks
            .iter()
            .map(|t| TrackHeader {
                bone: &t.bone,
                t: put(t.translation.as_flattened(), 3),
                r: put(t.rotation.as_flattened(), 4),
                s: put(t.scale.as_flattened(), 3),
            })
            .collect();
        let header = ClipHeader {
            id,
            file: &self.file,
            name: &self.name,
            fps: self.fps,
            frame_count: self.frame_count,
            looping: self.looping,
            samples: self.frame_count as usize + 1,
            unmatched: &self.unmatched,
            motion: &self.motion,
            tracks,
        };
        let json = serde_json::to_vec(&header).expect("a clip header is JSON");
        let mut out = Vec::with_capacity(4 + json.len() + 4 + floats.len() * 4);
        out.extend_from_slice(&(json.len() as u32).to_le_bytes());
        out.extend_from_slice(&json);
        while out.len() % 4 != 0 {
            out.push(0);
        }
        for f in floats {
            out.extend_from_slice(&f.to_le_bytes());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cutscenes_of_a_model_by_name() {
        assert!(is_cutscene_of(
            "Dm_Kokiri_0140_Player_Child",
            "Player_Child"
        ));
        assert!(is_cutscene_of(
            "Dm_DeathMtTrail_0030_Npc_Goron_GON_001_A",
            "Npc_Goron_GON_001"
        ));
        assert!(is_cutscene_of(
            "Dm_HrlField_0060_Animal_Horse_Zelda_Anim",
            "Animal_Horse_Zelda"
        ));
        // another model whose name starts with this one's
        assert!(!is_cutscene_of(
            "Dm_Shadow_0010_Player_Adult_Dark",
            "Player_Adult"
        ));
        assert!(!is_cutscene_of("Player_Child_01_Animation", "Player_Child"));
        assert!(!is_cutscene_of("Dm_X_Npc_A_Long", "Npc_A"));
    }

    /// A two-bone skeleton (`Root`, `Arm` one unit along x) and an animation that turns `Root` a quarter turn
    /// about z over 10 frames and leaves `Arm`, and moves `Ghost`, which the skeleton hasn't.
    fn clip() -> Clip {
        use modstudio_formats::bfres_anim::Curve;
        let skeleton = vec![
            SkelBone {
                name: "Root".into(),
                parent: None,
                translation: [0.0; 3],
                rotation: [0.0, 0.0, 0.0, 1.0],
                euler: [0.0; 3],
                scale: [1.0; 3],
            },
            SkelBone {
                name: "Arm".into(),
                parent: Some(0),
                translation: [1.0, 0.0, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                euler: [0.0; 3],
                scale: [1.0; 3],
            },
        ];
        let quarter = std::f32::consts::FRAC_PI_2;
        let curve = Curve {
            at: 0,
            flags: 0x4000 | modstudio_formats::bfres_anim::curve_flags::LINEAR,
            target: target::ROTATE_Z,
            start_frame: 0.0,
            end_frame: 10.0,
            scale: 0.0,
            offset: 0.0,
            delta: 0.0,
            frames: vec![0.0, 10.0],
            segments: vec![[0.0, quarter, 0.0, 0.0], [quarter, 0.0, 0.0, 0.0]],
            quaternions: vec![],
        };
        let bone = |name: &str, curves: Vec<Curve>| BoneAnim {
            at: 0,
            name: name.into(),
            flags: 0x38,
            begin_rotate: 0,
            begin_translate: 0,
            begin_base_translate: 0,
            begin_curve: 0,
            base_scale: Some([1.0; 3]),
            base_rotation: Some([0.0; 4]),
            base_translation: Some([0.0; 3]),
            curves,
        };
        let anim = SkeletalAnim {
            at: 0,
            name: "Turn".into(),
            path: String::new(),
            flags: modstudio_formats::bfres_anim::anim_flags::EULER,
            frame_count: 10,
            curve_count: 1,
            baked_size: 0,
            bind_indices: vec![],
            bones: vec![bone("Root", vec![curve]), bone("Ghost", vec![])],
        };
        Clip::sample("Model/X.anim.bfres.zs", &anim, skeleton)
    }

    #[test]
    fn a_clip_poses_children_through_their_animated_parents() {
        let c = clip();
        assert_eq!(c.unmatched, ["Ghost"]);
        assert_eq!(c.tracks.len(), 1);
        assert_eq!(c.tracks[0].rotation.len(), 11);
        // held components are one value
        assert_eq!(c.tracks[0].translation.len(), 1);
        let near = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4);
        let arm = |f: f32| math::transform_point(&c.pose(f)[1], [0.0; 3]);
        assert!(near(arm(0.0), [1.0, 0.0, 0.0]), "{:?}", arm(0.0));
        assert!(near(arm(10.0), [0.0, 1.0, 0.0]), "{:?}", arm(10.0));
        // between frames, blended; past the end, the last frame
        let half = std::f32::consts::FRAC_PI_4;
        assert!(
            near(arm(5.0), [half.cos(), half.sin(), 0.0]),
            "{:?}",
            arm(5.0)
        );
        assert!(near(arm(99.0), [0.0, 1.0, 0.0]));
        assert!(c.pose_by_name(0.0).contains_key("Arm"));
    }

    #[test]
    fn the_bytes_hold_a_header_and_the_samples() {
        let c = clip();
        let bytes = c.to_bytes(7);
        let n = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
        let header: serde_json::Value = serde_json::from_slice(&bytes[4..4 + n]).unwrap();
        assert_eq!(header["samples"], 11);
        assert_eq!(header["id"], 7);
        assert_eq!(header["tracks"][0]["bone"], "Root");
        let data_at = (4 + n).next_multiple_of(4);
        let floats: Vec<f32> = bytes[data_at..]
            .chunks(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
            .collect();
        let [at, count] = [
            header["tracks"][0]["r"][0].as_u64().unwrap() as usize,
            header["tracks"][0]["r"][1].as_u64().unwrap() as usize,
        ];
        assert_eq!(count, 11);
        // the last rotation: a quarter turn about z
        let q = &floats[at + 40..at + 44];
        assert!(
            (q[2] - std::f32::consts::FRAC_PI_4.sin()).abs() < 1e-4,
            "{q:?}"
        );
    }
}
