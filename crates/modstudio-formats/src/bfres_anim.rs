//! Skeletal animations (FSKA) in BFRES 10 files (`Model/*.anim.bfres.zs`), read.
//!
//! Ported from Syroot.NintenTools.Bfres as maintained in LegacySwitchLibraries (MIT): `ResFile.cs` (the header's
//! animation array), `SkeletalAnim/SkeletalAnim.cs`, `SkeletalAnim/BoneAnim.cs`, `SkeletalAnim/BoneAnimData.cs` and
//! `Common/AnimCurve.cs` (both key layouts: the older one, keys scaled by the curve's scale and offset, and the
//! "value keys" one, flag 0x4000, values quantised per curve).
//!
//! An animation has, per bone it moves (by name), base values (scale, rotation, translation, each only if the
//! bone's flags say so) and curves, each moving one component (its target: the offset of the component in the
//! bone's data, [`target`]). A component without a curve keeps its base value; one without either keeps the
//! skeleton's. [`Curve::value_at`] evaluates a curve at a frame.
//!
//! Rotations are Euler angles (XYZ, radians; one curve per angle) unless the bone's flags have
//! [`bone_flags::QUATERNION`] (found in this game's files, not in the library): then the base rotation is a
//! quaternion (x, y, z, w) and one curve, on [`target::ROTATE_X`] with [`curve_flags::QUATERNION`], holds a whole
//! quaternion per key (four f32, no slopes; [`Curve::quat_at`] blends them).

use crate::bfres::{ExternalStrings, Header, Reader, flags};
use crate::bytes::{ReadLe, check_magic};
use crate::error::{Result, invalid, unsupported};

const ANIM_SIZE: usize = 0x50;
const BONE_ANIM_SIZE: usize = 0x38;
const CURVE_SIZE: usize = 0x30;

/// The offsets curves target in a bone's data (`BoneAnimDataOffset`).
pub mod target {
    pub const SCALE_X: u32 = 0x04;
    pub const SCALE_Y: u32 = 0x08;
    pub const SCALE_Z: u32 = 0x0C;
    pub const TRANSLATE_X: u32 = 0x10;
    pub const TRANSLATE_Y: u32 = 0x14;
    pub const TRANSLATE_Z: u32 = 0x18;
    pub const ROTATE_X: u32 = 0x20;
    pub const ROTATE_Y: u32 = 0x24;
    pub const ROTATE_Z: u32 = 0x28;
    pub const ROTATE_W: u32 = 0x2C;
}

/// An animation's flags (`SkeletalAnimFlags`, the scale and rotation modes).
pub mod anim_flags {
    pub const BAKED: u32 = 1 << 0;
    pub const LOOPING: u32 = 1 << 2;
    pub const SCALE_MASK: u32 = 3 << 8;
    /// Rotations are Euler angles (x, y, z, radians) instead of quaternions.
    pub const EULER: u32 = 1 << 12;
}

/// A bone's flags: which base values it has (`BoneAnimFlagsBase`).
pub mod bone_flags {
    pub const BASE_SCALE: u32 = 1 << 3;
    pub const BASE_ROTATE: u32 = 1 << 4;
    pub const BASE_TRANSLATE: u32 = 1 << 5;
    /// The bone's rotation is a quaternion (its base, and its one rotation curve's keys); else Euler angles.
    pub const QUATERNION: u32 = 1 << 20;
}

/// A curve's flags (`AnimCurve`).
pub mod curve_flags {
    pub const FRAME_TYPE: u16 = 0b11;
    pub const KEY_TYPE: u16 = 0b1100;
    pub const CURVE_TYPE: u16 = 0b111_0000;
    pub const FRAME_SCALED: u16 = 0x0400;
    pub const SLOPE_QUANTISED: u16 = 0x0800;
    pub const VALUE_KEYS: u16 = 0x4000;
    /// Each key is a quaternion (four f32), blended without slopes.
    pub const QUATERNION: u16 = 0x0080;

    pub const CUBIC: u16 = 0 << 4;
    pub const LINEAR: u16 = 1 << 4;
    pub const BAKED_FLOAT: u16 = 2 << 4;
    pub const STEP_FLOAT: u16 = 3 << 4;
    pub const STEP_INT: u16 = 4 << 4;
    pub const BAKED_INT: u16 = 5 << 4;
    pub const STEP_BOOL: u16 = 6 << 4;
    pub const BAKED_BOOL: u16 = 7 << 4;
}

/// The animations of an animation file.
#[derive(Debug, Clone)]
pub struct AnimFile {
    pub name: String,
    pub anims: Vec<SkeletalAnim>,
}

/// One skeletal animation.
#[derive(Debug, Clone)]
pub struct SkeletalAnim {
    pub at: usize,
    pub name: String,
    pub path: String,
    pub flags: u32,
    pub frame_count: i32,
    pub curve_count: i32,
    pub baked_size: u32,
    /// Per bone animation, the skeleton bone it's bound to (0xFFFF: not bound in the file).
    pub bind_indices: Vec<u16>,
    pub bones: Vec<BoneAnim>,
}

impl SkeletalAnim {
    pub fn looping(&self) -> bool {
        self.flags & anim_flags::LOOPING != 0
    }
    pub fn euler(&self) -> bool {
        self.flags & anim_flags::EULER != 0
    }
}

/// What an animation does to one bone.
#[derive(Debug, Clone)]
pub struct BoneAnim {
    pub at: usize,
    pub name: String,
    pub flags: u32,
    pub begin_rotate: u8,
    pub begin_translate: u8,
    pub begin_base_translate: u8,
    pub begin_curve: i32,
    pub base_scale: Option<[f32; 3]>,
    /// A quaternion (x, y, z, w), or Euler angles (x, y, z, unused) when the animation says so.
    pub base_rotation: Option<[f32; 4]>,
    pub base_translation: Option<[f32; 3]>,
    pub curves: Vec<Curve>,
}

impl BoneAnim {
    /// The curve moving `target`, if one does.
    /// Whether the bone's rotation is a quaternion ([`bone_flags::QUATERNION`]).
    pub fn quaternion(&self) -> bool {
        self.flags & bone_flags::QUATERNION != 0
    }

    pub fn curve(&self, target: u32) -> Option<&Curve> {
        self.curves.iter().find(|c| c.target == target)
    }
}

/// A curve: one component's value over the frames.
#[derive(Debug, Clone)]
pub struct Curve {
    pub at: usize,
    pub flags: u16,
    /// The component it moves ([`target`]).
    pub target: u32,
    pub start_frame: f32,
    pub end_frame: f32,
    pub scale: f32,
    pub offset: f32,
    pub delta: f32,
    /// The frame of each key.
    pub frames: Vec<f32>,
    /// From each key to the next: the coefficients a, b, c, d of `a + b t + c t^2 + d t^3`, `t` going from 0 at the
    /// key to 1 at the next (a step keeps `a`; the last key's `a` holds after it).
    pub segments: Vec<[f32; 4]>,
    /// A quaternion curve's keys ([`curve_flags::QUATERNION`]; its `segments` are empty).
    pub quaternions: Vec<[f32; 4]>,
}

impl Curve {
    pub fn curve_type(&self) -> u16 {
        self.flags & curve_flags::CURVE_TYPE
    }
    /// A quaternion curve's rotation at `frame` ([`curve_flags::QUATERNION`]): the keys on either side blended
    /// (slerp, the short way), the first before the first key and the last after the last; `None` for a curve of
    /// single values.
    pub fn quat_at(&self, frame: f32) -> Option<[f32; 4]> {
        let n = self.frames.len().min(self.quaternions.len());
        if n == 0 {
            return None;
        }
        if frame <= self.frames[0] {
            return Some(self.quaternions[0]);
        }
        let i = match self.frames[..n].binary_search_by(|f| f.total_cmp(&frame)) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        if i + 1 >= n {
            return Some(self.quaternions[n - 1]);
        }
        let span = self.frames[i + 1] - self.frames[i];
        let t = if span > 0.0 {
            (frame - self.frames[i]) / span
        } else {
            0.0
        };
        Some(slerp(self.quaternions[i], self.quaternions[i + 1], t))
    }

    pub fn value_keys(&self) -> bool {
        self.flags & curve_flags::VALUE_KEYS != 0
    }

    /// The value at `frame` (before the first key: the first key's; after the last: the last's).
    pub fn value_at(&self, frame: f32) -> f32 {
        let n = self.frames.len().min(self.segments.len());
        if n == 0 {
            return 0.0;
        }
        if frame <= self.frames[0] {
            return self.segments[0][0];
        }
        // the segment whose key is the last at or before `frame`
        let i = match self.frames[..n].binary_search_by(|f| f.total_cmp(&frame)) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let [a, b, c, d] = self.segments[i];
        if i + 1 >= n {
            return a;
        }
        let ct = self.curve_type();
        if matches!(
            ct,
            curve_flags::STEP_FLOAT | curve_flags::STEP_INT | curve_flags::STEP_BOOL
        ) {
            return a;
        }
        let span = self.frames[i + 1] - self.frames[i];
        let t = if span > 0.0 {
            (frame - self.frames[i]) / span
        } else {
            0.0
        };
        a + t * (b + t * (c + t * d))
    }
}

/// Reads the skeletal animations of a decompressed `.anim.bfres` file.
pub fn parse(data: &[u8], external: Option<&ExternalStrings>) -> Result<AnimFile> {
    let header = Header::parse(data)?;
    if header.external_flags & flags::HOLDS_EXTERNAL_STRINGS != 0 {
        return Err(invalid(
            "this is the external string table, not an animation",
        ));
    }
    let r = Reader {
        data,
        external,
        uses_external: header.external_flags & flags::HAS_EXTERNAL_STRING != 0,
    };
    let name = r.string(0x20)?;
    let array = data.offset_at(0x58)?;
    let count = data.u16_at(0xE2)? as usize;
    let anims = (0..count)
        .map(|i| parse_anim(&r, array + i * ANIM_SIZE))
        .collect::<Result<Vec<_>>>()?;
    Ok(AnimFile { name, anims })
}

/// An animation's name and length, without its curves ([`list`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnimInfo {
    pub name: String,
    pub frame_count: i32,
    pub looping: bool,
    pub bones: u16,
}

/// The animations of a decompressed `.anim.bfres` file, by name and length (their curves aren't read).
pub fn list(data: &[u8], external: Option<&ExternalStrings>) -> Result<Vec<AnimInfo>> {
    let header = Header::parse(data)?;
    let r = Reader {
        data,
        external,
        uses_external: header.external_flags & flags::HAS_EXTERNAL_STRING != 0,
    };
    let array = data.offset_at(0x58)?;
    let count = data.u16_at(0xE2)? as usize;
    (0..count)
        .map(|i| {
            let at = array + i * ANIM_SIZE;
            check_magic(data, at, "FSKA")?;
            Ok(AnimInfo {
                name: r.string(at + 0x08)?,
                frame_count: data.i32_at(at + 0x40)?,
                looping: data.u32_at(at + 0x04)? & anim_flags::LOOPING != 0,
                bones: data.u16_at(at + 0x4C)?,
            })
        })
        .collect()
}

/// The animation named `name` of a decompressed `.anim.bfres` file, curves and all.
pub fn find(data: &[u8], external: Option<&ExternalStrings>, name: &str) -> Result<SkeletalAnim> {
    let header = Header::parse(data)?;
    let r = Reader {
        data,
        external,
        uses_external: header.external_flags & flags::HAS_EXTERNAL_STRING != 0,
    };
    let array = data.offset_at(0x58)?;
    let count = data.u16_at(0xE2)? as usize;
    for i in 0..count {
        let at = array + i * ANIM_SIZE;
        if r.string(at + 0x08)? == name {
            return parse_anim(&r, at);
        }
    }
    Err(invalid(format!("no animation named {name}")))
}

fn parse_anim(r: &Reader, at: usize) -> Result<SkeletalAnim> {
    let d = r.data;
    check_magic(d, at, "FSKA")?;
    let flags = d.u32_at(at + 0x04)?;
    let name = r.string(at + 0x08)?;
    let path = r.string(at + 0x10)?;
    let bind_at = d.offset_at(at + 0x20)?;
    let bones_at = d.offset_at(at + 0x28)?;
    let frame_count = d.i32_at(at + 0x40)?;
    let curve_count = d.i32_at(at + 0x44)?;
    let baked_size = d.u32_at(at + 0x48)?;
    let bone_count = d.u16_at(at + 0x4C)? as usize;
    let bind_indices = if bind_at == 0 {
        Vec::new()
    } else {
        (0..bone_count)
            .map(|i| d.u16_at(bind_at + i * 2))
            .collect::<Result<Vec<_>>>()?
    };
    let bones = (0..bone_count)
        .map(|i| parse_bone(r, bones_at + i * BONE_ANIM_SIZE))
        .collect::<Result<Vec<_>>>()
        .map_err(|e| invalid(format!("animation {name}: {e}")))?;
    Ok(SkeletalAnim {
        at,
        name,
        path,
        flags,
        frame_count,
        curve_count,
        baked_size,
        bind_indices,
        bones,
    })
}

fn parse_bone(r: &Reader, at: usize) -> Result<BoneAnim> {
    let d = r.data;
    let name = r.string(at)?;
    let curves_at = d.offset_at(at + 0x08)?;
    let base_at = d.offset_at(at + 0x10)?;
    let flags = d.u32_at(at + 0x28)?;
    let curve_count = d.u8_at(at + 0x2E)? as usize;
    let f32s = |at: usize, n: usize| -> Result<Vec<f32>> {
        (0..n).map(|i| d.f32_at(at + i * 4)).collect()
    };
    let mut p = base_at;
    let mut base_scale = None;
    let mut base_rotation = None;
    let mut base_translation = None;
    if base_at != 0 {
        if flags & bone_flags::BASE_SCALE != 0 {
            let v = f32s(p, 3)?;
            base_scale = Some([v[0], v[1], v[2]]);
            p += 12;
        }
        if flags & bone_flags::BASE_ROTATE != 0 {
            let v = f32s(p, 4)?;
            base_rotation = Some([v[0], v[1], v[2], v[3]]);
            p += 16;
        }
        if flags & bone_flags::BASE_TRANSLATE != 0 {
            let v = f32s(p, 3)?;
            base_translation = Some([v[0], v[1], v[2]]);
        }
    }
    let curves = (0..curve_count)
        .map(|i| parse_curve(d, curves_at + i * CURVE_SIZE))
        .collect::<Result<Vec<_>>>()
        .map_err(|e| invalid(format!("bone {name}: {e}")))?;
    Ok(BoneAnim {
        at,
        name,
        flags,
        begin_rotate: d.u8_at(at + 0x2C)?,
        begin_translate: d.u8_at(at + 0x2D)?,
        begin_base_translate: d.u8_at(at + 0x2F)?,
        begin_curve: d.i32_at(at + 0x30)?,
        base_scale,
        base_rotation,
        base_translation,
        curves,
    })
}

fn parse_curve(d: &[u8], at: usize) -> Result<Curve> {
    let frames_at = d.offset_at(at)?;
    let keys_at = d.offset_at(at + 0x08)?;
    let flags = d.u16_at(at + 0x10)?;
    let n = d.u16_at(at + 0x12)? as usize;
    let mut curve = Curve {
        at,
        flags,
        target: d.u32_at(at + 0x14)?,
        start_frame: d.f32_at(at + 0x18)?,
        end_frame: d.f32_at(at + 0x1C)?,
        scale: d.f32_at(at + 0x20)?,
        offset: d.f32_at(at + 0x24)?,
        delta: d.f32_at(at + 0x28)?,
        frames: Vec::new(),
        segments: Vec::new(),
        quaternions: Vec::new(),
    };
    let value_keys = flags & curve_flags::VALUE_KEYS != 0;
    let frame_type = flags & curve_flags::FRAME_TYPE;
    let key_type = (flags & curve_flags::KEY_TYPE) >> 2;
    let curve_type = flags & curve_flags::CURVE_TYPE;
    let frame_scaled = flags & curve_flags::FRAME_SCALED != 0;
    // frames
    curve.frames = (0..n)
        .map(|i| -> Result<f32> {
            Ok(match frame_type {
                0 => d.f32_at(frames_at + i * 4)?,
                1 if value_keys && frame_scaled => {
                    d.u16_at(frames_at + i * 2)? as f32 * curve.scale
                }
                1 => d.i16_at(frames_at + i * 2)? as f32 / 32.0,
                _ if value_keys && frame_scaled => d.u8_at(frames_at + i)? as f32 * curve.scale,
                _ => d.u8_at(frames_at + i)? as f32,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    if flags & curve_flags::QUATERNION != 0 {
        if key_type != 0 {
            return Err(unsupported(format!(
                "a quaternion curve with key type {key_type}"
            )));
        }
        curve.quaternions = (0..n)
            .map(|i| -> Result<[f32; 4]> {
                let at = keys_at + i * 16;
                Ok([
                    d.f32_at(at)?,
                    d.f32_at(at + 4)?,
                    d.f32_at(at + 8)?,
                    d.f32_at(at + 12)?,
                ])
            })
            .collect::<Result<Vec<_>>>()?;
        return Ok(curve);
    }
    if matches!(curve_type, curve_flags::STEP_BOOL | curve_flags::BAKED_BOOL) {
        curve.segments = (0..n)
            .map(|i| -> Result<[f32; 4]> {
                let word = d.u32_at(keys_at + (i >> 5) * 4)?;
                Ok([((word >> (i & 31)) & 1) as f32, 0.0, 0.0, 0.0])
            })
            .collect::<Result<Vec<_>>>()?;
        return Ok(curve);
    }
    let is_int = matches!(curve_type, curve_flags::STEP_INT | curve_flags::BAKED_INT);
    // one stored number: f32 (or i32 for integer curves), i16 or i8; unsigned for value-keyed Hermite curves
    let number = |at: usize, unsigned: bool| -> Result<(f32, usize)> {
        Ok(match key_type {
            0 if is_int => (d.i32_at(at)? as f32, 4),
            0 => (d.f32_at(at)?, 4),
            1 if unsigned => (d.u16_at(at)? as f32, 2),
            1 => (d.i16_at(at)? as f32, 2),
            2 if unsigned => (d.u8_at(at)? as f32, 1),
            2 => (d.u8_at(at)? as i8 as f32, 1),
            k => return Err(unsupported(format!("curve key type {k}"))),
        })
    };
    let quantised = key_type != 0;
    if value_keys {
        let (value_scale, value_offset) = (curve.offset, curve.delta);
        let hermite = curve_type == curve_flags::CUBIC;
        let baked = curve_type == curve_flags::BAKED_FLOAT;
        let mut p = keys_at;
        let (mut slope_scale, mut slope_offset) = (0.0, 0.0);
        if hermite && quantised {
            slope_scale = d.f32_at(p)?;
            slope_offset = d.f32_at(p + 4)?;
            p += 8;
        } else if baked {
            slope_scale = value_scale;
        }
        let read = |p: &mut usize, scale: f32, offset: f32| -> Result<f32> {
            let (v, size) = number(*p, hermite)?;
            *p += size;
            Ok(if quantised { v * scale + offset } else { v })
        };
        let mut values = Vec::with_capacity(n);
        let mut slopes = Vec::with_capacity(n);
        for _ in 0..n {
            values.push(read(&mut p, value_scale, value_offset)?);
            if hermite || baked {
                slopes.push(read(&mut p, slope_scale, slope_offset)?);
            }
        }
        curve.segments = (0..n)
            .map(|i| {
                let p0 = values[i];
                let Some(&p1) = values.get(i + 1) else {
                    return [p0, 0.0, 0.0, 0.0];
                };
                if hermite {
                    let dt = curve.frames[i + 1] - curve.frames[i];
                    let (m0, m1) = (slopes[i] * dt, slopes[i + 1] * dt);
                    [
                        p0,
                        m0,
                        3.0 * (p1 - p0) - 2.0 * m0 - m1,
                        2.0 * (p0 - p1) + m0 + m1,
                    ]
                } else if is_int {
                    [p0.round(), 0.0, 0.0, 0.0]
                } else if curve_type == curve_flags::STEP_FLOAT {
                    [p0, 0.0, 0.0, 0.0]
                } else {
                    [p0, p1 - p0, 0.0, 0.0]
                }
            })
            .collect();
        return Ok(curve);
    }
    // the older layout: elements per key by curve type, each scaled (the first also offset)
    let per_key = match curve_type {
        curve_flags::CUBIC => 4,
        curve_flags::LINEAR | curve_flags::BAKED_FLOAT => 2,
        _ => 1,
    };
    let mut p = keys_at;
    curve.segments = (0..n)
        .map(|_| -> Result<[f32; 4]> {
            let mut k = [0.0f32; 4];
            for (j, slot) in k.iter_mut().enumerate().take(per_key) {
                let (v, size) = number(p, false)?;
                p += size;
                let scaled = if quantised || !is_int {
                    v * curve.scale
                } else {
                    v
                };
                *slot = if j == 0 {
                    scaled + curve.offset
                } else {
                    scaled
                };
            }
            if curve_type == curve_flags::BAKED_FLOAT {
                // a value and its slope: the value holds to the next key, which is a frame on
                k[1] = 0.0;
            }
            Ok(k)
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(curve)
}

/// Spherical blend of two quaternions (x, y, z, w) the short way, normalised.
fn slerp(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let mut dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    let b = if dot < 0.0 {
        dot = -dot;
        [-b[0], -b[1], -b[2], -b[3]]
    } else {
        b
    };
    let (wa, wb) = if dot > 0.9995 {
        (1.0 - t, t)
    } else {
        let theta = dot.min(1.0).acos();
        let s = theta.sin();
        (((1.0 - t) * theta).sin() / s, (t * theta).sin() / s)
    };
    let q: [f32; 4] = std::array::from_fn(|i| a[i] * wa + b[i] * wb);
    let len = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if len > 0.0 { q.map(|v| v / len) } else { a }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quaternion_curve_blends_its_keys() {
        let half = std::f32::consts::FRAC_1_SQRT_2;
        let c = Curve {
            at: 0,
            flags: curve_flags::VALUE_KEYS | curve_flags::QUATERNION,
            target: target::ROTATE_X,
            start_frame: 0.0,
            end_frame: 10.0,
            scale: 0.0,
            offset: 0.0,
            delta: 0.0,
            frames: vec![0.0, 10.0],
            segments: vec![],
            // none, then a half turn about z
            quaternions: vec![[0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 1.0, 0.0]],
        };
        let near = |a: [f32; 4], b: [f32; 4]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5);
        assert!(near(c.quat_at(-1.0).unwrap(), [0.0, 0.0, 0.0, 1.0]));
        // halfway: a quarter turn
        assert!(
            near(c.quat_at(5.0).unwrap(), [0.0, 0.0, half, half]),
            "{:?}",
            c.quat_at(5.0)
        );
        assert!(near(c.quat_at(20.0).unwrap(), [0.0, 0.0, 1.0, 0.0]));
        // the short way round when the keys are on opposite sides
        assert!(near(
            slerp([0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 0.0, -1.0], 0.5),
            [0.0, 0.0, 0.0, 1.0]
        ));
    }
}
