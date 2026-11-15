//! New simulated parts in a model's cloth file, for chains of bones added to the model (`docs/plan.md`, new bones,
//! step 5).
//!
//! Every part of the game's 171 cloth files (1,134) is a cloth data of its own, built alike: one sim cloth; operators
//! skin (the particles' places as their bones carry them), move the fixed particles, simulate, set bones from
//! triangles, and two vertex copies; states `Default`, `AnimToSimCurrent` and `AnimToSimPrev`; three buffers; one
//! transform set, which is the part's own skeleton (the bones it uses and their parents up to the root, matched to the
//! model's by name). So a new part is a copy of one of the file's own (the smallest whose skin moves positions alone:
//! in object space where the file has one, else in bone space), every array of it made again for the new part:
//!
//! - **a strip** of particles two wide down the chain: at each joint (each bone's head, then the last bone's tip) one
//!   on the chain and one beside it (`across`); the top two fixed, carried by the bone the chain hangs from (its
//!   anchor), and with a stiffness a third (the brace) off the strip beside the top; two triangles a bone, the bone set from the one at its head (its stored matrix: the inverse of that
//!   triangle's frame at rest times the bone's rest place, `havok-cloth::deform`);
//! - **constraints**: links along every edge of the strip, with a stiffness links across each joint and from the
//!   brace to the first bone's tip (soft: they pull the chain back to its shape at rest), a stretch link down the
//!   chain, a range keeping each free particle near where the anchor would carry it at rest (half its distance down
//!   the chain), and the transition set (all particles); run in the order the game's parts run them;
//! - **skinning**: every particle on the anchor alone, in blocks of the fewest bones a vertex whose type the file
//!   spells out (the other bones the anchor too, without weight): in object space its place in the model packed as
//!   an `hkPackedVector3` and the anchor's inverse rest place; in bone space its place relative to the anchor, its
//!   weight in the fourth value;
//! - **colliders**: the file's own within reach of the chain (the nearest first, up to 22 as the game's parts have
//!   at most; capsules and spheres on a bone), but those the chain starts inside of (its anchor's own, mostly: they
//!   would throw it off at the first step); its free particles meet them all; no landscape collision, no pinch
//!   detection;
//! - **the skeleton**: the anchor and its parents from the root, then the chain; the transform set and every
//!   operator's and state's record of the bones it reads and writes made for it.
//!
//! The part is listed in the cloth container and its skeleton in the animation container; the parameter archive gets
//! an entry for it, a copy of the template's with its name and base bone (the anchor). The actor pack's `ClothParam`
//! isn't changed.

use std::collections::HashMap;

use havok_formats::cloth::{self, ConstraintKind, OperatorKind};
use havok_formats::{ImageItem, ItemKind, Tagfile, TagfileImage, Value};
use modstudio_formats::aamp::{self, Aamp};
use modstudio_formats::phive::{self, Phive};

use crate::math::{self, Mat4, Quat};
use crate::{Error, Result};

/// A bone of the model, at rest.
#[derive(Debug, Clone)]
pub struct RestBone {
    pub name: String,
    pub parent: Option<usize>,
    /// Its place relative to its parent: translation, rotation, scale.
    pub translation: [f32; 3],
    pub rotation: Quat,
    pub scale: [f32; 3],
}

/// A part's settings as the scene gives them (custom properties of the chain's top bone, `scene_tools.py`), each
/// optional: `swing`, a share of each point's distance down the chain it may swing from where its anchor would
/// carry it; `stiffness`, 0 to 1, how firmly it keeps the shape it was made in (its first bone's angle to the bone it
/// hangs from, and the angles at its joints: 0 hangs freely, a bone that sticks out springs back with more);
/// `gravity`, the game's times this; `colliders`, `auto`, `none` or collider names separated by commas.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SceneClothSettings {
    #[serde(default)]
    pub swing: Option<f32>,
    #[serde(default)]
    pub stiffness: Option<f32>,
    #[serde(default)]
    pub gravity: Option<f32>,
    #[serde(default)]
    pub colliders: Option<String>,
}

/// Which colliders a new part meets.
#[derive(Debug, Clone, PartialEq)]
pub enum ColliderChoice {
    /// The file's within reach of the chain, the nearest first ([`MOST_COLLIDERS`] at most), but those it starts
    /// inside of.
    Auto,
    None,
    /// These, by name.
    Named(Vec<String>),
}

/// A new part's settings.
#[derive(Debug, Clone, PartialEq)]
pub struct PartSettings {
    pub swing: f32,
    pub stiffness: f32,
    pub gravity: f32,
    pub colliders: ColliderChoice,
}

impl Default for PartSettings {
    fn default() -> Self {
        PartSettings {
            swing: 0.5,
            stiffness: 0.0,
            gravity: 1.0,
            colliders: ColliderChoice::Auto,
        }
    }
}

impl PartSettings {
    /// The scene's settings, the defaults where it gives none (values kept to what makes sense).
    pub fn from_scene(s: &SceneClothSettings) -> PartSettings {
        let d = PartSettings::default();
        let colliders = match s.colliders.as_deref().map(str::trim) {
            None | Some("") => ColliderChoice::Auto,
            Some(x) if x.eq_ignore_ascii_case("auto") => ColliderChoice::Auto,
            Some(x) if x.eq_ignore_ascii_case("none") => ColliderChoice::None,
            Some(x) => ColliderChoice::Named(
                x.split([',', ';'])
                    .map(|n| n.trim().to_owned())
                    .filter(|n| !n.is_empty())
                    .collect(),
            ),
        };
        PartSettings {
            swing: s
                .swing
                .filter(|x| x.is_finite())
                .map_or(d.swing, |x| x.clamp(0.01, 4.0)),
            stiffness: s
                .stiffness
                .filter(|x| x.is_finite())
                .map_or(d.stiffness, |x| x.clamp(0.0, 1.0)),
            gravity: s
                .gravity
                .filter(|x| x.is_finite())
                .map_or(d.gravity, |x| x.clamp(0.0, 10.0)),
            colliders,
        }
    }
}

/// A chain of bones to simulate as a new part.
#[derive(Debug, Clone)]
pub struct NewPart {
    /// The part's name (the game's are `SimulationMesh_<what>`).
    pub name: String,
    /// The bone the chain hangs from (an index into the skeleton).
    pub anchor: usize,
    /// The chain's bones, top first, each the one before's child (the first the anchor's).
    pub bones: Vec<usize>,
    /// The chain's joints at rest, in the model: each bone's head, then the last bone's tip.
    pub joints: Vec<[f32; 3]>,
    /// For each joint, where its second particle is: the joint plus this.
    pub across: Vec<[f32; 3]>,
    pub settings: PartSettings,
}

/// A model's skeleton as [`RestBone`]s (Euler angles turned into quaternions).
pub fn rest_bones(skeleton: &modstudio_formats::bfres::Skeleton) -> Vec<RestBone> {
    let euler = skeleton.euler_rotations();
    skeleton
        .bones
        .iter()
        .map(|b| RestBone {
            name: b.name.clone(),
            parent: b.parent,
            translation: b.translation,
            rotation: if euler {
                math::quat_from_euler_xyz([b.rotation[0], b.rotation[1], b.rotation[2]])
            } else {
                b.rotation
            },
            scale: b.scale,
        })
        .collect()
}

/// The prefix of the bones a chain is made of: the game's own for bones its physics moves.
pub const SIMULATED_PREFIX: &str = "Ph_";

/// The chains of `added` bones (names) of `skeleton` to simulate (named [`SIMULATED_PREFIX`]..., or in `ticked`: "Simulate
/// as cloth" ticked in the scene), as parts: each starts at such a bone whose parent isn't one, hangs from that parent
/// (its anchor), and goes on down the only such child of each bone. Its joints are the bones' heads and the last one's tip (`tips`, in the model's space; else as far on from
/// its head as the bone before it was long, or a tenth of the chain's height down); the strip across it is a quarter
/// of its first bone's length wide, along the model's x axis (across the chain); its settings those of the topmost
/// bone of it in `settings` (the scene's), the defaults else. Notes on what was left out.
pub fn chains_of(
    skeleton: &[RestBone],
    added: &[String],
    ticked: &[String],
    tips: &std::collections::BTreeMap<String, [f32; 3]>,
    settings: &std::collections::BTreeMap<String, SceneClothSettings>,
) -> (Vec<NewPart>, Vec<String>) {
    let world = rest_world(skeleton);
    let head = |i: usize| [world[i][12], world[i][13], world[i][14]];
    let simulated = |i: usize| {
        added.contains(&skeleton[i].name)
            && (skeleton[i].name.starts_with(SIMULATED_PREFIX)
                || ticked.contains(&skeleton[i].name))
    };
    let children = |i: usize| -> Vec<usize> {
        (0..skeleton.len())
            .filter(|&c| skeleton[c].parent == Some(i) && simulated(c))
            .collect()
    };
    let mut parts = Vec::new();
    let mut notes = Vec::new();
    for start in 0..skeleton.len() {
        if !simulated(start) {
            continue;
        }
        let Some(anchor) = skeleton[start].parent.filter(|&p| !simulated(p)) else {
            continue;
        };
        let mut bones = vec![start];
        loop {
            let last = *bones.last().expect("one");
            let next = children(last);
            if next.len() > 1 {
                notes.push(format!(
                    "{} has {} simulated bones under it: the chain goes on down {}; {} follow it",
                    skeleton[last].name,
                    next.len(),
                    skeleton[next[0]].name,
                    next[1..]
                        .iter()
                        .map(|&c| skeleton[c].name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            match next.first() {
                Some(&c) if !bones.contains(&c) && bones.len() < 255 => bones.push(c),
                _ => break,
            }
        }
        let mut joints: Vec<[f32; 3]> = bones.iter().map(|&b| head(b)).collect();
        let last = *bones.last().expect("one");
        let tip = match tips.get(&skeleton[last].name) {
            Some(&t) if distance(t, head(last)) > 1e-4 => t,
            _ => {
                let before = if bones.len() > 1 {
                    head(bones[bones.len() - 2])
                } else {
                    head(anchor)
                };
                let h = head(last);
                let d = [h[0] - before[0], h[1] - before[1], h[2] - before[2]];
                let len = distance(h, before);
                notes.push(format!(
                    "{}: its tip isn't known (the scene's export has no bones.json); taken as far on as the bone before it",
                    skeleton[last].name
                ));
                if len > 1e-4 {
                    [h[0] + d[0], h[1] + d[1], h[2] + d[2]]
                } else {
                    [h[0], h[1] - 0.1, h[2]]
                }
            }
        };
        joints.push(tip);
        let first = distance(joints[0], joints[1]).max(1e-3);
        let down = math::normalize3([
            joints[1][0] - joints[0][0],
            joints[1][1] - joints[0][1],
            joints[1][2] - joints[0][2],
        ]);
        // the model's x axis, less its part along the chain (its z where the chain runs along x)
        let pick = |a: [f32; 3]| {
            let k = a[0] * down[0] + a[1] * down[1] + a[2] * down[2];
            [a[0] - k * down[0], a[1] - k * down[1], a[2] - k * down[2]]
        };
        let mut side = pick([1.0, 0.0, 0.0]);
        if (side[0] * side[0] + side[1] * side[1] + side[2] * side[2]).sqrt() < 0.3 {
            side = pick([0.0, 0.0, 1.0]);
        }
        let side = math::normalize3(side);
        let width = (0.25 * first).max(0.005);
        let across = vec![[side[0] * width, side[1] * width, side[2] * width]; joints.len()];
        let what = skeleton[start]
            .name
            .strip_prefix(SIMULATED_PREFIX)
            .unwrap_or(&skeleton[start].name);
        // the settings of the topmost bone of the chain that has any
        let part_settings = bones
            .iter()
            .find_map(|&b| settings.get(&skeleton[b].name))
            .map(PartSettings::from_scene)
            .unwrap_or_default();
        parts.push(NewPart {
            name: format!("SimulationMesh_{what}"),
            anchor,
            bones,
            joints,
            across,
            settings: part_settings,
        });
    }
    (parts, notes)
}

/// A collider of the file a new part meets: its item, its name, the model's bone it follows and its place relative to
/// that bone.
#[derive(Debug, Clone)]
struct PartCollider {
    item: usize,
    name: String,
    bone: usize,
    offset: Mat4,
}

/// The most colliders a part of the game's has.
const MOST_COLLIDERS: usize = 22;

/// The distance from `p` to the segment `a`-`b`.
fn to_segment(p: [f32; 3], a: [f32; 3], b: [f32; 3]) -> f32 {
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let ap = [p[0] - a[0], p[1] - a[1], p[2] - a[2]];
    let len2 = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
    let t = if len2 > 0.0 {
        ((ap[0] * ab[0] + ap[1] * ab[1] + ap[2] * ab[2]) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    distance(p, [a[0] + ab[0] * t, a[1] + ab[1] * t, a[2] + ab[2] * t])
}

/// The colliders of the file (`colliders`, as its scene has them) a new part meets: on a bone of `skeleton`, a capsule
/// or a sphere, within reach of the chain (from its anchor, the chain's length and a particle's radius), but not one
/// a free particle of the strip (`free`, their places) starts inside of; the nearest first, at most [`MOST_COLLIDERS`]. Notes on the ones
/// left out.
fn colliders_for(
    colliders: &[crate::cloth_edit::SceneCollider],
    skeleton: &[RestBone],
    world: &[Mat4],
    part: &NewPart,
    free: &[[f32; 3]],
    particle_radius: f32,
) -> (Vec<PartCollider>, Vec<String>) {
    use crate::cloth_edit::SceneShape;
    let reach: f32 = part
        .joints
        .windows(2)
        .map(|w| distance(w[0], w[1]))
        .sum::<f32>()
        + particle_radius;
    let mut found: Vec<(f32, PartCollider)> = Vec::new();
    let mut inside = Vec::new();
    for c in colliders {
        let Some(bone) = c
            .bone
            .as_ref()
            .and_then(|n| skeleton.iter().position(|b| &b.name == n))
        else {
            continue;
        };
        let (a, b, r) = match &c.shape {
            SceneShape::Capsule { start, end, radius } => (
                math::transform_point(&c.place, *start),
                math::transform_point(&c.place, *end),
                *radius,
            ),
            SceneShape::Sphere { centre, radius } => {
                let p = math::transform_point(&c.place, *centre);
                (p, p, *radius)
            }
            _ => continue,
        };
        let near = to_segment(part.joints[0], a, b) - r;
        if near > reach {
            continue;
        }
        if free
            .iter()
            .any(|&p| to_segment(p, a, b) < r + particle_radius)
        {
            inside.push(c.name.clone());
            continue;
        }
        let Some(inverse) = math::inverse_affine(&world[bone]) else {
            continue;
        };
        found.push((
            near,
            PartCollider {
                item: c.id,
                name: c.name.clone(),
                bone,
                offset: math::mul(&inverse, &c.place),
            },
        ));
    }
    found.sort_by(|x, y| x.0.total_cmp(&y.0));
    let mut notes = Vec::new();
    if found.len() > MOST_COLLIDERS {
        notes.push(format!(
            "{}: {} colliders within reach; the nearest {MOST_COLLIDERS} are used",
            part.name,
            found.len()
        ));
        found.truncate(MOST_COLLIDERS);
    }
    if !inside.is_empty() {
        notes.push(format!(
            "{}: it starts inside {} (left out: they would throw it off)",
            part.name,
            inside.join(", ")
        ));
    }
    (found.into_iter().map(|(_, c)| c).collect(), notes)
}

/// The colliders of the file named in `names` (a part's choice in the scene), on bones of `skeleton`, any shape, at
/// most 30 (what a particle's mask holds besides the landscape's bit). Notes on names not found and the rest.
fn named_colliders(
    colliders: &[crate::cloth_edit::SceneCollider],
    skeleton: &[RestBone],
    world: &[Mat4],
    part: &NewPart,
    names: &[String],
) -> (Vec<PartCollider>, Vec<String>) {
    let mut out = Vec::new();
    let mut notes = Vec::new();
    for name in names {
        let Some(c) = colliders.iter().find(|c| &c.name == name) else {
            notes.push(format!(
                "{}: no collider {name} in the cloth file",
                part.name
            ));
            continue;
        };
        let Some(bone) = c
            .bone
            .as_ref()
            .and_then(|n| skeleton.iter().position(|b| &b.name == n))
        else {
            notes.push(format!(
                "{}: {name} follows no bone of the model; left out",
                part.name
            ));
            continue;
        };
        let Some(inverse) = math::inverse_affine(&world[bone]) else {
            continue;
        };
        if out.iter().any(|x: &PartCollider| x.item == c.id) {
            continue;
        }
        out.push(PartCollider {
            item: c.id,
            name: c.name.clone(),
            bone,
            offset: math::mul(&inverse, &c.place),
        });
    }
    if out.len() > 30 {
        notes.push(format!(
            "{}: {} colliders named; the first 30 are used (a particle meets 30 at most)",
            part.name,
            out.len()
        ));
        out.truncate(30);
    }
    (out, notes)
}

/// Each bone's place in the model at rest.
pub fn rest_world(skeleton: &[RestBone]) -> Vec<Mat4> {
    let mut world: Vec<Option<Mat4>> = vec![None; skeleton.len()];
    fn place(s: &[RestBone], i: usize, world: &mut Vec<Option<Mat4>>, depth: usize) -> Mat4 {
        if let Some(m) = world[i] {
            return m;
        }
        let b = &s[i];
        let local = math::from_trs(b.translation, b.rotation, b.scale);
        let m = match b.parent {
            Some(p) if p != i && depth < 1024 => math::mul(&place(s, p, world, depth + 1), &local),
            _ => local,
        };
        world[i] = Some(m);
        m
    }
    (0..skeleton.len())
        .map(|i| place(skeleton, i, &mut world, 0))
        .collect()
}

fn fail(what: &str) -> impl Fn(havok_formats::Error) -> Error + '_ {
    move |e| Error(format!("{what}: {e}"))
}

/// An editable copy of a tagfile, with the items of one part copied from a template's: values are found through the
/// template (as the reader sees them) and written into the copies.
struct Editor<'t, 'a> {
    tag: &'t Tagfile<'a>,
    image: TagfileImage,
    /// Each item's place in DATA (start, end, item), by start.
    spans: Vec<(usize, usize, usize)>,
    /// The template's items copied for the part being made: template item -> its copy.
    copies: HashMap<usize, usize>,
}

impl<'t, 'a> Editor<'t, 'a> {
    fn new(tag: &'t Tagfile<'a>) -> Result<Self> {
        let image = tag.image().map_err(fail("the cloth file"))?;
        let mut spans: Vec<(usize, usize, usize)> = tag
            .items
            .iter()
            .enumerate()
            .skip(1)
            .map(|(i, item)| (item.offset, item.offset + image.items[i].data.len(), i))
            .collect();
        spans.sort_unstable();
        Ok(Editor {
            tag,
            image,
            spans,
            copies: HashMap::new(),
        })
    }

    /// The item holding a value of the file, and the value's byte in it.
    fn locate(&self, v: Value) -> Result<(usize, usize)> {
        let k = self.spans.partition_point(|s| s.0 <= v.offset);
        let &(start, end, item) = k
            .checked_sub(1)
            .and_then(|k| self.spans.get(k))
            .filter(|s| v.offset < s.1.max(s.0 + 1))
            .ok_or_else(|| Error(format!("no item holds DATA offset {:#x}", v.offset)))?;
        let _ = end;
        Ok((item, v.offset - start))
    }

    /// Where a template value is in the part's copy: the copied item and the byte.
    fn in_copy(&self, v: Value) -> Result<(usize, usize)> {
        let (item, rel) = self.locate(v)?;
        let copy = self
            .copies
            .get(&item)
            .copied()
            .ok_or_else(|| Error(format!("item {item} wasn't copied for the new part")))?;
        Ok((copy, rel))
    }

    /// A copy of `item` and of everything it refers to (but items `keep` says to share), recorded in `copies`.
    fn deep_copy(
        &mut self,
        item: usize,
        keep: &dyn Fn(&TagfileImage, usize) -> bool,
    ) -> Result<usize> {
        if let Some(&c) = self.copies.get(&item) {
            return Ok(c);
        }
        let copy = self.image.push_item(self.image.items[item].clone());
        self.copies.insert(item, copy);
        let refs = self
            .image
            .references_of(item)
            .map_err(fail("copying a part"))?;
        for (position, target) in refs {
            if keep(&self.image, target) {
                continue;
            }
            let t = self.deep_copy(target, keep)?;
            let data = &mut self.image.items[copy].data;
            data[position..position + 4].copy_from_slice(&(t as u32).to_le_bytes());
        }
        Ok(copy)
    }

    fn put(&mut self, v: Value, bytes: &[u8]) -> Result<()> {
        let (item, rel) = self.in_copy(v)?;
        let data = &mut self.image.items[item].data;
        data.get_mut(rel..rel + bytes.len())
            .ok_or_else(|| Error(format!("item {item}: byte {rel} is outside it")))?
            .copy_from_slice(bytes);
        Ok(())
    }

    fn put_f32(&mut self, v: Value, x: f32) -> Result<()> {
        self.put(v, &x.to_le_bytes())
    }

    /// An integer, at the width its type has.
    fn put_int(&mut self, v: Value, x: i64) -> Result<()> {
        let size = self.tag.stride(v.type_index).map_err(fail("an integer"))?;
        self.put(v, &x.to_le_bytes()[..size.min(8)])
    }

    /// A string member of the copy.
    fn set_string(&mut self, v: Value, text: &str) -> Result<()> {
        let (item, rel) = self.in_copy(v)?;
        self.image
            .set_string(item, rel, text)
            .map_err(fail("a name"))?;
        Ok(())
    }

    /// The element type of an array member.
    fn element_type(&self, field: Value) -> Result<usize> {
        self.tag
            .inherited(field.type_index, |t| t.subtype)
            .ok()
            .filter(|&s| s != 0)
            .ok_or_else(|| {
                Error(format!(
                    "{}: an array without an element type",
                    field.type_name()
                ))
            })
    }

    /// The byte offset of member `name` of type `t` (or of a parent of it).
    fn member(&self, t: usize, name: &str) -> Result<usize> {
        let mut index = t;
        for _ in 0..64 {
            let Some(ty) = self.tag.types.get(index).filter(|_| index != 0) else {
                break;
            };
            if let Some(f) = ty.fields.iter().find(|f| f.name == name) {
                return Ok(f.offset);
            }
            index = ty.parent;
        }
        Err(Error(format!(
            "{} has no member {name}",
            self.tag.types.get(t).map_or("?", |ty| ty.name.as_str())
        )))
    }

    /// The size of member `name` of type `t`.
    fn member_size(&self, t: usize, name: &str) -> Result<usize> {
        let mut index = t;
        for _ in 0..64 {
            let Some(ty) = self.tag.types.get(index).filter(|_| index != 0) else {
                break;
            };
            if let Some(f) = ty.fields.iter().find(|f| f.name == name) {
                return self.tag.stride(f.type_index).map_err(fail("a member"));
            }
            index = ty.parent;
        }
        Err(Error(format!("no member {name}")))
    }

    /// The array member `field` (of the template) made `count` elements of `data` in the copy: a new run (empty: no
    /// run). Its index.
    fn set_array(&mut self, field: Value, count: usize, data: Vec<u8>) -> Result<usize> {
        let (item, rel) = self.in_copy(field)?;
        let element = self.element_type(field)?;
        let size = self.tag.stride(element).map_err(fail("an element"))?;
        if data.len() != size * count {
            return Err(Error(format!(
                "{} elements of {} bytes, {} bytes given",
                count,
                size,
                data.len()
            )));
        }
        if count == 0 {
            self.image
                .set_reference(item, rel, 0)
                .map_err(fail("an array"))?;
            return Ok(0);
        }
        let run = self.image.push_item(ImageItem {
            type_index: element,
            kind: ItemKind::Run,
            count,
            data,
        });
        self.image
            .set_reference(item, rel, run)
            .map_err(fail("an array"))?;
        Ok(run)
    }

    /// Elements built from named members: `size` bytes each, each member at its offset in the element type.
    fn elements(&self, field: Value, rows: &[Vec<(&str, Vec<u8>)>]) -> Result<Vec<u8>> {
        let element = self.element_type(field)?;
        let size = self.tag.stride(element).map_err(fail("an element"))?;
        let mut out = vec![0u8; size * rows.len()];
        for (i, row) in rows.iter().enumerate() {
            for (name, bytes) in row {
                let at = i * size + self.member(element, name)?;
                out.get_mut(at..at + bytes.len())
                    .ok_or_else(|| Error(format!("member {name} runs past its element")))?
                    .copy_from_slice(bytes);
            }
        }
        Ok(out)
    }

    /// An array of pointers to `targets` (items of the image).
    fn set_pointers(&mut self, field: Value, targets: &[usize]) -> Result<()> {
        let element = self.element_type(field)?;
        let size = self.tag.stride(element).map_err(fail("a pointer"))?;
        let mut data = vec![0u8; size * targets.len()];
        for (i, &t) in targets.iter().enumerate() {
            data[i * size..i * size + 4].copy_from_slice(&(t as u32).to_le_bytes());
        }
        self.set_array(field, targets.len(), data)?;
        Ok(())
    }

    /// The bit set `bits` (of `count` bits) into an `hkBitField` of the template's copy.
    fn set_bits(&mut self, field: Value, count: usize, bits: &[usize]) -> Result<()> {
        let storage = field.field("storage").map_err(fail("a bit field"))?;
        let mut words = vec![0u32; count.div_ceil(32)];
        for &b in bits {
            words[b / 32] |= 1 << (b % 32);
        }
        let data: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        self.set_array(
            storage.field("words").map_err(fail("a bit field"))?,
            words.len(),
            data,
        )?;
        self.put_int(
            storage.field("numBits").map_err(fail("a bit field"))?,
            count as i64,
        )
    }
}

fn le16(x: u16) -> Vec<u8> {
    x.to_le_bytes().to_vec()
}
fn le32(x: u32) -> Vec<u8> {
    x.to_le_bytes().to_vec()
}
fn f32s(xs: &[f32]) -> Vec<u8> {
    xs.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// A position as an `hkPackedVector3` (three 16-bit words and a shared exponent: `havok-cloth::deform::unpack`'s
/// inverse): each component is its word, as the high half of a 32-bit integer, times the float whose bits are the
/// fourth word's in the high half (a power of two here).
pub fn pack(v: [f32; 3]) -> [i16; 4] {
    let largest = v.iter().fold(0f32, |m, x| m.max(x.abs()));
    if largest == 0.0 {
        return [0; 4];
    }
    // the smallest power of two e with largest / (e * 65536) within 32767
    let mut e = 2f32.powi((largest / (32767.0 * 65536.0)).log2().ceil() as i32);
    while largest / (e * 65536.0) > 32767.0 {
        e *= 2.0;
    }
    let w = (e.to_bits() >> 16) as u16 as i16;
    let q = |x: f32| (x / (e * 65536.0)).round().clamp(-32768.0, 32767.0) as i16;
    [q(v[0]), q(v[1]), q(v[2]), w]
}

/// Its inverse (for checks).
pub fn unpack(w: [i16; 4]) -> [f32; 3] {
    let e = f32::from_bits((w[3] as u16 as u32) << 16);
    let c = |k: usize| ((w[k] as i32) << 16) as f32 * e;
    [c(0), c(1), c(2)]
}

/// The frame `havok-cloth` builds on a triangle: its centroid, two corners less it, their cross product.
fn triangle_frame(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> Mat4 {
    let centroid = [
        (a[0] + b[0] + c[0]) / 3.0,
        (a[1] + b[1] + c[1]) / 3.0,
        (a[2] + b[2] + c[2]) / 3.0,
    ];
    let s = [a[0] - centroid[0], a[1] - centroid[1], a[2] - centroid[2]];
    let t = [b[0] - centroid[0], b[1] - centroid[1], b[2] - centroid[2]];
    let n = [
        s[1] * t[2] - s[2] * t[1],
        s[2] * t[0] - s[0] * t[2],
        s[0] * t[1] - s[1] * t[0],
    ];
    [
        s[0],
        s[1],
        s[2],
        0.0,
        t[0],
        t[1],
        t[2],
        0.0,
        n[0],
        n[1],
        n[2],
        0.0,
        centroid[0],
        centroid[1],
        centroid[2],
        1.0,
    ]
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// The strip of a part: particles (on the chain at even indices, beside it at odd ones; with a stiffness, the brace
/// after them), triangles, and which triangle sets each bone.
struct Strip {
    positions: Vec<[f32; 3]>,
    /// The particles the anchor carries: the top two, and the brace.
    fixed: Vec<u16>,
    triangles: Vec<[u16; 3]>,
    /// Per bone of the chain: its triangle.
    bone_triangles: Vec<usize>,
    /// Edges of the strip (every link), and the ones down the chain.
    edges: Vec<(u16, u16)>,
    rails: Vec<(u16, u16)>,
    /// Links across two joints down each side (holding a chain straight: the part's stiffness).
    bends: Vec<(u16, u16)>,
    /// Links from the brace to the first bone's tip, both sides (holding the first bone at its angle to the anchor:
    /// the part's stiffness).
    braces: Vec<(u16, u16)>,
}

impl Strip {
    /// Whether particle `i` moves (isn't carried by the anchor).
    fn free(&self, i: usize) -> bool {
        !self.fixed.contains(&(i as u16))
    }
}

/// A part's strip of particles and links. The top two particles fix the first bone's hinge: the strip's segments are
/// rigid, so the chain folds at its top across the strip (and at each joint below), and with nothing else gravity
/// takes it down until its range stops it. With a stiffness, a third fixed particle (the brace), off the strip's
/// plane at the top, is linked to the first bone's tip: the link's length changes as the first bone turns on its
/// hinge, so it pulls the bone back to its angle at rest (a bone that sticks out springs back rather than hanging).
fn strip(part: &NewPart) -> Result<Strip> {
    let n = part.bones.len();
    if n == 0 || part.joints.len() != n + 1 || part.across.len() != n + 1 {
        return Err(Error(format!(
            "part {}: {} bones, {} joints, {} across",
            part.name,
            n,
            part.joints.len(),
            part.across.len()
        )));
    }
    let mut positions = Vec::with_capacity(2 * (n + 1));
    for (j, a) in part.joints.iter().zip(&part.across) {
        positions.push(*j);
        positions.push([j[0] + a[0], j[1] + a[1], j[2] + a[2]]);
    }
    let l = |j: usize| (2 * j) as u16;
    let r = |j: usize| (2 * j + 1) as u16;
    let mut triangles = Vec::with_capacity(2 * n);
    let mut bone_triangles = Vec::with_capacity(n);
    // (no link across the top: both its particles are fixed, and a link between two fixed particles weighs nothing)
    let mut edges = Vec::new();
    let mut rails = Vec::new();
    for k in 1..=n {
        bone_triangles.push(triangles.len());
        triangles.push([l(k - 1), r(k - 1), l(k)]);
        triangles.push([r(k - 1), r(k), l(k)]);
        // both diagonals: each segment rigid, so the chain only folds at its joints (one diagonal lets a segment
        // twist about the chain, flipping its bone)
        edges.extend([
            (l(k - 1), l(k)),
            (r(k - 1), r(k)),
            (r(k - 1), l(k)),
            (l(k - 1), r(k)),
            (l(k), r(k)),
        ]);
        rails.push((l(k - 1), l(k)));
    }
    let bends = (1..n)
        .flat_map(|k| [(l(k - 1), l(k + 1)), (r(k - 1), r(k + 1))])
        .collect();
    let mut fixed = vec![l(0), r(0)];
    let mut braces = Vec::new();
    if part.settings.stiffness > 0.0 {
        // off the strip's plane (square to the first bone and to the strip across it), as far from the top as the
        // first bone is long: the brace's links meet the bone at a right angle's worth of slant, so a turn either
        // way on its hinge changes their length at once
        let (top, next) = (part.joints[0], part.joints[1]);
        let along = [next[0] - top[0], next[1] - top[1], next[2] - top[2]];
        let length = distance(top, next);
        let normal = math::normalize3(cross(along, part.across[0]));
        if length > 1e-5 && normal.iter().all(|x| x.is_finite()) && dot(normal, normal) > 0.5 {
            let brace = positions.len() as u16;
            positions.push([
                top[0] + normal[0] * length,
                top[1] + normal[1] * length,
                top[2] + normal[2] * length,
            ]);
            fixed.push(brace);
            braces.extend([(brace, l(1)), (brace, r(1))]);
        }
    }
    Ok(Strip {
        positions,
        fixed,
        triangles,
        bone_triangles,
        edges,
        rails,
        bends,
        braces,
    })
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The cloth container's and the animation container's objects, from the root's named variants.
fn containers<'t, 'a>(tag: &'t Tagfile<'a>) -> Result<(Value<'t, 'a>, Value<'t, 'a>)> {
    let root = tag.root().map_err(fail("the cloth file's root"))?;
    let variants = root
        .field("namedVariants")
        .and_then(|v| v.array())
        .map_err(fail("the cloth file's root"))?;
    let (mut cloth, mut anim) = (None, None);
    for i in 0..variants.len() {
        let v = variants.get(i).map_err(fail("a root variant"))?;
        let class = v
            .field("className")
            .and_then(|c| c.string())
            .map_err(fail("a root variant"))?;
        let object = v
            .field("variant")
            .and_then(|o| o.dereference())
            .map_err(fail("a root variant"))?;
        match class.as_str() {
            "hclClothContainer" => cloth = Some(object),
            "hkaAnimationContainer" => anim = Some(object),
            _ => {}
        }
    }
    Ok((
        cloth.ok_or_else(|| Error("the cloth file has no cloth container".to_owned()))?,
        anim.ok_or_else(|| Error("the cloth file has no animation container".to_owned()))?,
    ))
}

/// The objects an array of pointers refers to.
fn pointed<'t, 'a>(v: Value<'t, 'a>) -> Result<Vec<Value<'t, 'a>>> {
    let a = v.array().map_err(fail("a list"))?;
    (0..a.len())
        .map(|i| {
            a.get(i)
                .and_then(|p| p.dereference())
                .map_err(fail("a list"))
        })
        .collect()
}

/// The cloth file `bytes` with a new part for each of `parts` (bones of `skeleton`, the model's skeleton with the
/// chains' bones in it), and what each part meets and was left out of meeting (a line each). Read back before it is
/// returned.
pub fn add_parts(
    bytes: &[u8],
    skeleton: &[RestBone],
    parts: &[NewPart],
) -> Result<(Vec<u8>, Vec<String>)> {
    let scene = crate::cloth_edit::scene_of(bytes, "the cloth file")?;
    let phive = Phive::parse(bytes).map_err(|e| Error(format!("not a cloth file: {e}")))?;
    let tag = Tagfile::read(phive.tagfile, None).map_err(fail("reading the cloth file"))?;
    let physics = cloth::read(&tag).map_err(fail("reading the cloth file"))?;
    let (cloth_container, anim_container) = containers(&tag)?;
    let datas = pointed(
        cloth_container
            .field("clothDatas")
            .map_err(fail("the cloth container"))?,
    )?;
    let skeletons = pointed(
        anim_container
            .field("skeletons")
            .map_err(fail("the animation container"))?,
    )?;
    // the template: the smallest part (by particles) whose skin is in object space, positions only
    let mut template: Option<(usize, (bool, usize))> = None;
    for (i, d) in datas.iter().enumerate() {
        let name = d
            .field("name")
            .and_then(|n| n.string())
            .map_err(fail("a part"))?;
        let Some(c) = physics.cloths.iter().find(|c| c.name == name) else {
            continue;
        };
        let fits = c.sim_cloths.len() == 1
            && c.operators.len() == 6
            && c.transform_sets.len() == 1
            && matches!(
                c.operators[0].kind,
                OperatorKind::ObjectSpaceSkin {
                    components: cloth::Components::P,
                    ..
                } | OperatorKind::BoneSpaceSkin {
                    components: cloth::Components::P,
                    ..
                }
            )
            && matches!(c.operators[1].kind, OperatorKind::MoveParticles { .. })
            && matches!(c.operators[2].kind, OperatorKind::Simulate { .. })
            && matches!(
                c.operators[3].kind,
                OperatorKind::SimpleMeshBoneDeform { .. }
            )
            && c.states.len() == 3;
        if !fits {
            continue;
        }
        // object space first (the most of the game's parts), then the fewest particles
        let key = (
            matches!(c.operators[0].kind, OperatorKind::BoneSpaceSkin { .. }),
            c.sim_cloths[0].particles.len(),
        );
        if template.is_none_or(|(_, m)| key < m) {
            template = Some((i, key));
        }
    }
    let (ti, _) = template.ok_or_else(|| {
        Error(
            "the cloth file has no part to make new ones from (one skinned by positions alone)"
                .to_owned(),
        )
    })?;
    let t_value = datas[ti];
    let t_name = t_value
        .field("name")
        .and_then(|n| n.string())
        .map_err(fail("the template"))?;
    let t_cloth = physics
        .cloths
        .iter()
        .find(|c| c.name == t_name)
        .expect("found above");
    let t_skeleton = skeletons
        .iter()
        .copied()
        .find(|s| {
            s.field("name")
                .and_then(|n| n.string())
                .is_ok_and(|n| n == t_cloth.transform_sets[0].name)
        })
        .ok_or_else(|| Error(format!("the template part {t_name} has no skeleton")))?;

    let mut ed = Editor::new(&tag)?;
    let world = rest_world(skeleton);
    // an entry of each per-collider array of a sim cloth, from a part of the file that has one (its bytes): what a
    // new part's colliders get (zeros where no part has one)
    let mut samples: HashMap<&str, Vec<u8>> = HashMap::new();
    for name in ["collidablePinchingDatas", "setupCollidableSidedness"] {
        'found: for d in &datas {
            let Ok(sims) = d
                .field("simClothDatas")
                .map_err(fail("a part"))
                .and_then(pointed)
            else {
                continue;
            };
            for sim in sims {
                if !sim.has_field(name) {
                    continue;
                }
                let Ok(a) = sim.field(name).and_then(|f| f.array()) else {
                    continue;
                };
                if a.is_empty() {
                    continue;
                }
                let Ok(first) = a.get(0) else { continue };
                let Ok(size) = tag.stride(first.type_index) else {
                    continue;
                };
                let (item, rel) = ed.locate(first)?;
                samples.insert(name, ed.image.items[item].data[rel..rel + size].to_vec());
                break 'found;
            }
        }
    }
    let free_radius = t_cloth.sim_cloths[0]
        .particles
        .iter()
        .enumerate()
        .filter(|(i, _)| !t_cloth.sim_cloths[0].fixed_particles.contains(&(*i as u16)))
        .map(|(_, p)| p.radius)
        .fold(0f32, f32::max);
    let mut notes = Vec::new();
    let mut new_datas = Vec::new();
    let mut new_skeletons = Vec::new();
    for part in parts {
        ed.copies.clear();
        let s = strip(part)?;
        let moving: Vec<[f32; 3]> = (0..s.positions.len())
            .filter(|&i| s.free(i))
            .map(|i| s.positions[i])
            .collect();
        let (colliders, part_notes) = match &part.settings.colliders {
            ColliderChoice::Auto => colliders_for(
                &scene.colliders,
                skeleton,
                &world,
                part,
                &moving,
                free_radius,
            ),
            ColliderChoice::None => (Vec::new(), Vec::new()),
            ColliderChoice::Named(names) => {
                named_colliders(&scene.colliders, skeleton, &world, part, names)
            }
        };
        notes.push(if colliders.is_empty() {
            format!("{}: no collider within reach", part.name)
        } else {
            format!(
                "{} meets {}",
                part.name,
                colliders
                    .iter()
                    .map(|c| c.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        });
        notes.extend(part_notes);
        let (data_item, skeleton_item) = make_part(
            &mut ed,
            Template {
                value: t_value,
                cloth: t_cloth,
                skeleton: t_skeleton,
                samples: &samples,
            },
            skeleton,
            &world,
            part,
            &colliders,
        )?;
        new_datas.push(data_item);
        new_skeletons.push(skeleton_item);
    }
    // listed in the containers: their arrays made again with the new ones after
    let mut list = |container: Value, field: &str, extra: &[usize]| -> Result<()> {
        let f = container.field(field).map_err(fail("a container"))?;
        let (item, rel) = ed.locate(f)?;
        let run = {
            let d = &ed.image.items[item].data;
            u32::from_le_bytes(d[rel..rel + 4].try_into().expect("4")) as usize
        };
        let element = ed.element_type(f)?;
        let size = tag.stride(element).map_err(fail("a pointer"))?;
        let (mut data, count) = if run == 0 {
            (Vec::new(), 0)
        } else {
            (ed.image.items[run].data.clone(), ed.image.items[run].count)
        };
        for &x in extra {
            let mut b = (x as u32).to_le_bytes().to_vec();
            b.resize(size, 0);
            data.extend(b);
        }
        let new = ed.image.push_item(ImageItem {
            type_index: element,
            kind: ItemKind::Run,
            count: count + extra.len(),
            data,
        });
        ed.image
            .set_reference(item, rel, new)
            .map_err(fail("a container"))?;
        Ok(())
    };
    list(cloth_container, "clothDatas", &new_datas)?;
    list(anim_container, "skeletons", &new_skeletons)?;
    let tagfile = ed.image.write().map_err(fail("writing the cloth file"))?;

    // the parameter archive: an entry for each new part, the template's with its name and base bone
    let mut archive =
        Aamp::read(phive.params).map_err(|e| Error(format!("the cloth file's parameters: {e}")))?;
    let name_hash = aamp::hash("Name");
    let base_hash = aamp::hash("BaseBone");
    let list = archive
        .root
        .lists
        .iter_mut()
        .find(|l| {
            l.objects.iter().any(|o| {
                o.params
                    .iter()
                    .any(|p| p.hash == name_hash && p.text() == Some(t_name.as_str()))
            })
        })
        .ok_or_else(|| Error(format!("the cloth file's parameters don't list {t_name}")))?;
    let entry = list
        .objects
        .iter()
        .find(|o| {
            o.params
                .iter()
                .any(|p| p.hash == name_hash && p.text() == Some(t_name.as_str()))
        })
        .cloned()
        .expect("found above");
    for part in parts {
        let mut o = entry.clone();
        o.hash = aamp::hash(&part.name);
        for p in &mut o.params {
            if p.hash == name_hash {
                p.value = part.name.as_bytes().to_vec();
            } else if p.hash == base_hash {
                p.value = skeleton[part.anchor].name.as_bytes().to_vec();
            }
        }
        list.objects.push(o);
    }
    let out = phive::with_parts(bytes, &tagfile, &archive.write())
        .map_err(|e| Error(format!("the cloth file: {e}")))?;

    // read back: the parts are there, with their particles and bones
    let back_phive =
        Phive::parse(&out).map_err(|e| Error(format!("the written cloth file: {e}")))?;
    let back_tag =
        Tagfile::read(back_phive.tagfile, None).map_err(fail("reading the written cloth file"))?;
    let back = cloth::read(&back_tag).map_err(fail("reading the written cloth file"))?;
    for part in parts {
        let c = back
            .cloths
            .iter()
            .find(|c| c.name == part.name)
            .ok_or_else(|| Error(format!("the written file has no part {}", part.name)))?;
        let n = strip(part)?.positions.len();
        if c.sim_cloths[0].particles.len() != n {
            return Err(Error(format!(
                "the written part {}: {} particles, not {n}",
                part.name,
                c.sim_cloths[0].particles.len()
            )));
        }
    }
    Ok((out, notes))
}

/// The part new ones are copied from, and entries of the file's per-collider arrays.
struct Template<'t, 'a, 'c> {
    value: Value<'t, 'a>,
    cloth: &'c cloth::Cloth,
    skeleton: Value<'t, 'a>,
    samples: &'c HashMap<&'c str, Vec<u8>>,
}

/// One new part: its cloth data and skeleton, copied from the template's and made for `part`. Their items.
fn make_part(
    ed: &mut Editor,
    template: Template,
    skeleton: &[RestBone],
    world: &[Mat4],
    part: &NewPart,
    colliders: &[PartCollider],
) -> Result<(usize, usize)> {
    let (t_value, t_cloth, t_skeleton) = (template.value, template.cloth, template.skeleton);
    let s = strip(part)?;
    let n = s.positions.len();
    let fixed = s.fixed.clone();
    // the part's skeleton: the anchor, the colliders' bones and their parents from the root (in the model's order,
    // every parent before its children), then the chain
    let mut base: Vec<usize> = Vec::new();
    for &start in std::iter::once(&part.anchor).chain(colliders.iter().map(|c| &c.bone)) {
        let mut b = Some(start);
        let mut steps = 0;
        while let Some(i) = b {
            if base.contains(&i) {
                break;
            }
            base.push(i);
            steps += 1;
            if steps > skeleton.len() {
                return Err(Error(format!(
                    "bone {}: its parents loop",
                    skeleton[start].name
                )));
            }
            b = skeleton[i].parent;
        }
    }
    base.sort_unstable();
    let mut bones: Vec<usize> = base;
    bones.extend(&part.bones);
    let count = bones.len();
    let index_of = |model_bone: usize| bones.iter().position(|&b| b == model_bone).expect("listed");
    let anchor_index = index_of(part.anchor);
    let chain_indices: Vec<usize> = part.bones.iter().map(|&b| index_of(b)).collect();
    let collider_indices: Vec<usize> = colliders.iter().map(|c| index_of(c.bone)).collect();
    let local_parent = |model_bone: usize| -> i16 {
        skeleton[model_bone]
            .parent
            .and_then(|p| bones.iter().position(|&b| b == p))
            .map_or(-1, |i| i as i16)
    };

    // copies of the template's objects: everything its cloth data and skeleton refer to, but colliders
    let keep = |image: &TagfileImage, item: usize| {
        let t = image.items[item].type_index;
        image
            .types
            .get(t)
            .is_some_and(|ty| ty.name.starts_with("hclCollidable") || ty.name.ends_with("Shape"))
    };
    let (t_data_item, _) = ed.locate(t_value)?;
    let (t_skel_item, _) = ed.locate(t_skeleton)?;
    let data_item = ed.deep_copy(t_data_item, &keep)?;
    let skeleton_item = ed.deep_copy(t_skel_item, &keep)?;
    let skeleton_name = format!("cloth_skeleton_{}", part.name);

    // --- the skeleton
    ed.set_string(
        t_skeleton.field("name").map_err(fail("a skeleton"))?,
        &skeleton_name,
    )?;
    let f = t_skeleton
        .field("parentIndices")
        .map_err(fail("a skeleton"))?;
    let parents: Vec<u8> = bones
        .iter()
        .flat_map(|&b| local_parent(b).to_le_bytes())
        .collect();
    ed.set_array(f, count, parents)?;
    let f = t_skeleton.field("bones").map_err(fail("a skeleton"))?;
    let rows: Vec<Vec<(&str, Vec<u8>)>> = bones
        .iter()
        .map(|_| vec![("lockTranslation", vec![0])])
        .collect();
    let data = ed.elements(f, &rows)?;
    let run = ed.set_array(f, count, data)?;
    let bone_type = ed.element_type(f)?;
    let bone_size = ed.tag.stride(bone_type).map_err(fail("a bone"))?;
    let name_at = ed.member(bone_type, "name")?;
    for (i, &b) in bones.iter().enumerate() {
        ed.image
            .set_string(run, i * bone_size + name_at, &skeleton[b].name)
            .map_err(fail("a bone's name"))?;
    }
    let f = t_skeleton
        .field("referencePose")
        .map_err(fail("a skeleton"))?;
    let mut pose = Vec::new();
    for &b in &bones {
        let r = &skeleton[b];
        pose.extend(f32s(&[
            r.translation[0],
            r.translation[1],
            r.translation[2],
            0.0,
        ]));
        pose.extend(f32s(&r.rotation));
        pose.extend(f32s(&[r.scale[0], r.scale[1], r.scale[2], 1.0]));
    }
    ed.set_array(f, count, pose)?;
    for name in ["referenceFloats", "floatSlots", "localFrames", "partitions"] {
        if t_skeleton.has_field(name) {
            ed.set_array(
                t_skeleton.field(name).map_err(fail("a skeleton"))?,
                0,
                Vec::new(),
            )?;
        }
    }

    // --- the cloth data
    ed.set_string(t_value.field("name").map_err(fail("a part"))?, &part.name)?;
    let sets = pointed(
        t_value
            .field("transformSetDefinitions")
            .map_err(fail("a part"))?,
    )?;
    ed.set_string(
        sets[0].field("name").map_err(fail("a transform set"))?,
        &skeleton_name,
    )?;
    ed.put_int(
        sets[0]
            .field("numTransforms")
            .map_err(fail("a transform set"))?,
        count as i64,
    )?;
    let buffers = pointed(t_value.field("bufferDefinitions").map_err(fail("a part"))?)?;
    for b in &buffers {
        let scratch = b.type_name().contains("Scratch");
        ed.put_int(b.field("numVertices").map_err(fail("a buffer"))?, n as i64)?;
        ed.put_int(
            b.field("numTriangles").map_err(fail("a buffer"))?,
            if scratch { 0 } else { s.triangles.len() as i64 },
        )?;
        if scratch && b.has_field("triangleIndices") {
            ed.set_array(
                b.field("triangleIndices").map_err(fail("a buffer"))?,
                0,
                Vec::new(),
            )?;
        }
    }

    // --- the sim cloth
    let sim = pointed(t_value.field("simClothDatas").map_err(fail("a part"))?)?[0];
    let t_sim = &t_cloth.sim_cloths[0];
    let free = t_sim
        .particles
        .iter()
        .enumerate()
        .find(|(i, _)| !t_sim.fixed_particles.contains(&(*i as u16)))
        .map(|(_, p)| *p)
        .ok_or_else(|| Error("the template part has no free particle".to_owned()))?;
    let held = t_sim
        .fixed_particles
        .first()
        .and_then(|&i| t_sim.particles.get(i as usize))
        .copied()
        .unwrap_or(cloth::Particle {
            mass: 0.0,
            inv_mass: 0.0,
            radius: free.radius * 0.01,
            friction: free.friction,
        });
    let particles: Vec<cloth::Particle> = (0..n)
        .map(|i| if s.free(i) { free } else { held })
        .collect();
    let f = sim.field("particleDatas").map_err(fail("a sim cloth"))?;
    let rows: Vec<Vec<(&str, Vec<u8>)>> = particles
        .iter()
        .map(|p| {
            vec![
                ("mass", le32(p.mass.to_bits())),
                ("invMass", le32(p.inv_mass.to_bits())),
                ("radius", le32(p.radius.to_bits())),
                ("friction", le32(p.friction.to_bits())),
            ]
        })
        .collect();
    let data = ed.elements(f, &rows)?;
    ed.set_array(f, n, data)?;
    let f = sim.field("fixedParticles").map_err(fail("a sim cloth"))?;
    ed.set_array(
        f,
        fixed.len(),
        fixed.iter().flat_map(|x| x.to_le_bytes()).collect(),
    )?;
    for p in pointed(sim.field("simClothPoses").map_err(fail("a sim cloth"))?)? {
        let f = p.field("positions").map_err(fail("a pose"))?;
        let data: Vec<u8> = s
            .positions
            .iter()
            .flat_map(|q| f32s(&[q[0], q[1], q[2], 1.0]))
            .collect();
        ed.set_array(f, n, data)?;
    }
    // gravity: the template's times the part's setting
    let g = t_sim.gravity;
    let k = part.settings.gravity;
    let gravity_field = sim
        .field("simulationInfo")
        .and_then(|i| i.field("gravity"))
        .map_err(fail("a sim cloth"))?;
    ed.put(gravity_field, &f32s(&[g[0] * k, g[1] * k, g[2] * k, g[3]]))?;
    let max_radius = particles.iter().fold(0f32, |m, p| m.max(p.radius));
    ed.put_f32(
        sim.field("maxParticleRadius")
            .map_err(fail("a sim cloth"))?,
        max_radius,
    )?;
    let total: f32 = particles.iter().map(|p| p.mass).sum();
    ed.put_f32(sim.field("totalMass").map_err(fail("a sim cloth"))?, total)?;
    // the colliders: each free particle meets all of them (a bit each)
    let f = sim
        .field("staticCollisionMasks")
        .map_err(fail("a sim cloth"))?;
    let size = ed.tag.stride(ed.element_type(f)?).map_err(fail("a mask"))?;
    let all: u64 = (0..colliders.len()).fold(0, |m, k| m | 1 << k);
    let masks: Vec<u8> = (0..n)
        .flat_map(|i| {
            let m: u64 = if s.free(i) { all } else { 0 };
            m.to_le_bytes()[..size].to_vec()
        })
        .collect();
    ed.set_array(f, n, masks)?;
    let items: Vec<usize> = colliders.iter().map(|c| c.item).collect();
    ed.set_pointers(
        sim.field("perInstanceCollidables")
            .map_err(fail("a sim cloth"))?,
        &items,
    )?;
    let map = sim
        .field("collidableTransformMap")
        .map_err(fail("a sim cloth"))?;
    let f = map
        .field("transformIndices")
        .map_err(fail("a collider map"))?;
    let size = ed
        .tag
        .stride(ed.element_type(f)?)
        .map_err(fail("a collider map"))?;
    let data: Vec<u8> = collider_indices
        .iter()
        .flat_map(|&i| (i as u64).to_le_bytes()[..size].to_vec())
        .collect();
    ed.set_array(f, colliders.len(), data)?;
    let data: Vec<u8> = colliders.iter().flat_map(|c| f32s(&c.offset)).collect();
    ed.set_array(
        map.field("offsets").map_err(fail("a collider map"))?,
        colliders.len(),
        data,
    )?;
    for name in ["collidablePinchingDatas", "setupCollidableSidedness"] {
        if !sim.has_field(name) {
            continue;
        }
        let f = sim.field(name).map_err(fail("a sim cloth"))?;
        let size = ed
            .tag
            .stride(ed.element_type(f)?)
            .map_err(fail("a sim cloth"))?;
        let one = template
            .samples
            .get(name)
            .filter(|s| s.len() == size)
            .cloned()
            .unwrap_or_else(|| vec![0; size]);
        let data: Vec<u8> = (0..colliders.len()).flat_map(|_| one.clone()).collect();
        ed.set_array(f, colliders.len(), data)?;
    }
    for name in ["antiPinchConstraintSets", "triangleFlips"] {
        if sim.has_field(name) {
            ed.set_array(sim.field(name).map_err(fail("a sim cloth"))?, 0, Vec::new())?;
        }
    }
    if sim.has_field("perParticlePinchDetectionEnabledFlags") {
        let f = sim
            .field("perParticlePinchDetectionEnabledFlags")
            .map_err(fail("a sim cloth"))?;
        let size = ed.tag.stride(ed.element_type(f)?).map_err(fail("a flag"))?;
        ed.set_array(f, n, vec![0; size * n])?;
    }
    for name in [
        "numLandscapeCollidableParticles",
        "minPinchedParticleIndex",
        "maxPinchedParticleIndex",
    ] {
        if sim.has_field(name) {
            ed.put_int(sim.field(name).map_err(fail("a sim cloth"))?, 0)?;
        }
    }
    if sim.has_field("pinchDetectionEnabled") {
        ed.put(
            sim.field("pinchDetectionEnabled")
                .map_err(fail("a sim cloth"))?,
            &[0],
        )?;
    }
    let f = sim.field("triangleIndices").map_err(fail("a sim cloth"))?;
    let data: Vec<u8> = s
        .triangles
        .iter()
        .flat_map(|t| t.iter().flat_map(|i| i.to_le_bytes()).collect::<Vec<u8>>())
        .collect();
    ed.set_array(f, 3 * s.triangles.len(), data)?;

    // the constraint sets: standard links, a stretch link, local ranges, the transition, in that order (as the
    // template's simulate configuration runs them: local ranges, the links three times, the stretch link,
    // collisions, the transition)
    let t_sets = pointed(
        sim.field("staticConstraintSets")
            .map_err(fail("a sim cloth"))?,
    )?;
    let kind_of = |i: usize| t_sim.constraint_sets.get(i).map(|c| &c.kind);
    let find = |want: fn(&ConstraintKind) -> bool| -> Option<Value> {
        t_sets
            .iter()
            .enumerate()
            .find(|(i, _)| kind_of(*i).is_some_and(want))
            .map(|(_, v)| *v)
    };
    let standard = find(|k| matches!(k, ConstraintKind::StandardLink(_)))
        .ok_or_else(|| Error("the template part has no standard links".to_owned()))?;
    let stretch = find(|k| matches!(k, ConstraintKind::StretchLink(_)));
    let range = find(|k| matches!(k, ConstraintKind::LocalRange { .. }))
        .ok_or_else(|| Error("the template part has no local ranges".to_owned()))?;
    let transition = find(|k| matches!(k, ConstraintKind::Transition { .. }));
    // a standard link moves each of its particles by the stored stiffness times that particle's inverse mass (not
    // divided by the two's sum: `havok-cloth::constraints`), so the game's files store the share of the error a pass
    // corrects divided by the pair's inverse masses (the feather: 0.05 for a particle of 10 a kilogram, a half). The
    // template's share (its largest), on every link of the strip, but half at least: the strip's segments are to be
    // rigid, and some parts' links are soft (a tenth: a stiff part's brace then bends its strip out of shape, and
    // the bone it sets swings about where it stands).
    let inv_mass = |i: u16| particles[i as usize].inv_mass;
    let share = match kind_of(
        t_sets
            .iter()
            .position(|v| v.offset == standard.offset)
            .expect("one of them"),
    ) {
        Some(ConstraintKind::StandardLink(links)) => links
            .iter()
            .map(|l| {
                let a = t_sim
                    .particles
                    .get(l.particle_a as usize)
                    .map_or(0.0, |p| p.inv_mass);
                let b = t_sim
                    .particles
                    .get(l.particle_b as usize)
                    .map_or(0.0, |p| p.inv_mass);
                l.stiffness * (a + b)
            })
            .fold(0f32, f32::max),
        _ => 0.5,
    }
    .clamp(0.5, 1.0);
    // stretch links move particle B by the stiffness alone (`None`); standard ones by `Some(share)`
    let links = |edges: &[(u16, u16)], standard: Option<f32>| -> Vec<Vec<(&str, Vec<u8>)>> {
        edges
            .iter()
            .map(|&(a, b)| {
                let rest = distance(s.positions[a as usize], s.positions[b as usize]);
                let w = inv_mass(a) + inv_mass(b);
                let stiffness = match standard {
                    None => 1.0,
                    Some(share) if w > 0.0 => share / w,
                    Some(_) => 0.0,
                };
                vec![
                    ("particleA", le16(a)),
                    ("particleB", le16(b)),
                    ("restLength", le32(rest.to_bits())),
                    ("stiffness", le32(stiffness.to_bits())),
                ]
            })
            .collect()
    };
    let f = standard.field("links").map_err(fail("links"))?;
    // the strip's edges, then (with a stiffness) the links across two joints, which hold the chain straight, and
    // the brace's, which hold its first bone at its angle to the anchor
    let mut rows = links(&s.edges, Some(share));
    if part.settings.stiffness > 0.0 {
        rows.extend(links(&s.bends, Some(part.settings.stiffness)));
        rows.extend(links(&s.braces, Some(part.settings.stiffness)));
    }
    let count_links = rows.len();
    let data = ed.elements(f, &rows)?;
    ed.set_array(f, count_links, data)?;
    if let Some(st) = stretch {
        let f = st.field("links").map_err(fail("stretch links"))?;
        let data = ed.elements(f, &links(&s.rails, None))?;
        ed.set_array(f, s.rails.len(), data)?;
    }
    // ranges: each free particle within half its distance down the chain of its place at rest
    let mut down = vec![0f32; n];
    for k in 1..=part.bones.len() {
        let step = distance(part.joints[k - 1], part.joints[k]);
        down[2 * k] = down[2 * k - 2] + step;
        down[2 * k + 1] = down[2 * k];
    }
    // in the set's own list, or (in a file whose parts all use it, so that the plain entry's type has no members
    // there) in the list of entries with a stiffness of their own: the set's
    let plain = range
        .field("localConstraints")
        .map_err(fail("local ranges"))?;
    let own = if range.has_field("localStiffnessConstraints") {
        Some(
            range
                .field("localStiffnessConstraints")
                .map_err(fail("local ranges"))?,
        )
    } else {
        None
    };
    let plain_spelled = ed.member(ed.element_type(plain)?, "particleIndex").is_ok();
    let (f, other) = match own {
        Some(o) if !plain_spelled => (o, Some(plain)),
        _ => (plain, own),
    };
    let element = ed.element_type(f)?;
    let distance_member = if ed.member(element, "maximumDistance").is_ok() {
        "maximumDistance"
    } else {
        "shapeRadius"
    };
    let with_stiffness = ed.member(element, "stiffness").is_ok();
    let set_stiffness = t_sim
        .constraint_sets
        .iter()
        .find_map(|c| match &c.kind {
            ConstraintKind::LocalRange { stiffness, .. } => Some(*stiffness),
            _ => None,
        })
        .unwrap_or(1.0);
    let rows: Vec<Vec<(&str, Vec<u8>)>> = (0..n)
        .filter(|&i| s.free(i))
        .map(|i| {
            let mut row = vec![
                ("particleIndex", le16(i as u16)),
                ("referenceVertex", le16(i as u16)),
                (
                    distance_member,
                    le32((part.settings.swing * down[i]).max(0.01).to_bits()),
                ),
                ("maxNormalDistance", le32(3.40282e38f32.to_bits())),
                ("minNormalDistance", le32((-3.40282e38f32).to_bits())),
            ];
            if with_stiffness {
                row.push(("stiffness", le32(set_stiffness.to_bits())));
            }
            row
        })
        .collect();
    let data = ed.elements(f, &rows)?;
    ed.set_array(f, n - fixed.len(), data)?;
    if let Some(o) = other {
        ed.set_array(o, 0, Vec::new())?;
    }
    if let Some(tr) = transition {
        let f = tr
            .field("perParticleData")
            .map_err(fail("the transition"))?;
        let element = ed.element_type(f)?;
        // the template's own values for each particle
        let first = match t_sim.constraint_sets.iter().find_map(|c| match &c.kind {
            ConstraintKind::Transition { particles, .. } => particles.first().copied(),
            _ => None,
        }) {
            Some(p) => p,
            None => cloth::TransitionParticle {
                particle: 0,
                reference_vertex: 0,
                to_anim_delay: 0.0,
                to_sim_delay: 0.0,
                to_sim_max_distance: 1.0,
            },
        };
        let _ = element;
        let rows: Vec<Vec<(&str, Vec<u8>)>> = (0..n)
            .map(|i| {
                vec![
                    ("particleIndex", le16(i as u16)),
                    ("referenceVertex", le16(i as u16)),
                    ("toAnimDelay", le32(first.to_anim_delay.to_bits())),
                    ("toSimDelay", le32(first.to_sim_delay.to_bits())),
                    (
                        "toSimMaxDistance",
                        le32(first.to_sim_max_distance.to_bits()),
                    ),
                ]
            })
            .collect();
        let data = ed.elements(f, &rows)?;
        ed.set_array(f, n, data)?;
    }
    // the sets, in order, and the simulate configuration's order of them
    let mut ordered: Vec<Value> = vec![standard];
    if let Some(st) = stretch {
        ordered.push(st);
    }
    ordered.push(range);
    if let Some(tr) = transition {
        ordered.push(tr);
    }
    let set_items: Vec<usize> = ordered
        .iter()
        .map(|v| ed.in_copy(*v).map(|(item, _)| item))
        .collect::<Result<_>>()?;
    for (k, v) in ordered.iter().enumerate() {
        if v.has_field("constraintId") {
            let id = v.field("constraintId").map_err(fail("a constraint set"))?;
            let target = if id.has_field("value") {
                id.field("value").map_err(fail("a constraint set"))?
            } else {
                id
            };
            ed.put_int(target, k as i64)?;
        }
    }
    ed.set_pointers(
        sim.field("staticConstraintSets")
            .map_err(fail("a sim cloth"))?,
        &set_items,
    )?;
    let i_standard = 0i32;
    let i_stretch = stretch.map(|_| 1i32);
    let i_range = 1 + i32::from(stretch.is_some());
    let i_transition = transition.map(|_| i_range + 1);
    let mut execution = vec![i_range, i_standard, i_standard, i_standard];
    execution.extend(i_stretch);
    execution.push(-1);
    execution.extend(i_transition);

    // --- the operators
    let ops = pointed(t_value.field("operators").map_err(fail("a part"))?)?;
    let anchor_world = world[part.anchor];
    // 0, skin: every particle on the anchor, its place at rest in the model (object space: the model's space and the
    // anchor's inverse rest place; bone space: its place relative to the anchor, its weight in the fourth value)
    let skin = ops[0];
    let inverse = math::inverse_affine(&anchor_world).ok_or_else(|| {
        Error(format!(
            "bone {}: scaled to nothing",
            skeleton[part.anchor].name
        ))
    })?;
    if skin.has_field("objectSpaceDeformer") {
        ed.set_array(
            skin.field("boneFromSkinMeshTransforms")
                .map_err(fail("the skin"))?,
            1,
            f32s(&inverse),
        )?;
        ed.set_array(
            skin.field("transformSubset").map_err(fail("the skin"))?,
            1,
            le16(anchor_index as u16),
        )?;
        let deformer = skin
            .field("objectSpaceDeformer")
            .map_err(fail("the skin"))?;
        let blocks = n.div_ceil(16);
        for name in [
            "eightBlendEntries",
            "sevenBlendEntries",
            "sixBlendEntries",
            "fiveBlendEntries",
            "fourBlendEntries",
            "threeBlendEntries",
            "twoBlendEntries",
            "oneBlendEntries",
        ] {
            if deformer.has_field(name) {
                ed.set_array(
                    deformer.field(name).map_err(fail("the deformer"))?,
                    0,
                    Vec::new(),
                )?;
            }
        }
        // blocks of the fewest bones a vertex whose type the file spells out (one whose blocks it stores), every bone
        // the anchor, all the weight on the first
        const BLOCK_NAMES: [&str; 8] = [
            "oneBlendEntries",
            "twoBlendEntries",
            "threeBlendEntries",
            "fourBlendEntries",
            "fiveBlendEntries",
            "sixBlendEntries",
            "sevenBlendEntries",
            "eightBlendEntries",
        ];
        const CONTROL: [u8; 8] = [3, 2, 1, 0, 8, 7, 6, 5];
        let mut chosen = None;
        for (k, name) in BLOCK_NAMES.iter().enumerate() {
            if !deformer.has_field(name) {
                continue;
            }
            let f = deformer.field(name).map_err(fail("the deformer"))?;
            if ed.member(ed.element_type(f)?, "vertexIndices").is_ok() {
                chosen = Some((k + 1, f));
                break;
            }
        }
        let (per_vertex, f) = chosen.ok_or_else(|| {
            Error("the template's skin has no blocks of a type the file spells out".to_owned())
        })?;
        let bone_indices = ed.member_size(ed.element_type(f)?, "boneIndices")?;
        let rows: Vec<Vec<(&str, Vec<u8>)>> = (0..blocks)
            .map(|b| {
                let vertices: Vec<u8> = (0..16)
                    .flat_map(|k| ((16 * b + k).min(n - 1) as u16).to_le_bytes())
                    .collect();
                let mut row = vec![
                    ("vertexIndices", vertices),
                    ("boneIndices", vec![0u8; bone_indices]),
                ];
                if per_vertex > 1 {
                    // 8-bit weights for two to four bones, 16-bit for five to eight
                    let wide = per_vertex > 4;
                    let mut weights = Vec::new();
                    for _ in 0..16 {
                        for j in 0..per_vertex {
                            let full = j == 0;
                            if wide {
                                weights.extend(if full { 65535u16 } else { 0 }.to_le_bytes());
                            } else {
                                weights.push(if full { 255u8 } else { 0 });
                            }
                        }
                    }
                    row.push(("boneWeights", weights));
                }
                row
            })
            .collect();
        let data = ed.elements(f, &rows)?;
        ed.set_array(f, blocks, data)?;
        let f = deformer
            .field("controlBytes")
            .map_err(fail("the deformer"))?;
        ed.set_array(f, blocks, vec![CONTROL[per_vertex - 1]; blocks])?;
        ed.put_int(
            deformer
                .field("startVertexIndex")
                .map_err(fail("the deformer"))?,
            0,
        )?;
        ed.put_int(
            deformer
                .field("endVertexIndex")
                .map_err(fail("the deformer"))?,
            (n - 1) as i64,
        )?;
        let f = skin.field("localPs").map_err(fail("the skin"))?;
        let rows: Vec<Vec<(&str, Vec<u8>)>> = (0..blocks)
            .map(|b| {
                let packed: Vec<u8> = (0..16)
                    .flat_map(|k| {
                        let p = s.positions[(16 * b + k).min(n - 1)];
                        pack(p)
                            .iter()
                            .flat_map(|w| w.to_le_bytes())
                            .collect::<Vec<u8>>()
                    })
                    .collect();
                vec![("localPosition", packed)]
            })
            .collect();
        let data = ed.elements(f, &rows)?;
        ed.set_array(f, blocks, data)?;
    } else {
        ed.set_array(
            skin.field("transformSubset").map_err(fail("the skin"))?,
            1,
            le16(anchor_index as u16),
        )?;
        let deformer = skin.field("boneSpaceDeformer").map_err(fail("the skin"))?;
        // blocks of one to four bones a vertex: 16, 8, 5 or 4 vertices each, sixteen local positions (a vertex's for
        // each of its bones in turn)
        const NAMES: [&str; 4] = [
            "oneBlendEntries",
            "twoBlendEntries",
            "threeBlendEntries",
            "fourBlendEntries",
        ];
        const PER_BLOCK: [usize; 4] = [16, 8, 5, 4];
        const CONTROL: [u8; 4] = [3, 2, 1, 0];
        for name in NAMES {
            if deformer.has_field(name) {
                ed.set_array(
                    deformer.field(name).map_err(fail("the deformer"))?,
                    0,
                    Vec::new(),
                )?;
            }
        }
        let mut chosen = None;
        for (k, name) in NAMES.iter().enumerate() {
            if !deformer.has_field(name) {
                continue;
            }
            let f = deformer.field(name).map_err(fail("the deformer"))?;
            if ed.member(ed.element_type(f)?, "vertexIndices").is_ok() {
                chosen = Some((k, f));
                break;
            }
        }
        let (k, f) = chosen.ok_or_else(|| {
            Error("the template's skin has no blocks of a type the file spells out".to_owned())
        })?;
        let (per_vertex, per_block) = (k + 1, PER_BLOCK[k]);
        let blocks = n.div_ceil(per_block);
        let bone_indices = ed.member_size(ed.element_type(f)?, "boneIndices")?;
        let rows: Vec<Vec<(&str, Vec<u8>)>> = (0..blocks)
            .map(|b| {
                let vertices: Vec<u8> = (0..per_block)
                    .flat_map(|j| ((per_block * b + j).min(n - 1) as u16).to_le_bytes())
                    .collect();
                vec![
                    ("vertexIndices", vertices),
                    ("boneIndices", vec![0u8; bone_indices]),
                ]
            })
            .collect();
        let data = ed.elements(f, &rows)?;
        ed.set_array(f, blocks, data)?;
        ed.set_array(
            deformer
                .field("controlBytes")
                .map_err(fail("the deformer"))?,
            blocks,
            vec![CONTROL[k]; blocks],
        )?;
        ed.put_int(
            deformer
                .field("startVertexIndex")
                .map_err(fail("the deformer"))?,
            0,
        )?;
        ed.put_int(
            deformer
                .field("endVertexIndex")
                .map_err(fail("the deformer"))?,
            (n - 1) as i64,
        )?;
        let f = skin.field("localPs").map_err(fail("the skin"))?;
        let rows: Vec<Vec<(&str, Vec<u8>)>> = (0..blocks)
            .map(|b| {
                let mut positions = Vec::with_capacity(16 * 16);
                for slot in 0..16 {
                    let (j, bone) = (slot / per_vertex, slot % per_vertex);
                    let v = if j < per_block && bone == 0 {
                        let p = math::transform_point(
                            &inverse,
                            s.positions[(per_block * b + j).min(n - 1)],
                        );
                        [p[0], p[1], p[2], 1.0]
                    } else {
                        [0.0; 4]
                    };
                    positions.extend(f32s(&v));
                }
                vec![("localPosition", positions)]
            })
            .collect();
        let data = ed.elements(f, &rows)?;
        ed.set_array(f, blocks, data)?;
    }
    if skin.has_field("localUnpackedPs") {
        ed.set_array(
            skin.field("localUnpackedPs").map_err(fail("the skin"))?,
            0,
            Vec::new(),
        )?;
    }
    // 1, the fixed particles moved from the skinned vertices
    let f = ops[1]
        .field("vertexParticlePairs")
        .map_err(fail("moving particles"))?;
    let rows: Vec<Vec<(&str, Vec<u8>)>> = fixed
        .iter()
        .map(|&i| vec![("vertexIndex", le16(i)), ("particleIndex", le16(i))])
        .collect();
    let data = ed.elements(f, &rows)?;
    ed.set_array(f, fixed.len(), data)?;
    // 2, simulate: the order of the sets, every collider (none)
    let configs = ops[2]
        .field("simulateOpConfigs")
        .map_err(fail("simulating"))?;
    let configs = configs.array().map_err(fail("simulating"))?;
    for c in 0..configs.len() {
        let config = configs.get(c).map_err(fail("simulating"))?;
        let f = config
            .field("constraintExecution")
            .map_err(fail("simulating"))?;
        let size = ed
            .tag
            .stride(ed.element_type(f)?)
            .map_err(fail("simulating"))?;
        let data: Vec<u8> = execution
            .iter()
            .flat_map(|x| x.to_le_bytes()[..size].to_vec())
            .collect();
        ed.set_array(f, execution.len(), data)?;
        if config.has_field("instanceCollidablesUsed") {
            ed.set_array(
                config
                    .field("instanceCollidablesUsed")
                    .map_err(fail("simulating"))?,
                0,
                Vec::new(),
            )?;
        }
    }
    // 3, each bone set from its triangle
    let f = ops[3]
        .field("triangleBonePairs")
        .map_err(fail("bones from triangles"))?;
    let rows: Vec<Vec<(&str, Vec<u8>)>> = chain_indices
        .iter()
        .zip(&s.bone_triangles)
        .map(|(&b, &t)| {
            vec![
                ("boneOffset", le16((b * 64) as u16)),
                ("triangleOffset", le16((t * 6) as u16)),
            ]
        })
        .collect();
    let data = ed.elements(f, &rows)?;
    ed.set_array(f, rows.len(), data)?;
    let mut locals = Vec::new();
    for (&model_bone, &t) in part.bones.iter().zip(&s.bone_triangles) {
        let [a, b, c] = s.triangles[t];
        let frame = triangle_frame(
            s.positions[a as usize],
            s.positions[b as usize],
            s.positions[c as usize],
        );
        let inv = math::inverse_affine(&frame).ok_or_else(|| {
            Error(format!(
                "bone {}: its triangle has no area",
                skeleton[model_bone].name
            ))
        })?;
        locals.extend(f32s(&math::mul(&inv, &world[model_bone])));
    }
    ed.set_array(
        ops[3]
            .field("localBoneTransforms")
            .map_err(fail("bones from triangles"))?,
        part.bones.len(),
        locals,
    )?;
    // 4, 5: the skinned vertices copied into the simulation's buffers
    for op in &ops[4..6] {
        if op.has_field("numberOfVertices") {
            ed.put_int(
                op.field("numberOfVertices").map_err(fail("a copy"))?,
                n as i64,
            )?;
        }
        if op.has_field("vertexInputFromVertexOutput") {
            let f = op
                .field("vertexInputFromVertexOutput")
                .map_err(fail("a gather"))?;
            let size = ed
                .tag
                .stride(ed.element_type(f)?)
                .map_err(fail("a gather"))?;
            let data: Vec<u8> = (0..n as i64)
                .flat_map(|i| i.to_le_bytes()[..size].to_vec())
                .collect();
            ed.set_array(f, n, data)?;
        }
    }
    // which bones each operator and state reads and writes: the skin reads the anchor, the simulation the colliders'
    // bones, the bones from triangles write the chain
    let mut collider_bones = collider_indices.clone();
    collider_bones.sort_unstable();
    collider_bones.dedup();
    let reads = |k: usize| -> (Vec<usize>, Vec<usize>) {
        match k {
            0 => (vec![anchor_index], vec![]),
            2 => (collider_bones.clone(), vec![]),
            3 => (vec![], chain_indices.clone()),
            _ => (vec![], vec![]),
        }
    };
    let trackers =
        |ed: &mut Editor, access: Value, read: &[usize], written: &[usize]| -> Result<()> {
            let a = access.array().map_err(fail("transform set use"))?;
            for i in 0..a.len() {
                let usage = a
                    .get(i)
                    .and_then(|x| x.field("transformSetUsage"))
                    .map_err(fail("transform set use"))?;
                let t = usage
                    .field("perComponentTransformTrackers")
                    .and_then(|x| x.array())
                    .map_err(fail("transform set use"))?;
                for j in 0..t.len() {
                    let tracker = t.get(j).map_err(fail("transform set use"))?;
                    ed.set_bits(
                        tracker.field("read").map_err(fail("a tracker"))?,
                        count,
                        read,
                    )?;
                    ed.set_bits(
                        tracker
                            .field("readBeforeWrite")
                            .map_err(fail("a tracker"))?,
                        count,
                        read,
                    )?;
                    ed.set_bits(
                        tracker.field("written").map_err(fail("a tracker"))?,
                        count,
                        written,
                    )?;
                }
            }
            Ok(())
        };
    for (k, op) in ops.iter().enumerate() {
        let (r, w) = reads(k);
        trackers(
            ed,
            op.field("usedTransformSets").map_err(fail("an operator"))?,
            &r,
            &w,
        )?;
    }
    let states = pointed(t_value.field("clothStateDatas").map_err(fail("a part"))?)?;
    for (st, t_state) in states.iter().zip(&t_cloth.states) {
        let mut r = Vec::new();
        let mut w = Vec::new();
        for &o in &t_state.operators {
            let (a, b) = reads(o as usize);
            r.extend(a);
            w.extend(b);
        }
        r.sort_unstable();
        r.dedup();
        w.sort_unstable();
        w.dedup();
        trackers(
            ed,
            st.field("usedTransformSets").map_err(fail("a state"))?,
            &r,
            &w,
        )?;
    }
    Ok((data_item, skeleton_item))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_vectors_round_trip() {
        for v in [
            [0.0, 0.0, 0.0],
            [0.1244, 1.491, -0.0696],
            [-0.555, 0.039, 0.795],
            [12.5, -3.0, 0.001],
            [1e-4, 2e-4, -3e-4],
        ] {
            let back = unpack(pack(v));
            let largest = v.iter().fold(0f32, |m, x: &f32| m.max(x.abs()));
            for k in 0..3 {
                assert!(
                    (back[k] - v[k]).abs() <= largest / 30000.0 + 1e-12,
                    "{v:?} -> {back:?}"
                );
            }
        }
    }

    #[test]
    fn chains_of_new_ph_bones() {
        let bone = |name: &str, parent: Option<usize>, t: [f32; 3]| RestBone {
            name: name.into(),
            parent,
            translation: t,
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        };
        // Root, Hand (game bones); Ph_Tail_1, Ph_Tail_2 under Hand (new); Ph_Game (a game bone); Extra (new, not Ph_)
        let skeleton = vec![
            bone("Root", None, [0.0; 3]),
            bone("Hand", Some(0), [0.5, 1.0, 0.0]),
            bone("Ph_Tail_1", Some(1), [0.0, -0.1, 0.0]),
            bone("Ph_Tail_2", Some(2), [0.0, -0.2, 0.0]),
            bone("Ph_Game", Some(1), [0.0, -0.1, 0.0]),
            bone("Extra", Some(1), [0.1, 0.0, 0.0]),
        ];
        let added = vec![
            "Ph_Tail_1".to_owned(),
            "Ph_Tail_2".to_owned(),
            "Extra".to_owned(),
        ];
        let tips = std::collections::BTreeMap::from([("Ph_Tail_2".to_owned(), [0.5, 0.5, 0.0])]);
        let (parts, notes) = chains_of(&skeleton, &added, &[], &tips, &Default::default());
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(parts.len(), 1);
        let p = &parts[0];
        assert_eq!(p.name, "SimulationMesh_Tail_1");
        assert_eq!((p.anchor, p.bones.clone()), (1, vec![2, 3]));
        let near = |a: [f32; 3], b: [f32; 3]| distance(a, b) < 1e-5;
        assert!(near(p.joints[0], [0.5, 0.9, 0.0]));
        assert!(near(p.joints[1], [0.5, 0.7, 0.0]));
        assert!(near(p.joints[2], [0.5, 0.5, 0.0]));
        // across: along x, a quarter of the first bone (0.2 m)
        assert!(near(p.across[0], [0.05, 0.0, 0.0]));
        // without the tips: the last bone as long as the one before
        let (parts, notes) = chains_of(
            &skeleton,
            &added,
            &[],
            &Default::default(),
            &Default::default(),
        );
        assert_eq!(notes.len(), 1);
        assert!(near(parts[0].joints[2], [0.5, 0.5, 0.0]));
        // a new bone ticked is simulated whatever its name (its own part, hanging from Hand); a game bone isn't
        let ticked = vec!["Extra".to_owned(), "Ph_Game".to_owned()];
        let (parts, _) = chains_of(&skeleton, &added, &ticked, &tips, &Default::default());
        let names: Vec<&str> = parts.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["SimulationMesh_Tail_1", "SimulationMesh_Extra"]);
        assert_eq!((parts[1].anchor, parts[1].bones.clone()), (1, vec![5]));
    }

    #[test]
    fn settings_from_the_scene() {
        assert_eq!(
            PartSettings::from_scene(&SceneClothSettings::default()),
            PartSettings::default()
        );
        let s = PartSettings::from_scene(&SceneClothSettings {
            swing: Some(1.0),
            stiffness: Some(2.0),
            gravity: Some(0.5),
            colliders: Some(" Collidable_Leg_1_R, Collidable_Waist ;".into()),
        });
        assert_eq!((s.swing, s.stiffness, s.gravity), (1.0, 1.0, 0.5));
        assert_eq!(
            s.colliders,
            ColliderChoice::Named(vec!["Collidable_Leg_1_R".into(), "Collidable_Waist".into()])
        );
        let none = |c: &str| {
            PartSettings::from_scene(&SceneClothSettings {
                colliders: Some(c.into()),
                ..Default::default()
            })
            .colliders
        };
        assert_eq!(none("None"), ColliderChoice::None);
        assert_eq!(none("AUTO"), ColliderChoice::Auto);
        assert_eq!(none(""), ColliderChoice::Auto);
        // a chain takes the settings of its topmost bone that has any
        let bone = |name: &str, parent: Option<usize>| RestBone {
            name: name.into(),
            parent,
            translation: [0.0, -0.1, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        };
        let skeleton = vec![
            bone("Root", None),
            bone("Ph_A", Some(0)),
            bone("Ph_B", Some(1)),
        ];
        let added = vec!["Ph_A".to_owned(), "Ph_B".to_owned()];
        let settings = std::collections::BTreeMap::from([(
            "Ph_B".to_owned(),
            SceneClothSettings {
                gravity: Some(2.0),
                ..Default::default()
            },
        )]);
        let (parts, _) = chains_of(&skeleton, &added, &[], &Default::default(), &settings);
        assert_eq!(parts[0].settings.gravity, 2.0);
    }

    #[test]
    fn a_strip_down_a_chain() {
        let part = NewPart {
            name: "SimulationMesh_Test".into(),
            anchor: 0,
            bones: vec![1, 2],
            joints: vec![[0.0, 1.0, 0.0], [0.0, 0.8, 0.0], [0.0, 0.6, 0.0]],
            across: vec![[0.05, 0.0, 0.0]; 3],
            settings: PartSettings::default(),
        };
        let s = strip(&part).unwrap();
        assert_eq!(s.positions.len(), 6);
        assert_eq!(s.triangles, [[0, 1, 2], [1, 3, 2], [2, 3, 4], [3, 5, 4]]);
        assert_eq!(s.bone_triangles, [0, 2]);
        assert_eq!(s.rails, [(0, 2), (2, 4)]);
        // across the middle joint, down each side: what holds the chain straight
        assert_eq!(s.bends, [(0, 4), (1, 5)]);
        // no stiffness: the top two fixed, no brace
        assert_eq!(s.fixed, [0, 1]);
        assert!(s.braces.is_empty());
        // every edge of every triangle is a link, but the one between the two fixed particles
        assert!(!s.edges.contains(&(0, 1)) && !s.edges.contains(&(1, 0)));
        for t in &s.triangles {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[0], t[2])] {
                if (a.min(b), a.max(b)) == (0, 1) {
                    continue;
                }
                assert!(
                    s.edges.contains(&(a, b)) || s.edges.contains(&(b, a)),
                    "{a}-{b}"
                );
            }
        }
    }

    #[test]
    fn a_stiff_part_is_braced_at_its_top() {
        // one bone sticking out forward (+z), the strip across it along x
        let part = NewPart {
            name: "SimulationMesh_Test".into(),
            anchor: 0,
            bones: vec![1],
            joints: vec![[0.0, 1.0, 0.0], [0.0, 1.0, 0.2]],
            across: vec![[0.05, 0.0, 0.0]; 2],
            settings: PartSettings {
                stiffness: 0.3,
                ..PartSettings::default()
            },
        };
        let s = strip(&part).unwrap();
        // the brace after the strip, fixed, linked to the bone's tip on both sides
        assert_eq!(s.positions.len(), 5);
        assert_eq!(s.fixed, [0, 1, 4]);
        assert_eq!(s.braces, [(4, 2), (4, 3)]);
        assert!(!s.free(4) && s.free(2) && s.free(3));
        // off the strip's plane (here up or down), a bone's length from the top
        let b = s.positions[4];
        assert!(b[0].abs() < 1e-6 && b[2].abs() < 1e-6, "{b:?}");
        assert!(((b[1] - 1.0).abs() - 0.2).abs() < 1e-5, "{b:?}");
        // turning the bone on its hinge (about x at the top) changes the brace's link: it pulls it back
        let rest = distance(b, s.positions[2]);
        let turned = [0.0, 1.0 - 0.2 * 0.1f32.sin(), 0.2 * 0.1f32.cos()];
        assert!((distance(b, turned) - rest).abs() > 0.01);
    }

    #[test]
    fn a_bone_on_its_triangle_comes_back_at_rest() {
        // the stored matrix times the triangle's frame gives the bone's place
        let a = [0.1, 1.2, 0.0];
        let b = [0.15, 1.2, 0.0];
        let c = [0.1, 1.0, 0.02];
        let frame = triangle_frame(a, b, c);
        let bone = math::from_trs(
            [0.1, 1.2, 0.0],
            math::quat_from_euler_xyz([0.3, 0.2, 0.1]),
            [1.0; 3],
        );
        let local = math::mul(&math::inverse_affine(&frame).unwrap(), &bone);
        let back = math::mul(&frame, &local);
        for k in 0..16 {
            assert!((back[k] - bone[k]).abs() < 1e-4, "{back:?} {bone:?}");
        }
    }
}
