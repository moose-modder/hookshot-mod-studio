//! Editing a model's cloth file (`Phive/Cloth/*.bphcl`) through its Blender scene.
//!
//! [`scene_of`] is what the scene shows of a file: each collider (the bone it follows, its place and its shape) and
//! each simulated part (its settings, and which of its colliders its particles meet). [`apply`] writes a scene's
//! version back into the game's file. Only values that differ from the file's (by more than [`same`] allows) are
//! written, into an editable copy of its tagfile (havok-formats' writer gives the game's 171 files back byte for
//! byte, and [`phive::with_tagfile`] the files around them), so a scene left as it was makes no file at all. What
//! was written is read back and compared with the scene before the file is returned.
//!
//! What can change, all in place (the file keeps its objects and sizes):
//! - a collider's place (moved, turned), its size (a capsule's ends and radius, a sphere's centre and radius, a
//!   plane's normal and distance) and the bone it follows (one the skeleton of every part using it has);
//! - a part's gravity, damping, substeps and solve iterations; scales of its particles' radius and friction, of its
//!   links' stiffness (stretch, bend) and of how far its particles may swing from the skinned mesh (local ranges);
//! - which of its colliders a part's particles meet.
//!
//! New colliders, and colliders a part doesn't have yet, need new objects in the file (not yet).

use std::collections::HashMap;

use havok_formats::cloth::{self, Shape};
use havok_formats::{Tagfile, TagfileImage, Value, animation};
use modstudio_formats::phive::{self, Phive};
use serde::{Deserialize, Serialize};

use crate::math::{self, Mat4};
use crate::{Error, Result};

/// The cloth as a scene shows it, in the project's folder: what `make_scene.py` and `cloth_scene.py` make the
/// collection "Cloth" from.
pub const SCENE_FILE: &str = "cloth_scene.json";
/// The scene's cloth as the build reads it (prep_for_game.py), in the work folder.
pub const EDITS_FILE: &str = "cloth_edits.json";

/// Distances from this on are no limit (the files store the largest float): scales leave them.
pub const UNLIMITED: f32 = 1e30;

/// What a scene shows of a cloth file. Places are column-major 4x4s in the game's space (y up, metres).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClothScene {
    /// The cloth file, in the romfs.
    pub source: String,
    pub colliders: Vec<SceneCollider>,
    pub parts: Vec<ScenePart>,
    /// The rest places of the bones the colliders follow (for the scene to tell its size; not read back).
    #[serde(default)]
    pub bones: Vec<SceneBone>,
}

/// A bone's place in the model at rest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneBone {
    pub name: String,
    pub place: Mat4,
}

/// A collider (`hclCollidable`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneCollider {
    /// Its item in the game's file: what the scene's object stands for.
    pub id: usize,
    pub name: String,
    /// The bone it follows; none: it stays where it is in the model.
    pub bone: Option<String>,
    /// Its place in the model at rest (a rotation and a translation): its bone's rest place times its offset.
    pub place: Mat4,
    pub shape: SceneShape,
}

/// A collider's shape, in the collider's own frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SceneShape {
    Capsule {
        start: [f32; 3],
        end: [f32; 3],
        radius: f32,
    },
    Sphere {
        centre: [f32; 3],
        radius: f32,
    },
    /// The points with `normal . x + distance < 0` are inside.
    Plane {
        normal: [f32; 3],
        distance: f32,
    },
    /// A shape only moved, not resized.
    Other {
        class: String,
    },
}

/// A simulated part: one sim cloth of a cloth data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScenePart {
    /// The cloth data's name (with the sim cloth's number after it where a cloth data has several).
    pub name: String,
    /// Its colliders, and whether its particles meet each.
    pub colliders: Vec<PartCollider>,
    pub gravity: [f32; 3],
    /// `globalDampingPerSecond`.
    pub damping: f32,
    pub substeps: u8,
    pub iterations: u8,
    /// Scales of the file's values: 1 keeps them.
    pub particle_radius: f32,
    pub friction: f32,
    pub stretch_stiffness: f32,
    pub bend_stiffness: f32,
    pub swing_range: f32,
    /// Where the part is, at rest: the middle of the bones it moves (for the scene to put its settings there; not
    /// read back).
    #[serde(default)]
    pub centre: [f32; 3],
}

/// One of a part's colliders.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartCollider {
    pub id: usize,
    pub meets: bool,
}

/// The cloth of a model in the game's romfs, as a scene shows it, written to `path` as JSON (for `make_scene.py` or
/// `cloth_scene.py`); `None` and nothing written if the model has no cloth.
pub fn write_scene(
    romfs: &std::path::Path,
    model: &str,
    path: &std::path::Path,
) -> Result<Option<ClothScene>> {
    let Some(file) = crate::cloth::cloth_file(romfs, model) else {
        return Ok(None);
    };
    let source = file
        .strip_prefix(romfs)
        .unwrap_or(&file)
        .to_string_lossy()
        .replace('\\', "/");
    let bytes = std::fs::read(&file).map_err(|e| Error(format!("{}: {e}", file.display())))?;
    let scene = scene_of(&bytes, &source)?;
    let text = serde_json::to_string_pretty(&scene).map_err(|e| Error(e.to_string()))?;
    std::fs::write(path, text).map_err(|e| Error(format!("{}: {e}", path.display())))?;
    Ok(Some(scene))
}

/// Adds the model's cloth to the project's existing scene (`cloth_scene.py`, Blender in the background; Blender keeps
/// the scene before it as `<scene>.blend1`), and marks the project so. The scene mustn't be open in Blender: saving it
/// there would drop the cloth again.
pub fn add_to_scene(
    settings: &crate::settings::Settings,
    project: &mut crate::project::Project,
    progress: &mut crate::progress::Progress,
) -> Result<ClothScene> {
    let blender = settings.blender()?;
    progress.step("The model's cloth");
    let json = project.dir().join(SCENE_FILE);
    let scene =
        write_scene(&settings.game_romfs, &project.data.model, &json)?.ok_or_else(|| {
            Error(format!(
                "{}: the game has no cloth for this model",
                crate::project::model_name(&project.data.model)
            ))
        })?;
    progress.line(format!(
        "{}: {} collider(s), {} part(s)",
        scene.source,
        scene.colliders.len(),
        scene.parts.len()
    ));
    progress.step("Adding it to the scene");
    let scripts = crate::tools::scripts_dir(&settings.cache_dir())?;
    let armature = project
        .data
        .prep
        .get("armature")
        .and_then(|a| a.as_str())
        .unwrap_or("Armature")
        .to_owned();
    let logs = project.work_dir();
    std::fs::create_dir_all(&logs).map_err(|e| Error(format!("{}: {e}", logs.display())))?;
    crate::tools::run(
        blender,
        &crate::args![
            "--background",
            project.blend(),
            "--python",
            scripts.join("cloth_scene.py"),
            "--",
            "--cloth",
            &json,
            "--armature",
            &armature
        ],
        &logs.join("add_cloth.log"),
        &|l| l.starts_with("[cloth]") || l.contains("Error"),
        progress,
    )?;
    project.data.cloth_scene = true;
    project.save()?;
    Ok(scene)
}

/// What [`apply`] made of a scene.
#[derive(Debug, Clone, Default)]
pub struct Applied {
    /// The edited cloth file; none when the scene changes nothing.
    pub bytes: Option<Vec<u8>>,
    /// What changed, a line each.
    pub changes: Vec<String>,
    pub warnings: Vec<String>,
}

/// Whether two values are the same as far as a scene can tell: within 1e-5 of the larger (1e-5 below 1).
pub fn same(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1e-5 * a.abs().max(b.abs()).max(1.0)
}

fn same_all(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same(*x, *y))
}

fn havok(source: &str) -> impl Fn(havok_formats::Error) -> Error + '_ {
    move |e| Error(format!("{source}: {e}"))
}

/// A cloth data's objects as values: its sim cloths, and its simulate operators with the sim cloth each runs.
type DataValues<'t, 'a> = (Vec<Value<'t, 'a>>, Vec<(usize, Value<'t, 'a>)>);

/// The cloth's objects as values of the tagfile, beside havok-formats' reading of them (the same order).
struct Values<'t, 'a> {
    /// Per cloth data.
    datas: Vec<DataValues<'t, 'a>>,
    /// The item an object's first byte starts.
    item_at: HashMap<usize, usize>,
}

impl<'t, 'a> Values<'t, 'a> {
    fn read(tag: &'t Tagfile<'a>) -> havok_formats::Result<Self> {
        let mut datas = Vec::new();
        let mut item_at = HashMap::new();
        for (index, item) in tag.items.iter().enumerate().skip(1) {
            item_at.insert(item.offset, index);
            if tag
                .types
                .get(item.type_index)
                .is_none_or(|t| t.name != "hclClothData")
            {
                continue;
            }
            let array = tag.item(index)?;
            for i in 0..array.len() {
                let data = array.get(i)?;
                let sims_field = data.field("simClothDatas")?.array()?;
                let mut sims = Vec::new();
                for k in 0..sims_field.len() {
                    sims.push(sims_field.get(k)?.dereference()?);
                }
                let ops_field = data.field("operators")?.array()?;
                let mut simulates = Vec::new();
                for k in 0..ops_field.len() {
                    let op = ops_field.get(k)?.dereference()?;
                    if op.type_name() == "hclSimulateOperator" {
                        simulates.push((op.field("simClothIndex")?.unsigned()? as usize, op));
                    }
                }
                datas.push((sims, simulates));
            }
        }
        Ok(Values { datas, item_at })
    }

    /// The items of a sim cloth's colliders, in its order.
    fn collider_items(&self, sim: Value<'t, 'a>) -> havok_formats::Result<Vec<usize>> {
        let list = sim.field("perInstanceCollidables")?.array()?;
        let mut out = Vec::new();
        for c in 0..list.len() {
            let collidable = list.get(c)?.dereference()?;
            out.push(self.item_at.get(&collidable.offset).copied().unwrap_or(0));
        }
        Ok(out)
    }
}

/// Changes to an editable copy of a tagfile, by the values a [`Tagfile`] reads: each written where the value is.
struct Patcher {
    image: TagfileImage,
    /// Each item's place in DATA (start, end, item), by start.
    spans: Vec<(usize, usize, usize)>,
    changed: bool,
}

impl Patcher {
    fn new(tag: &Tagfile) -> havok_formats::Result<Self> {
        let image = tag.image()?;
        let mut spans: Vec<(usize, usize, usize)> = tag
            .items
            .iter()
            .enumerate()
            .skip(1)
            .map(|(i, item)| (item.offset, item.offset + image.items[i].data.len(), i))
            .collect();
        spans.sort_unstable();
        Ok(Patcher {
            image,
            spans,
            changed: false,
        })
    }

    fn write(&mut self, offset: usize, bytes: &[u8]) -> Result<()> {
        let k = self.spans.partition_point(|s| s.0 <= offset);
        let Some(&(start, end, item)) = k.checked_sub(1).and_then(|k| self.spans.get(k)) else {
            return Err(Error(format!("no item holds DATA offset {offset:#x}")));
        };
        if offset + bytes.len() > end {
            return Err(Error(format!(
                "DATA offset {offset:#x} runs past item {item}"
            )));
        }
        let slot = &mut self.image.items[item].data[offset - start..offset - start + bytes.len()];
        if slot != bytes {
            slot.copy_from_slice(bytes);
            self.changed = true;
        }
        Ok(())
    }

    /// A float32 value.
    fn float(&mut self, v: Value, x: f32) -> Result<()> {
        v.float().map_err(|e| Error(e.to_string()))?;
        self.write(v.offset, &x.to_le_bytes())
    }

    /// The first `xs.len()` floats of an array of floats (a vector or matrix), each where it is.
    fn floats(&mut self, v: Value, xs: &[f32]) -> Result<()> {
        let a = v.array().map_err(|e| Error(e.to_string()))?;
        for (i, &x) in xs.iter().enumerate() {
            self.float(a.get(i).map_err(|e| Error(e.to_string()))?, x)?;
        }
        Ok(())
    }

    /// `xyz` into a vector of four floats, its fourth kept.
    fn vec3(&mut self, v: Value, xyz: [f32; 3]) -> Result<()> {
        self.floats(v, &xyz)
    }

    /// An unsigned integer value, at its own width.
    fn uint(&mut self, v: Value, x: u64) -> Result<()> {
        v.unsigned().map_err(|e| Error(e.to_string()))?;
        let bytes = (v.format().map_err(|e| Error(e.to_string()))? >> 10) / 8;
        if bytes < 8 && x >> (8 * bytes) != 0 {
            return Err(Error(format!("{x} doesn't fit {bytes} bytes")));
        }
        self.write(v.offset, &x.to_le_bytes()[..bytes])
    }
}

/// The world transform at rest of every bone of each transform set of a cloth data (its skeleton of that name).
fn rest_sets(
    data: &cloth::Cloth,
    skeletons: &[animation::Skeleton],
) -> Vec<Option<(Vec<String>, Vec<Mat4>)>> {
    data.transform_sets
        .iter()
        .map(|set| {
            skeletons.iter().find(|s| s.name == set.name).map(|s| {
                (
                    s.bones.iter().map(|b| b.name.clone()).collect(),
                    crate::cloth::rest_pose(s)
                        .iter()
                        .map(|m| m.to_columns())
                        .collect(),
                )
            })
        })
        .collect()
}

fn xyz(v: [f32; 4]) -> [f32; 3] {
    [v[0], v[1], v[2]]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// A capsule's `dir` and `capLenSqrdInv`: the unit vector from start to end, and one over the squared length.
pub fn capsule_axis(start: [f32; 3], end: [f32; 3]) -> ([f32; 3], f32) {
    let d = sub(end, start);
    let len_sq = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
    if len_sq <= f32::EPSILON {
        return ([1.0, 0.0, 0.0], 0.0);
    }
    let len = len_sq.sqrt();
    ([d[0] / len, d[1] / len, d[2] / len], 1.0 / len_sq)
}

fn shape_of(shape: &Shape) -> SceneShape {
    match shape {
        Shape::Capsule {
            start, end, radius, ..
        } => SceneShape::Capsule {
            start: xyz(*start),
            end: xyz(*end),
            radius: *radius,
        },
        Shape::Sphere { pos } => SceneShape::Sphere {
            centre: xyz(*pos),
            radius: pos[3],
        },
        Shape::Plane { equation } => SceneShape::Plane {
            normal: xyz(*equation),
            distance: equation[3],
        },
        other => SceneShape::Other {
            class: format!("{other:?}")
                .split([' ', '{', '('])
                .next()
                .unwrap_or("")
                .to_owned(),
        },
    }
}

/// The names of a file's parts: the cloth data's, numbered where it has several sim cloths.
fn part_name(data: &cloth::Cloth, sim: usize) -> String {
    if data.sim_cloths.len() == 1 {
        data.name.clone()
    } else {
        format!("{} {}", data.name, sim + 1)
    }
}

/// A cloth file, read as the scene shows it.
pub fn scene_of(bytes: &[u8], source: &str) -> Result<ClothScene> {
    let e = havok(source);
    let parts_of_file = Phive::parse(bytes).map_err(|x| Error(format!("{source}: {x}")))?;
    let tag = Tagfile::read(parts_of_file.tagfile, None).map_err(&e)?;
    let physics = cloth::read(&tag).map_err(&e)?;
    let skeletons = animation::container(&tag).map_err(&e)?.skeletons;
    let values = Values::read(&tag).map_err(&e)?;
    let mut scene = ClothScene {
        source: source.to_owned(),
        colliders: Vec::new(),
        parts: Vec::new(),
        bones: Vec::new(),
    };
    for (data, (sims, _)) in physics.cloths.iter().zip(&values.datas) {
        let rest = rest_sets(data, &skeletons);
        for (s, (sim, sim_value)) in data.sim_cloths.iter().zip(sims).enumerate() {
            let items = values.collider_items(*sim_value).map_err(&e)?;
            let mut used = 0u32;
            for &m in &sim.static_collision_masks {
                used |= m;
            }
            let mut colliders = Vec::new();
            for (c, (collidable, &id)) in sim.collidables.iter().zip(&items).enumerate() {
                colliders.push(PartCollider {
                    id,
                    meets: used & (1 << c.min(30)) != 0,
                });
                if scene.colliders.iter().any(|x| x.id == id) {
                    continue;
                }
                let on_bone = sim.collidable_transform_map.as_ref().and_then(|map| {
                    let bone = usize::try_from(*map.transform_indices.get(c)?).ok()?;
                    let (names, places) = rest.get(map.transform_set as usize)?.as_ref()?;
                    Some((
                        names.get(bone)?.clone(),
                        math::mul(places.get(bone)?, map.offsets.get(c)?),
                    ))
                });
                let (bone, place) = match on_bone {
                    Some((b, p)) => (Some(b), p),
                    None => (None, collidable.transform),
                };
                if let Some(b) = &bone
                    && !scene.bones.iter().any(|x| &x.name == b)
                    && let Some(place) = rest.iter().flatten().find_map(|(names, places)| {
                        names.iter().position(|n| n == b).map(|i| places[i])
                    })
                {
                    scene.bones.push(SceneBone {
                        name: b.clone(),
                        place,
                    });
                }
                scene.colliders.push(SceneCollider {
                    id,
                    name: collidable.name.clone(),
                    bone,
                    place: rigid(&place),
                    shape: shape_of(&collidable.shape),
                });
            }
            // the first simulate operator of this sim cloth: its first configuration is the one run
            let config = data.operators.iter().find_map(|op| match &op.kind {
                cloth::OperatorKind::Simulate { sim_cloth, configs }
                    if *sim_cloth as usize == s =>
                {
                    configs.first()
                }
                _ => None,
            });
            scene.parts.push(ScenePart {
                name: part_name(data, s),
                colliders,
                gravity: xyz(sim.gravity),
                damping: sim.global_damping_per_second,
                substeps: config.map_or(1, |c| c.sub_steps),
                iterations: config.map_or(1, |c| c.solve_iterations),
                particle_radius: 1.0,
                friction: 1.0,
                stretch_stiffness: 1.0,
                bend_stiffness: 1.0,
                swing_range: 1.0,
                centre: driven_centre(data, &rest),
            });
        }
    }
    Ok(scene)
}

/// The middle of the rest places of the bones a cloth data sets (the origin if it sets none).
fn driven_centre(data: &cloth::Cloth, rest: &[Option<(Vec<String>, Vec<Mat4>)>]) -> [f32; 3] {
    let mut sum = [0f32; 3];
    let mut n = 0;
    for (set, bone) in havok_cloth::instance::driven_bones(data) {
        if let Some(place) = rest
            .get(set as usize)
            .and_then(Option::as_ref)
            .and_then(|(_, places)| places.get(bone))
        {
            for k in 0..3 {
                sum[k] += place[12 + k];
            }
            n += 1;
        }
    }
    if n == 0 {
        return sum;
    }
    sum.map(|v| v / n as f32)
}

/// A place with the fourth row exactly `0 0 0 1` (the file's matrices hold other numbers in unused lanes).
fn rigid(m: &Mat4) -> Mat4 {
    let mut out = *m;
    out[3] = 0.0;
    out[7] = 0.0;
    out[11] = 0.0;
    out[15] = 1.0;
    out
}

/// Writes `edited`'s differences from the cloth file `bytes` into it (see the module's notes).
pub fn apply(bytes: &[u8], edited: &ClothScene) -> Result<Applied> {
    let source = edited.source.as_str();
    let e = havok(source);
    let original = scene_of(bytes, source)?;
    let file = Phive::parse(bytes).map_err(|x| Error(format!("{source}: {x}")))?;
    let tag = Tagfile::read(file.tagfile, None).map_err(&e)?;
    let physics = cloth::read(&tag).map_err(&e)?;
    let skeletons = animation::container(&tag).map_err(&e)?.skeletons;
    let values = Values::read(&tag).map_err(&e)?;
    let mut patch = Patcher::new(&tag).map_err(&e)?;
    let mut out = Applied::default();

    // the scene's colliders and parts, by id and name
    for c in &edited.colliders {
        if !original.colliders.iter().any(|o| o.id == c.id) {
            return Err(Error(format!(
                "{}: the scene has a collider the file doesn't ({}): new colliders can't be added yet",
                source, c.name
            )));
        }
    }
    for p in &edited.parts {
        if !original.parts.iter().any(|o| o.name == p.name) {
            return Err(Error(format!("{source}: the file has no part {}", p.name)));
        }
    }

    // colliders: where each sim cloth has them
    let mut sims_of: HashMap<usize, Vec<(usize, usize, usize)>> = HashMap::new(); // id -> (data, sim, slot)
    for (d, (sims, _)) in values.datas.iter().enumerate() {
        for (s, sim) in sims.iter().enumerate() {
            for (c, id) in values
                .collider_items(*sim)
                .map_err(&e)?
                .into_iter()
                .enumerate()
            {
                sims_of.entry(id).or_default().push((d, s, c));
            }
        }
    }
    for c in &edited.colliders {
        let Some(o) = original.colliders.iter().find(|o| o.id == c.id) else {
            continue;
        };
        let moved = !same_all(&rigid(&c.place), &o.place) || c.bone != o.bone;
        let resized = !same_shape(&c.shape, &o.shape);
        if !moved && !resized {
            continue;
        }
        let place = rigid(&c.place);
        let collidable_value = values
            .datas
            .iter()
            .flat_map(|(sims, _)| sims.iter())
            .find_map(|sim| {
                let list = sim.field("perInstanceCollidables").ok()?.array().ok()?;
                (0..list.len()).find_map(|k| {
                    let v = list.get(k).ok()?.dereference().ok()?;
                    (values.item_at.get(&v.offset) == Some(&c.id)).then_some(v)
                })
            })
            .ok_or_else(|| Error(format!("{source}: {} not found", c.name)))?;
        if moved {
            // each part's map: the bone (by name, in that part's skeleton) and the offset from it
            for &(d, s, slot) in sims_of.get(&c.id).map(Vec::as_slice).unwrap_or(&[]) {
                let data = &physics.cloths[d];
                let sim_value = values.datas[d].0[s];
                let Some(map) = &data.sim_cloths[s].collidable_transform_map else {
                    if c.bone.is_some() {
                        return Err(Error(format!(
                            "{}: {} can't follow a bone in {} (the file keeps it in place)",
                            source,
                            c.name,
                            part_name(data, s)
                        )));
                    }
                    continue;
                };
                let rest = rest_sets(data, &skeletons);
                let Some((names, places)) = rest
                    .get(map.transform_set as usize)
                    .and_then(Option::as_ref)
                else {
                    return Err(Error(format!(
                        "{}: {}'s skeleton isn't in the file",
                        source,
                        part_name(data, s)
                    )));
                };
                let Some(bone_name) = &c.bone else {
                    return Err(Error(format!(
                        "{}: {} has to follow a bone in {}",
                        source,
                        c.name,
                        part_name(data, s)
                    )));
                };
                let Some(bone) = names.iter().position(|n| n == bone_name) else {
                    return Err(Error(format!(
                        "{}: {} can't follow {}: {}'s skeleton doesn't have that bone",
                        source,
                        c.name,
                        bone_name,
                        part_name(data, s)
                    )));
                };
                let inverse = math::inverse_affine(&places[bone]).ok_or_else(|| {
                    Error(format!("{source}: bone {bone_name} can't be inverted"))
                })?;
                let offset = rigid(&math::mul(&inverse, &place));
                let map_value = sim_value.field("collidableTransformMap").map_err(&e)?;
                let index = map_value
                    .field("transformIndices")
                    .and_then(|v| v.array())
                    .and_then(|a| a.get(slot))
                    .map_err(&e)?;
                patch.uint(index, bone as u64)?;
                let offset_value = map_value
                    .field("offsets")
                    .and_then(|v| v.array())
                    .and_then(|a| a.get(slot))
                    .map_err(&e)?;
                patch.floats(offset_value, &offset)?;
            }
            // the collidable's own place: rotation and translation, the translation's adjustment zeroed
            let t = collidable_value.field("transform").map_err(&e)?;
            if t.type_name() == "hkPreciseTransform" {
                let mut f = t.vector::<20>().map_err(&e)?;
                for col in 0..3 {
                    f[col * 4..col * 4 + 3].copy_from_slice(&place[col * 4..col * 4 + 3]);
                }
                f[12..15].copy_from_slice(&place[12..15]);
                f[16..19].fill(0.0);
                patch.floats(t, &f[..19])?;
            } else {
                let mut f = t.vector::<16>().map_err(&e)?;
                for col in 0..4 {
                    f[col * 4..col * 4 + 3].copy_from_slice(&place[col * 4..col * 4 + 3]);
                }
                patch.floats(t, &f)?;
            }
            let bone_text = match (&o.bone, &c.bone) {
                (a, b) if a == b => String::new(),
                (_, Some(b)) => format!(", now on {b}"),
                (_, None) => ", now on no bone".to_owned(),
            };
            out.changes.push(format!(
                "{}: moved {:.1} cm{bone_text}",
                c.name,
                100.0
                    * ((place[12] - o.place[12]).powi(2)
                        + (place[13] - o.place[13]).powi(2)
                        + (place[14] - o.place[14]).powi(2))
                    .sqrt()
            ));
        }
        if resized {
            let shape = collidable_value
                .field("shape")
                .and_then(|v| v.dereference())
                .map_err(&e)?;
            match (&c.shape, &o.shape) {
                (
                    SceneShape::Capsule { start, end, radius },
                    SceneShape::Capsule {
                        radius: old_radius,
                        start: old_start,
                        end: old_end,
                    },
                ) => {
                    let (dir, inverse_len_sq) = capsule_axis(*start, *end);
                    patch.vec3(shape.field("start").map_err(&e)?, *start)?;
                    patch.vec3(shape.field("end").map_err(&e)?, *end)?;
                    patch.vec3(shape.field("dir").map_err(&e)?, dir)?;
                    patch.float(shape.field("radius").map_err(&e)?, *radius)?;
                    patch.float(shape.field("capLenSqrdInv").map_err(&e)?, inverse_len_sq)?;
                    let length = |a: [f32; 3], b: [f32; 3]| {
                        let d = sub(b, a);
                        (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
                    };
                    out.changes.push(format!(
                        "{}: capsule radius {:.1} -> {:.1} cm, length {:.1} -> {:.1} cm",
                        c.name,
                        old_radius * 100.0,
                        radius * 100.0,
                        length(*old_start, *old_end) * 100.0,
                        length(*start, *end) * 100.0
                    ));
                }
                (SceneShape::Sphere { centre, radius }, SceneShape::Sphere { radius: old, .. }) => {
                    let pos = shape
                        .field("sphere")
                        .and_then(|v| v.field("pos"))
                        .map_err(&e)?;
                    patch.floats(pos, &[centre[0], centre[1], centre[2], *radius])?;
                    out.changes.push(format!(
                        "{}: sphere radius {:.1} -> {:.1} cm",
                        c.name,
                        old * 100.0,
                        radius * 100.0
                    ));
                }
                (SceneShape::Plane { normal, distance }, SceneShape::Plane { .. }) => {
                    let eq = shape.field("planeEquation").map_err(&e)?;
                    patch.floats(eq, &[normal[0], normal[1], normal[2], *distance])?;
                    out.changes
                        .push(format!("{}: plane turned or moved", c.name));
                }
                (SceneShape::Other { .. }, SceneShape::Other { .. }) => {}
                _ => {
                    return Err(Error(format!(
                        "{}: {} can't change its kind of shape",
                        source, c.name
                    )));
                }
            }
        }
    }

    // parts
    let mut index = 0;
    for (d, data) in physics.cloths.iter().enumerate() {
        let (sim_values, simulates) = &values.datas[d];
        for (s, sim) in data.sim_cloths.iter().enumerate() {
            let o = &original.parts[index];
            index += 1;
            let Some(p) = edited.parts.iter().find(|p| p.name == o.name) else {
                continue;
            };
            let v = sim_values[s];
            let mut lines = Vec::new();
            if !same_all(&p.gravity, &o.gravity) {
                let info = v.field("simulationInfo").map_err(&e)?;
                patch.vec3(info.field("gravity").map_err(&e)?, p.gravity)?;
                lines.push(format!(
                    "gravity ({:.2} {:.2} {:.2}) -> ({:.2} {:.2} {:.2})",
                    o.gravity[0],
                    o.gravity[1],
                    o.gravity[2],
                    p.gravity[0],
                    p.gravity[1],
                    p.gravity[2]
                ));
            }
            if !same(p.damping, o.damping) {
                let info = v.field("simulationInfo").map_err(&e)?;
                patch.float(info.field("globalDampingPerSecond").map_err(&e)?, p.damping)?;
                lines.push(format!("damping {} -> {}", o.damping, p.damping));
            }
            if p.substeps != o.substeps || p.iterations != o.iterations {
                if p.substeps == 0 || p.iterations == 0 {
                    return Err(Error(format!(
                        "{}: {} needs at least one substep and one solve iteration",
                        source, p.name
                    )));
                }
                for (_, op) in simulates.iter().filter(|(k, _)| *k == s) {
                    let configs = op
                        .field("simulateOpConfigs")
                        .and_then(|c| c.array())
                        .map_err(&e)?;
                    for k in 0..configs.len() {
                        let config = configs.get(k).map_err(&e)?;
                        patch.uint(config.field("subSteps").map_err(&e)?, u64::from(p.substeps))?;
                        patch.uint(
                            config.field("numberOfSolveIterations").map_err(&e)?,
                            u64::from(p.iterations),
                        )?;
                    }
                }
                lines.push(format!(
                    "substeps {} -> {}, solve iterations {} -> {}",
                    o.substeps, p.substeps, o.iterations, p.iterations
                ));
            }
            // per-particle scales
            if !same(p.particle_radius, 1.0) || !same(p.friction, 1.0) {
                if p.particle_radius <= 0.0 || p.friction < 0.0 {
                    return Err(Error(format!(
                        "{}: {}: a particle radius scale above 0 and a friction scale of 0 or more",
                        source, p.name
                    )));
                }
                let particles = v
                    .field("particleDatas")
                    .and_then(|a| a.array())
                    .map_err(&e)?;
                for (k, particle) in sim.particles.iter().enumerate() {
                    let pv = particles.get(k).map_err(&e)?;
                    if !same(p.particle_radius, 1.0) {
                        patch.float(
                            pv.field("radius").map_err(&e)?,
                            particle.radius * p.particle_radius,
                        )?;
                    }
                    if !same(p.friction, 1.0) {
                        patch.float(
                            pv.field("friction").map_err(&e)?,
                            (particle.friction * p.friction).clamp(0.0, 1.0),
                        )?;
                    }
                }
                if !same(p.particle_radius, 1.0) {
                    patch.float(
                        v.field("maxParticleRadius").map_err(&e)?,
                        sim.max_particle_radius * p.particle_radius,
                    )?;
                    lines.push(format!("particle radius x{}", p.particle_radius));
                }
                if !same(p.friction, 1.0) {
                    lines.push(format!("friction x{}", p.friction));
                }
            }
            // constraint scales
            let stretch = !same(p.stretch_stiffness, 1.0);
            let bend = !same(p.bend_stiffness, 1.0);
            let swing = !same(p.swing_range, 1.0);
            if stretch || bend || swing {
                if p.stretch_stiffness < 0.0 || p.bend_stiffness < 0.0 || p.swing_range <= 0.0 {
                    return Err(Error(format!(
                        "{}: {}: stiffness scales of 0 or more and a swing range scale above 0",
                        source, p.name
                    )));
                }
                let sets = v
                    .field("staticConstraintSets")
                    .and_then(|a| a.array())
                    .map_err(&e)?;
                let mut untouched = Vec::new();
                for k in 0..sets.len() {
                    let set = sets.get(k).and_then(|x| x.dereference()).map_err(&e)?;
                    let class = set.type_name().to_owned();
                    let scale_links =
                        |patch: &mut Patcher, field: &str, by: f32, cap: bool| -> Result<()> {
                            let links = set.field("links").and_then(|a| a.array()).map_err(&e)?;
                            for l in 0..links.len() {
                                let f = links.get(l).and_then(|x| x.field(field)).map_err(&e)?;
                                let old = f.float().map_err(&e)?;
                                let new = old * by;
                                patch.float(f, if cap { new.clamp(0.0, 1.0) } else { new })?;
                            }
                            Ok(())
                        };
                    match class.as_str() {
                        "hclStandardLinkConstraintSet"
                        | "hclStretchLinkConstraintSet"
                        | "hclCompressibleLinkConstraintSet" => {
                            if stretch {
                                scale_links(&mut patch, "stiffness", p.stretch_stiffness, true)?;
                            }
                        }
                        "hclBendLinkConstraintSet" => {
                            if stretch {
                                scale_links(
                                    &mut patch,
                                    "stretchStiffness",
                                    p.stretch_stiffness,
                                    true,
                                )?;
                            }
                            if bend {
                                scale_links(&mut patch, "bendStiffness", p.bend_stiffness, true)?;
                            }
                        }
                        "hclBendStiffnessConstraintSet" => {
                            if bend {
                                scale_links(&mut patch, "bendStiffness", p.bend_stiffness, false)?;
                            }
                        }
                        "hclLocalRangeConstraintSet" => {
                            if swing {
                                for list in ["localConstraints", "localStiffnessConstraints"] {
                                    if !set.has_field(list) {
                                        continue;
                                    }
                                    let entries =
                                        set.field(list).and_then(|a| a.array()).map_err(&e)?;
                                    for k in 0..entries.len() {
                                        let entry = entries.get(k).map_err(&e)?;
                                        for field in [
                                            "maximumDistance",
                                            "shapeRadius",
                                            "maxNormalDistance",
                                            "minNormalDistance",
                                        ] {
                                            if entry.has_field(field) {
                                                let f = entry.field(field).map_err(&e)?;
                                                let old = f.float().map_err(&e)?;
                                                // (a limit of the largest float is none: it stays)
                                                if old.abs() < UNLIMITED {
                                                    patch.float(f, old * p.swing_range)?;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        "hclTransitionConstraintSet" => {}
                        other => untouched.push(other.to_owned()),
                    }
                }
                if stretch {
                    lines.push(format!("stretch stiffness x{}", p.stretch_stiffness));
                }
                if bend {
                    lines.push(format!("bend stiffness x{}", p.bend_stiffness));
                }
                if swing {
                    lines.push(format!("swing range x{}", p.swing_range));
                }
                untouched.sort();
                untouched.dedup();
                if !untouched.is_empty() {
                    out.warnings.push(format!(
                        "{}: its {} kept as the game has them (the scales don't reach them)",
                        p.name,
                        untouched.join(", ")
                    ));
                }
            }
            // which colliders it meets
            let ids: Vec<usize> = o.colliders.iter().map(|c| c.id).collect();
            let edited_ids: Vec<usize> = p.colliders.iter().map(|c| c.id).collect();
            if edited_ids != ids {
                return Err(Error(format!(
                    "{}: {}'s colliders can't change yet, only whether it meets each (it has {})",
                    source,
                    p.name,
                    ids.iter()
                        .filter_map(|id| original.colliders.iter().find(|c| c.id == *id))
                        .map(|c| c.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
            let masks = v
                .field("staticCollisionMasks")
                .and_then(|a| a.array())
                .map_err(&e)?;
            for (slot, (now, before)) in p.colliders.iter().zip(&o.colliders).enumerate() {
                if now.meets == before.meets {
                    continue;
                }
                let name = original
                    .colliders
                    .iter()
                    .find(|c| c.id == now.id)
                    .map_or("?", |c| c.name.as_str());
                if slot >= 30 {
                    return Err(Error(format!(
                        "{}: {}: {} is its collider number {}; only its first 30 can be turned on or off",
                        source,
                        p.name,
                        name,
                        slot + 1
                    )));
                }
                let bit = 1u32 << slot;
                for (k, particle) in sim.particles.iter().enumerate() {
                    let old = sim.static_collision_masks.get(k).copied().unwrap_or(0);
                    let new = if now.meets && particle.inv_mass > 0.0 {
                        old | bit
                    } else if now.meets {
                        old
                    } else {
                        old & !bit
                    };
                    patch.uint(masks.get(k).map_err(&e)?, u64::from(new))?;
                }
                lines.push(format!(
                    "{} {name}",
                    if now.meets {
                        "meets"
                    } else {
                        "no longer meets"
                    }
                ));
            }
            if !lines.is_empty() {
                out.changes
                    .push(format!("{}: {}", p.name, lines.join(", ")));
            }
        }
    }

    if !patch.changed {
        return Ok(out);
    }
    let tagfile = patch.image.write().map_err(&e)?;
    let written =
        phive::with_tagfile(bytes, &tagfile).map_err(|x| Error(format!("{source}: {x}")))?;
    check_written(&written, edited, &original)?;
    out.bytes = Some(written);
    Ok(out)
}

/// Shapes the same within [`same`].
fn same_shape(a: &SceneShape, b: &SceneShape) -> bool {
    match (a, b) {
        (
            SceneShape::Capsule { start, end, radius },
            SceneShape::Capsule {
                start: s2,
                end: e2,
                radius: r2,
            },
        ) => same_all(start, s2) && same_all(end, e2) && same(*radius, *r2),
        (
            SceneShape::Sphere { centre, radius },
            SceneShape::Sphere {
                centre: c2,
                radius: r2,
            },
        ) => same_all(centre, c2) && same(*radius, *r2),
        (
            SceneShape::Plane { normal, distance },
            SceneShape::Plane {
                normal: n2,
                distance: d2,
            },
        ) => same_all(normal, n2) && same(*distance, *d2),
        (SceneShape::Other { class }, SceneShape::Other { class: c2 }) => class == c2,
        _ => false,
    }
}

/// The written file read back: it has to read, and show what the scene shows (the colliders and parts in the same
/// order as the original's; the writer numbers items anew, so they're matched by place in that order, not by id).
fn check_written(written: &[u8], edited: &ClothScene, original: &ClothScene) -> Result<()> {
    let back = scene_of(written, &edited.source)
        .map_err(|e| Error(format!("the written file doesn't read: {e}")))?;
    let bug = |what: &str| {
        Error(format!(
            "the written file has {what} otherwise than the scene (a bug: nothing was written)"
        ))
    };
    if back.colliders.len() != original.colliders.len() || back.parts.len() != original.parts.len()
    {
        return Err(bug("its colliders or parts"));
    }
    for (o, b) in original.colliders.iter().zip(&back.colliders) {
        let want = edited.colliders.iter().find(|c| c.id == o.id).unwrap_or(o);
        if b.name != o.name || b.bone != want.bone || !same_all(&b.place, &rigid(&want.place)) {
            return Err(bug(&format!("{}'s place", o.name)));
        }
        if !same_shape(&b.shape, &want.shape) {
            return Err(bug(&format!("{}'s shape", o.name)));
        }
    }
    for (o, b) in original.parts.iter().zip(&back.parts) {
        let want = edited.parts.iter().find(|p| p.name == o.name).unwrap_or(o);
        let meets = |p: &ScenePart| {
            p.colliders
                .iter()
                .take(30)
                .map(|c| c.meets)
                .collect::<Vec<_>>()
        };
        if !same_all(&b.gravity, &want.gravity)
            || !same(b.damping, want.damping)
            || b.substeps != want.substeps
            || b.iterations != want.iterations
            || meets(b) != meets(want)
        {
            return Err(bug(&format!("{}'s settings", o.name)));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_a_scene_cannot_tell_apart_are_the_same() {
        assert!(same(1.0, 1.000_005));
        assert!(!same(1.0, 1.000_02));
        // small values: 1e-5 absolute
        assert!(same(0.0, 0.000_005));
        assert!(!same(0.001, 0.0011));
        // large ones: relative
        assert!(same(-9.81, -9.810_09));
        assert!(!same(-9.81, -9.82));
    }

    #[test]
    fn a_capsule_axis_is_its_unit_direction_and_inverse_squared_length() {
        let (dir, inv) = capsule_axis([-1.5, 0.0, 0.0], [1.5, 0.0, 0.0]);
        assert_eq!(dir, [1.0, 0.0, 0.0]);
        assert!((inv - 1.0 / 9.0).abs() < 1e-7);
        // a capsule with no length: a sphere, no inverse
        assert_eq!(capsule_axis([0.2; 3], [0.2; 3]).1, 0.0);
    }

    #[test]
    fn a_place_is_made_rigid_in_its_last_row_only() {
        let mut m = math::IDENTITY;
        m[3] = 0.5;
        m[12] = 2.0;
        m[15] = 0.0;
        let r = rigid(&m);
        assert_eq!(r[3], 0.0);
        assert_eq!(r[12], 2.0);
        assert_eq!(r[15], 1.0);
    }
}
