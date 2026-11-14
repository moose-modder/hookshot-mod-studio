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
//!
//! An animation can also be given tracks for bones it doesn't move ([`AnimWriter::add_bones`]): bones added to the
//! model, or the game's own its animation leaves at rest. Animations bind to bones by name, and the game skips a
//! name its model hasn't (its own animations name bones 204 of the 580 models given them lack), so a file shared by
//! several models can take a bone only one of them has. The animation's track array is written again after the
//! file's data (the game's tracks copied, the new ones after), with its bind indices, and the new names.
//!
//! A track's flags say, besides which base values and curves it has, whether its scale is uniform, its scale's volume
//! one, its rotation none and its translation none (bits 24 to 27): from its base values, each cleared where a curve
//! moves that part. So in 99.8% of the game's 3.1 million tracks; the others have fewer set. Edited tracks keep only
//! the ones that still hold (a bone that didn't turn and now does mustn't keep "no rotation").

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

/// A track for a bone an animation doesn't move yet ([`AnimWriter::add_bones`]).
#[derive(Debug, Clone, PartialEq)]
pub struct NewTrack {
    /// The bone's name (animations bind to bones by name).
    pub name: String,
    /// Its rotation a quaternion (its base rotation one, and its rotation one quaternion curve), else Euler angles.
    pub quaternion: bool,
    /// Its flags besides the base, curve, rotation and transform ones (most of the game's tracks have bit 23).
    pub flags: u32,
    pub edit: BoneEdit,
}

/// A track's size.
const TRACK_SIZE: usize = 0x38;

/// The transform bits of a track's flags (24 to 27: scale uniform, scale volume one, no rotation, no translation).
const TRANSFORM_BITS: u32 = 0x0F00_0000;

/// The transform bits ([`TRANSFORM_BITS`]) a track's base values give, each cleared where one of its curves
/// (`targets`) moves that part.
pub fn transform_bits(edit: &BoneEdit, quaternion: bool, targets: &[u32]) -> u32 {
    let moved = |lo: u32, hi: u32| targets.iter().any(|t| (lo..=hi).contains(t));
    let (s, r, t) = (edit.base_scale, edit.base_rotation, edit.base_translation);
    let mut bits = 0;
    if !moved(0x04, 0x0C) {
        if s[0] == s[1] && s[1] == s[2] {
            bits |= 1 << 24;
        }
        if s[0] * s[1] * s[2] == 1.0 {
            bits |= 1 << 25;
        }
    }
    let no_rotation = r[0] == 0.0 && r[1] == 0.0 && r[2] == 0.0 && (!quaternion || r[3] == 1.0);
    if !moved(0x20, 0x2C) && no_rotation {
        bits |= 1 << 26;
    }
    if !moved(0x10, 0x18) && t == [0.0; 3] {
        bits |= 1 << 27;
    }
    bits
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
    /// Per animation given new tracks (its offset): where its track array is now, and how many tracks it gained.
    moved: BTreeMap<usize, (usize, usize)>,
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
            moved: BTreeMap::new(),
        })
    }

    /// The bytes at `at` as they are now.
    fn get(&self, at: usize, len: usize) -> Vec<u8> {
        if at + len <= self.head.len() {
            self.head[at..at + len].to_vec()
        } else {
            let t = at - self.base;
            self.tail[t..t + len].to_vec()
        }
    }

    /// Where track `index` of `anim` is now (its array moves when tracks are added).
    fn record(&self, anim: &SkeletalAnim, index: usize) -> usize {
        match self.moved.get(&anim.at) {
            Some(&(array, _)) => array + index * TRACK_SIZE,
            None => anim.bones[index].at,
        }
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
        let rec = self.record(anim, index);
        let n = self.write_track(
            data,
            anim.frame_count,
            rec,
            Some(bone),
            &bone.name,
            bone.flags,
            edit,
        )?;
        self.counts.entry(anim.at).or_default().insert(index, n);
        Ok(())
    }

    /// Gives `anim` a track for each of `tracks`' bones (none of which it has): its track array written again after
    /// the file's data, the game's tracks as they are now (edited ones too) and the new ones after, with their bind
    /// indices (unset, as the game's are: bound by name) and names. Once per animation; its own tracks may be
    /// edited before or after.
    pub fn add_bones(
        &mut self,
        data: &[u8],
        anim: &SkeletalAnim,
        tracks: &[NewTrack],
    ) -> Result<()> {
        if tracks.is_empty() {
            return Ok(());
        }
        if self.moved.contains_key(&anim.at) {
            return Err(invalid(format!(
                "animation {}: tracks added twice",
                anim.name
            )));
        }
        let mut names: HashSet<&str> = anim.bones.iter().map(|b| b.name.as_str()).collect();
        for t in tracks {
            if !names.insert(t.name.as_str()) {
                return Err(invalid(format!(
                    "animation {}: it has a track for {} already",
                    anim.name, t.name
                )));
            }
            if t.name.len() > u16::MAX as usize {
                return Err(invalid(format!("a bone name of {} bytes", t.name.len())));
            }
        }
        let n = anim.bones.len();
        let total = n + tracks.len();
        if total > u16::MAX as usize {
            return Err(unsupported(format!(
                "animation {}: {total} tracks",
                anim.name
            )));
        }
        // the game's tracks, copied, their pointers listed as they were (by runs, a word at a time)
        let array = self.alloc(total * TRACK_SIZE, 8);
        for (i, b) in anim.bones.iter().enumerate() {
            let raw = self.get(b.at, TRACK_SIZE);
            self.put(array + i * TRACK_SIZE, &raw);
        }
        for w in 0..TRACK_SIZE / 8 {
            let mut i = 0;
            while i < n {
                if !self.relocated.contains(&(anim.bones[i].at + w * 8)) {
                    i += 1;
                    continue;
                }
                let start = i;
                while i < n
                    && i - start < u16::MAX as usize
                    && self.relocated.contains(&(anim.bones[i].at + w * 8))
                {
                    self.relocated.insert(array + i * TRACK_SIZE + w * 8);
                    i += 1;
                }
                self.entries.push(Entry {
                    pos: (array + start * TRACK_SIZE + w * 8) as u32,
                    structs: (i - start) as u16,
                    ptrs: 1,
                    pad: (TRACK_SIZE / 8 - 1) as u8,
                });
            }
        }
        self.point(anim.at + 0x28, array);
        let bind_at = data.offset_at(anim.at + 0x20)?;
        if bind_at != 0 {
            let bind = self.alloc(total * 2, 8);
            for i in 0..total {
                let v = anim.bind_indices.get(i).copied().unwrap_or(0xFFFF);
                self.put(bind + i * 2, &v.to_le_bytes());
            }
            self.point(anim.at + 0x20, bind);
        }
        self.put(anim.at + 0x4C, &(total as u16).to_le_bytes());
        self.moved.insert(anim.at, (array, tracks.len()));
        for (j, t) in tracks.iter().enumerate() {
            let rec = array + (n + j) * TRACK_SIZE;
            let name = self.alloc(2 + t.name.len() + 1, 8);
            self.put(name, &(t.name.len() as u16).to_le_bytes());
            self.put(name + 2, t.name.as_bytes());
            self.point(rec, name);
            // the two unused pointer words, null and listed as the game's are
            self.point(rec + 0x18, 0);
            self.point(rec + 0x20, 0);
            let quaternion = if t.quaternion {
                bone_flags::QUATERNION
            } else {
                0
            };
            let flags =
                (t.flags & !(CURVE_BITS | bone_flags::QUATERNION)) | quaternion | TRANSFORM_BITS;
            let count =
                self.write_track(data, anim.frame_count, rec, None, &t.name, flags, &t.edit)?;
            self.counts.entry(anim.at).or_default().insert(n + j, count);
        }
        Ok(())
    }

    /// Writes a track's curves, base values and flags into the track at `rec`: `bone` the game's it was (whose
    /// curves can be kept), else a new one (`flags`: what it starts from). Its number of curves.
    #[allow(clippy::too_many_arguments)]
    fn write_track(
        &mut self,
        data: &[u8],
        frame_count: i32,
        rec: usize,
        bone: Option<&BoneAnim>,
        name: &str,
        flags: u32,
        edit: &BoneEdit,
    ) -> Result<usize> {
        let quaternion = flags & bone_flags::QUATERNION != 0;
        // the curves, in the game's order
        let mut curves: Vec<(u32, bool, &CurveEdit)> = Vec::new();
        for c in &edit.curves {
            let (t, q) = match c {
                CurveEdit::Keep(i) => {
                    let kept: &Curve = bone
                        .and_then(|b| b.curves.get(*i))
                        .ok_or_else(|| invalid(format!("bone {name}: no curve {i} to keep")))?;
                    (kept.target, kept.flags & curve_flags::QUATERNION != 0)
                }
                CurveEdit::Scalar { target, .. } => (*target, false),
                CurveEdit::Quaternion { .. } => {
                    if !quaternion {
                        return Err(invalid(format!(
                            "bone {name}: a quaternion curve on a bone of Euler angles"
                        )));
                    }
                    (target::ROTATE_X, true)
                }
            };
            if quaternion && !q && (0x20..=0x2C).contains(&t) {
                return Err(invalid(format!(
                    "bone {name}: an angle curve on a quaternion bone"
                )));
            }
            if curves.iter().any(|(other, _, _)| *other == t) {
                return Err(invalid(format!("bone {name}: two curves for 0x{t:x}")));
            }
            curves.push((t, q, c));
        }
        curves.sort_by_key(|(t, _, _)| curve_order(*t));
        if curves.len() > u8::MAX as usize {
            return Err(invalid(format!("bone {name}: too many curves")));
        }
        let mut bits = 0;
        if curves.is_empty() {
            // none: a null pointer, as the game's tracks without curves have
            self.put(rec + 0x08, &0u64.to_le_bytes());
        } else {
            // a new array of curve headers (kept ones copied: their frames and keys stay where they are)
            let array = self.alloc(curves.len() * 0x30, 8);
            for (i, (t, q, c)) in curves.iter().enumerate() {
                let header = array + i * 0x30;
                bits |= curve_bit(*t, *q);
                match (c, bone) {
                    (CurveEdit::Keep(k), Some(bone)) => {
                        let from = bone.curves[*k].at;
                        let raw = data.slice_at(from, 0x30)?.to_vec();
                        let frames_at =
                            u64::from_le_bytes(raw[0..8].try_into().expect("8")) as usize;
                        let keys_at =
                            u64::from_le_bytes(raw[8..16].try_into().expect("8")) as usize;
                        self.put(header + 0x10, &raw[0x10..]);
                        self.set_listed(header, frames_at);
                        self.set_listed(header + 8, keys_at);
                    }
                    _ => self.new_curve(header, c, frame_count)?,
                }
            }
            // the new headers' pointers, listed by one entry
            self.entries.push(Entry {
                pos: array as u32,
                structs: curves.len() as u16,
                ptrs: 2,
                pad: 4,
            });
            self.point(rec + 0x08, array);
        }
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
        let old_base = match bone {
            Some(b) if b.flags & base_flags == base_flags => data.offset_at(b.at + 0x10)?,
            _ => 0,
        };
        if old_base != 0 {
            self.put(old_base, &base);
        } else {
            let at = self.alloc(base.len(), 8);
            self.put(at, &base);
            self.point(rec + 0x10, at);
        }
        // the transform bits that still hold
        let targets: Vec<u32> = curves.iter().map(|(t, _, _)| *t).collect();
        let transform = flags & TRANSFORM_BITS & transform_bits(edit, quaternion, &targets);
        let flags = (flags & !(CURVE_BITS | TRANSFORM_BITS)) | bits | base_flags | transform;
        self.put(rec + 0x28, &flags.to_le_bytes());
        // where its rotation and translation slots begin: scale 0 to 2, rotation 3 to 5 (one slot for a
        // quaternion), translation after; the base translation after scale and a 4-float rotation
        let begin_translate: u8 = if quaternion { 4 } else { 6 };
        self.put(rec + 0x2C, &[3, begin_translate, curves.len() as u8, 7]);
        Ok(curves.len())
    }

    /// Counts the edited animations' curves again (each bone's first, the animation's total and its baked size),
    /// and writes the file: its data, what was added after it, and the relocation table.
    pub fn finish(mut self, anims: &[SkeletalAnim]) -> Result<Vec<u8>> {
        let counts = std::mem::take(&mut self.counts);
        for a in anims {
            let Some(edited) = counts.get(&a.at) else {
                continue;
            };
            let added = self.moved.get(&a.at).map_or(0, |&(_, k)| k);
            let mut total = 0usize;
            for i in 0..a.bones.len() + added {
                let rec = self.record(a, i);
                self.put(rec + 0x30, &(total as i32).to_le_bytes());
                total += edited
                    .get(&i)
                    .copied()
                    .unwrap_or_else(|| a.bones.get(i).map_or(0, |b| b.curves.len()));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bfres_anim;
    use crate::testing::tiny_anim;

    fn wave(frames: usize, from: f32, to: f32) -> Vec<f32> {
        (0..frames)
            .map(|f| from + (to - from) * f as f32 / (frames - 1) as f32)
            .collect()
    }

    fn plume() -> NewTrack {
        NewTrack {
            name: "Plume".into(),
            quaternion: false,
            flags: 1 << 23,
            edit: BoneEdit {
                base_scale: [1.0; 3],
                base_rotation: [0.0, 0.0, 0.0, 1.0],
                base_translation: [0.2, 0.0, 0.0],
                curves: vec![CurveEdit::Scalar {
                    target: target::ROTATE_X,
                    values: wave(11, 0.0, 1.0),
                }],
            },
        }
    }

    fn arm_turning() -> BoneEdit {
        BoneEdit {
            base_scale: [1.0; 3],
            base_rotation: [0.0, 0.0, 0.5, 1.0],
            base_translation: [1.0, 0.0, 0.0],
            curves: vec![CurveEdit::Scalar {
                target: target::ROTATE_Z,
                values: wave(11, 0.5, -0.5),
            }],
        }
    }

    /// The tiny animation with `Arm` turning and a track for `Plume` added, the edit before the new track or after.
    fn written(edit_first: bool) -> Vec<u8> {
        let data = tiny_anim();
        let file = bfres_anim::parse(&data, None).unwrap();
        let anim = &file.anims[0];
        let mut w = AnimWriter::new(&data).unwrap();
        if edit_first {
            w.edit_bone(&data, anim, 1, &arm_turning()).unwrap();
            w.add_bones(&data, anim, &[plume()]).unwrap();
        } else {
            w.add_bones(&data, anim, &[plume()]).unwrap();
            w.edit_bone(&data, anim, 1, &arm_turning()).unwrap();
        }
        w.finish(&file.anims).unwrap()
    }

    #[test]
    fn a_track_is_added_for_a_new_bone() {
        let original = bfres_anim::parse(&tiny_anim(), None).unwrap();
        let root = &original.anims[0].bones[0];
        for edit_first in [true, false] {
            let out = written(edit_first);
            check_relocations(&out).unwrap();
            let back = bfres_anim::parse(&out, None).unwrap();
            let a = &back.anims[0];
            let names: Vec<&str> = a.bones.iter().map(|b| b.name.as_str()).collect();
            assert_eq!(names, ["Root", "Arm", "Plume"]);
            assert_eq!(a.bind_indices, [0xFFFF; 3]);
            assert_eq!(a.curve_count, 2);
            assert_eq!(a.baked_size, baked_size(2, 10));
            assert_eq!(
                a.bones.iter().map(|b| b.begin_curve).collect::<Vec<_>>(),
                [0, 0, 1]
            );
            // Root as it was
            assert_eq!(a.bones[0].flags, root.flags);
            assert_eq!(a.bones[0].base_rotation, root.base_rotation);
            assert!(a.bones[0].curves.is_empty());
            // Arm turning, Plume nodding, each to within a step of its 16-bit values
            let arm = a.bones[1].curve(target::ROTATE_Z).unwrap();
            let p = &a.bones[2];
            let nod = p.curve(target::ROTATE_X).unwrap();
            for f in 0..=10 {
                let t = f as f32 / 10.0;
                assert!((arm.value_at(f as f32) - (0.5 - t)).abs() < 1e-4);
                assert!((nod.value_at(f as f32) - t).abs() < 1e-4);
            }
            assert_eq!(p.base_translation, Some([0.2, 0.0, 0.0]));
            assert_eq!(p.base_rotation, Some([0.0, 0.0, 0.0, 1.0]));
            assert!(!p.quaternion());
            // bit 23 as given; base values and the X angle curve; scale uniform and one, but it turns and isn't at
            // its parent's origin
            assert_eq!(p.flags, 1 << 23 | 0x38 | 1 << 9 | 0x0300_0000);
            assert_eq!(
                (p.begin_rotate, p.begin_translate, p.begin_base_translate),
                (3, 6, 7)
            );
            // written again with nothing changed, it stays the same
            let again = AnimWriter::new(&out).unwrap().finish(&back.anims).unwrap();
            assert_eq!(
                bfres_anim::parse(&again, None).unwrap().anims[0]
                    .bones
                    .len(),
                3
            );
        }
    }

    #[test]
    fn an_edit_drops_the_transform_bits_that_no_longer_hold() {
        let data = tiny_anim();
        let file = bfres_anim::parse(&data, None).unwrap();
        let anim = &file.anims[0];
        // Root: no rotation, no translation; now it turns about y
        assert_eq!(anim.bones[0].flags & TRANSFORM_BITS, 0x0F00_0000);
        let mut w = AnimWriter::new(&data).unwrap();
        let turn = BoneEdit {
            base_scale: [1.0; 3],
            base_rotation: [0.0, 0.0, 0.0, 1.0],
            base_translation: [0.0; 3],
            curves: vec![CurveEdit::Scalar {
                target: target::ROTATE_Y,
                values: wave(11, 0.0, 0.3),
            }],
        };
        w.edit_bone(&data, anim, 0, &turn).unwrap();
        let out = w.finish(&file.anims).unwrap();
        let root = &bfres_anim::parse(&out, None).unwrap().anims[0].bones[0];
        assert_eq!(root.flags & TRANSFORM_BITS, 0x0B00_0000);
        assert_eq!(root.flags & CURVE_BITS, 1 << 10);
    }

    #[test]
    fn a_track_isnt_added_twice() {
        let data = tiny_anim();
        let file = bfres_anim::parse(&data, None).unwrap();
        let anim = &file.anims[0];
        let mut w = AnimWriter::new(&data).unwrap();
        let mut arm = plume();
        arm.name = "Arm".into();
        assert!(w.add_bones(&data, anim, &[arm]).is_err());
        assert!(w.add_bones(&data, anim, &[plume(), plume()]).is_err());
        w.add_bones(&data, anim, &[plume()]).unwrap();
        let mut other = plume();
        other.name = "Other".into();
        assert!(w.add_bones(&data, anim, &[other]).is_err());
    }

    #[test]
    fn transform_bits_from_base_values_and_curves() {
        let still = BoneEdit {
            base_scale: [1.0; 3],
            base_rotation: [0.0, 0.0, 0.0, 1.0],
            base_translation: [0.0; 3],
            curves: vec![],
        };
        assert_eq!(transform_bits(&still, false, &[]), 0x0F00_0000);
        assert_eq!(transform_bits(&still, true, &[]), 0x0F00_0000);
        // a quaternion of w 0 is a half turn
        let mut half = still.clone();
        half.base_rotation = [0.0, 0.0, 0.0, 0.0];
        assert_eq!(transform_bits(&half, true, &[]), 0x0B00_0000);
        assert_eq!(transform_bits(&half, false, &[]), 0x0F00_0000);
        // stretched: uniform but not one; curves clear their part
        let mut big = still.clone();
        big.base_scale = [2.0; 3];
        assert_eq!(transform_bits(&big, false, &[]), 0x0D00_0000);
        assert_eq!(
            transform_bits(&still, false, &[target::SCALE_Y]),
            0x0C00_0000
        );
        assert_eq!(
            transform_bits(&still, false, &[target::TRANSLATE_Z]),
            0x0700_0000
        );
    }
}
