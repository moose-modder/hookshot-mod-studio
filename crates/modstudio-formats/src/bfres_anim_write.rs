//! Skeletal animations written back: an animation file ([`crate::bfres_anim`]) with some bones' base values and
//! curves replaced.
//!
//! The file is kept as it is up to the end of its first relocation section (everything but the relocation table:
//! the header, the animations, their curves and the string table); edited fields are patched there. A bone's new
//! curves go after it, each a header (a new array of them per edited bone) with its frames and keys, and so does a
//! base-value block that changed size. The relocation table (`_RLT`: where the file's pointers are, for the game to
//! turn them into addresses) is written again after them, with entries for every new pointer.
//!
//! New curves are written as most of the game's are: value-keyed Hermite curves with a key at every frame, frames
//! as u8 (or 10.5 fixed point frames scaled by 1 for longer animations), values and slopes quantised to 16 bits
//! (unsigned, times a scale plus the minimum). A bone whose rotation is a quaternion gets one quaternion curve (four
//! f32 per key). An animation's curve count, its baked size (curves x (4 x frames + 32), rounded up to 8: so in
//! all 22,875 of the game's), and each bone's index of its first curve are counted again.

use std::collections::{BTreeMap, HashSet};

use crate::bfres_anim::{BoneAnim, Curve, SkeletalAnim, bone_flags, curve_flags, target};
use crate::bytes::{ReadLe, check_magic};
use crate::error::{Result, invalid, unsupported};

/// One way a curve of an edited bone is had.
#[derive(Debug, Clone, PartialEq)]
pub enum CurveEdit {
    /// The bone's curve of this index, kept as it is.
    Keep(usize),
    /// A component (`target`) with a value at every frame, from 0.
    Scalar { target: u32, values: Vec<f32> },
    /// A quaternion bone's rotation (x, y, z, w) at every frame, from 0.
    Quaternion { values: Vec<[f32; 4]> },
}

/// What a bone's animation becomes: its base values (all three written) and its curves.
#[derive(Debug, Clone, PartialEq)]
pub struct BoneEdit {
    pub base_scale: [f32; 3],
    /// Euler angles (x, y, z, then 1) or a quaternion, as the bone's rotation is ([`BoneAnim::quaternion`]).
    pub base_rotation: [f32; 4],
    pub base_translation: [f32; 3],
    pub curves: Vec<CurveEdit>,
}

/// An entry of the relocation table: at `pos`, `structs` times: `ptrs` pointers (8 bytes each), then `pad` words
/// that aren't.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    pos: u32,
    structs: u16,
    ptrs: u8,
    pad: u8,
}

/// A section of the relocation table (its own pointer, always 0, isn't kept).
#[derive(Debug, Clone, Copy)]
struct Section {
    pos: u32,
    size: u32,
    count: u32,
}

/// An animation file being edited ([`AnimWriter::edit_bone`], then [`AnimWriter::finish`]).
pub struct AnimWriter {
    /// The file up to the end of its first relocation section, patched.
    head: Vec<u8>,
    /// What goes after it, from `base`.
    tail: Vec<u8>,
    base: usize,
    /// How many sections the table has (the first is the file's data; the others are empty).
    section_count: usize,
    entries: Vec<Entry>,
    /// The pointer fields the table relocates.
    relocated: HashSet<usize>,
    /// Per animation (its offset): per bone, its curve count now (edited bones), to count again at the end.
    counts: BTreeMap<usize, BTreeMap<usize, usize>>,
}

fn write_f32(buf: &mut [u8], at: usize, v: f32) {
    buf[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// Where a target's curve goes among a bone's (scale, rotation, translation, as the game orders them).
fn curve_order(target: u32) -> (u32, u32) {
    let group = match target {
        0x04..=0x0C => 0,
        0x20..=0x2C => 1,
        _ => 2,
    };
    (group, target)
}

/// The bone flag bit saying a target has a curve (the quaternion curve's is the W bit).
fn curve_bit(target: u32, quaternion: bool) -> u32 {
    if quaternion {
        return 1 << 12;
    }
    match target {
        target::SCALE_X => 1 << 6,
        target::SCALE_Y => 1 << 7,
        target::SCALE_Z => 1 << 8,
        target::ROTATE_X => 1 << 9,
        target::ROTATE_Y => 1 << 10,
        target::ROTATE_Z => 1 << 11,
        target::ROTATE_W => 1 << 12,
        target::TRANSLATE_X => 1 << 13,
        target::TRANSLATE_Y => 1 << 14,
        target::TRANSLATE_Z => 1 << 15,
        _ => 0,
    }
}

/// The curve bits of a bone's flags (6 to 15).
const CURVE_BITS: u32 = 0x3FF << 6;

/// The baked size of an animation of `curves` curves and `frames` frames.
pub fn baked_size(curves: usize, frames: i32) -> u32 {
    (curves * (4 * frames.max(0) as usize + 32).next_multiple_of(8)) as u32
}

impl AnimWriter {
    /// Starts editing the decompressed animation file `data`.
    pub fn new(data: &[u8]) -> Result<Self> {
        check_magic(data, 0, "FRES")?;
        let rlt = data.u32_at(0x18)? as usize;
        check_magic(data, rlt, "_RLT")?;
        let count = data.u32_at(rlt + 8)? as usize;
        let sections = (0..count)
            .map(|i| {
                let s = rlt + 16 + i * 24;
                Ok(Section {
                    pos: data.u32_at(s + 8)?,
                    size: data.u32_at(s + 12)?,
                    count: data.u32_at(s + 20)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let first = *sections
            .first()
            .ok_or_else(|| invalid("a relocation table without sections"))?;
        if first.pos != 0 || sections.iter().skip(1).any(|s| s.count != 0) {
            return Err(unsupported(
                "an animation file whose relocation table has more than its first section in use",
            ));
        }
        let entries_at = rlt + 16 + count * 24;
        let entries = (0..first.count as usize)
            .map(|i| {
                let e = entries_at + i * 8;
                Ok(Entry {
                    pos: data.u32_at(e)?,
                    structs: data.u16_at(e + 4)?,
                    ptrs: data.u8_at(e + 6)?,
                    pad: data.u8_at(e + 7)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let mut relocated = HashSet::new();
        for e in &entries {
            let stride = (e.ptrs as usize + e.pad as usize) * 8;
            for s in 0..e.structs as usize {
                for p in 0..e.ptrs as usize {
                    relocated.insert(e.pos as usize + s * stride + p * 8);
                }
            }
        }
        let end = first.size as usize;
        if end > rlt {
            return Err(invalid("the first relocation section runs into the table"));
        }
        Ok(AnimWriter {
            head: data[..end].to_vec(),
            tail: Vec::new(),
            base: end.next_multiple_of(8),
            section_count: sections.len(),
            entries,
            relocated,
            counts: BTreeMap::new(),
        })
    }

    /// Room for `size` bytes after the file's own data, aligned to `align`: its position in the written file.
    fn alloc(&mut self, size: usize, align: usize) -> usize {
        let at = (self.base + self.tail.len()).next_multiple_of(align);
        self.tail.resize(at - self.base + size, 0);
        at
    }

    fn bytes_mut(&mut self, at: usize, len: usize) -> &mut [u8] {
        if at + len <= self.head.len() {
            &mut self.head[at..at + len]
        } else {
            let t = at - self.base;
            &mut self.tail[t..t + len]
        }
    }

    fn put(&mut self, at: usize, bytes: &[u8]) {
        self.bytes_mut(at, bytes.len()).copy_from_slice(bytes);
    }

    /// Sets a pointer field that an entry of its own will list (a new curve header's).
    fn set_listed(&mut self, field: usize, to: usize) {
        self.put(field, &(to as u64).to_le_bytes());
        self.relocated.insert(field);
    }

    /// Sets the pointer field at `field` to `to`, and has the table relocate it.
    fn point(&mut self, field: usize, to: usize) {
        self.put(field, &(to as u64).to_le_bytes());
        if self.relocated.insert(field) {
            self.entries.push(Entry {
                pos: field as u32,
                structs: 1,
                ptrs: 1,
                pad: 0,
            });
        }
    }

    /// Writes one new curve's header at `header` and its frames and keys after the file's data.
    fn new_curve(&mut self, header: usize, edit: &CurveEdit, frame_count: i32) -> Result<()> {
        let frames: Vec<usize> = (0..=frame_count.max(0) as usize).collect();
        let n = frames.len();
        let target = match edit {
            CurveEdit::Scalar { target, values } => {
                if values.len() != n {
                    return Err(invalid(format!(
                        "curve 0x{target:x}: {} values for {n} frames",
                        values.len()
                    )));
                }
                *target
            }
            CurveEdit::Quaternion { values } => {
                if values.len() != n {
                    return Err(invalid(format!(
                        "quaternion curve: {} values for {n} frames",
                        values.len()
                    )));
                }
                target::ROTATE_X
            }
            CurveEdit::Keep(_) => return Err(invalid("a kept curve isn't new")),
        };
        if n > u16::MAX as usize {
            return Err(unsupported(format!("an animation of {frame_count} frames")));
        }
        // frames: u8 up to 255, else 10.5 fixed point scaled by 1 (a u16 frame number)
        let small = frame_count <= 255;
        let frames_at = self.alloc(n * if small { 1 } else { 2 }, 8);
        for (i, &f) in frames.iter().enumerate() {
            if small {
                self.put(frames_at + i, &[f as u8]);
            } else {
                self.put(frames_at + i * 2, &(f as u16).to_le_bytes());
            }
        }
        let frame_bits = if small {
            2
        } else {
            1 | curve_flags::FRAME_SCALED
        };
        let frame_scale = if small { 0.0 } else { 1.0 };
        let (flags, keys_at, value_scale, value_offset) = match edit {
            CurveEdit::Quaternion { values } => {
                let at = self.alloc(n * 16, 8);
                for (i, q) in values.iter().enumerate() {
                    for (j, v) in q.iter().enumerate() {
                        self.put(at + i * 16 + j * 4, &v.to_le_bytes());
                    }
                }
                (
                    curve_flags::VALUE_KEYS | curve_flags::QUATERNION | frame_bits,
                    at,
                    0.0,
                    0.0,
                )
            }
            CurveEdit::Scalar { values, .. } => {
                // slopes per frame: the difference across each key (one-sided at the ends)
                let slopes: Vec<f32> = (0..n)
                    .map(|i| match (i.checked_sub(1), values.get(i + 1)) {
                        (Some(p), Some(&next)) => (next - values[p]) / 2.0,
                        (None, Some(&next)) => next - values[i],
                        (Some(p), None) => values[i] - values[p],
                        (None, None) => 0.0,
                    })
                    .collect();
                let quantise = |v: &[f32]| {
                    let lo = v.iter().copied().fold(f32::INFINITY, f32::min);
                    let hi = v.iter().copied().fold(f32::NEG_INFINITY, f32::max);
                    let scale = if hi > lo { (hi - lo) / 65535.0 } else { 0.0 };
                    let q: Vec<u16> = v
                        .iter()
                        .map(|&x| {
                            if scale > 0.0 {
                                ((x - lo) / scale).round().clamp(0.0, 65535.0) as u16
                            } else {
                                0
                            }
                        })
                        .collect();
                    (lo, scale, q)
                };
                let (v_lo, v_scale, qv) = quantise(values);
                let (s_lo, s_scale, qs) = quantise(&slopes);
                let at = self.alloc(8 + n * 4, 8);
                let mut keys = Vec::with_capacity(8 + n * 4);
                keys.extend_from_slice(&s_scale.to_le_bytes());
                keys.extend_from_slice(&s_lo.to_le_bytes());
                for i in 0..n {
                    keys.extend_from_slice(&qv[i].to_le_bytes());
                    keys.extend_from_slice(&qs[i].to_le_bytes());
                }
                self.put(at, &keys);
                (
                    curve_flags::VALUE_KEYS
                        | curve_flags::SLOPE_QUANTISED
                        | (1 << 2)
                        | curve_flags::CUBIC
                        | frame_bits,
                    at,
                    v_scale,
                    v_lo,
                )
            }
            CurveEdit::Keep(_) => unreachable!(),
        };
        self.set_listed(header, frames_at);
        self.set_listed(header + 8, keys_at);
        let mut h = [0u8; 0x20];
        h[0..2].copy_from_slice(&flags.to_le_bytes());
        h[2..4].copy_from_slice(&(n as u16).to_le_bytes());
        h[4..8].copy_from_slice(&target.to_le_bytes());
        write_f32(&mut h, 8, 0.0);
        write_f32(&mut h, 12, frame_count.max(0) as f32);
        write_f32(&mut h, 16, frame_scale);
        write_f32(&mut h, 20, value_scale);
        write_f32(&mut h, 24, value_offset);
        self.put(header + 0x10, &h);
        Ok(())
    }

    /// Replaces what `anim` does to its bone `index` (`data`: the file as read, for the kept curves' headers).
    pub fn edit_bone(
        &mut self,
        data: &[u8],
        anim: &SkeletalAnim,
        index: usize,
        edit: &BoneEdit,
    ) -> Result<()> {
        let bone: &BoneAnim = anim
            .bones
            .get(index)
            .ok_or_else(|| invalid(format!("animation {}: no bone {index}", anim.name)))?;
        let quaternion = bone.quaternion();
        // the curves, in the game's order
        let mut curves: Vec<(u32, bool, &CurveEdit)> = Vec::new();
        for c in &edit.curves {
            let (t, q) = match c {
                CurveEdit::Keep(i) => {
                    let kept: &Curve = bone.curves.get(*i).ok_or_else(|| {
                        invalid(format!("bone {}: no curve {i} to keep", bone.name))
                    })?;
                    (kept.target, kept.flags & curve_flags::QUATERNION != 0)
                }
                CurveEdit::Scalar { target, .. } => (*target, false),
                CurveEdit::Quaternion { .. } => {
                    if !quaternion {
                        return Err(invalid(format!(
                            "bone {}: a quaternion curve on a bone of Euler angles",
                            bone.name
                        )));
                    }
                    (target::ROTATE_X, true)
                }
            };
            if quaternion && !q && (0x20..=0x2C).contains(&t) {
                return Err(invalid(format!(
                    "bone {}: an angle curve on a quaternion bone",
                    bone.name
                )));
            }
            if curves.iter().any(|(other, _, _)| *other == t) {
                return Err(invalid(format!(
                    "bone {}: two curves for 0x{t:x}",
                    bone.name
                )));
            }
            curves.push((t, q, c));
        }
        curves.sort_by_key(|(t, _, _)| curve_order(*t));
        if curves.len() > u8::MAX as usize {
            return Err(invalid(format!("bone {}: too many curves", bone.name)));
        }
        // a new array of curve headers (kept ones copied: their frames and keys stay where they are)
        let array = self.alloc(curves.len().max(1) * 0x30, 8);
        let mut bits = 0;
        for (i, (t, q, c)) in curves.iter().enumerate() {
            let header = array + i * 0x30;
            bits |= curve_bit(*t, *q);
            match c {
                CurveEdit::Keep(k) => {
                    let from = bone.curves[*k].at;
                    let raw = data.slice_at(from, 0x30)?.to_vec();
                    let frames_at = u64::from_le_bytes(raw[0..8].try_into().expect("8")) as usize;
                    let keys_at = u64::from_le_bytes(raw[8..16].try_into().expect("8")) as usize;
                    self.put(header + 0x10, &raw[0x10..]);
                    self.set_listed(header, frames_at);
                    self.set_listed(header + 8, keys_at);
                }
                _ => self.new_curve(header, c, anim.frame_count)?,
            }
        }
        if !curves.is_empty() {
            // the new headers' pointers, listed by one entry
            self.entries.push(Entry {
                pos: array as u32,
                structs: curves.len() as u16,
                ptrs: 2,
                pad: 4,
            });
        }
        self.point(bone.at + 0x08, array);
        // the base values: in place where the bone had all three, else a new block
        let mut base = Vec::with_capacity(40);
        for v in edit
            .base_scale
            .iter()
            .chain(&edit.base_rotation)
            .chain(&edit.base_translation)
        {
            base.extend_from_slice(&v.to_le_bytes());
        }
        let base_flags =
            bone_flags::BASE_SCALE | bone_flags::BASE_ROTATE | bone_flags::BASE_TRANSLATE;
        let old_base = data.offset_at(bone.at + 0x10)?;
        if old_base != 0 && bone.flags & base_flags == base_flags {
            self.put(old_base, &base);
        } else {
            let at = self.alloc(base.len(), 8);
            self.put(at, &base);
            self.point(bone.at + 0x10, at);
        }
        let flags = (bone.flags & !CURVE_BITS) | bits | base_flags;
        self.put(bone.at + 0x28, &flags.to_le_bytes());
        // where its rotation and translation slots begin: scale 0 to 2, rotation 3 to 5 (one slot for a
        // quaternion), translation after; the base translation after scale and a 4-float rotation
        let begin_translate: u8 = if quaternion { 4 } else { 6 };
        self.put(bone.at + 0x2C, &[3, begin_translate, curves.len() as u8, 7]);
        self.counts
            .entry(anim.at)
            .or_default()
            .insert(index, curves.len());
        Ok(())
    }

    /// Counts the edited animations' curves again (each bone's first, the animation's total and its baked size),
    /// and writes the file: its data, what was added after it, and the relocation table.
    pub fn finish(mut self, anims: &[SkeletalAnim]) -> Result<Vec<u8>> {
        let counts = std::mem::take(&mut self.counts);
        for a in anims {
            let Some(edited) = counts.get(&a.at) else {
                continue;
            };
            let mut total = 0usize;
            for (i, b) in a.bones.iter().enumerate() {
                self.put(b.at + 0x30, &(total as i32).to_le_bytes());
                total += edited.get(&i).copied().unwrap_or(b.curves.len());
            }
            self.put(a.at + 0x44, &(total as i32).to_le_bytes());
            self.put(a.at + 0x48, &baked_size(total, a.frame_count).to_le_bytes());
        }
        let mut out = std::mem::take(&mut self.head);
        if !self.tail.is_empty() {
            out.resize(self.base, 0);
            out.extend_from_slice(&self.tail);
        }
        let end = out.len();
        // the table, after the data
        let rlt = end.next_multiple_of(8);
        out.resize(rlt, 0);
        let mut entries = self.entries;
        entries.sort_by_key(|e| e.pos);
        out.extend_from_slice(b"_RLT");
        out.extend_from_slice(&(rlt as u32).to_le_bytes());
        out.extend_from_slice(&(self.section_count as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        for i in 0..self.section_count {
            out.extend_from_slice(&0u64.to_le_bytes());
            let (pos, size, first, count) = if i == 0 {
                (0, end as u32, 0, entries.len() as u32)
            } else {
                (end as u32, 0, entries.len() as u32, 0)
            };
            out.extend_from_slice(&pos.to_le_bytes());
            out.extend_from_slice(&size.to_le_bytes());
            out.extend_from_slice(&first.to_le_bytes());
            out.extend_from_slice(&count.to_le_bytes());
        }
        for e in &entries {
            out.extend_from_slice(&e.pos.to_le_bytes());
            out.extend_from_slice(&e.structs.to_le_bytes());
            out.extend_from_slice(&[e.ptrs, e.pad]);
        }
        let size = out.len() as u32;
        out[0x18..0x1C].copy_from_slice(&(rlt as u32).to_le_bytes());
        out[0x1C..0x20].copy_from_slice(&size.to_le_bytes());
        Ok(out)
    }
}

/// Every pointer the relocation table of `data` lists, checked to point inside the file (or be null): the count of
/// them.
pub fn check_relocations(data: &[u8]) -> Result<usize> {
    let rlt = data.u32_at(0x18)? as usize;
    check_magic(data, rlt, "_RLT")?;
    let count = data.u32_at(rlt + 8)? as usize;
    let mut total = 0;
    let entries_at = rlt + 16 + count * 24;
    let entry_count: usize = (0..count)
        .map(|i| data.u32_at(rlt + 16 + i * 24 + 20).map(|c| c as usize))
        .sum::<Result<usize>>()?;
    for i in 0..entry_count {
        let e = entries_at + i * 8;
        let pos = data.u32_at(e)? as usize;
        let structs = data.u16_at(e + 4)? as usize;
        let ptrs = data.u8_at(e + 6)? as usize;
        let pad = data.u8_at(e + 7)? as usize;
        for s in 0..structs {
            for p in 0..ptrs {
                let field = pos + s * (ptrs + pad) * 8 + p * 8;
                let to = data.u64_at(field)? as usize;
                // (the game's own tables list null pointers too: an animation file's model array)
                if to >= data.len() {
                    return Err(invalid(format!(
                        "the pointer at 0x{field:x} points to 0x{to:x}, outside the file"
                    )));
                }
                total += 1;
            }
        }
    }
    Ok(total)
}
