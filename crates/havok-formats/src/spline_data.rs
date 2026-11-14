//! The data of a spline compressed animation, block by block, as it is stored: masks, knot bytes, bounds, quantized
//! control points and packed quaternions, kept exactly ([`SplineBlocks::read`]) and laid out again byte for byte
//! ([`SplineBlocks::write`]). [`crate::spline::SplineAnimation`] decodes these values; the encoder
//! ([`crate::spline::encode`]) produces them.
//!
//! The layout is the one [`crate::spline`] describes. What the decoded values do not keep and these do: the
//! quantization each track's mask selects (also for a rotation it does not store), the whole flag byte of each part
//! (a spline rotation's mask also flags its static components), the bounds of a spline component (its largest control
//! point need not be the top of the scale), the quantized integers themselves, the bytes of a packed quaternion, and
//! whether the data ends with the 16 zero bytes the Elden Ring files have after some last blocks.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use crate::animation::{Animation, Tracks};
use crate::{Error, Result, invalid};
use primase_foundations::bytes::Endian;

/// The knots and degree a spline track's control points are evaluated with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Knots {
    /// The degree of the spline (the stored byte).
    pub degree: u8,
    /// Frames within the block, `points + degree + 1` of them, not decreasing.
    pub values: Vec<u8>,
}

/// How a rotation is stored (`RotationQuantization`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RotationQuantization {
    /// Code 1: 40 bits a quaternion, byte aligned.
    ThreeComp40,
    /// Code 2: 48 bits a quaternion, 2-byte aligned.
    ThreeComp48,
    /// Code 5: four f32, 4-byte aligned.
    Uncompressed,
}

impl RotationQuantization {
    pub(crate) fn of(code: u8, at: usize) -> Result<RotationQuantization> {
        Ok(match code {
            1 => RotationQuantization::ThreeComp40,
            2 => RotationQuantization::ThreeComp48,
            5 => RotationQuantization::Uncompressed,
            // No transform track of the 60,209 spline animations of Sekiro's and Elden Ring's binders, animation and
            // cutscene (5,526,279 tracks, absent rotations included), selects code 0, 3 or 4, so no real stream shows
            // how they are stored. The cutscene tracks select code 1 only in Sekiro (17,014 tracks) and code 5 only in
            // Elden Ring (267,433).
            0 => return Err(invalid(at, "rotation quantization POLAR32 is not read")),
            3 => return Err(invalid(at, "rotation quantization THREECOMP24 is not read")),
            4 => return Err(invalid(at, "rotation quantization STRAIGHT16 is not read")),
            other => return Err(invalid(at, format!("rotation quantization {other} is unknown"))),
        })
    }

    /// The code a track's mask stores for it (bits 2-5 of the quantization byte).
    pub fn code(self) -> u8 {
        match self {
            RotationQuantization::ThreeComp40 => 1,
            RotationQuantization::ThreeComp48 => 2,
            RotationQuantization::Uncompressed => 5,
        }
    }

    /// The alignment of a quantized quaternion within the data.
    pub(crate) fn align(self) -> usize {
        match self {
            RotationQuantization::Uncompressed => 4,
            RotationQuantization::ThreeComp40 => 1,
            RotationQuantization::ThreeComp48 => 2,
        }
    }
}

/// How the control points of a position or scale spline are stored: 8 or 16 bits each, between the component's bounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScalarQuantization {
    /// Code 0: a byte, the bounds' span divided into 255 steps.
    Bits8,
    /// Code 1: two bytes, the span divided into 65535 steps.
    Bits16,
}

impl ScalarQuantization {
    pub(crate) fn of(code: u8, at: usize) -> Result<ScalarQuantization> {
        match code {
            0 => Ok(ScalarQuantization::Bits8),
            1 => Ok(ScalarQuantization::Bits16),
            other => Err(invalid(at, format!("scalar quantization {other} is unknown"))),
        }
    }

    /// The code a track's mask stores for it (bits 0-1 for the position, 6-7 for the scale).
    pub fn code(self) -> u8 {
        match self {
            ScalarQuantization::Bits8 => 0,
            ScalarQuantization::Bits16 => 1,
        }
    }

    /// The largest stored integer: 255 or 65535.
    pub fn top(self) -> u16 {
        match self {
            ScalarQuantization::Bits8 => 255,
            ScalarQuantization::Bits16 => 65535,
        }
    }

    /// The value a stored integer stands for: `min + (max - min) * value / top`, computed in f64 and rounded once.
    pub fn value(self, min: f32, max: f32, value: u16) -> f32 {
        let ratio = f64::from(value) / f64::from(self.top());
        round_f32(f64::from(min) + (f64::from(max) - f64::from(min)) * ratio)
    }
}

/// `v` rounded to the nearest f32: a value computed in f64 rounded once.
#[allow(
    clippy::cast_possible_truncation,
    reason = "rounding to f32 is the stated conversion"
)]
pub(crate) fn round_f32(v: f64) -> f32 {
    v as f32
}

/// One component (x, y or z) of a stored position or scale.
#[derive(Clone, Debug, PartialEq)]
pub enum ScalarData {
    /// Neither flag: nothing stored.
    Absent,
    /// The static flag: one f32 (kept as stored, NaN included).
    Static(f32),
    /// The spline flag: the bounds (as stored) and a quantized value for each control point.
    Spline {
        /// The lower bound, the value of integer 0.
        min: f32,
        /// The upper bound, the value of the top integer.
        max: f32,
        /// One integer a control point (below 256 when the vector's quantization is 8-bit).
        values: Vec<u16>,
    },
}

/// A stored position or scale.
#[derive(Clone, Debug, PartialEq)]
pub struct VectorData {
    /// The mask's flag byte: static components in the low nibble (x y z w), spline components in the high nibble.
    pub flags: u8,
    /// The quantization the mask selects for the spline components (stored even when there are none).
    pub quantization: ScalarQuantization,
    /// The knots, when a component of x, y and z is a spline.
    pub knots: Option<Knots>,
    /// x, y and z.
    pub components: [ScalarData; 3],
}

/// A quaternion as one of the rotation quantizations stores it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PackedQuaternion {
    /// THREECOMP40: five bytes, three 12-bit components, the index of the largest and its sign.
    ThreeComp40([u8; 5]),
    /// THREECOMP48: three little-endian 16-bit words, 15 bits a component, the index and sign in the top bits.
    ThreeComp48([u16; 3]),
    /// Four f32 `x y z w`, kept as stored (NaN included).
    Uncompressed([f32; 4]),
}

/// The step of a THREECOMP48 component: the f32 nearest `1 / (16383 sqrt 2)`, so the 15-bit range spans
/// `+-1/sqrt 2`, the largest a quaternion's three smallest components can be.
pub(crate) const THREE_COMP48_STEP: f32 = 4.316_100_6e-5;
/// The step of a THREECOMP40 component: the f32 nearest `1 / (2047 sqrt 2)`, so the 12-bit range spans `+-1/sqrt 2`.
pub(crate) const THREE_COMP40_STEP: f32 = 3.454_356_6e-4;

/// A quaternion stored as three components and the index and sign of the fourth, which is what makes it a unit.
fn three_components(c: [f32; 3], shift: usize, negative: bool) -> [f32; 4] {
    let rest = 1.0 - (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]);
    let mut largest = if rest <= 0.0 { 0.0 } else { rest.sqrt() };
    if negative {
        largest = -largest;
    }
    let mut out = [0.0; 4];
    let mut next = c.into_iter();
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = if i == shift {
            largest
        } else {
            next.next().unwrap_or_default()
        };
    }
    out
}

impl PackedQuaternion {
    /// The quantization that stores this form.
    pub fn quantization(&self) -> RotationQuantization {
        match self {
            PackedQuaternion::ThreeComp40(_) => RotationQuantization::ThreeComp40,
            PackedQuaternion::ThreeComp48(_) => RotationQuantization::ThreeComp48,
            PackedQuaternion::Uncompressed(_) => RotationQuantization::Uncompressed,
        }
    }

    /// The quaternion `x y z w` it stands for.
    pub fn unpack(&self) -> [f32; 4] {
        match *self {
            PackedQuaternion::Uncompressed(q) => q,
            PackedQuaternion::ThreeComp48([x, y, z]) => {
                // 15 bits each; the index of the largest is bit 15 of y (high) and of x (low), its sign bit 15 of z
                let shift = usize::from(y >> 15) << 1 | usize::from(x >> 15);
                // a 15-bit value less 16383 is exact in f32
                let value = |v: u16| (f32::from(v & 0x7FFF) - 16383.0) * THREE_COMP48_STEP;
                three_components([value(x), value(y), value(z)], shift, z >> 15 != 0)
            }
            PackedQuaternion::ThreeComp40([b0, b1, b2, b3, b4]) => {
                let c = u64::from_le_bytes([b0, b1, b2, b3, b4, 0, 0, 0]);
                // 12 bits each, then the index in bits 36-37 and the sign in bit 38; a 12-bit value less 2047 is
                // exact in f32
                let v = |shift: u32| {
                    let bits = u16::try_from((c >> shift) & 0xFFF).unwrap_or_default();
                    (f32::from(bits) - 2047.0) * THREE_COMP40_STEP
                };
                let shift = usize::try_from((c >> 36) & 3).unwrap_or_default();
                three_components([v(0), v(12), v(24)], shift, (c >> 38) & 1 != 0)
            }
        }
    }
}

/// What a stored rotation holds.
#[derive(Clone, Debug, PartialEq)]
pub enum QuaternionData {
    /// Neither flag: nothing stored.
    Absent,
    /// A static flag (and no spline flag): one quaternion.
    Static(PackedQuaternion),
    /// A spline flag: the knots and a quaternion a control point.
    Spline {
        /// The knots.
        knots: Knots,
        /// The control points.
        points: Vec<PackedQuaternion>,
    },
}

/// A stored rotation.
#[derive(Clone, Debug, PartialEq)]
pub struct RotationData {
    /// The mask's flag byte, low nibble static and high nibble spline, a bit a quaternion component (x y z w). A spline
    /// rotation stores every component as a spline whatever its bits say; a static one stores the whole quaternion.
    pub flags: u8,
    /// The quantization the mask selects (stored even when the rotation is absent).
    pub quantization: RotationQuantization,
    /// The stored quaternions.
    pub value: QuaternionData,
}

/// One stored transform track of one block.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackData {
    /// The position.
    pub position: VectorData,
    /// The rotation.
    pub rotation: RotationData,
    /// The scale.
    pub scale: VectorData,
}

/// One stored float track of one block.
#[derive(Clone, Debug, PartialEq)]
pub enum FloatData {
    /// Mask byte `0x03`: one f32.
    Static(f32),
    /// Mask byte `0x12`: a spline of 16-bit control points between two bounds.
    Spline {
        /// The knots.
        knots: Knots,
        /// The lower bound.
        min: f32,
        /// The upper bound.
        max: f32,
        /// One integer a control point.
        values: Vec<u16>,
    },
}

/// The mask byte of a float track that is a static value: the static flag `0x01` and the 16-bit quantization `0x02`.
pub(crate) const FLOAT_STATIC: u8 = 0x03;
/// The mask byte of a float track that is a spline of 16-bit control points: the spline flag `0x10` and `0x02`.
pub(crate) const FLOAT_SPLINE_16: u8 = 0x12;

impl FloatData {
    fn mask(&self) -> u8 {
        match self {
            FloatData::Static(_) => FLOAT_STATIC,
            FloatData::Spline { .. } => FLOAT_SPLINE_16,
        }
    }
}

/// One stored block: its transform tracks and float tracks.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockData {
    /// One a transform track.
    pub tracks: Vec<TrackData>,
    /// One a float track.
    pub floats: Vec<FloatData>,
}

/// Every block of a spline compressed animation's data, as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct SplineBlocks {
    /// The blocks, in order.
    pub blocks: Vec<BlockData>,
    /// Whether the data ends with 16 zero bytes after the last block's padding. The 2018.1 encoder adds them when the
    /// last transform track's rotation is THREECOMP40 and ends (or would, when absent) less than 3 bytes before the
    /// padded end of the last block ([`SplineBlocks::guard_applies`]); no other block has them.
    pub guard: bool,
}

/// The data [`SplineBlocks::write`] lays out, and where each block and its float data begin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WrittenBlocks {
    /// `data`.
    pub data: Vec<u8>,
    /// `blockOffsets`: where each block begins, a multiple of 16.
    pub block_offsets: Vec<u32>,
    /// `floatBlockOffsets`: where each block's float data begins, counted from the block's start.
    pub float_block_offsets: Vec<u32>,
    /// `maskAndQuantizationSize`: `4 * transform tracks + float tracks`, rounded up to 4.
    pub mask_and_quantization_size: u32,
}

const STATIC: [u8; 4] = [0x01, 0x02, 0x04, 0x08];
const SPLINE: [u8; 4] = [0x10, 0x20, 0x40, 0x80];

/// The block data's cursor ([`primase_foundations::bytes::Cursor`], little-endian), with its floats and padding as methods.
struct Reader<'a>(primase_foundations::bytes::Cursor<'a>);

impl<'a> std::ops::Deref for Reader<'a> {
    type Target = primase_foundations::bytes::Cursor<'a>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for Reader<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<'a> Reader<'a> {
    /// A reader of `bytes` at `at`.
    fn at(bytes: &'a [u8], at: usize) -> Result<Self> {
        let mut c = primase_foundations::bytes::Cursor::new(bytes);
        c.seek(at)?;
        Ok(Reader(c))
    }

    /// A float that must be finite (a float track's value or range).
    fn f32(&mut self) -> Result<f32> {
        let at = self.position();
        let v = self.raw_f32()?;
        if v.is_finite() {
            Ok(v)
        } else {
            Err(invalid(at, "a stored float is not finite"))
        }
    }

    /// A float as stored, finite or not.
    fn raw_f32(&mut self) -> Result<f32> {
        Ok(self.0.f32(Endian::Little)?)
    }

    /// Skip to the next multiple of `align` (a position in the whole data).
    fn pad(&mut self, align: usize) -> Result<()> {
        let to = self.position().next_multiple_of(align);
        if to > self.bytes().len() {
            return Err(invalid(self.position(), "padding runs past the animation data"));
        }
        self.seek(to)?;
        Ok(())
    }
}

/// A track's mask as stored.
#[derive(Clone, Copy, Debug)]
struct Mask {
    position: ScalarQuantization,
    rotation: u8,
    scale: ScalarQuantization,
    position_flags: u8,
    rotation_flags: u8,
    scale_flags: u8,
}

fn mask(r: &mut Reader) -> Result<Mask> {
    let at = r.position();
    let [quantization, position, rotation, scale] = r.array::<4>()?;
    Ok(Mask {
        position: ScalarQuantization::of(quantization & 3, at)?,
        rotation: (quantization >> 2) & 0xF,
        scale: ScalarQuantization::of(quantization >> 6, at)?,
        position_flags: position,
        rotation_flags: rotation,
        scale_flags: scale,
    })
}

/// A spline track's head: the point count, the degree and the knots. The degree is 1, 2 or 3: of the 4,268,889 spline
/// tracks (positions, rotations and scales) of the 60,209 spline animations in Sekiro's and Elden Ring's binders,
/// 2,298,250 have degree 1 and 1,970,639 degree 3; none has degree 2, and none a higher one. Of the 164 float splines, 7
/// have degree 1 and 157 degree 3.
fn knots(r: &mut Reader) -> Result<(usize, Knots)> {
    let at = r.position();
    let items = r.i16(Endian::Little)?;
    let degree = r.u8()?;
    let points = usize::try_from(items)
        .ok()
        .and_then(|n| n.checked_add(1))
        .ok_or_else(|| invalid(at, "a spline track with a negative count"))?;
    if !(1..=3).contains(&degree) || points <= usize::from(degree) {
        return Err(invalid(
            at,
            format!("a spline track of degree {degree} with {points} control points"),
        ));
    }
    // points + degree + 1 knots
    let count = points
        .checked_add(usize::from(degree))
        .and_then(|n| n.checked_add(1))
        .ok_or_else(|| invalid(at, "the knot count overflows"))?;
    let values = r.take(count)?.to_vec();
    if values.windows(2).any(|w| matches!(*w, [a, b] if a > b)) {
        return Err(invalid(at, "spline knots decrease"));
    }
    Ok((points, Knots { degree, values }))
}

fn quantized(r: &mut Reader, scalar: ScalarQuantization) -> Result<u16> {
    Ok(match scalar {
        ScalarQuantization::Bits8 => u16::from(r.u8()?),
        ScalarQuantization::Bits16 => r.u16(Endian::Little)?,
    })
}

/// What a vector's flags say of component `i` (0, 1, 2: x, y, z): a spline, static or nothing. A spline flag wins.
#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Absent,
    Static,
    Spline,
}

fn state(flags: u8, i: usize) -> State {
    if SPLINE.get(i).is_some_and(|f| flags & f != 0) {
        State::Spline
    } else if STATIC.get(i).is_some_and(|f| flags & f != 0) {
        State::Static
    } else {
        State::Absent
    }
}

/// Whether a vector's flags make it a spline track (a spline flag on x, y or z; w is a rotation's).
fn vector_has_spline(flags: u8) -> bool {
    SPLINE.iter().take(3).any(|f| flags & f != 0)
}

fn read_vector(r: &mut Reader, flags: u8, quantization: ScalarQuantization) -> Result<VectorData> {
    let mut components = [ScalarData::Absent, ScalarData::Absent, ScalarData::Absent];
    if !vector_has_spline(flags) {
        for (component, i) in components.iter_mut().zip(0..) {
            if state(flags, i) == State::Static {
                *component = ScalarData::Static(r.raw_f32()?);
            }
        }
        return Ok(VectorData {
            flags,
            quantization,
            knots: None,
            components,
        });
    }
    let (points, knots) = knots(r)?;
    r.pad(4)?;
    for (component, i) in components.iter_mut().zip(0..) {
        match state(flags, i) {
            State::Spline => {
                *component = ScalarData::Spline {
                    min: r.raw_f32()?,
                    max: r.raw_f32()?,
                    values: Vec::with_capacity(points),
                }
            }
            State::Static => *component = ScalarData::Static(r.raw_f32()?),
            State::Absent => {}
        }
    }
    for _ in 0..points {
        for component in &mut components {
            if let ScalarData::Spline { values, .. } = component {
                values.push(quantized(r, quantization)?);
            }
        }
    }
    Ok(VectorData {
        flags,
        quantization,
        knots: Some(knots),
        components,
    })
}

fn read_quaternion(r: &mut Reader, how: RotationQuantization) -> Result<PackedQuaternion> {
    Ok(match how {
        RotationQuantization::Uncompressed => {
            PackedQuaternion::Uncompressed([r.raw_f32()?, r.raw_f32()?, r.raw_f32()?, r.raw_f32()?])
        }
        RotationQuantization::ThreeComp48 => PackedQuaternion::ThreeComp48([
            r.u16(Endian::Little)?,
            r.u16(Endian::Little)?,
            r.u16(Endian::Little)?,
        ]),
        RotationQuantization::ThreeComp40 => PackedQuaternion::ThreeComp40(r.array::<5>()?),
    })
}

fn read_rotation(r: &mut Reader, flags: u8, code: u8) -> Result<RotationData> {
    let at = r.position();
    let quantization = RotationQuantization::of(code, at)?;
    let value = if SPLINE.iter().any(|f| flags & f != 0) {
        let (points, knots) = knots(r)?;
        r.pad(quantization.align())?;
        let points = (0..points)
            .map(|_| read_quaternion(r, quantization))
            .collect::<Result<Vec<_>>>()?;
        QuaternionData::Spline { knots, points }
    } else if STATIC.iter().any(|f| flags & f != 0) {
        r.pad(quantization.align())?;
        QuaternionData::Static(read_quaternion(r, quantization)?)
    } else {
        QuaternionData::Absent
    };
    Ok(RotationData {
        flags,
        quantization,
        value,
    })
}

/// One float track, by its mask byte.
fn read_float(r: &mut Reader, mask: u8) -> Result<FloatData> {
    match mask {
        FLOAT_STATIC => Ok(FloatData::Static(r.f32()?)),
        FLOAT_SPLINE_16 => {
            let (points, knots) = knots(r)?;
            r.pad(4)?;
            let (min, max) = (r.f32()?, r.f32()?);
            let values = (0..points)
                .map(|_| r.u16(Endian::Little).map_err(Error::from))
                .collect::<Result<Vec<_>>>()?;
            Ok(FloatData::Spline {
                knots,
                min,
                max,
                values,
            })
        }
        other => Err(invalid(
            r.position(),
            format!(
                "float track mask {other:#04x} is not read (only {FLOAT_STATIC:#04x}, a static value, and {FLOAT_SPLINE_16:#04x}, a 16-bit spline, occur)"
            ),
        )),
    }
}

/// Where a block's tracks end: where its float data begins; and where, after padding to 16, the next block begins
/// (the end of the data after the last).
#[derive(Clone, Copy)]
struct Ends {
    float_start: usize,
    next: usize,
    /// Whether this is the animation's last block.
    last: bool,
}

/// Whether the 16 guard bytes belong after a block whose last transform track's rotation has quantization `code` and
/// ends at `rotation_end`, the block's padded end being `end`: a THREECOMP40 rotation read 8 bytes at a time must not
/// run past the data.
fn guard_rule(code: u8, rotation_end: usize, end: usize) -> bool {
    code == 1 && end.saturating_sub(rotation_end) < 3
}

/// One block, number `index`: its transform tracks and its `float_tracks` float tracks, ending where `ends` says.
/// Returns the block and whether the 16 guard bytes follow it.
fn read_block(
    r: &mut Reader,
    tracks: usize,
    float_tracks: usize,
    index: usize,
    ends: Ends,
) -> Result<(BlockData, bool)> {
    let masks = (0..tracks).map(|_| mask(r)).collect::<Result<Vec<_>>>()?;
    let float_masks = r.take(float_tracks)?.to_vec();
    r.pad(4)?;
    let mut out = Vec::with_capacity(tracks);
    // the last track's rotation quantization code and where its rotation ends (where it would, if it is absent)
    let mut last_rotation = None;
    for (track, m) in masks.into_iter().enumerate() {
        // a failure names where it is: the block, the track and which of its three parts
        let at = |part: &'static str| {
            move |e: Error| {
                invalid(
                    e.offset,
                    format!("block {index} track {track} {part}: {}", e.detail),
                )
            }
        };
        let position = read_vector(r, m.position_flags, m.position).map_err(at("position"))?;
        r.pad(4)?;
        let rotation = read_rotation(r, m.rotation_flags, m.rotation).map_err(at("rotation"))?;
        last_rotation = Some((m.rotation, r.position()));
        r.pad(4)?;
        let scale = read_vector(r, m.scale_flags, m.scale).map_err(at("scale"))?;
        r.pad(4)?;
        out.push(TrackData {
            position,
            rotation,
            scale,
        });
    }
    let Ends {
        float_start,
        next,
        last,
    } = ends;
    if r.position() != float_start {
        return Err(invalid(
            r.position(),
            format!(
                "block {index} ends at {}, the float data begins at {float_start}",
                r.position()
            ),
        ));
    }
    let floats = float_masks
        .iter()
        .enumerate()
        .map(|(i, &m)| {
            read_float(r, m)
                .map_err(|e| invalid(e.offset, format!("block {index} float track {i}: {}", e.detail)))
        })
        .collect::<Result<Vec<_>>>()?;
    r.pad(16)?;
    let mut guard = false;
    if r.position() != next {
        // The 2018.1 encoder pads the last block of an animation by 16 more zero bytes when the last track's rotation
        // is 40 bits (code 1) and ends within two bytes of the padded end of the block: a 40-bit rotation is read with
        // an 8-byte load, which must not run past the data. The rotation counts where it would be stored when its
        // mask states none. In Elden Ring that is 4,975 of the 33,794 + 4,975 last blocks, exactly those the rule
        // names, and no other block of either game has the bytes (Sekiro's 2016.2 data has none, though 1,243 of its
        // last blocks have the geometry). They are accepted there and nowhere else. (After float data the rule cannot
        // hold: the data is at least four bytes past the last rotation, and none of the 1,110 blocks with float tracks
        // has the bytes.)
        guard = last
            && last_rotation.is_some_and(|(code, end)| guard_rule(code, end, r.position()))
            && r.position().checked_add(16) == Some(next)
            && r.bytes()
                .get(r.position()..next)
                .is_some_and(|pad| pad.iter().all(|&b| b == 0));
        if !guard {
            return Err(invalid(
                r.position(),
                format!(
                    "block {index} ends at {}, the next begins at {next}",
                    r.position()
                ),
            ));
        }
    }
    Ok((BlockData { tracks: out, floats }, guard))
}

impl SplineBlocks {
    /// The stored blocks of `animation` (which must be a spline compressed one, little endian, with at most one float
    /// track). Each block must end where the data says the next thing begins: the float data at `floatBlockOffsets`,
    /// then, after padding to 16, the next block or the end of the data.
    pub fn read(animation: &Animation) -> Result<SplineBlocks> {
        let Tracks::Spline(s) = &animation.tracks else {
            return Err(invalid(
                0,
                format!("{} is not a spline compressed animation", animation.class),
            ));
        };
        // Every animation of the games' files with float tracks has exactly one (66 in Sekiro, 542 in Elden Ring), so
        // what pads between two float tracks has never been seen.
        if animation.float_tracks > 1 {
            return Err(invalid(
                0,
                format!(
                    "an animation with {} float tracks is not read (every stored one has one)",
                    animation.float_tracks
                ),
            ));
        }
        if s.endian != 0 {
            return Err(invalid(0, "a big endian animation is not read"));
        }
        // The mask area of a block: a 4-byte mask a transform track and a byte a float track, padded to 4. The header
        // states it, and does in all 60,209 spline animations of Sekiro's (11,581) and Elden Ring's (48,628) binders,
        // the 608 with a float track included.
        let masks = mask_area(animation.transform_tracks, animation.float_tracks);
        if masks.is_none_or(|n| u32::try_from(n).ok() != Some(s.mask_and_quantization_size)) {
            return Err(invalid(
                0,
                format!(
                    "maskAndQuantizationSize {} disagrees with the track counts ({} transform tracks, {} float tracks)",
                    s.mask_and_quantization_size, animation.transform_tracks, animation.float_tracks
                ),
            ));
        }
        if s.max_frames_per_block < 2
            || s.num_frames == 0
            || s.frame_duration.is_nan()
            || s.frame_duration <= 0.0
        {
            return Err(invalid(
                0,
                "the animation's frame counts or duration are not usable",
            ));
        }
        let blocks = s.num_blocks as usize;
        if s.block_offsets.len() != blocks || s.float_block_offsets.len() != blocks || blocks == 0 {
            return Err(invalid(0, "the block offsets do not match the block count"));
        }
        let mut out = Vec::with_capacity(blocks);
        let mut guard = false;
        // each block's start and float data offset, and where the next begins (the end of the data after the last)
        let nexts = s
            .block_offsets
            .iter()
            .skip(1)
            .map(|&o| o as usize)
            .chain([s.data.len()]);
        let starts = s.block_offsets.iter().zip(&s.float_block_offsets).zip(nexts);
        for (b, ((&start, &float), next)) in starts.enumerate() {
            let start = start as usize;
            if !start.is_multiple_of(16) {
                return Err(invalid(start, "a block is not 16-byte aligned"));
            }
            // `floatBlockOffsets` count from the block's own start
            let float_start = start
                .checked_add(float as usize)
                .ok_or_else(|| invalid(start, format!("block {b}'s float data offset overflows")))?;
            let mut r = Reader::at(&s.data, start)?;
            let ends = Ends {
                float_start,
                next,
                last: b.checked_add(1) == Some(blocks),
            };
            let (block, guarded) = read_block(
                &mut r,
                animation.transform_tracks,
                animation.float_tracks,
                b,
                ends,
            )?;
            guard |= guarded;
            out.push(block);
        }
        Ok(SplineBlocks { blocks: out, guard })
    }

    /// Lay the blocks out as `data`, each block on a 16-byte boundary, with the 16 guard bytes after the last when
    /// [`SplineBlocks::guard`] is set. Refused: blocks with different track counts, more than one float track, values
    /// that disagree with the flags or quantization that state them (a component the flags make a spline without
    /// knots, an 8-bit value above 255, a quaternion of another quantization, ...), knots that are not
    /// `points + degree + 1` non-decreasing bytes of degree 1 to 3, and the guard where the rule does not put it.
    pub fn write(&self) -> Result<WrittenBlocks> {
        let first = self
            .blocks
            .first()
            .ok_or_else(|| invalid(0, "an animation has at least one block"))?;
        let (tracks, floats) = (first.tracks.len(), first.floats.len());
        if floats > 1 {
            return Err(invalid(
                0,
                format!("an animation with {floats} float tracks is not written (every stored one has one)"),
            ));
        }
        let masks = mask_area(tracks, floats).ok_or_else(|| invalid(0, "the mask area overflows"))?;
        let mut w = Writer::default();
        let mut block_offsets = Vec::with_capacity(self.blocks.len());
        let mut float_block_offsets = Vec::with_capacity(self.blocks.len());
        let mut last_guard = false;
        for (b, block) in self.blocks.iter().enumerate() {
            if block.tracks.len() != tracks || block.floats.len() != floats {
                return Err(invalid(
                    0,
                    format!(
                        "block {b} has {} transform and {} float tracks, block 0 {tracks} and {floats}",
                        block.tracks.len(),
                        block.floats.len()
                    ),
                ));
            }
            let start = w.len();
            block_offsets.push(word(start)?);
            let end = write_block(&mut w, block, masks)
                .map_err(|e| invalid(e.offset, format!("block {b}: {}", e.detail)))?;
            float_block_offsets.push(word(end.float_start.saturating_sub(start))?);
            last_guard = end.guard;
        }
        if self.guard {
            if !last_guard {
                return Err(invalid(
                    w.len(),
                    "the 16 guard bytes follow only a last block whose last rotation is THREECOMP40 and ends less than 3 bytes before the block's padded end",
                ));
            }
            w.zeros(16);
        }
        Ok(WrittenBlocks {
            data: w.0,
            block_offsets,
            float_block_offsets,
            mask_and_quantization_size: word(masks)?,
        })
    }

    /// Whether the 2018.1 encoder's rule puts the 16 guard bytes after these blocks: the last block's last transform
    /// track has a THREECOMP40 rotation that ends (or would, when it is absent) less than 3 bytes before the block's
    /// padded end. Refused as [`SplineBlocks::write`] refuses the last block.
    pub fn guard_applies(&self) -> Result<bool> {
        let Some(last) = self.blocks.last() else {
            return Ok(false);
        };
        let masks = mask_area(last.tracks.len(), last.floats.len())
            .ok_or_else(|| invalid(0, "the mask area overflows"))?;
        // a block's layout depends only on its position modulo 16, and every block starts on a multiple of 16
        Ok(write_block(&mut Writer::default(), last, masks)?.guard)
    }
}

/// The mask area of a block: `4 * transform tracks + float tracks`, rounded up to 4.
fn mask_area(tracks: usize, floats: usize) -> Option<usize> {
    tracks
        .checked_mul(4)
        .and_then(|n| n.checked_add(floats))
        .map(|n| n.next_multiple_of(4))
}

fn word(n: usize) -> Result<u32> {
    u32::try_from(n).map_err(|_| invalid(n, "the animation data passes 4 GiB"))
}

/// The bytes being laid out.
#[derive(Default)]
struct Writer(Vec<u8>);

impl Writer {
    fn len(&self) -> usize {
        self.0.len()
    }
    fn zeros(&mut self, n: usize) {
        self.0.resize(self.0.len().saturating_add(n), 0);
    }
    fn pad(&mut self, align: usize) {
        let to = self.0.len().next_multiple_of(align);
        self.0.resize(to, 0);
    }
    fn f32(&mut self, v: f32) {
        self.0.extend(v.to_le_bytes());
    }
    fn u16(&mut self, v: u16) {
        self.0.extend(v.to_le_bytes());
    }
    fn knots(&mut self, knots: &Knots, points: usize) -> Result<()> {
        let count = points
            .checked_add(usize::from(knots.degree))
            .and_then(|n| n.checked_add(1));
        if !(1..=3).contains(&knots.degree)
            || points <= usize::from(knots.degree)
            || count != Some(knots.values.len())
            || knots.values.windows(2).any(|w| matches!(*w, [a, b] if a > b))
        {
            return Err(invalid(
                self.len(),
                format!(
                    "{} knots of degree {} do not fit {points} control points (points + degree + 1, not decreasing, degree 1 to 3)",
                    knots.values.len(),
                    knots.degree
                ),
            ));
        }
        let items = points
            .checked_sub(1)
            .and_then(|n| i16::try_from(n).ok())
            .ok_or_else(|| {
                invalid(
                    self.len(),
                    format!("{points} control points do not fit the count"),
                )
            })?;
        self.0.extend(items.to_le_bytes());
        self.0.push(knots.degree);
        self.0.extend(&knots.values);
        Ok(())
    }
}

/// Where a written block's float data began, and whether the guard rule holds after it.
struct BlockEnd {
    float_start: usize,
    guard: bool,
}

fn write_block(w: &mut Writer, block: &BlockData, masks: usize) -> Result<BlockEnd> {
    let start = w.len();
    for t in &block.tracks {
        let q = t.position.quantization.code()
            | t.rotation.quantization.code() << 2
            | t.scale.quantization.code() << 6;
        w.0.extend([q, t.position.flags, t.rotation.flags, t.scale.flags]);
    }
    w.0.extend(block.floats.iter().map(FloatData::mask));
    w.0.resize(start.saturating_add(masks), 0);
    let mut last_rotation = None;
    for (i, t) in block.tracks.iter().enumerate() {
        let at =
            |part: &'static str| move |e: Error| invalid(e.offset, format!("track {i} {part}: {}", e.detail));
        write_vector(w, &t.position).map_err(at("position"))?;
        w.pad(4);
        write_rotation(w, &t.rotation).map_err(at("rotation"))?;
        last_rotation = Some((t.rotation.quantization.code(), w.len()));
        w.pad(4);
        write_vector(w, &t.scale).map_err(at("scale"))?;
        w.pad(4);
    }
    let float_start = w.len();
    for (i, f) in block.floats.iter().enumerate() {
        write_float(w, f).map_err(|e| invalid(e.offset, format!("float track {i}: {}", e.detail)))?;
    }
    w.pad(16);
    let guard =
        block.floats.is_empty() && last_rotation.is_some_and(|(code, end)| guard_rule(code, end, w.len()));
    Ok(BlockEnd { float_start, guard })
}

fn write_vector(w: &mut Writer, v: &VectorData) -> Result<()> {
    let at = w.len();
    let mismatch = |i: usize| {
        invalid(
            at,
            format!("component {i} disagrees with the flags {:#04x}", v.flags),
        )
    };
    for (c, i) in v.components.iter().zip(0..) {
        let agrees = matches!(
            (state(v.flags, i), c),
            (State::Absent, ScalarData::Absent)
                | (State::Static, ScalarData::Static(_))
                | (State::Spline, ScalarData::Spline { .. })
        );
        if !agrees {
            return Err(mismatch(i));
        }
    }
    if !vector_has_spline(v.flags) {
        if v.knots.is_some() {
            return Err(invalid(at, "knots for a vector with no spline component"));
        }
        for c in &v.components {
            if let ScalarData::Static(value) = c {
                w.f32(*value);
            }
        }
        return Ok(());
    }
    let knots = v
        .knots
        .as_ref()
        .ok_or_else(|| invalid(at, "a spline vector without knots"))?;
    let points = v
        .components
        .iter()
        .find_map(|c| match c {
            ScalarData::Spline { values, .. } => Some(values.len()),
            _ => None,
        })
        .unwrap_or_default();
    w.knots(knots, points)?;
    w.pad(4);
    for c in &v.components {
        match c {
            ScalarData::Spline { min, max, values } => {
                if values.len() != points {
                    return Err(invalid(
                        w.len(),
                        format!(
                            "spline components of {points} and {} control points",
                            values.len()
                        ),
                    ));
                }
                w.f32(*min);
                w.f32(*max);
            }
            ScalarData::Static(value) => w.f32(*value),
            ScalarData::Absent => {}
        }
    }
    let top = v.quantization.top();
    for p in 0..points {
        for c in &v.components {
            if let ScalarData::Spline { values, .. } = c {
                let value = values.get(p).copied().unwrap_or_default();
                if value > top {
                    return Err(invalid(
                        w.len(),
                        format!("control point value {value} is above the quantization's {top}"),
                    ));
                }
                match v.quantization {
                    ScalarQuantization::Bits8 => w.0.push(u8::try_from(value).unwrap_or(u8::MAX)),
                    ScalarQuantization::Bits16 => w.u16(value),
                }
            }
        }
    }
    Ok(())
}

fn write_quaternion(w: &mut Writer, q: &PackedQuaternion, how: RotationQuantization) -> Result<()> {
    if q.quantization() != how {
        return Err(invalid(
            w.len(),
            format!("a {:?} quaternion in a {how:?} rotation", q.quantization()),
        ));
    }
    match q {
        PackedQuaternion::ThreeComp40(bytes) => w.0.extend(bytes),
        PackedQuaternion::ThreeComp48(words) => words.iter().for_each(|&v| w.u16(v)),
        PackedQuaternion::Uncompressed(values) => values.iter().for_each(|&v| w.f32(v)),
    }
    Ok(())
}

fn write_rotation(w: &mut Writer, r: &RotationData) -> Result<()> {
    let spline = r.flags & 0xF0 != 0;
    let static_ = !spline && r.flags & 0x0F != 0;
    match &r.value {
        QuaternionData::Spline { knots, points } if spline => {
            w.knots(knots, points.len())?;
            w.pad(r.quantization.align());
            for q in points {
                write_quaternion(w, q, r.quantization)?;
            }
        }
        QuaternionData::Static(q) if static_ => {
            w.pad(r.quantization.align());
            write_quaternion(w, q, r.quantization)?;
        }
        QuaternionData::Absent if !spline && !static_ => {}
        _ => {
            return Err(invalid(
                w.len(),
                format!("the rotation disagrees with its flags {:#04x}", r.flags),
            ));
        }
    }
    Ok(())
}

fn write_float(w: &mut Writer, f: &FloatData) -> Result<()> {
    // the reader refuses a float track's value or bounds that are not finite (none of the 1,110 stored blocks has one)
    let finite = match f {
        FloatData::Static(v) => v.is_finite(),
        FloatData::Spline { min, max, .. } => min.is_finite() && max.is_finite(),
    };
    if !finite {
        return Err(invalid(w.len(), "a float track's value or bounds are not finite"));
    }
    match f {
        FloatData::Static(v) => w.f32(*v),
        FloatData::Spline {
            knots,
            min,
            max,
            values,
        } => {
            w.knots(knots, values.len())?;
            w.pad(4);
            w.f32(*min);
            w.f32(*max);
            values.iter().for_each(|&v| w.u16(v));
        }
    }
    w.pad(4);
    Ok(())
}
