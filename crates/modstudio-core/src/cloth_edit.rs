//! Editing a model's cloth file (`Phive/Cloth/*.bphcl`) through its Blender scene.
//!
//! [`scene_of`] is what the scene shows of a file: each collider (the bone it follows, its place and its shape) and
//! each simulated part (its settings, and which of its colliders its particles meet). [`apply`] writes a scene's
//! version back into the game's file. Only values that differ from the file's (by more than [`same`] allows) are
//! written, into an editable copy of its tagfile (havok-formats' writer gives the game's 171 files back byte for
//! byte, and [`phive::with_tagfile`] the files around them), so a scene left as it was makes no file at all. What
//! was written is read back and compared with the scene before the file is returned.
//!
//! What can change:
//! - in place: a collider's place (moved, turned), its size (a capsule's ends and radius, a sphere's centre and
//!   radius, a plane's normal and distance) and the bone it follows (one the skeleton of every part using it has); a
//!   part's gravity, damping, substeps and solve iterations; scales of its particles' radius and friction, of its
//!   links' stiffness (stretch, bend) and of how far its particles may swing from the skinned mesh (local ranges);
//!   which of its colliders a part's particles meet; a collider deleted (no part meets it any more: the file keeps the
//!   object, as it keeps 11 unused ones of its own) or taken from a part (likewise for that part);
//! - with new objects: a collider for a part that doesn't have it (the part's collider list, its map's bone indices
//!   and offsets, its sidedness and pinch data each get a new run with one more element, and its free particles the
//!   collider's mask bit, up to 30 colliders a part); a new collider (a copy of one of the file's collidables and of a
//!   shape of its class, named, placed and sized, in the container's list and the parameter archive's list of
//!   colliders, keyed `collidable_<n>` as the game's 2,130 entries are).
//!
//! The actor pack's own list of colliders (`Phive/ClothParam/<model>.phive__ClothParam.bgyml`) isn't changed.

use std::collections::HashMap;

use havok_formats::cloth::{self, Shape};
use havok_formats::{ImageItem, ItemKind, Tagfile, TagfileImage, Value, animation};
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
    /// Colliders of the file the scene deleted (their ids): no part collides with them any more.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deleted: Vec<usize>,
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
    /// Its item in the game's file: what the scene's object stands for; 0 for a collider the scene adds.
    pub id: usize,
    pub name: String,
    /// The bone it follows; none: it stays where it is in the model.
    pub bone: Option<String>,
    /// Its place in the model at rest (a rotation and a translation): its bone's rest place times its offset.
    pub place: Mat4,
    pub shape: SceneShape,
    /// The parts that have it (their names); none given: the file's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parts: Option<Vec<String>>,
    /// For a new collider: the one it was copied from (its flags are taken), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copy_of: Option<usize>,
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

    /// A copy of an item (its type, kind, count and bytes), added; its index.
    fn copy_item(&mut self, item: usize) -> usize {
        let copy = self.image.items[item].clone();
        self.changed = true;
        self.image.push_item(copy)
    }

    /// `bytes` at byte `rel` of item `item`.
    fn write_at(
        &mut self,
        item: usize,
        rel: usize,
        bytes: &[u8],
    ) -> std::result::Result<(), String> {
        let data = &mut self
            .image
            .items
            .get_mut(item)
            .ok_or_else(|| format!("no item {item}"))?
            .data;
        let slot = data
            .get_mut(rel..rel + bytes.len())
            .ok_or_else(|| format!("item {item}: byte {rel} is outside it"))?;
        if slot != bytes {
            slot.copy_from_slice(bytes);
            self.changed = true;
        }
        Ok(())
    }

    /// The item the reference slot at byte `rel` of item `item` refers to (0: none).
    fn target(&self, item: usize, rel: usize) -> usize {
        self.image
            .items
            .get(item)
            .and_then(|i| i.data.get(rel..rel + 4))
            .and_then(|b| <[u8; 4]>::try_from(b).ok())
            .map_or(0, |b| u32::from_le_bytes(b) as usize)
    }

    /// One more element at the end of the array `field` of the object `object` (item `item`): `element` gives its
    /// bytes for the element's size. The array gets a new run with the old elements and the new one (a run may be
    /// another array's too, so none is changed in place).
    fn append(
        &mut self,
        tag: &Tagfile,
        item: usize,
        object: Value,
        field: Value,
        element: &dyn Fn(usize) -> Vec<u8>,
    ) -> std::result::Result<(), String> {
        let rel = field.offset - object.offset;
        let run = self.target(item, rel);
        let element_type = if run != 0 {
            self.image.items[run].type_index
        } else {
            inherited(tag, field.type_index, |t| t.subtype)
                .filter(|&s| s != 0)
                .ok_or("an array without an element type")?
        };
        let size =
            inherited(tag, element_type, |t| t.size).ok_or("an element type without a size")?;
        let (mut data, count) = if run == 0 {
            (Vec::new(), 0)
        } else {
            (
                self.image.items[run].data.clone(),
                self.image.items[run].count,
            )
        };
        let mut bytes = element(size);
        if bytes.len() != size {
            return Err(format!("an element of {} bytes for {size}", bytes.len()));
        }
        data.append(&mut bytes);
        let new = self.image.push_item(ImageItem {
            type_index: element_type,
            kind: ItemKind::Run,
            count: count + 1,
            data,
        });
        self.image
            .set_reference(item, rel, new)
            .map_err(|e| e.to_string())?;
        self.changed = true;
        Ok(())
    }

    /// A reference to item `target` added at the end of the pointer array `field` of `object` (item `item`).
    fn append_reference(
        &mut self,
        tag: &Tagfile,
        item: usize,
        object: Value,
        field: &str,
        target: usize,
    ) -> std::result::Result<(), String> {
        let f = object.field(field).map_err(|e| e.to_string())?;
        let index = u32::try_from(target).map_err(|_| "too many items")?;
        self.append(tag, item, object, f, &|size| {
            let mut b = index.to_le_bytes().to_vec();
            b.resize(size, 0);
            b
        })
    }

    /// Collider `collider` (an item) added to the part `sim` (item `sim_item`): its colliders, its map's bone and offset,
    /// and its per-collider sidedness and pinch data (as its first collider's, zeros if it has none).
    fn add_to_part(
        &mut self,
        tag: &Tagfile,
        sim_item: usize,
        sim: Value,
        collider: usize,
        bone: usize,
        offset: &Mat4,
    ) -> std::result::Result<(), String> {
        let s = |e: havok_formats::Error| e.to_string();
        self.append_reference(tag, sim_item, sim, "perInstanceCollidables", collider)?;
        let map = sim.field("collidableTransformMap").map_err(s)?;
        let bone = bone as u64;
        self.append(
            tag,
            sim_item,
            sim,
            map.field("transformIndices").map_err(s)?,
            &|size| bone.to_le_bytes()[..size.min(8)].to_vec(),
        )?;
        let offset: Vec<u8> = offset.iter().flat_map(|f| f.to_le_bytes()).collect();
        self.append(
            tag,
            sim_item,
            sim,
            map.field("offsets").map_err(s)?,
            &|_| offset.clone(),
        )?;
        for name in ["setupCollidableSidedness", "collidablePinchingDatas"] {
            if !sim.has_field(name) {
                continue;
            }
            let field = sim.field(name).map_err(s)?;
            let run = self.target(sim_item, field.offset - sim.offset);
            let first = (run != 0)
                .then(|| self.image.items[run].clone())
                .filter(|r| r.count > 0)
                .map(|r| r.data[..r.data.len() / r.count].to_vec());
            self.append(tag, sim_item, sim, field, &|size| {
                first.clone().unwrap_or_else(|| vec![0; size])
            })?;
        }
        Ok(())
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

/// A type's property, from it or the nearest of its parents that has it.
fn inherited<T>(
    tag: &Tagfile,
    mut t: usize,
    get: impl Fn(&havok_formats::Type) -> Option<T>,
) -> Option<T> {
    for _ in 0..64 {
        let ty = tag.types.get(t)?;
        if let Some(v) = get(ty) {
            return Some(v);
        }
        if ty.parent == 0 {
            return None;
        }
        t = ty.parent;
    }
    None
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
        deleted: Vec::new(),
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
                    parts: Some(Vec::new()),
                    copy_of: None,
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
    // which parts have each collider
    for part in &scene.parts {
        for pc in &part.colliders {
            if let Some(c) = scene.colliders.iter_mut().find(|c| c.id == pc.id)
                && let Some(parts) = &mut c.parts
            {
                parts.push(part.name.clone());
            }
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

/// A collider a part gains: one of the file's, or one the scene adds (its index in the scene's colliders).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Gained {
    Existing(usize),
    New(usize),
}

/// What the written file has to show, part by part: its colliders (by name) and whether it meets each.
type ExpectedParts = Vec<(String, Vec<(String, bool)>)>;

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
    let name_of = |id: usize| {
        original
            .colliders
            .iter()
            .find(|c| c.id == id)
            .map_or_else(|| format!("collider {id}"), |c| c.name.clone())
    };

    // the scene's colliders and parts against the file's
    let mut new_names: Vec<&str> = Vec::new();
    for c in &edited.colliders {
        if c.id == 0 {
            if c.name.is_empty() || !c.name.is_ascii() {
                return Err(Error(format!(
                    "{source}: a new collider needs a name of plain letters, digits and underscores ({:?})",
                    c.name
                )));
            }
            if original.colliders.iter().any(|o| o.name == c.name)
                || new_names.contains(&c.name.as_str())
            {
                return Err(Error(format!(
                    "{source}: two colliders are named {}: rename the new one",
                    c.name
                )));
            }
            if c.bone.is_none() {
                return Err(Error(format!(
                    "{source}: the new collider {} has to follow a bone (parent it to one)",
                    c.name
                )));
            }
            if matches!(c.shape, SceneShape::Other { .. }) {
                return Err(Error(format!(
                    "{source}: the new collider {} has to be a capsule, a sphere or a plane",
                    c.name
                )));
            }
            if c.parts.as_ref().is_none_or(Vec::is_empty) {
                out.warnings.push(format!(
                    "{}: no part has the new collider, so it's left out (list the parts in its cloth_parts)",
                    c.name
                ));
            }
            new_names.push(&c.name);
        } else if !original.colliders.iter().any(|o| o.id == c.id) {
            return Err(Error(format!(
                "{source}: the file has no collider {} ({})",
                c.id, c.name
            )));
        }
        for part in c.parts.iter().flatten() {
            if !original.parts.iter().any(|o| &o.name == part) {
                return Err(Error(format!(
                    "{source}: {} lists a part the file doesn't have: {part}",
                    c.name
                )));
            }
        }
    }
    for p in &edited.parts {
        if !original.parts.iter().any(|o| o.name == p.name) {
            return Err(Error(format!("{source}: the file has no part {}", p.name)));
        }
    }
    for id in &edited.deleted {
        if !original.colliders.iter().any(|o| o.id == *id) {
            return Err(Error(format!(
                "{source}: the scene deleted collider {id}, which the file doesn't have"
            )));
        }
        out.changes.push(format!(
            "{}: deleted (no part collides with it any more)",
            name_of(*id)
        ));
    }
    // what each part gains, and which of its colliders it loses
    let mut gained: HashMap<&str, Vec<Gained>> = HashMap::new();
    for (k, c) in edited.colliders.iter().enumerate() {
        let Some(parts) = &c.parts else { continue };
        let had = original
            .colliders
            .iter()
            .find(|o| o.id == c.id)
            .and_then(|o| o.parts.clone())
            .unwrap_or_default();
        for part in parts {
            if c.id == 0 {
                gained.entry(part).or_default().push(Gained::New(k));
            } else if !had.contains(part) {
                gained.entry(part).or_default().push(Gained::Existing(c.id));
            }
        }
    }
    let lost = |part: &str, id: usize| {
        edited.deleted.contains(&id)
            || edited
                .colliders
                .iter()
                .find(|c| c.id == id)
                .and_then(|c| c.parts.as_ref())
                .is_some_and(|parts| !parts.iter().any(|p| p == part))
    };

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
    for c in edited.colliders.iter().filter(|c| c.id != 0) {
        let Some(o) = original.colliders.iter().find(|o| o.id == c.id) else {
            continue;
        };
        let moved = !same_all(&rigid(&c.place), &o.place) || c.bone != o.bone;
        let resized = !same_shape(&c.shape, &o.shape);
        if !moved && !resized {
            continue;
        }
        let place = rigid(&c.place);
        let collidable_value = tag.item(c.id).and_then(|a| a.get(0)).map_err(&e)?;
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
                let (bone, offset) = bone_offset(data, &skeletons, map.transform_set, c, &place, s)
                    .map_err(|m| Error(format!("{source}: {m}")))?;
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
            let f = transform_floats(t, &place).map_err(&e)?;
            patch.floats(t, &f)?;
            let bone_text = match (&o.bone, &c.bone) {
                (a, b) if a == b => String::new(),
                (_, Some(b)) => format!(", now on {b}"),
                (_, None) => ", now on no bone".to_owned(),
            };
            out.changes.push(format!(
                "{}: moved {:.1} cm{bone_text}",
                o.name,
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
            if std::mem::discriminant(&c.shape) != std::mem::discriminant(&o.shape) {
                return Err(Error(format!(
                    "{}: {} can't change its kind of shape",
                    source, o.name
                )));
            }
            for (field, floats) in shape_fields(&c.shape) {
                let v = path(shape, field).map_err(&e)?;
                if floats.len() == 1 {
                    patch.float(v, floats[0])?;
                } else {
                    patch.floats(v, &floats)?;
                }
            }
            out.changes.push(format!(
                "{}: {} -> {}",
                o.name,
                describe(&o.shape),
                describe(&c.shape)
            ));
        }
    }

    // parts
    let mut index = 0;
    let mut expected: ExpectedParts = Vec::new();
    for (d, data) in physics.cloths.iter().enumerate() {
        let (sim_values, simulates) = &values.datas[d];
        for (s, sim) in data.sim_cloths.iter().enumerate() {
            let o = &original.parts[index];
            index += 1;
            let p = edited.parts.iter().find(|p| p.name == o.name);
            let v = sim_values[s];
            let mut lines = Vec::new();
            if let Some(p) = p {
                settings(
                    &mut patch,
                    &e,
                    source,
                    p,
                    o,
                    sim,
                    v,
                    simulates,
                    s,
                    &mut lines,
                    &mut out.warnings,
                )?;
                let ids: Vec<usize> = o.colliders.iter().map(|c| c.id).collect();
                let edited_ids: Vec<usize> = p.colliders.iter().map(|c| c.id).collect();
                if edited_ids != ids {
                    return Err(Error(format!(
                        "{}: {}'s switches don't match its colliders (it has {})",
                        source,
                        p.name,
                        ids.iter()
                            .map(|id| name_of(*id))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )));
                }
            }
            // which colliders it meets: its switches, less what it lost, and what it gains
            let mut masks = sim.static_collision_masks.clone();
            let mut part_expected = Vec::new();
            for (slot, oc) in o.colliders.iter().enumerate() {
                let switched = p
                    .and_then(|p| p.colliders.get(slot))
                    .map_or(oc.meets, |c| c.meets);
                let gone = lost(&o.name, oc.id);
                let meets = switched && !gone;
                part_expected.push((name_of(oc.id), meets));
                if meets == oc.meets {
                    continue;
                }
                if slot >= 30 {
                    return Err(Error(format!(
                        "{}: {}: {} is its collider number {}; only its first 30 can be turned on or off",
                        source,
                        o.name,
                        name_of(oc.id),
                        slot + 1
                    )));
                }
                set_bit(&mut masks, &sim.particles, slot, meets);
                if !gone {
                    lines.push(format!(
                        "{} {}",
                        if meets { "meets" } else { "no longer meets" },
                        name_of(oc.id)
                    ));
                } else if !edited.deleted.contains(&oc.id) {
                    lines.push(format!("no longer has {}", name_of(oc.id)));
                }
            }
            for (j, g) in gained
                .get(o.name.as_str())
                .into_iter()
                .flatten()
                .enumerate()
            {
                let slot = o.colliders.len() + j;
                let name = match g {
                    Gained::Existing(id) => name_of(*id),
                    Gained::New(k) => edited.colliders[*k].name.clone(),
                };
                if slot >= 30 {
                    return Err(Error(format!(
                        "{source}: {} can't have more than 30 colliders ({name} would be its {})",
                        o.name,
                        slot + 1
                    )));
                }
                set_bit(&mut masks, &sim.particles, slot, true);
                part_expected.push((name.clone(), true));
                lines.push(format!("gets {name}"));
            }
            if masks != sim.static_collision_masks {
                let mask_values = v
                    .field("staticCollisionMasks")
                    .and_then(|a| a.array())
                    .map_err(&e)?;
                for (k, m) in masks.iter().enumerate() {
                    patch.uint(mask_values.get(k).map_err(&e)?, u64::from(*m))?;
                }
            }
            expected.push((o.name.clone(), part_expected));
            if !lines.is_empty() {
                out.changes
                    .push(format!("{}: {}", o.name, lines.join(", ")));
            }
        }
    }

    // new colliders: an object and a shape each (copies of a collider of the file), in the model's list
    let mut new_items: HashMap<usize, usize> = HashMap::new();
    let mut new_in_order: Vec<String> = Vec::new();
    for (k, c) in edited.colliders.iter().enumerate() {
        if c.id != 0 || c.parts.as_ref().is_none_or(Vec::is_empty) {
            continue;
        }
        let item = new_collidable(&mut patch, &tag, &original, &values, c)
            .map_err(|m| Error(format!("{source}: {}: {m}", c.name)))?;
        new_items.insert(k, item);
        new_in_order.push(c.name.clone());
        out.changes.push(format!(
            "{}: new, on {}: {}",
            c.name,
            c.bone.as_deref().unwrap_or("?"),
            describe(&c.shape)
        ));
        let container = (1..tag.items.len())
            .find(|&i| {
                tag.types
                    .get(tag.items[i].type_index)
                    .is_some_and(|t| t.name == "hclClothContainer")
            })
            .ok_or_else(|| Error(format!("{source}: the file has no hclClothContainer")))?;
        let container_value = tag.item(container).and_then(|a| a.get(0)).map_err(&e)?;
        patch
            .append_reference(&tag, container, container_value, "collidables", item)
            .map_err(|m| Error(format!("{source}: {m}")))?;
    }
    // each part's new colliders: the collider, its bone and offset, and the rest of its per-collider data
    let mut index = 0;
    for (d, data) in physics.cloths.iter().enumerate() {
        for (s, _) in data.sim_cloths.iter().enumerate() {
            let name = &original.parts[index].name;
            index += 1;
            let Some(list) = gained.get(name.as_str()) else {
                continue;
            };
            let sim_value = values.datas[d].0[s];
            let sim_item = *values
                .item_at
                .get(&sim_value.offset)
                .ok_or_else(|| Error(format!("{source}: {name}'s item")))?;
            let Some(map) = &data.sim_cloths[s].collidable_transform_map else {
                return Err(Error(format!(
                    "{source}: {name} keeps its colliders in place (no bones): it can't get new ones"
                )));
            };
            for g in list {
                let (item, collider) = match g {
                    Gained::Existing(id) => (
                        *id,
                        edited
                            .colliders
                            .iter()
                            .find(|c| c.id == *id)
                            .ok_or_else(|| Error(format!("{source}: collider {id}")))?,
                    ),
                    Gained::New(k) => (new_items[k], &edited.colliders[*k]),
                };
                let (bone, offset) = bone_offset(
                    data,
                    &skeletons,
                    map.transform_set,
                    collider,
                    &rigid(&collider.place),
                    s,
                )
                .map_err(|m| Error(format!("{source}: {m}")))?;
                patch
                    .add_to_part(&tag, sim_item, sim_value, item, bone, &offset)
                    .map_err(|m| Error(format!("{source}: {name}: {m}")))?;
            }
        }
    }

    if !patch.changed {
        return Ok(out);
    }
    let tagfile = patch.image.write().map_err(&e)?;
    let written = if new_in_order.is_empty() {
        phive::with_tagfile(bytes, &tagfile)
    } else {
        let params = add_to_params(file.params, &new_in_order)?;
        phive::with_parts(bytes, &tagfile, &params)
    }
    .map_err(|x| Error(format!("{source}: {x}")))?;
    check_written(&written, edited, &original, &expected)?;
    out.bytes = Some(written);
    Ok(out)
}

/// A part's settings (see [`apply`]): `lines` get what changed.
#[allow(
    clippy::too_many_arguments,
    reason = "the part's values in the file and in the scene"
)]
fn settings(
    patch: &mut Patcher,
    e: &impl Fn(havok_formats::Error) -> Error,
    source: &str,
    p: &ScenePart,
    o: &ScenePart,
    sim: &cloth::SimCloth,
    v: Value,
    simulates: &[(usize, Value)],
    s: usize,
    lines: &mut Vec<String>,
    warnings: &mut Vec<String>,
) -> Result<()> {
    if !same_all(&p.gravity, &o.gravity) {
        let info = v.field("simulationInfo").map_err(e)?;
        patch.vec3(info.field("gravity").map_err(e)?, p.gravity)?;
        lines.push(format!(
            "gravity ({:.2} {:.2} {:.2}) -> ({:.2} {:.2} {:.2})",
            o.gravity[0], o.gravity[1], o.gravity[2], p.gravity[0], p.gravity[1], p.gravity[2]
        ));
    }
    if !same(p.damping, o.damping) {
        let info = v.field("simulationInfo").map_err(e)?;
        patch.float(info.field("globalDampingPerSecond").map_err(e)?, p.damping)?;
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
                .map_err(e)?;
            for k in 0..configs.len() {
                let config = configs.get(k).map_err(e)?;
                patch.uint(config.field("subSteps").map_err(e)?, u64::from(p.substeps))?;
                patch.uint(
                    config.field("numberOfSolveIterations").map_err(e)?,
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
            .map_err(e)?;
        for (k, particle) in sim.particles.iter().enumerate() {
            let pv = particles.get(k).map_err(e)?;
            if !same(p.particle_radius, 1.0) {
                patch.float(
                    pv.field("radius").map_err(e)?,
                    particle.radius * p.particle_radius,
                )?;
            }
            if !same(p.friction, 1.0) {
                patch.float(
                    pv.field("friction").map_err(e)?,
                    (particle.friction * p.friction).clamp(0.0, 1.0),
                )?;
            }
        }
        if !same(p.particle_radius, 1.0) {
            patch.float(
                v.field("maxParticleRadius").map_err(e)?,
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
    if !(stretch || bend || swing) {
        return Ok(());
    }
    if p.stretch_stiffness < 0.0 || p.bend_stiffness < 0.0 || p.swing_range <= 0.0 {
        return Err(Error(format!(
            "{}: {}: stiffness scales of 0 or more and a swing range scale above 0",
            source, p.name
        )));
    }
    let sets = v
        .field("staticConstraintSets")
        .and_then(|a| a.array())
        .map_err(e)?;
    let mut untouched = Vec::new();
    for k in 0..sets.len() {
        let set = sets.get(k).and_then(|x| x.dereference()).map_err(e)?;
        let class = set.type_name().to_owned();
        let scale_links = |patch: &mut Patcher, field: &str, by: f32, cap: bool| -> Result<()> {
            let links = set.field("links").and_then(|a| a.array()).map_err(e)?;
            for l in 0..links.len() {
                let f = links.get(l).and_then(|x| x.field(field)).map_err(e)?;
                let old = f.float().map_err(e)?;
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
                    scale_links(patch, "stiffness", p.stretch_stiffness, true)?;
                }
            }
            "hclBendLinkConstraintSet" => {
                if stretch {
                    scale_links(patch, "stretchStiffness", p.stretch_stiffness, true)?;
                }
                if bend {
                    scale_links(patch, "bendStiffness", p.bend_stiffness, true)?;
                }
            }
            "hclBendStiffnessConstraintSet" => {
                if bend {
                    scale_links(patch, "bendStiffness", p.bend_stiffness, false)?;
                }
            }
            "hclLocalRangeConstraintSet" => {
                if swing {
                    for list in ["localConstraints", "localStiffnessConstraints"] {
                        if !set.has_field(list) {
                            continue;
                        }
                        let entries = set.field(list).and_then(|a| a.array()).map_err(e)?;
                        for k in 0..entries.len() {
                            let entry = entries.get(k).map_err(e)?;
                            for field in [
                                "maximumDistance",
                                "shapeRadius",
                                "maxNormalDistance",
                                "minNormalDistance",
                            ] {
                                if entry.has_field(field) {
                                    let f = entry.field(field).map_err(e)?;
                                    let old = f.float().map_err(e)?;
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
        warnings.push(format!(
            "{}: its {} kept as the game has them (the scales don't reach them)",
            p.name,
            untouched.join(", ")
        ));
    }
    Ok(())
}

/// Bit `slot` of the masks: set on the free particles (fixed ones follow their bones and never collide), or cleared on
/// all.
fn set_bit(masks: &mut [u32], particles: &[cloth::Particle], slot: usize, on: bool) {
    let bit = 1u32 << slot;
    for (m, particle) in masks.iter_mut().zip(particles) {
        if !on {
            *m &= !bit;
        } else if particle.inv_mass > 0.0 {
            *m |= bit;
        }
    }
}

/// The bone a collider follows in a part (its index in the part's skeleton) and its offset from it.
fn bone_offset(
    data: &cloth::Cloth,
    skeletons: &[animation::Skeleton],
    set: u32,
    c: &SceneCollider,
    place: &Mat4,
    sim: usize,
) -> std::result::Result<(usize, Mat4), String> {
    let rest = rest_sets(data, skeletons);
    let part = part_name(data, sim);
    let Some((names, places)) = rest.get(set as usize).and_then(Option::as_ref) else {
        return Err(format!("{part}'s skeleton isn't in the file"));
    };
    let Some(bone_name) = &c.bone else {
        return Err(format!("{} has to follow a bone in {part}", c.name));
    };
    let Some(bone) = names.iter().position(|n| n == bone_name) else {
        return Err(format!(
            "{} can't follow {bone_name}: {part}'s skeleton doesn't have that bone",
            c.name
        ));
    };
    let inverse = math::inverse_affine(&places[bone])
        .ok_or_else(|| format!("bone {bone_name} can't be inverted"))?;
    Ok((bone, rigid(&math::mul(&inverse, place))))
}

/// The floats of a collidable's `transform` for `place`: an `hkPreciseTransform`'s first 19 (the rotation's columns
/// and the translation, their fourth lanes kept, the translation's adjustment zeroed), or an `hkTransform`'s 16.
fn transform_floats(t: Value, place: &Mat4) -> havok_formats::Result<Vec<f32>> {
    if t.type_name() == "hkPreciseTransform" {
        let mut f = t.vector::<20>()?;
        for col in 0..3 {
            f[col * 4..col * 4 + 3].copy_from_slice(&place[col * 4..col * 4 + 3]);
        }
        f[12..15].copy_from_slice(&place[12..15]);
        f[16..19].fill(0.0);
        Ok(f[..19].to_vec())
    } else {
        let mut f = t.vector::<16>()?;
        for col in 0..4 {
            f[col * 4..col * 4 + 3].copy_from_slice(&place[col * 4..col * 4 + 3]);
        }
        Ok(f.to_vec())
    }
}

/// A member of `v` by a dotted path (`sphere.pos`).
fn path<'t, 'a>(v: Value<'t, 'a>, path: &str) -> havok_formats::Result<Value<'t, 'a>> {
    let mut v = v;
    for name in path.split('.') {
        v = v.field(name)?;
    }
    Ok(v)
}

/// The fields of a shape object for `shape`, and their floats (a vector's first three, or all four).
fn shape_fields(shape: &SceneShape) -> Vec<(&'static str, Vec<f32>)> {
    match shape {
        SceneShape::Capsule { start, end, radius } => {
            let (dir, inverse_len_sq) = capsule_axis(*start, *end);
            vec![
                ("start", start.to_vec()),
                ("end", end.to_vec()),
                ("dir", dir.to_vec()),
                ("radius", vec![*radius]),
                ("capLenSqrdInv", vec![inverse_len_sq]),
            ]
        }
        SceneShape::Sphere { centre, radius } => {
            vec![("sphere.pos", vec![centre[0], centre[1], centre[2], *radius])]
        }
        SceneShape::Plane { normal, distance } => {
            vec![(
                "planeEquation",
                vec![normal[0], normal[1], normal[2], *distance],
            )]
        }
        SceneShape::Other { .. } => Vec::new(),
    }
}

/// A shape in words, for the build report.
fn describe(shape: &SceneShape) -> String {
    match shape {
        SceneShape::Capsule { start, end, radius } => {
            let d = sub(*end, *start);
            format!(
                "capsule of radius {:.1} cm, {:.1} cm long",
                radius * 100.0,
                (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() * 100.0
            )
        }
        SceneShape::Sphere { radius, .. } => format!("sphere of radius {:.1} cm", radius * 100.0),
        SceneShape::Plane { .. } => "plane".to_owned(),
        SceneShape::Other { class } => class.clone(),
    }
}

/// The class of shape object a scene shape is written as.
fn shape_class(shape: &SceneShape) -> &'static str {
    match shape {
        SceneShape::Capsule { .. } => "hclCapsuleShape",
        SceneShape::Sphere { .. } => "hclSphereShape",
        SceneShape::Plane { .. } => "hclPlaneShape",
        SceneShape::Other { .. } => "",
    }
}

/// A new collidable for `c`: a copy of one of the file's (the one it was copied from, else the first with a shape of
/// its kind) with its name, place and a copy of a shape object of its class (a sphere is written as a capsule with no
/// length where the file has no sphere to copy). Returns its item.
fn new_collidable(
    patch: &mut Patcher,
    tag: &Tagfile,
    original: &ClothScene,
    values: &Values,
    c: &SceneCollider,
) -> std::result::Result<usize, String> {
    let s = |x: havok_formats::Error| x.to_string();
    let object = |id: usize| tag.item(id).and_then(|a| a.get(0)).map_err(s);
    let shape_of_id = |id: usize| -> std::result::Result<Value, String> {
        object(id)?
            .field("shape")
            .and_then(|v| v.dereference())
            .map_err(s)
    };
    let wanted = shape_class(&c.shape);
    let template = c
        .copy_of
        .filter(|id| original.colliders.iter().any(|o| o.id == *id))
        .or_else(|| {
            original
                .colliders
                .iter()
                .find(|o| shape_class(&o.shape) == wanted)
                .or_else(|| original.colliders.first())
                .map(|o| o.id)
        })
        .ok_or("the file has no collider to copy")?;
    let collidable = object(template)?;
    // the shape: the template's when it's of the class wanted, else any of that class, else (a sphere) a capsule
    let mut shape = c.shape.clone();
    let shape_template = std::iter::once(template)
        .chain(original.colliders.iter().map(|o| o.id))
        .filter_map(|id| shape_of_id(id).ok())
        .find(|v| v.type_name() == wanted);
    let shape_template = match (shape_template, &c.shape) {
        (Some(v), _) => v,
        (None, SceneShape::Sphere { centre, radius }) => {
            shape = SceneShape::Capsule {
                start: *centre,
                end: *centre,
                radius: *radius,
            };
            original
                .colliders
                .iter()
                .filter_map(|o| shape_of_id(o.id).ok())
                .find(|v| v.type_name() == "hclCapsuleShape")
                .ok_or("the file has no sphere or capsule to copy")?
        }
        (None, _) => {
            return Err(format!(
                "the file has no {wanted} to copy: make it another kind of shape"
            ));
        }
    };
    let item_of = |v: Value| {
        values
            .item_at
            .get(&v.offset)
            .copied()
            .ok_or("an object's item")
    };
    let template_item = item_of(collidable)?;
    let shape_item = item_of(shape_template)?;
    let new = patch.copy_item(template_item);
    let new_shape = patch.copy_item(shape_item);
    let rel = |v: Value, of: Value| v.offset - of.offset;
    let name = collidable.field("name").map_err(s)?;
    patch
        .image
        .set_string(new, rel(name, collidable), &c.name)
        .map_err(s)?;
    let t = collidable.field("transform").map_err(s)?;
    let floats = transform_floats(t, &rigid(&c.place)).map_err(s)?;
    let elements = t.array().map_err(s)?;
    for (i, x) in floats.iter().enumerate() {
        let at = rel(elements.get(i).map_err(s)?, collidable);
        patch.write_at(new, at, &x.to_le_bytes())?;
    }
    for (field, floats) in shape_fields(&shape) {
        let v = path(shape_template, field).map_err(s)?;
        if floats.len() == 1 {
            v.float().map_err(s)?;
            patch.write_at(new_shape, rel(v, shape_template), &floats[0].to_le_bytes())?;
        } else {
            let elements = v.array().map_err(s)?;
            for (i, x) in floats.iter().enumerate() {
                let at = rel(elements.get(i).map_err(s)?, shape_template);
                patch.write_at(new_shape, at, &x.to_le_bytes())?;
            }
        }
    }
    let shape_slot = collidable.field("shape").map_err(s)?;
    patch
        .image
        .set_reference(new, rel(shape_slot, collidable), new_shape)
        .map_err(s)?;
    Ok(new)
}

/// The cloth file's parameter archive with `names` added to its list of colliders (keyed `collidable_<n>`, each a
/// `Name` and `ForReplace` off, as the game's are).
fn add_to_params(params: &[u8], names: &[String]) -> Result<Vec<u8>> {
    use modstudio_formats::aamp::{self, Aamp, Object, Param};
    let mut archive =
        Aamp::read(params).map_err(|e| Error(format!("the cloth file's parameters: {e}")))?;
    let first = aamp::hash("collidable_0");
    // the list of colliders: the one whose first object is keyed so (the game's files' is 0x066bac7e)
    let list = archive
        .root
        .lists
        .iter_mut()
        .find(|l| {
            l.objects.first().is_some_and(|o| o.hash == first)
                || (l.objects.is_empty() && l.hash == 0x066b_ac7e)
        })
        .ok_or_else(|| Error("the cloth file's parameters have no list of colliders".to_owned()))?;
    for name in names {
        let k = list.objects.len();
        let mut params = list.objects.first().map_or_else(
            || {
                vec![
                    Param {
                        hash: aamp::hash("Name"),
                        kind: 20,
                        value: Vec::new(),
                    },
                    Param {
                        hash: aamp::hash("ForReplace"),
                        kind: 0,
                        value: 0u32.to_le_bytes().to_vec(),
                    },
                ]
            },
            |o| o.params.clone(),
        );
        for p in &mut params {
            if p.hash == aamp::hash("Name") {
                p.value = name.as_bytes().to_vec();
            } else if p.hash == aamp::hash("ForReplace") {
                p.value = 0u32.to_le_bytes().to_vec();
            }
        }
        list.objects.push(Object {
            hash: aamp::hash(&format!("collidable_{k}")),
            params,
        });
    }
    Ok(archive.write())
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

/// The written file read back: it has to read, and show what the scene shows: every collider (the file's by their
/// names in the file, the new ones by theirs) where the scene has it, and each part's colliders and switches as
/// `expected` has them.
fn check_written(
    written: &[u8],
    edited: &ClothScene,
    original: &ClothScene,
    expected: &ExpectedParts,
) -> Result<()> {
    let back = scene_of(written, &edited.source)
        .map_err(|e| Error(format!("the written file doesn't read: {e}")))?;
    let bug = |what: &str| {
        Error(format!(
            "the written file has {what} otherwise than the scene (a bug: nothing was written)"
        ))
    };
    for o in &original.colliders {
        let want = edited.colliders.iter().find(|c| c.id == o.id).unwrap_or(o);
        let Some(b) = back.colliders.iter().find(|b| b.name == o.name) else {
            return Err(bug(&format!("{} (it's gone)", o.name)));
        };
        if b.bone != want.bone
            || !same_all(&b.place, &rigid(&want.place))
            || !same_shape(&b.shape, &want.shape)
        {
            return Err(bug(&format!("{}'s place or shape", o.name)));
        }
    }
    for c in edited
        .colliders
        .iter()
        .filter(|c| c.id == 0 && c.parts.as_ref().is_some_and(|p| !p.is_empty()))
    {
        let Some(b) = back.colliders.iter().find(|b| b.name == c.name) else {
            return Err(bug(&format!("the new {} (it isn't there)", c.name)));
        };
        // (a sphere the file had no sphere for comes back a capsule with no length)
        let shape_ok = same_shape(&b.shape, &c.shape)
            || matches!((&b.shape, &c.shape), (
                SceneShape::Capsule { start, end, radius: r1 },
                SceneShape::Sphere { centre, radius: r2 },
            ) if same_all(start, centre) && same_all(end, centre) && same(*r1, *r2));
        if b.bone != c.bone || !same_all(&b.place, &rigid(&c.place)) || !shape_ok {
            return Err(bug(&format!("the new {}'s place or shape", c.name)));
        }
    }
    for (o, b) in original.parts.iter().zip(&back.parts) {
        let want = edited.parts.iter().find(|p| p.name == o.name).unwrap_or(o);
        if !same_all(&b.gravity, &want.gravity)
            || !same(b.damping, want.damping)
            || b.substeps != want.substeps
            || b.iterations != want.iterations
        {
            return Err(bug(&format!("{}'s settings", o.name)));
        }
        let Some((_, colliders)) = expected.iter().find(|(name, _)| name == &o.name) else {
            continue;
        };
        let got: Vec<(String, bool)> = b
            .colliders
            .iter()
            .map(|pc| {
                let name = back
                    .colliders
                    .iter()
                    .find(|c| c.id == pc.id)
                    .map_or_else(String::new, |c| c.name.clone());
                (name, pc.meets)
            })
            .collect();
        let limit = |v: &[(String, bool)]| v.iter().take(30).cloned().collect::<Vec<_>>();
        if limit(&got) != limit(colliders) {
            return Err(bug(&format!("{}'s colliders", o.name)));
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
