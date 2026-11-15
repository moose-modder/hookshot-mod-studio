//! New simulated parts in a model's cloth file, for chains of bones added to the model (`docs/plan.md`, new bones,
//! step 5).
//!
//! Every part of the game's 171 cloth files (1,134) is a cloth data of its own, built alike: one sim cloth; operators
//! skin (the particles' places as their bones carry them), move the fixed particles, simulate, set bones from
//! triangles, and two vertex copies; states `Default`, `AnimToSimCurrent` and `AnimToSimPrev`; three buffers; one
//! transform set, which is the part's own skeleton (the bones it uses and their parents up to the root, matched to the
//! model's by name). So a new part is a copy of one of the file's own (the smallest whose skin is in object space),
//! every array of it made again for the new part:
//!
//! - **a strip** of particles two wide down the chain: at each joint (each bone's head, then the last bone's tip) one
//!   on the chain and one beside it (`across`); the top two fixed, carried by the bone the chain hangs from (its
//!   anchor); two triangles a bone, the bone set from the one at its head (its stored matrix: the inverse of that
//!   triangle's frame at rest times the bone's rest place, `havok-cloth::deform`);
//! - **constraints**: links along every edge of the strip, a stretch link down the chain, a range keeping each free
//!   particle near where the anchor would carry it at rest (half its distance down the chain), and the transition
//!   set (all particles); run in the order the game's parts run them;
//! - **skinning**: every particle on the anchor alone (one-bone blocks of the object space deformer, positions packed
//!   as `hkPackedVector3`, in the model's space);
//! - **no colliders**, no landscape collision particles, no pinch detection;
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
/// of its first bone's length wide, along the model's x axis (across the chain). Notes on what was left out.
pub fn chains_of(
    skeleton: &[RestBone],
    added: &[String],
    ticked: &[String],
    tips: &std::collections::BTreeMap<String, [f32; 3]>,
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
        parts.push(NewPart {
            name: format!("SimulationMesh_{what}"),
            anchor,
            bones,
            joints,
            across,
        });
    }
    (parts, notes)
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

/// The strip of a part: particles (on the chain at even indices, beside it at odd ones), triangles, and which
/// triangle sets each bone.
struct Strip {
    positions: Vec<[f32; 3]>,
    triangles: Vec<[u16; 3]>,
    /// Per bone of the chain: its triangle.
    bone_triangles: Vec<usize>,
    /// Edges of the strip (every link), and the ones down the chain.
    edges: Vec<(u16, u16)>,
    rails: Vec<(u16, u16)>,
}

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
        edges.extend([
            (l(k - 1), l(k)),
            (r(k - 1), r(k)),
            (r(k - 1), l(k)),
            (l(k), r(k)),
        ]);
        rails.push((l(k - 1), l(k)));
    }
    Ok(Strip {
        positions,
        triangles,
        bone_triangles,
        edges,
        rails,
    })
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
/// chains' bones in it). Read back before it is returned.
pub fn add_parts(bytes: &[u8], skeleton: &[RestBone], parts: &[NewPart]) -> Result<Vec<u8>> {
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
    let mut template: Option<(usize, usize)> = None;
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
        let n = c.sim_cloths[0].particles.len();
        if template.is_none_or(|(_, m)| n < m) {
            template = Some((i, n));
        }
    }
    let (ti, _) = template.ok_or_else(|| {
        Error(
            "the cloth file has no part to make new ones from (one whose skin is in object space)"
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
    let mut new_datas = Vec::new();
    let mut new_skeletons = Vec::new();
    for part in parts {
        ed.copies.clear();
        let (data_item, skeleton_item) = make_part(
            &mut ed, t_value, t_cloth, t_skeleton, skeleton, &world, part,
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
        let n = 2 * (part.bones.len() + 1);
        if c.sim_cloths[0].particles.len() != n {
            return Err(Error(format!(
                "the written part {}: {} particles, not {n}",
                part.name,
                c.sim_cloths[0].particles.len()
            )));
        }
    }
    Ok(out)
}

/// One new part: its cloth data and skeleton, copied from the template's and made for `part`. Their items.
fn make_part(
    ed: &mut Editor,
    t_value: Value,
    t_cloth: &cloth::Cloth,
    t_skeleton: Value,
    skeleton: &[RestBone],
    world: &[Mat4],
    part: &NewPart,
) -> Result<(usize, usize)> {
    let s = strip(part)?;
    let n = s.positions.len();
    let fixed: Vec<u16> = vec![0, 1];
    // the part's skeleton: the anchor and its parents from the root, then the chain
    let mut path = vec![part.anchor];
    while let Some(p) = skeleton[*path.last().expect("one")].parent {
        if path.contains(&p) || path.len() > skeleton.len() {
            return Err(Error(format!(
                "bone {}: its parents loop",
                skeleton[part.anchor].name
            )));
        }
        path.push(p);
    }
    path.reverse();
    let anchor_index = path.len() - 1;
    let mut bones: Vec<usize> = path;
    bones.extend(&part.bones);
    let count = bones.len();
    let chain_indices: Vec<usize> = (anchor_index + 1..count).collect();
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
    let particles: Vec<cloth::Particle> = (0..n).map(|i| if i < 2 { held } else { free }).collect();
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
    let max_radius = particles.iter().fold(0f32, |m, p| m.max(p.radius));
    ed.put_f32(
        sim.field("maxParticleRadius")
            .map_err(fail("a sim cloth"))?,
        max_radius,
    )?;
    let total: f32 = particles.iter().map(|p| p.mass).sum();
    ed.put_f32(sim.field("totalMass").map_err(fail("a sim cloth"))?, total)?;
    let f = sim
        .field("staticCollisionMasks")
        .map_err(fail("a sim cloth"))?;
    let size = ed.tag.stride(ed.element_type(f)?).map_err(fail("a mask"))?;
    ed.set_array(f, n, vec![0; size * n])?;
    for name in [
        "perInstanceCollidables",
        "collidablePinchingDatas",
        "setupCollidableSidedness",
        "antiPinchConstraintSets",
        "triangleFlips",
    ] {
        if sim.has_field(name) {
            ed.set_array(sim.field(name).map_err(fail("a sim cloth"))?, 0, Vec::new())?;
        }
    }
    let map = sim
        .field("collidableTransformMap")
        .map_err(fail("a sim cloth"))?;
    for name in ["transformIndices", "offsets"] {
        ed.set_array(
            map.field(name).map_err(fail("a collider map"))?,
            0,
            Vec::new(),
        )?;
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
    // template's share (its largest), on every link of the strip.
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
    .clamp(0.1, 1.0);
    // stretch links move particle B by the stiffness alone
    let links = |edges: &[(u16, u16)], standard: bool| -> Vec<Vec<(&str, Vec<u8>)>> {
        edges
            .iter()
            .map(|&(a, b)| {
                let rest = distance(s.positions[a as usize], s.positions[b as usize]);
                let w = inv_mass(a) + inv_mass(b);
                let stiffness = if !standard {
                    1.0
                } else if w > 0.0 {
                    share / w
                } else {
                    0.0
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
    let data = ed.elements(f, &links(&s.edges, true))?;
    ed.set_array(f, s.edges.len(), data)?;
    if let Some(st) = stretch {
        let f = st.field("links").map_err(fail("stretch links"))?;
        let data = ed.elements(f, &links(&s.rails, false))?;
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
    let rows: Vec<Vec<(&str, Vec<u8>)>> = (2..n)
        .map(|i| {
            let mut row = vec![
                ("particleIndex", le16(i as u16)),
                ("referenceVertex", le16(i as u16)),
                (distance_member, le32((0.5 * down[i]).max(0.01).to_bits())),
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
    ed.set_array(f, n - 2, data)?;
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
    // 0, skin: every particle on the anchor, its place at rest in the model
    let skin = ops[0];
    let inverse = math::inverse_affine(&anchor_world).ok_or_else(|| {
        Error(format!(
            "bone {}: scaled to nothing",
            skeleton[part.anchor].name
        ))
    })?;
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
    let rows: Vec<Vec<(&str, Vec<u8>)>> = (0..blocks)
        .map(|b| {
            let vertices: Vec<u8> = (0..16)
                .flat_map(|k| ((16 * b + k).min(n - 1) as u16).to_le_bytes())
                .collect();
            let mut row = vec![
                ("vertexIndices", vertices),
                ("boneIndices", vec![0u8; 2 * 16 * per_vertex]),
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
    // which bones each operator and state reads and writes: the skin reads the anchor, the bones from triangles
    // write the chain
    let reads = |k: usize| -> (Vec<usize>, Vec<usize>) {
        match k {
            0 => (vec![anchor_index], vec![]),
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
        let (parts, notes) = chains_of(&skeleton, &added, &[], &tips);
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
        let (parts, notes) = chains_of(&skeleton, &added, &[], &Default::default());
        assert_eq!(notes.len(), 1);
        assert!(near(parts[0].joints[2], [0.5, 0.5, 0.0]));
        // a new bone ticked is simulated whatever its name (its own part, hanging from Hand); a game bone isn't
        let ticked = vec!["Extra".to_owned(), "Ph_Game".to_owned()];
        let (parts, _) = chains_of(&skeleton, &added, &ticked, &tips);
        let names: Vec<&str> = parts.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["SimulationMesh_Tail_1", "SimulationMesh_Extra"]);
        assert_eq!((parts[1].anchor, parts[1].bones.clone()), (1, vec![5]));
    }

    #[test]
    fn a_strip_down_a_chain() {
        let part = NewPart {
            name: "SimulationMesh_Test".into(),
            anchor: 0,
            bones: vec![1, 2],
            joints: vec![[0.0, 1.0, 0.0], [0.0, 0.8, 0.0], [0.0, 0.6, 0.0]],
            across: vec![[0.05, 0.0, 0.0]; 3],
        };
        let s = strip(&part).unwrap();
        assert_eq!(s.positions.len(), 6);
        assert_eq!(s.triangles, [[0, 1, 2], [1, 3, 2], [2, 3, 4], [3, 5, 4]]);
        assert_eq!(s.bone_triangles, [0, 2]);
        assert_eq!(s.rails, [(0, 2), (2, 4)]);
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
