//! Cloth previews. The game simulates hair, hoods, belts and capes with Havok Cloth, from the model's cloth file
//! (`Phive/Cloth/<model>.Nin_Ounce_NVN2.bphcl`), and the simulation sets the `Ph_` bones the drawn meshes are
//! skinned to. [`simulate`] runs that file (with the vendored havok-formats and havok-cloth) against a test motion
//! and keeps where each of those bones is in every frame; `modstudio cloth --open` makes an action of it on a Blender
//! scene's armature (`pipeline/cloth_preview.py`), for meshes not built yet. The app's 3D view runs the simulation
//! live instead ([`LiveCloth`], [`open_model`]: through a mod's overlay, the mod's own cloth file where it has one),
//! moved by the user's hand or by the test motion.
//!
//! No animation is read yet (docs/plan.md, step 2): the test motion moves the whole model ([`test_motion`]). The
//! simulation is havok-cloth's, adapted to the game's Havok version; what it leaves out (volume and bone plane
//! constraints, wind) is listed in the preview's warnings where a file uses it. It is a preview: the game's own
//! code decides how the cloth moves in game.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use havok_cloth::collide::WorldShape;
use havok_cloth::instance::{ClothInstance, driven_bones};
use havok_cloth::matrix::Mat4 as ClothMat4;
use havok_formats::Tagfile;
use havok_formats::animation::{self, QsTransform};
use havok_formats::cloth::{self, ConstraintKind};
use modstudio_formats::phive::Phive;
use modstudio_formats::romfs::Romfs;
use serde::{Deserialize, Serialize};

use crate::math::{self, Mat4};
use crate::progress::Progress;
use crate::project::Project;
use crate::settings::Settings;
use crate::{Context, Error, Result, args, catalog, tools};

/// Frames a second, of the simulation and of the action made from it.
pub const FPS: u32 = 30;
/// How long the test motion lasts, for a model of [`REFERENCE_SIZE`].
pub const SECONDS: f32 = 6.5;
/// The size the test motion is made for, in metres: child Link's skeleton's largest extent at rest.
pub const REFERENCE_SIZE: f32 = 1.18;
/// The preview, in the project's work folder.
pub const PREVIEW_FILE: &str = "cloth_preview.json";
/// How long the cloth settles at rest before the preview's first frame (not recorded): where it starts inside a
/// collidable, it's pushed out then, out of sight.
pub const SETTLE_SECONDS: f32 = 2.0;
/// A collidable the cloth starts deeper inside at rest than this share of the model's size is named in the
/// warnings: the game's own poses probably keep the two apart (a floor that fits a seated pose).
pub const DEEP_START: f32 = 0.1;

/// What [`simulate`] found: the bones the cloth moves, frame by frame. Matrices are column-major 4x4, in the game's
/// space (y up, metres).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Preview {
    /// The cloth file, in the romfs.
    pub source: String,
    pub fps: u32,
    /// Frames, the rest pose (frame 0) included.
    pub frames: usize,
    /// The model's size (its skeleton's largest extent at rest, metres), which the test motion is fitted to.
    pub size: f32,
    /// The test motion's distances, times those of [`REFERENCE_SIZE`] (its times are the square root of this).
    pub motion_scale: f32,
    /// The simulated parts (the file's cloth data names).
    pub parts: Vec<String>,
    /// Where the test motion has the model in each frame: what moves every bone the cloth doesn't.
    pub motion: Vec<Mat4>,
    pub bones: Vec<PreviewBone>,
    /// What the preview leaves out, part by part.
    pub warnings: Vec<String>,
}

/// One bone the cloth moves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreviewBone {
    pub name: String,
    /// Its place in the model at rest (the skeleton's reference pose).
    pub rest: Mat4,
    /// Its place in each frame, in the world the motion moves the model through.
    pub frames: Vec<Mat4>,
}

/// Where a model's cloth file would be, in the romfs: named after the model's archive (158 of the 171 files are),
/// else after the model (`model`: `Model/<archive>.<model>.bfres.zs` or a model's name).
pub fn cloth_paths(model: &str) -> [String; 2] {
    let (archive, name) = catalog::split_name(model);
    [archive, name].map(|n| format!("Phive/Cloth/{n}.Nin_Ounce_NVN2.bphcl"))
}

/// The cloth file of a model in the game's romfs folder, if it has one.
pub fn cloth_file(romfs: &Path, model: &str) -> Option<PathBuf> {
    cloth_paths(model)
        .iter()
        .map(|p| romfs.join(p))
        .find(|p| p.is_file())
}

/// The cloth file of a model as `romfs` has it (a mod's own where an overlay has one): its path in the romfs and
/// where it's read from.
pub fn find_cloth(romfs: &Romfs, model: &str) -> Option<(String, PathBuf)> {
    cloth_paths(model)
        .into_iter()
        .map(|p| {
            let file = romfs.resolve(&p);
            (p, file)
        })
        .find(|(_, file)| file.is_file())
}

/// A model's cloth as `romfs` has it, settled at rest and ready to step ([`LiveCloth`]), or `None` if it has no
/// cloth file.
pub fn open_model(romfs: &Romfs, model: &str) -> Result<Option<LiveCloth>> {
    let Some((source, file)) = find_cloth(romfs, model) else {
        return Ok(None);
    };
    let bytes = std::fs::read(&file).context(file.display())?;
    LiveCloth::new(&bytes, &source).map(Some)
}

/// The test motion for a model `size` metres large ([`Motion::for_size`]), one place a frame at `fps`.
pub fn test_motion_frames(size: f32, fps: f32) -> Vec<Mat4> {
    let fitted = Motion::for_size(size);
    let frames = (fitted.seconds() * fps).round() as usize + 1;
    (0..frames).map(|f| fitted.at(f as f32 / fps)).collect()
}

/// The test motion at `t` seconds: where the whole model is (a turn about the vertical and a move), so the cloth
/// swings. It stands for a second while the cloth settles under gravity, turns 60 degrees to each side (1 to 3 s),
/// runs 1.5 m forward and stops (3 to 4.2 s), jumps 35 cm (4.5 to 5.1 s) and stands until the end.
pub fn test_motion(t: f32) -> Mat4 {
    // 0 to 1 with zero speed and acceleration at both ends
    let smooth = |u: f32| {
        let u = u.clamp(0.0, 1.0);
        u * u * u * (u * (u * 6.0 - 15.0) + 10.0)
    };
    let yaw = if (1.0..3.0).contains(&t) {
        60f32.to_radians() * (std::f32::consts::TAU * smooth((t - 1.0) / 2.0)).sin()
    } else {
        0.0
    };
    let forward = 1.5 * smooth((t - 3.0) / 1.2);
    let u = (t - 4.5) / 0.6;
    let up = if (0.0..1.0).contains(&u) {
        4.0 * 0.35 * u * (1.0 - u)
    } else {
        0.0
    };
    let (s, c) = yaw.sin_cos();
    [
        c, 0., -s, 0., 0., 1., 0., 0., s, 0., c, 0., 0., up, forward, 1.,
    ]
}

/// The test motion fitted to a model's size: distances in proportion to it, times by the square root (as a larger
/// body moving alike under the same gravity does), so a giant's cloth swings as a child's does, only larger. Within
/// half and four times the distances of [`REFERENCE_SIZE`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Motion {
    /// Distances, times [`test_motion`]'s.
    pub scale: f32,
    /// Times, times [`test_motion`]'s.
    pub time: f32,
}

impl Motion {
    pub fn for_size(size: f32) -> Motion {
        let scale = if size > 0.0 {
            (size / REFERENCE_SIZE).clamp(0.5, 4.0)
        } else {
            1.0
        };
        Motion {
            scale,
            time: scale.sqrt(),
        }
    }

    /// How long it lasts.
    pub fn seconds(self) -> f32 {
        SECONDS * self.time
    }

    /// Where it has the model at `t` seconds.
    pub fn at(self, t: f32) -> Mat4 {
        let mut m = test_motion(t / self.time);
        for v in &mut m[12..15] {
            *v *= self.scale;
        }
        m
    }
}

/// A skeleton's largest extent at rest: the longest side of the box around its bones.
pub fn extent(world: &[ClothMat4]) -> f32 {
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for m in world {
        let t = m.translation().0;
        for k in 0..3 {
            lo[k] = lo[k].min(t[k]);
            hi[k] = hi[k].max(t[k]);
        }
    }
    (0..3).map(|k| hi[k] - lo[k]).fold(0.0, f32::max)
}

/// A model's cloth, simulated as it goes: the caller says where the model is, step by step. It starts settled at
/// rest ([`SETTLE_SECONDS`]). [`simulate`] runs one against the test motion; the app's 3D view runs one against the
/// user's hand.
pub struct LiveCloth {
    /// The cloth file, in the romfs.
    pub source: String,
    /// The model's size (its skeleton's largest extent at rest, metres).
    pub size: f32,
    /// The simulated parts (the file's cloth data names).
    pub parts: Vec<String>,
    /// The bones the cloth moves (one two parts move is the later part's), with their rest place in the model.
    pub bones: Vec<LiveBone>,
    /// What the simulation leaves out, part by part.
    pub warnings: Vec<String>,
    simulated: Vec<Part>,
}

/// A bone the cloth moves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveBone {
    pub name: String,
    /// Its place in the model at rest (the skeleton's reference pose).
    pub rest: Mat4,
}

/// One cloth data of the file, running.
struct Part {
    name: String,
    instance: ClothInstance<Arc<cloth::Cloth>>,
    /// Each transform set at rest, the model at the origin.
    rest: Vec<Vec<ClothMat4>>,
    /// The bones it sets: (transform set, bone, index in [`LiveCloth::bones`]).
    driven: Vec<(usize, usize, usize)>,
}

impl LiveCloth {
    /// The cloth file `bytes` (`source` names it), read and settled at rest.
    pub fn new(bytes: &[u8], source: &str) -> Result<LiveCloth> {
        let fail = |what: &str, e: &dyn std::fmt::Display| Error(format!("{source}: {what}: {e}"));
        let phive = Phive::parse(bytes).map_err(|e| fail("not a cloth file", &e))?;
        let tag =
            Tagfile::read(phive.tagfile, None).map_err(|e| fail("reading its tagfile", &e))?;
        let physics = cloth::read(&tag).map_err(|e| fail("reading its cloth", &e))?;
        let skeletons = animation::container(&tag)
            .map_err(|e| fail("reading its skeletons", &e))?
            .skeletons;
        let size = skeletons
            .iter()
            .map(|s| extent(&rest_pose(s)))
            .fold(0.0, f32::max);
        let mut live = LiveCloth {
            source: source.to_owned(),
            size,
            parts: Vec::new(),
            bones: Vec::new(),
            warnings: Vec::new(),
            simulated: Vec::new(),
        };
        for data in physics.cloths.into_iter().map(Arc::new) {
            // each transform set is the skeleton of its name, in the world
            let mut sets = Vec::new();
            for set in &data.transform_sets {
                let Some(skeleton) = skeletons.iter().find(|s| s.name == set.name) else {
                    live.warnings.push(format!(
                        "{}: not simulated (no skeleton {} in the file)",
                        data.name, set.name
                    ));
                    break;
                };
                sets.push(skeleton);
            }
            if sets.len() != data.transform_sets.len() {
                continue;
            }
            let rest: Vec<Vec<ClothMat4>> = sets.iter().map(|s| rest_pose(s)).collect();
            let mut instance = match ClothInstance::new(Arc::clone(&data)) {
                Ok(i) => i,
                Err(e) => {
                    live.warnings
                        .push(format!("{}: not simulated ({e})", data.name));
                    continue;
                }
            };
            for (sim, depths) in data.sim_cloths.iter().zip(start_depths(&data, &rest)) {
                for (collidable, depth) in sim.collidables.iter().zip(depths) {
                    if depth > DEEP_START * size {
                        live.warnings.push(format!(
                            "{}: starts {:.0} cm inside {} at rest, and is pushed out before the preview begins (the game's own poses probably keep the two apart)",
                            data.name,
                            depth * 100.0,
                            collidable.name
                        ));
                    }
                }
            }
            for sim in &data.sim_cloths {
                for set in &sim.constraint_sets {
                    if let ConstraintKind::Other { class } = &set.kind {
                        live.warnings.push(format!(
                            "{}: its {class} isn't simulated, so it moves more freely than in the game",
                            data.name
                        ));
                    }
                }
            }
            // settled at rest, out of sight
            for (target, r) in instance.transform_sets.iter_mut().zip(&rest) {
                target.clone_from(r);
            }
            for _ in 0..(SETTLE_SECONDS * FPS as f32) as usize {
                instance
                    .step(1.0 / FPS as f32, &mut [])
                    .map_err(|e| fail(&format!("settling {}", data.name), &e))?;
            }
            let mut driven = Vec::new();
            for (set, bone) in driven_bones(&data) {
                let set = set as usize;
                let Some(name) = sets
                    .get(set)
                    .and_then(|s| s.bones.get(bone))
                    .map(|b| &b.name)
                else {
                    continue;
                };
                let index = match live.bones.iter().position(|b| &b.name == name) {
                    Some(i) => i,
                    None => {
                        live.bones.push(LiveBone {
                            name: name.clone(),
                            rest: rest[set][bone].to_columns(),
                        });
                        live.bones.len() - 1
                    }
                };
                driven.push((set, bone, index));
            }
            live.parts.push(data.name.clone());
            live.simulated.push(Part {
                name: data.name.clone(),
                instance,
                rest,
                driven,
            });
        }
        live.warnings.dedup();
        Ok(live)
    }

    /// One step, `dt` seconds long, with the model at `model` (its place in the world: a rotation and a translation).
    pub fn step(&mut self, model: &Mat4, dt: f32) -> Result<()> {
        let at = ClothMat4::from_columns(*model);
        for part in &mut self.simulated {
            for (target, rest) in part.instance.transform_sets.iter_mut().zip(&part.rest) {
                *target = rest.iter().map(|r| at.mul(r)).collect();
            }
            part.instance
                .step(dt, &mut [])
                .map_err(|e| Error(format!("{}: simulating {}: {e}", self.source, part.name)))?;
            let ran_off = part.driven.iter().any(|&(set, bone, _)| {
                part.instance.transform_sets[set][bone]
                    .to_columns()
                    .iter()
                    .any(|v| !v.is_finite())
            });
            if ran_off {
                return Err(Error(format!(
                    "{}: simulating {}: the simulation ran off (not a number)",
                    self.source, part.name
                )));
            }
        }
        Ok(())
    }

    /// Where each of [`LiveCloth::bones`] is in the world now: where the last step left it (settled at rest, the model
    /// at the origin, before the first).
    pub fn places(&self) -> Vec<Mat4> {
        let mut out: Vec<Mat4> = self.bones.iter().map(|b| b.rest).collect();
        for part in &self.simulated {
            for &(set, bone, index) in &part.driven {
                out[index] = part.instance.transform_sets[set][bone].to_columns();
            }
        }
        out
    }
}

/// Simulates the cloth file `bytes` (`source` names it in the preview) against the test motion, fitted to the size
/// of the file's skeletons ([`Motion::for_size`]).
pub fn simulate(bytes: &[u8], source: &str) -> Result<Preview> {
    let mut live = LiveCloth::new(bytes, source)?;
    let fitted = Motion::for_size(live.size);
    let frames = (fitted.seconds() * FPS as f32).round() as usize + 1;
    let dt = 1.0 / FPS as f32;
    let motion: Vec<Mat4> = (0..frames).map(|f| fitted.at(f as f32 * dt)).collect();
    let mut bones: Vec<PreviewBone> = live
        .bones
        .iter()
        .map(|b| PreviewBone {
            name: b.name.clone(),
            rest: b.rest,
            frames: Vec::with_capacity(frames),
        })
        .collect();
    for (f, at) in motion.iter().enumerate() {
        if f > 0 {
            live.step(at, dt)?;
        }
        for (bone, place) in bones.iter_mut().zip(live.places()) {
            bone.frames.push(place);
        }
    }
    Ok(Preview {
        source: source.to_owned(),
        fps: FPS,
        frames,
        size: live.size,
        motion_scale: fitted.scale,
        parts: live.parts,
        motion,
        bones,
        warnings: live.warnings,
    })
}

/// How deep the cloth starts inside each of its collidables at rest, per sim cloth and collidable (metres; 0 where
/// no free particle that meets it starts inside it): the particles where one step at rest without collisions or
/// gravity puts them, the collidable on its bone. `rest` is each transform set at rest.
pub fn start_depths(data: &cloth::Cloth, rest: &[Vec<ClothMat4>]) -> Vec<Vec<f32>> {
    let mut loose = data.clone();
    for sim in &mut loose.sim_cloths {
        sim.static_collision_masks.iter_mut().for_each(|m| *m = 0);
    }
    let Ok(mut probe) = ClothInstance::new(&loose) else {
        return Vec::new();
    };
    for (target, r) in probe.transform_sets.iter_mut().zip(rest) {
        target.clone_from(r);
    }
    for state in &mut probe.sims {
        state.gravity = havok_cloth::simd::V4([0.0; 4]);
    }
    if probe.step(1.0 / FPS as f32, &mut []).is_err() {
        return Vec::new();
    }
    data.sim_cloths
        .iter()
        .zip(&probe.sims)
        .map(|(sim, state)| {
            sim.collidables
                .iter()
                .enumerate()
                .map(|(c, collidable)| {
                    // on its bone when the sim cloth maps it to one, else where the file has it
                    let place = sim
                        .collidable_transform_map
                        .as_ref()
                        .and_then(|map| {
                            let bone = usize::try_from(*map.transform_indices.get(c)?).ok()?;
                            let bones = rest.get(map.transform_set as usize)?;
                            Some(
                                bones
                                    .get(bone)?
                                    .mul_affine(&ClothMat4::from_columns(*map.offsets.get(c)?)),
                            )
                        })
                        .unwrap_or_else(|| ClothMat4::from_columns(collidable.transform));
                    let shape = WorldShape::new(&collidable.shape, &place);
                    let bit = 1u32 << c.min(30);
                    sim.particles
                        .iter()
                        .zip(&state.positions)
                        .zip(&sim.static_collision_masks)
                        // free particles: fixed ones follow the bones, wherever the collidables are
                        .filter(|((particle, _), mask)| *mask & bit != 0 && particle.inv_mass > 0.0)
                        .map(|((particle, &at), _)| {
                            -(shape.closest(at, particle.radius).distance - particle.radius)
                        })
                        .fold(0f32, f32::max)
                })
                .collect()
        })
        .collect()
}

/// A skeleton's bones at rest, in the world: what a transform set naming it holds when the model stands still at
/// the origin.
pub fn rest_pose(skeleton: &animation::Skeleton) -> Vec<ClothMat4> {
    world_pose(&skeleton.parents, &skeleton.reference_pose)
}

/// The world transform of every bone at rest: each parent's times the bone's own.
fn world_pose(parents: &[i16], pose: &[QsTransform]) -> Vec<ClothMat4> {
    let mut world: Vec<ClothMat4> = Vec::with_capacity(pose.len());
    for (i, q) in pose.iter().enumerate() {
        let local = ClothMat4::from_columns(math::from_trs(
            [q.translation[0], q.translation[1], q.translation[2]],
            q.rotation,
            [q.scale[0], q.scale[1], q.scale[2]],
        ));
        let w = match parents
            .get(i)
            .and_then(|&p| usize::try_from(p).ok())
            .and_then(|p| world.get(p))
        {
            Some(parent) => parent.mul(&local),
            None => local,
        };
        world.push(w);
    }
    world
}

/// The project's cloth preview, simulated from the game's cloth file for its model and written to
/// `<work>/cloth_preview.json`.
pub fn write_preview(settings: &Settings, project: &Project) -> Result<(PathBuf, Preview)> {
    let model = &project.data.model;
    let file = cloth_file(&settings.game_romfs, model).ok_or_else(|| {
        Error(format!(
            "{}: the game has no cloth for this model (nothing in Phive/Cloth is named after it)",
            crate::project::model_name(model)
        ))
    })?;
    let bytes = std::fs::read(&file).context(file.display())?;
    let source = file
        .strip_prefix(&settings.game_romfs)
        .unwrap_or(&file)
        .to_string_lossy()
        .replace('\\', "/");
    let preview = simulate(&bytes, &source)?;
    if preview.bones.is_empty() {
        return Err(Error(format!(
            "{source}: no part of it could be simulated{}",
            if preview.warnings.is_empty() {
                String::new()
            } else {
                format!(" ({})", preview.warnings.join("; "))
            }
        )));
    }
    let dir = project.work_dir();
    std::fs::create_dir_all(&dir).context(dir.display())?;
    let path = dir.join(PREVIEW_FILE);
    let text = serde_json::to_string(&preview).map_err(|e| Error(e.to_string()))?;
    std::fs::write(&path, text).context(path.display())?;
    Ok((path, preview))
}

/// Simulates the project's cloth ([`write_preview`]) and opens its scene in Blender with the preview as an action on
/// the armature (`cloth_preview.py`). Blender keeps running on its own.
pub fn open_preview(
    settings: &Settings,
    project: &Project,
    progress: &mut Progress,
) -> Result<Preview> {
    let blender = settings.blender()?;
    progress.step("simulating the cloth");
    let (path, preview) = write_preview(settings, project)?;
    progress.line(format!(
        "{} part(s), {} bone(s), {} frames",
        preview.parts.len(),
        preview.bones.len(),
        preview.frames
    ));
    for w in &preview.warnings {
        progress.warn(w.clone());
    }
    let scripts = tools::scripts_dir(&settings.cache_dir())?;
    tools::open_in_blender_with(
        blender,
        &project.blend(),
        &args![
            "--python",
            scripts.join("cloth_preview.py"),
            "--",
            "--preview",
            &path,
            "--armature",
            project
                .data
                .prep
                .get("armature")
                .and_then(|a| a.as_str())
                .unwrap_or("Armature")
        ],
    )?;
    progress.line(format!("opened {} in Blender", project.blend().display()));
    Ok(preview)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn the_test_motion_stands_still_at_both_ends_and_moves_between() {
        assert_eq!(test_motion(0.0), math::IDENTITY);
        assert_eq!(test_motion(0.99), math::IDENTITY);
        let end = test_motion(SECONDS);
        // stopped 1.5 m forward, facing forward, on the ground
        assert!(
            close(end[14], 1.5) && close(end[13], 0.0) && close(end[12], 0.0),
            "{end:?}"
        );
        assert!(close(end[0], 1.0) && close(end[10], 1.0), "{end:?}");
        // turned at a quarter of the turn, up in the middle of the jump
        let turned = test_motion(1.0 + 2.0 * 0.3);
        assert!(turned[8].abs() > 0.5, "{turned:?}");
        assert!(close(test_motion(4.8)[13], 0.35), "{:?}", test_motion(4.8));
    }

    #[test]
    fn the_test_motion_is_a_rigid_motion_without_jumps() {
        let dt = 1.0 / FPS as f32;
        let mut last = test_motion(0.0);
        for f in 1..=(SECONDS * FPS as f32) as usize {
            let m = test_motion(f as f32 * dt);
            // columns of a rotation: unit length and square to each other
            for c in 0..3 {
                let col = &m[c * 4..c * 4 + 3];
                assert!(
                    close(col.iter().map(|v| v * v).sum::<f32>(), 1.0),
                    "frame {f}: {m:?}"
                );
            }
            let dot: f32 = (0..3).map(|k| m[k] * m[8 + k]).sum();
            assert!(close(dot, 0.0), "frame {f}");
            // no frame moves the model more than 15 cm or turns it more than 15 degrees
            let moved = ((m[12] - last[12]).powi(2)
                + (m[13] - last[13]).powi(2)
                + (m[14] - last[14]).powi(2))
            .sqrt();
            let turned = (m[0] * last[0] + m[2] * last[2])
                .clamp(-1.0, 1.0)
                .acos()
                .to_degrees();
            assert!(
                moved < 0.15 && turned < 15.0,
                "frame {f}: {moved} m, {turned} degrees"
            );
            last = m;
        }
    }

    #[test]
    fn the_cloth_file_is_named_after_the_archive_or_the_model() {
        let dir = std::env::temp_dir().join(format!("modstudio-cloth-{}", std::process::id()));
        let cloth = dir.join("Phive").join("Cloth");
        std::fs::create_dir_all(&cloth).unwrap();
        std::fs::write(cloth.join("Npc_A.Nin_Ounce_NVN2.bphcl"), b"").unwrap();
        std::fs::write(cloth.join("Npc_B_Body.Nin_Ounce_NVN2.bphcl"), b"").unwrap();
        assert_eq!(
            cloth_file(&dir, "Model/Npc_A.Npc_A.bfres.zs"),
            Some(cloth.join("Npc_A.Nin_Ounce_NVN2.bphcl"))
        );
        assert_eq!(
            cloth_file(&dir, "Model/Npc_B.Npc_B_Body.bfres.zs"),
            Some(cloth.join("Npc_B_Body.Nin_Ounce_NVN2.bphcl"))
        );
        assert_eq!(cloth_file(&dir, "Model/Npc_C.Npc_C.bfres.zs"), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_that_is_not_a_cloth_file_is_refused_by_name() {
        let e = simulate(b"not a cloth file at all", "Phive/Cloth/X.bphcl").unwrap_err();
        assert!(
            e.0.starts_with("Phive/Cloth/X.bphcl: not a cloth file"),
            "{e}"
        );
    }
}
