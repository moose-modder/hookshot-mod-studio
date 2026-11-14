//! Spline compression: an animation of sampled transform and float tracks (`hkaInterleavedUncompressedAnimation`)
//! made into the blocks of a `hkaSplineCompressedAnimation` ([`encode`], [`encode_blocks`]).
//!
//! The frames are cut into blocks of `max_frames_per_block` (neighbours share their boundary frame). In each block, each
//! part of each track (position, rotation, scale; or a float track's value) is judged component by component against its
//! tolerance: a component whose samples all lie within the tolerance of their mean is static (stored as that mean), or
//! absent when the mean is also within the tolerance of the neutral value (zero, the identity rotation, one); a part with
//! any other component is a spline. A spline is fitted to the part's samples as a whole ([`crate::spline_fit`]), at the
//! frame times of the block, within the tolerance in four dimensions, of the requested degree, or of degree 1 when that
//! needs more control points than the block has frames. Knots are stored as frame numbers; a position's or scale's
//! control points as 8- or 16-bit integers between the smallest and largest control point of each spline component; a
//! rotation's as normalised quaternions in the track's rotation quantization; a static rotation as the normalised mean.
//!
//! What this does as the games' files show it (all 60,209 stored spline animations of Sekiro and Elden Ring re-written
//! byte for byte from their stored values, and the header of every one): the block and mask layout, the header
//! arithmetic in f32, Elden Ring's 16 guard bytes, truncated control points, and the packing of a zero control point
//! (normalised to NaN, it packs as the bytes `00 80 00 00 80`, which the games' files hold 1,006 times). What it does
//! its own way: the games' encoders divide with the processor's approximate reciprocal (the top control point of a
//! spline component is 65534 in about a third of them) and normalise with its approximate square root; this divides
//! exactly, so it is the same on every machine. Its curves are fitted in f32 in a fixed order, so the same input gives
//! the same bytes everywhere. [`Flavor::CompressAnim`] follows instead the arithmetic of `CompressAnim.exe` (Havok
//! 2010.2.0-r1) outside the fit: on the cases of tests/compress_anim.rs it writes that tool's header, layout, flags and
//! knots, two cases byte for byte, the others with fitted values differing in their last bits (the tool fits in the
//! x87's extended precision).
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use crate::animation::{Animation, AnimationType, InterleavedData, QsTransform, SplineData, Tracks};
use crate::spline_data::{
    BlockData, FloatData, Knots, PackedQuaternion, QuaternionData, RotationData, RotationQuantization,
    ScalarData, ScalarQuantization, SplineBlocks, TrackData, VectorData,
};
use crate::spline_fit::{self, Curve, MAX_DEGREE, Point};
use crate::{Result, invalid};

/// How one track is compressed (`TrackCompressionParams`): a tolerance and a degree for each part, and the
/// quantizations its mask selects. A float track uses `float_tolerance` and `float_degree` and is stored in 16 bits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackParams {
    /// How far a position may stray from its samples (in the animation's length unit).
    pub translation_tolerance: f32,
    /// How far a rotation may stray, as the four-dimensional distance between quaternions.
    pub rotation_tolerance: f32,
    /// How far a scale may stray.
    pub scale_tolerance: f32,
    /// How far a float track's value may stray.
    pub float_tolerance: f32,
    /// The degree of a position spline (1 to 3).
    pub translation_degree: u8,
    /// The degree of a rotation spline (1 to 3).
    pub rotation_degree: u8,
    /// The degree of a scale spline (1 to 3).
    pub scale_degree: u8,
    /// The degree of a float track's spline (1 to 3).
    pub float_degree: u8,
    /// How a position spline's control points are stored.
    pub translation_quantization: ScalarQuantization,
    /// How rotations are stored.
    pub rotation_quantization: RotationQuantization,
    /// How a scale spline's control points are stored.
    pub scale_quantization: ScalarQuantization,
}

impl Default for TrackParams {
    /// Tolerance 0.001 and degree 3 for every part, 16-bit positions and scales, THREECOMP40 rotations: the defaults
    /// of `CompressAnim.exe` with rotation code 1.
    fn default() -> Self {
        TrackParams {
            translation_tolerance: 0.001,
            rotation_tolerance: 0.001,
            scale_tolerance: 0.001,
            float_tolerance: 0.001,
            translation_degree: 3,
            rotation_degree: 3,
            scale_degree: 3,
            float_degree: 3,
            translation_quantization: ScalarQuantization::Bits16,
            rotation_quantization: RotationQuantization::ThreeComp40,
            scale_quantization: ScalarQuantization::Bits16,
        }
    }
}

impl TrackParams {
    /// What `CompressAnim.exe <in> <out> <rotation code> <rotation tolerance>` compresses every track with: its
    /// defaults (tolerance 0.001 and degree 3 everywhere, 16-bit positions and scales) with the rotation quantization
    /// and tolerance it is given.
    pub fn compress_anim(
        rotation_quantization: RotationQuantization,
        rotation_tolerance: f32,
    ) -> TrackParams {
        TrackParams {
            rotation_quantization,
            rotation_tolerance,
            ..TrackParams::default()
        }
    }
}

/// How an animation is compressed: the frames a block holds and each track's [`TrackParams`].
#[derive(Clone, Debug, PartialEq)]
pub struct EncodeParams {
    /// `maxFramesPerBlock`, 2 to 256 (256 in every stored animation of both games).
    pub max_frames_per_block: u32,
    /// One for each transform track.
    pub transform: Vec<TrackParams>,
    /// One for each float track (at most one float track: what pads between two is not known).
    pub float: Vec<TrackParams>,
    /// Add the 16 zero bytes the 2018.1 encoder puts after a last block whose last rotation is THREECOMP40 and ends less
    /// than 3 bytes before the block's padded end, as Elden Ring's files have them; Sekiro's (2016.2) never do.
    pub guard: bool,
    /// Whose arithmetic to follow where the encoders differ.
    pub flavor: Flavor,
}

impl EncodeParams {
    /// The same parameters for every track, 256 frames a block, the games' arithmetic.
    pub fn uniform(
        params: TrackParams,
        transform_tracks: usize,
        float_tracks: usize,
        guard: bool,
    ) -> EncodeParams {
        EncodeParams {
            max_frames_per_block: 256,
            transform: vec![params; transform_tracks],
            float: vec![params; float_tracks],
            guard,
            flavor: Flavor::Games,
        }
    }

    /// What `CompressAnim.exe <in> <out> <rotation code> <rotation tolerance>` does: [`TrackParams::compress_anim`] for
    /// every track, 256 frames a block, no guard bytes, and its arithmetic ([`Flavor::CompressAnim`]).
    pub fn compress_anim(
        rotation_quantization: RotationQuantization,
        rotation_tolerance: f32,
        transform_tracks: usize,
        float_tracks: usize,
    ) -> EncodeParams {
        EncodeParams {
            flavor: Flavor::CompressAnim,
            ..EncodeParams::uniform(
                TrackParams::compress_anim(rotation_quantization, rotation_tolerance),
                transform_tracks,
                float_tracks,
                false,
            )
        }
    }
}

/// Whose arithmetic the encoder follows where encoders differ: in rounding and a few conventions, never in the
/// algorithm. The fit itself is f32 in both.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Flavor {
    /// The games' encoders (2016.2 and 2018.1, 64-bit): every operation rounded to f32; control points truncated to
    /// integers; THREECOMP components scaled by the f32 nearest `2047 sqrt 2` (`16383 sqrt 2`) and truncated, the last
    /// of equal largest components left out; a zero quaternion normalised to NaN (the games' files hold 1,006 such
    /// points). Dividing exactly where they used the processor's approximate reciprocal (see the module
    /// documentation).
    #[default]
    Games,
    /// `CompressAnim.exe` (Havok 2010.2.0-r1, 32-bit, x87): the frame times, the block duration, a quaternion's squared
    /// length and a control point's integer computed in extended precision and rounded once; THREECOMP components
    /// scaled by `f32(f32(sqrt 2) * 2047)` (one f32 step below the nearest), the exact product truncated; the first of
    /// equal largest components left out; a zero quaternion left zero.
    CompressAnim,
}

/// `v` widened: every f32 is exact in f64, and so is a product of two.
fn wide(v: f32) -> f64 {
    f64::from(v)
}

/// `v` rounded to f32 once, as a value computed with more precision and then stored.
#[allow(
    clippy::cast_possible_truncation,
    reason = "rounding to f32 is the stated conversion"
)]
fn narrow(v: f64) -> f32 {
    v as f32
}

const ZERO: Point = [0.0; 4];
const IDENTITY: Point = [0.0, 0.0, 0.0, 1.0];
const ONE: Point = [1.0, 1.0, 1.0, 0.0];

/// The f32 nearest `2047 sqrt 2`: a THREECOMP40 component in units of its step.
const THREE_COMP40_SCALE: f32 = 2_894.895_2;
/// The f32 nearest `16383 sqrt 2`: a THREECOMP48 component in units of its step.
const THREE_COMP48_SCALE: f32 = 23_169.06;

/// The number of frames of an interleaved animation: its transforms (or floats) divided by its tracks.
fn frames_of(animation: &Animation, data: &InterleavedData) -> Result<usize> {
    let count = |values: usize, tracks: usize| -> Result<Option<usize>> {
        if tracks == 0 {
            return Ok(None);
        }
        if !values.is_multiple_of(tracks) {
            return Err(invalid(
                0,
                format!("{values} samples do not divide into {tracks} tracks"),
            ));
        }
        Ok(values.checked_div(tracks))
    };
    let from_transforms = count(data.transforms.len(), animation.transform_tracks)?;
    let from_floats = count(data.floats.len(), animation.float_tracks)?;
    let frames = match (from_transforms, from_floats) {
        (Some(a), Some(b)) if a != b => {
            return Err(invalid(0, format!("{a} frames of transforms and {b} of floats")));
        }
        (Some(a), _) | (None, Some(a)) => a,
        (None, None) => return Err(invalid(0, "an animation without tracks is not encoded")),
    };
    if frames < 2 {
        return Err(invalid(
            0,
            format!("{frames} frames: spline compression needs at least 2"),
        ));
    }
    Ok(frames)
}

fn usize_to_f32(v: usize) -> Result<f32> {
    // every count here is at most a block's 256 frames or an animation's frames, well inside f32's exact integers
    let v = u32::try_from(v).map_err(|_| invalid(0, "a count beyond u32"))?;
    if v > 1 << 24 {
        return Err(invalid(0, "a frame count beyond f32's exact integers"));
    }
    #[allow(clippy::cast_precision_loss, reason = "checked to be exact above")]
    Ok(v as f32)
}

/// `m + 1` times from `a` to `b`: `i / m * (b - a) + a`, the ends exactly `a` and `b`.
fn linspace(m: usize, a: f32, b: f32, flavor: Flavor) -> Result<Vec<f32>> {
    let span = usize_to_f32(m)?;
    let mut t = Vec::with_capacity(m.saturating_add(1));
    for i in 0..=m {
        let at = usize_to_f32(i)?;
        t.push(if i == 0 {
            a
        } else if i == m {
            b
        } else {
            match flavor {
                Flavor::Games => at / span * (b - a) + a,
                Flavor::CompressAnim => narrow(wide(at) / wide(span) * (wide(b) - wide(a)) + wide(a)),
            }
        });
    }
    Ok(t)
}

/// The seconds `frames` frame intervals last: `duration / intervals * frames`, in f32 (games) or in extended precision
/// rounded once (CompressAnim).
fn block_seconds(duration: f32, intervals: f32, frames: f32, flavor: Flavor) -> f32 {
    match flavor {
        Flavor::Games => duration / intervals * frames,
        Flavor::CompressAnim => narrow(wide(duration) / wide(intervals) * wide(frames)),
    }
}

/// `p` scaled to length one. A zero point becomes NaN in the games' flavor (no length to divide by) and stays zero in
/// CompressAnim's.
fn normalized(p: Point, flavor: Flavor) -> Point {
    let length2 = match flavor {
        Flavor::Games => spline_fit::dot(p, p),
        Flavor::CompressAnim => {
            let [x, y, z, w] = p.map(wide);
            let length2 = narrow(x * x + y * y + z * z + w * w);
            if length2 == 0.0 {
                return ZERO;
            }
            length2
        }
    };
    let inverse = 1.0 / length2.sqrt();
    p.map(|c| c * inverse)
}

/// What one part of one track in one block is: its mask flags, the mean of its samples, and its curve if it is a
/// spline.
struct Part {
    flags: u8,
    mean: Point,
    curve: Option<Curve>,
}

/// Judge and fit one part: `samples` at `times`, against `neutral`.
fn part(
    samples: &[Point],
    times: &[f32],
    degree: u8,
    tolerance: f32,
    neutral: Point,
    rotation: bool,
    flavor: Flavor,
) -> Result<Part> {
    let first = samples
        .first()
        .copied()
        .ok_or_else(|| invalid(0, "a block without samples"))?;
    let (mut sum, mut low, mut high) = (first, first, first);
    for s in samples.iter().skip(1) {
        for (((sum, low), high), &v) in sum.iter_mut().zip(&mut low).zip(&mut high).zip(s) {
            *sum += v;
            // the running bound first: `bound < v ? bound : v`, as a vector minimum orders them (a signed zero
            // keeps the later one)
            *low = if *low < v { *low } else { v };
            *high = if *high > v { *high } else { v };
        }
    }
    let inverse = 1.0 / usize_to_f32(samples.len())?;
    let mut mean = sum.map(|v| v * inverse);
    if rotation {
        let length = spline_fit::dot(mean, mean);
        mean = if length > 0.0 {
            normalized(mean, flavor)
        } else {
            ZERO
        };
    }
    let mut flags = 0u8;
    for (lane, ((&m, (&lo, &hi)), &n)) in mean.iter().zip(low.iter().zip(&high)).zip(&neutral).enumerate() {
        if (m - lo).abs() <= tolerance && (m - hi).abs() <= tolerance {
            if (m - n).abs() > tolerance {
                flags |= 0x01 << lane;
            }
        } else {
            flags |= 0x10 << lane;
        }
    }
    let curve = if flags & 0xF0 != 0 {
        let mut c = spline_fit::approximate(times, samples, usize::from(degree), tolerance)?;
        if c.points.len() > samples.len() {
            c = spline_fit::approximate(times, samples, 1, tolerance)?;
        }
        Some(c)
    } else {
        None
    };
    Ok(Part { flags, mean, curve })
}

/// The knot bytes of a curve: each knot time in frames, rounded (`time * (frames - 1) / duration + 0.5`, truncated).
fn knot_bytes(c: &Curve, frames_per_second: f32) -> Result<Knots> {
    let degree = u8::try_from(c.degree).map_err(|_| invalid(0, "a degree beyond a byte"))?;
    let mut values = Vec::with_capacity(c.knots.len());
    for &k in &c.knots {
        let frame = (k * frames_per_second + 0.5).trunc();
        if !(0.0..256.0).contains(&frame) {
            return Err(invalid(
                0,
                format!("knot time {k} is frame {frame}, outside 0 to 255"),
            ));
        }
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a whole number in 0..256"
        )]
        values.push(frame as u8);
    }
    Ok(Knots { degree, values })
}

/// A value between `min` and `max` as an integer of `top + 1` steps: `(v - min) / (max - min) * top`, truncated, and
/// 0 when the range is empty (NaN).
fn quantize(min: f32, max: f32, v: f32, top: u16, flavor: Flavor) -> u16 {
    let top_f = wide(f32::from(top));
    let d = match flavor {
        Flavor::Games => wide((v - min) / (max - min) * f32::from(top)),
        // the expression in extended precision, truncated without rounding to f32 first
        Flavor::CompressAnim => (wide(v) - wide(min)) / (wide(max) - wide(min)) * top_f,
    };
    if d.is_nan() || d <= 0.0 {
        0
    } else if d >= top_f {
        top
    } else {
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "in 0..top"
        )]
        let q = d as u16;
        q
    }
}

/// A component in units of a quantization step (`scaled`, the product), truncated toward zero, plus the offset of
/// zero, saturated to 16 bits (a NaN converts to the most negative integer).
fn three_component(scaled: f64, zero: i32) -> i16 {
    #[allow(
        clippy::cast_possible_truncation,
        reason = "the conversion truncates and saturates, as stated"
    )]
    let whole = if scaled.is_nan() || scaled >= 2_147_483_648.0 || scaled < -2_147_483_648.0 {
        i32::MIN
    } else {
        scaled as i32
    };
    let shifted = whole.wrapping_add(zero);
    i16::try_from(shifted).unwrap_or(if shifted < 0 { i16::MIN } else { i16::MAX })
}

/// The index of the component of largest magnitude: the last of equal ones (games), the first (CompressAnim); 0 when
/// none compares (NaN).
fn largest(q: Point, flavor: Flavor) -> usize {
    let a = q.map(f32::abs);
    match flavor {
        Flavor::Games => {
            let top = a
                .iter()
                .copied()
                .fold(f32::NEG_INFINITY, |m, v| if v > m { v } else { m });
            a.iter().rposition(|&v| v >= top).unwrap_or(0)
        }
        Flavor::CompressAnim => {
            let mut best = 0;
            for (i, &v) in a.iter().enumerate().skip(1) {
                if a.get(best).is_some_and(|&b| b < v) {
                    best = i;
                }
            }
            best
        }
    }
}

/// Pack a quaternion (already normalised) in `how`.
pub(crate) fn pack(q: Point, how: RotationQuantization, flavor: Flavor) -> PackedQuaternion {
    match how {
        RotationQuantization::Uncompressed => PackedQuaternion::Uncompressed(q),
        RotationQuantization::ThreeComp40 | RotationQuantization::ThreeComp48 => {
            let big = largest(q, flavor);
            let negative = q.get(big).is_some_and(|&v| v < 0.0);
            let (nearest, levels, zero) = match how {
                RotationQuantization::ThreeComp40 => (THREE_COMP40_SCALE, 2047.0, 2047),
                _ => (THREE_COMP48_SCALE, 16383.0, 16383),
            };
            let mut c = [0i16; 3];
            for (out, (_, &v)) in c.iter_mut().zip(q.iter().enumerate().filter(|&(i, _)| i != big)) {
                let scaled = match flavor {
                    Flavor::Games => wide(v * nearest),
                    Flavor::CompressAnim => {
                        // the scale computed at run time: sqrt 2 as an f32, times the levels, stored as an f32
                        let scale = narrow(wide(std::f32::consts::SQRT_2) * levels);
                        wide(v) * wide(scale)
                    }
                };
                *out = three_component(scaled, zero);
            }
            let [a, b, d] = c;
            let index = u8::try_from(big).unwrap_or(0);
            if how == RotationQuantization::ThreeComp48 {
                // the 16-bit integer with the flag ORed into its top bit (a saturated NaN keeps its own)
                let word = |v: i16, top: bool| u16::from_le_bytes(v.to_le_bytes()) | (u16::from(top) << 15);
                PackedQuaternion::ThreeComp48([
                    word(a, index & 1 != 0),
                    word(b, index & 2 != 0),
                    word(d, negative),
                ])
            } else {
                // as 16-bit integers shifted arithmetically, then cut to bytes
                let byte = |v: i32| v.to_le_bytes().first().copied().unwrap_or_default();
                let (a, b, d) = (i32::from(a), i32::from(b), i32::from(d));
                PackedQuaternion::ThreeComp40([
                    byte(a),
                    byte((a >> 8) | (b << 4)),
                    byte(b >> 4),
                    byte(d),
                    byte(i32::from(index) << 4 | i32::from(negative) << 6 | (d >> 8)),
                ])
            }
        }
    }
}

/// The smallest and largest value of each lane of `points`.
fn bounds(points: &[Point]) -> (Point, Point) {
    let first = points.first().copied().unwrap_or(ZERO);
    points.iter().skip(1).fold((first, first), |(mut lo, mut hi), p| {
        for ((lo, hi), &v) in lo.iter_mut().zip(&mut hi).zip(p) {
            *lo = if *lo < v { *lo } else { v };
            *hi = if *hi > v { *hi } else { v };
        }
        (lo, hi)
    })
}

/// A position or scale part stored.
fn vector(
    p: &Part,
    quantization: ScalarQuantization,
    frames_per_second: f32,
    flavor: Flavor,
) -> Result<VectorData> {
    let mut components = [ScalarData::Absent, ScalarData::Absent, ScalarData::Absent];
    let knots = match &p.curve {
        Some(c) => Some(knot_bytes(c, frames_per_second)?),
        None => None,
    };
    let (low, high) = p.curve.as_ref().map_or((ZERO, ZERO), |c| bounds(&c.points));
    for (lane, component) in components.iter_mut().enumerate() {
        let get = |v: Point| v.get(lane).copied().unwrap_or_default();
        if p.flags & (0x10 << lane) != 0 {
            let c = p
                .curve
                .as_ref()
                .ok_or_else(|| invalid(0, "a spline component without a curve"))?;
            let (min, max) = (get(low), get(high));
            *component = ScalarData::Spline {
                min,
                max,
                values: c
                    .points
                    .iter()
                    .map(|q| quantize(min, max, get(*q), quantization.top(), flavor))
                    .collect(),
            };
        } else if p.flags & (0x01 << lane) != 0 {
            *component = ScalarData::Static(get(p.mean));
        }
    }
    Ok(VectorData {
        flags: p.flags,
        quantization,
        knots,
        components,
    })
}

/// A rotation part stored.
fn rotation(
    p: &Part,
    quantization: RotationQuantization,
    frames_per_second: f32,
    flavor: Flavor,
) -> Result<RotationData> {
    let value = if let Some(c) = &p.curve {
        QuaternionData::Spline {
            knots: knot_bytes(c, frames_per_second)?,
            points: c
                .points
                .iter()
                .map(|&q| pack(normalized(q, flavor), quantization, flavor))
                .collect(),
        }
    } else if p.flags & 0x0F != 0 {
        QuaternionData::Static(pack(normalized(p.mean, flavor), quantization, flavor))
    } else {
        QuaternionData::Absent
    };
    Ok(RotationData {
        flags: p.flags,
        quantization,
        value,
    })
}

/// A float track part stored: a static value (also when it is within the tolerance of zero) or a 16-bit spline.
fn float(p: &Part, frames_per_second: f32, flavor: Flavor) -> Result<FloatData> {
    let Some(c) = &p.curve else {
        return Ok(FloatData::Static(p.mean.first().copied().unwrap_or_default()));
    };
    let (low, high) = bounds(&c.points);
    let (min, max) = (
        low.first().copied().unwrap_or_default(),
        high.first().copied().unwrap_or_default(),
    );
    Ok(FloatData::Spline {
        knots: knot_bytes(c, frames_per_second)?,
        min,
        max,
        values: c
            .points
            .iter()
            .map(|q| quantize(min, max, q.first().copied().unwrap_or_default(), 65535, flavor))
            .collect(),
    })
}

fn check_params(p: &TrackParams, what: &str) -> Result<()> {
    for (name, degree) in [
        ("translation", p.translation_degree),
        ("rotation", p.rotation_degree),
        ("scale", p.scale_degree),
        ("float", p.float_degree),
    ] {
        if !(1..=MAX_DEGREE).contains(&usize::from(degree)) {
            return Err(invalid(0, format!("{what}: {name} degree {degree} (1 to 3)")));
        }
    }
    for (name, t) in [
        ("translation", p.translation_tolerance),
        ("rotation", p.rotation_tolerance),
        ("scale", p.scale_tolerance),
        ("float", p.float_tolerance),
    ] {
        if !(t.is_finite() && t >= 0.0) {
            return Err(invalid(0, format!("{what}: {name} tolerance {t}")));
        }
    }
    Ok(())
}

fn finite(t: &QsTransform) -> bool {
    t.translation
        .iter()
        .take(3)
        .chain(&t.rotation)
        .chain(t.scale.iter().take(3))
        .all(|v| v.is_finite())
}

/// The stored blocks of `animation` (an interleaved one) compressed with `params`, and the number of frames.
pub fn encode_blocks(animation: &Animation, params: &EncodeParams) -> Result<(SplineBlocks, usize)> {
    let Tracks::Interleaved(data) = &animation.tracks else {
        return Err(invalid(
            0,
            format!("{} is not an interleaved animation", animation.class),
        ));
    };
    let frames = frames_of(animation, data)?;
    let (tracks, float_tracks) = (animation.transform_tracks, animation.float_tracks);
    if params.transform.len() != tracks || params.float.len() != float_tracks {
        return Err(invalid(
            0,
            format!(
                "parameters for {} transform and {} float tracks, the animation has {tracks} and {float_tracks}",
                params.transform.len(),
                params.float.len()
            ),
        ));
    }
    if float_tracks > 1 {
        return Err(invalid(
            0,
            "more than one float track is not encoded (what pads between two is not known)",
        ));
    }
    if !(2..=256).contains(&params.max_frames_per_block) {
        return Err(invalid(
            0,
            format!("{} frames a block (2 to 256)", params.max_frames_per_block),
        ));
    }
    for (i, p) in params.transform.iter().enumerate() {
        check_params(p, &format!("transform track {i}"))?;
    }
    for (i, p) in params.float.iter().enumerate() {
        check_params(p, &format!("float track {i}"))?;
    }
    if !(animation.duration.is_finite() && animation.duration > 0.0) {
        return Err(invalid(0, format!("duration {}", animation.duration)));
    }
    if !data.transforms.iter().all(finite) || !data.floats.iter().all(|v| v.is_finite()) {
        return Err(invalid(0, "a sample is not finite"));
    }
    let per_block = usize::try_from(params.max_frames_per_block).map_err(|_| invalid(0, "frames a block"))?;
    let last = frames.saturating_sub(1);
    let intervals = usize_to_f32(last)?;
    let flavor = params.flavor;
    let frames_per_second = (1.0 / animation.duration) * intervals;
    // `ceil((frames - 1) / (per_block - 1))`, at least 1; per_block is at least 2
    let block_count = frames
        .saturating_add(per_block)
        .saturating_sub(3)
        .checked_div(per_block.saturating_sub(1))
        .unwrap_or(1)
        .max(1);
    let mut blocks = Vec::with_capacity(block_count);
    let mut start = 0usize;
    for _ in 0..block_count {
        let block_frames = std::cmp::min(per_block, frames.saturating_sub(start));
        let span = block_frames.saturating_sub(1);
        let seconds = block_seconds(animation.duration, intervals, usize_to_f32(span)?, flavor);
        let times = linspace(span, 0.0, seconds, flavor)?;
        let frame_range = start..start.saturating_add(block_frames);
        let mut block_tracks = Vec::with_capacity(tracks);
        for (t, p) in params.transform.iter().enumerate() {
            let sample = |f: usize| -> Result<&QsTransform> {
                f.checked_mul(tracks)
                    .and_then(|i| i.checked_add(t))
                    .and_then(|i| data.transforms.get(i))
                    .ok_or_else(|| invalid(0, "a frame outside the samples"))
            };
            let mut positions = Vec::with_capacity(block_frames);
            let mut rotations = Vec::with_capacity(block_frames);
            let mut scales = Vec::with_capacity(block_frames);
            for f in frame_range.clone() {
                let s = sample(f)?;
                let [x, y, z, _] = s.translation;
                positions.push([x, y, z, 0.0]);
                rotations.push(s.rotation);
                let [x, y, z, _] = s.scale;
                scales.push([x, y, z, 0.0]);
            }
            let position = part(
                &positions,
                &times,
                p.translation_degree,
                p.translation_tolerance,
                ZERO,
                false,
                flavor,
            )?;
            let rotation_part = part(
                &rotations,
                &times,
                p.rotation_degree,
                p.rotation_tolerance,
                IDENTITY,
                true,
                flavor,
            )?;
            let scale = part(
                &scales,
                &times,
                p.scale_degree,
                p.scale_tolerance,
                ONE,
                false,
                flavor,
            )?;
            block_tracks.push(TrackData {
                position: vector(&position, p.translation_quantization, frames_per_second, flavor)?,
                rotation: rotation(&rotation_part, p.rotation_quantization, frames_per_second, flavor)?,
                scale: vector(&scale, p.scale_quantization, frames_per_second, flavor)?,
            });
        }
        let mut block_floats = Vec::with_capacity(float_tracks);
        for (t, p) in params.float.iter().enumerate() {
            let mut values = Vec::with_capacity(block_frames);
            for f in frame_range.clone() {
                let v = f
                    .checked_mul(float_tracks)
                    .and_then(|i| i.checked_add(t))
                    .and_then(|i| data.floats.get(i))
                    .ok_or_else(|| invalid(0, "a frame outside the float samples"))?;
                values.push([*v, 0.0, 0.0, 0.0]);
            }
            let mut value = part(
                &values,
                &times,
                p.float_degree,
                p.float_tolerance,
                ZERO,
                false,
                flavor,
            )?;
            // a float track is never absent: within the tolerance of zero it is static
            if value.flags == 0 {
                value.flags = 0x01;
            }
            block_floats.push(float(&value, frames_per_second, flavor)?);
        }
        blocks.push(BlockData {
            tracks: block_tracks,
            floats: block_floats,
        });
        start = start.saturating_add(span);
    }
    let mut stored = SplineBlocks { blocks, guard: false };
    stored.guard = params.guard && stored.guard_applies()?;
    Ok((stored, frames))
}

/// `animation` (an interleaved one: `hkaInterleavedUncompressedAnimation`) compressed with `params` as a
/// `hkaSplineCompressedAnimation`: the same duration, track counts, extracted motion and annotation tracks, and the
/// header and data [`crate::spline::SplineAnimation::parse`] reads. Refused: an animation that is not interleaved,
/// fewer than two frames, a sample or the duration that is not finite (or a duration that is not positive), parameters
/// for another number of tracks, more than one float track, degrees outside 1 to 3 or tolerances that are negative or
/// not finite, and knots that would need a frame number outside a byte.
pub fn encode(animation: &Animation, params: &EncodeParams) -> Result<Animation> {
    let (stored, frames) = encode_blocks(animation, params)?;
    let written = stored.write()?;
    let num_frames = u32::try_from(frames).map_err(|_| invalid(0, "frames beyond u32"))?;
    let num_blocks = u32::try_from(stored.blocks.len()).map_err(|_| invalid(0, "blocks beyond u32"))?;
    let intervals = usize_to_f32(frames.saturating_sub(1))?;
    let frame_duration = animation.duration / intervals;
    let per_block = params.max_frames_per_block.saturating_sub(1);
    let per_block = usize_to_f32(usize::try_from(per_block).map_err(|_| invalid(0, "frames a block"))?)?;
    let block_duration = block_seconds(animation.duration, intervals, per_block, params.flavor);
    Ok(Animation {
        class: "hkaSplineCompressedAnimation".into(),
        kind: AnimationType::SplineCompressed,
        duration: animation.duration,
        transform_tracks: animation.transform_tracks,
        float_tracks: animation.float_tracks,
        extracted_motion: animation.extracted_motion.clone(),
        annotation_tracks: animation.annotation_tracks.clone(),
        tracks: Tracks::Spline(SplineData {
            num_frames,
            num_blocks,
            max_frames_per_block: params.max_frames_per_block,
            mask_and_quantization_size: written.mask_and_quantization_size,
            block_duration,
            block_inverse_duration: 1.0 / block_duration,
            frame_duration,
            block_offsets: written.block_offsets,
            float_block_offsets: written.float_block_offsets,
            transform_offsets: Vec::new(),
            float_offsets: Vec::new(),
            data: written.data,
            endian: 0,
        }),
    })
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::unwrap_used,
    reason = "test fixtures built by hand"
)]
mod tests {
    use super::*;
    use crate::spline::{Absent, SplineAnimation};

    const STILL: QsTransform = QsTransform {
        translation: [0.0; 4],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0, 1.0, 1.0, 0.0],
    };

    /// An interleaved animation of `frames` frames of `tracks` tracks at 30 frames a second, track `t` of frame `f`
    /// being `at(f, t)`, and `floats` float tracks of values `float(f, t)`.
    fn source(
        frames: usize,
        tracks: usize,
        at: impl Fn(usize, usize) -> QsTransform,
        floats: usize,
        float: impl Fn(usize, usize) -> f32,
    ) -> Animation {
        let transforms = (0..frames)
            .flat_map(|f| (0..tracks).map(move |t| (f, t)))
            .map(|(f, t)| at(f, t))
            .collect();
        let values = (0..frames)
            .flat_map(|f| (0..floats).map(move |t| (f, t)))
            .map(|(f, t)| float(f, t))
            .collect();
        Animation {
            class: "hkaInterleavedUncompressedAnimation".into(),
            kind: AnimationType::Interleaved,
            duration: frames.saturating_sub(1) as f32 / 30.0,
            transform_tracks: tracks,
            float_tracks: floats,
            extracted_motion: None,
            annotation_tracks: vec![String::new(); tracks],
            tracks: Tracks::Interleaved(InterleavedData {
                transforms,
                floats: values,
            }),
        }
    }

    fn params(a: &Animation) -> EncodeParams {
        EncodeParams::uniform(TrackParams::default(), a.transform_tracks, a.float_tracks, false)
    }

    fn translated(x: f32, y: f32, z: f32) -> QsTransform {
        QsTransform {
            translation: [x, y, z, 0.0],
            ..STILL
        }
    }

    /// The largest distance of a position and of a rotation (as a quaternion, either sign) of any track at any frame
    /// of `encoded`, decoded, from `source`.
    fn worst_error(source: &Animation, encoded: &Animation) -> (f32, f32) {
        let Tracks::Interleaved(src) = &source.tracks else {
            panic!("not interleaved")
        };
        let s = SplineAnimation::parse(encoded).unwrap();
        let back = s
            .interleaved(&vec![STILL; source.transform_tracks], Absent::NEUTRAL)
            .unwrap();
        let mut worst = (0.0f32, 0.0f32);
        for (a, b) in src.transforms.iter().zip(&back.transforms) {
            worst.0 = worst.0.max(spline_fit::distance(a.translation, b.translation));
            let flip = if spline_fit::dot(a.rotation, b.rotation) < 0.0 {
                -1.0
            } else {
                1.0
            };
            worst.1 = worst
                .1
                .max(spline_fit::distance(a.rotation, b.rotation.map(|c| c * flip)));
        }
        worst
    }

    #[test]
    fn constant_parts_are_static_and_neutral_ones_absent() {
        let a = source(
            12,
            2,
            |_, t| {
                if t == 0 {
                    translated(1.0, 2.0, 3.0)
                } else {
                    QsTransform {
                        translation: [0.0; 4],
                        rotation: [0.6, 0.0, 0.0, 0.8],
                        scale: [2.0, 2.0, 2.0, 0.0],
                    }
                }
            },
            0,
            |_, _| 0.0,
        );
        let (blocks, frames) = encode_blocks(&a, &params(&a)).unwrap();
        assert_eq!(frames, 12);
        let [t0, t1] = &blocks.blocks[0].tracks[..] else {
            panic!("two tracks")
        };
        // track 0: a static position; the identity rotation and the unit scale are absent
        assert_eq!(t0.position.flags, 0x07);
        assert_eq!(
            t0.position.components,
            [
                ScalarData::Static(1.0),
                ScalarData::Static(2.0),
                ScalarData::Static(3.0)
            ]
        );
        assert_eq!(
            (t0.rotation.flags, &t0.rotation.value),
            (0x00, &QuaternionData::Absent)
        );
        assert_eq!(t0.scale.flags, 0x00);
        // track 1: a zero position is absent; the rotation is static in x and w; the scale is 2
        assert_eq!(t1.position.flags, 0x00);
        assert_eq!(t1.rotation.flags, 0x09);
        let QuaternionData::Static(q) = t1.rotation.value else {
            panic!("a static rotation")
        };
        let q = q.unpack();
        assert!(spline_fit::distance(q, [0.6, 0.0, 0.0, 0.8]) < 1e-3, "{q:?}");
        assert_eq!(t1.scale.flags, 0x07);
        assert_eq!(t1.scale.components[0], ScalarData::Static(2.0));
        // nothing needed a spline
        assert!(
            blocks
                .blocks
                .iter()
                .flat_map(|b| &b.tracks)
                .all(|t| t.position.knots.is_none() && t.scale.knots.is_none())
        );
    }

    /// A float track is never absent: zero is a static zero (mask `0x03`); a moving one is a 16-bit spline (`0x12`).
    #[test]
    fn a_float_track_is_static_or_a_spline() {
        let a = source(10, 1, |_, _| STILL, 1, |_, _| 0.0);
        let (blocks, _) = encode_blocks(&a, &params(&a)).unwrap();
        assert_eq!(blocks.blocks[0].floats, [FloatData::Static(0.0)]);
        let a = source(10, 1, |_, _| STILL, 1, |f, _| f as f32 * 0.5);
        let (blocks, _) = encode_blocks(&a, &params(&a)).unwrap();
        let FloatData::Spline {
            knots,
            min,
            max,
            values,
        } = &blocks.blocks[0].floats[0]
        else {
            panic!("a float spline")
        };
        assert_eq!((*min, *max), (0.0, 4.5));
        assert_eq!((knots.values.first(), knots.values.last()), (Some(&0), Some(&9)));
        assert_eq!((values[0], values[values.len() - 1]), (0, 65535));
        let out = encode(&a, &params(&a)).unwrap();
        let Tracks::Spline(d) = &out.tracks else {
            panic!("spline")
        };
        // the mask area is 4 bytes of transform mask and the float mask byte, padded to 4
        assert_eq!(d.mask_and_quantization_size, 8);
        let s = SplineAnimation::parse(&out).unwrap();
        for f in 0..10 {
            assert!((s.sample_float_frame(0, f).unwrap() - f as f32 * 0.5).abs() < 1e-3);
        }
    }

    /// A position moving at a constant speed is one cubic span: knots only at the ends, four control points a third of
    /// the way apart, every frame back within the quantization.
    #[test]
    fn a_linear_track_needs_only_end_knots() {
        let a = source(20, 1, |f, _| translated(f as f32 * 0.25, 0.0, 0.0), 0, |_, _| 0.0);
        let out = encode(&a, &params(&a)).unwrap();
        let blocks = SplineBlocks::read(&out).unwrap();
        let p = &blocks.blocks[0].tracks[0].position;
        assert_eq!(p.flags, 0x10);
        let knots = p.knots.as_ref().unwrap();
        assert_eq!(knots.degree, 3);
        assert_eq!(knots.values, [0, 0, 0, 0, 19, 19, 19, 19]);
        let ScalarData::Spline { min, max, values } = &p.components[0] else {
            panic!("a spline x")
        };
        assert_eq!((*min, *max), (0.0, 4.75));
        assert_eq!(values.len(), 4);
        for (i, &v) in values.iter().enumerate() {
            let expected = 65535.0 * i as f32 / 3.0;
            assert!((f32::from(v) - expected).abs() <= 1.0, "{i}: {v} {expected}");
        }
        assert!(worst_error(&a, &out).0 < 1e-4);
    }

    /// Samples of a gently curved cubic Bezier segment give that segment back: end knots only and its own control
    /// points. (A strongly curved one keeps a knot at nearly every frame after the degree 1 pass, no least-squares fit
    /// is then well posed, and the result is degree 1: the algorithm's own behaviour, as CompressAnim's.)
    #[test]
    fn a_known_cubic_is_recovered() {
        let control = [0.0f32, 0.03, -0.01, 0.02];
        let bezier = |t: f32| {
            let s = 1.0 - t;
            s * s * s * control[0]
                + 3.0 * s * s * t * control[1]
                + 3.0 * s * t * t * control[2]
                + t * t * t * control[3]
        };
        let a = source(
            31,
            1,
            |f, _| translated(0.0, bezier(f as f32 / 30.0), 0.0),
            0,
            |_, _| 0.0,
        );
        let out = encode(&a, &params(&a)).unwrap();
        let blocks = SplineBlocks::read(&out).unwrap();
        let p = &blocks.blocks[0].tracks[0].position;
        assert_eq!(p.flags, 0x20);
        assert_eq!(p.knots.as_ref().unwrap().values, [0, 0, 0, 0, 30, 30, 30, 30]);
        let ScalarData::Spline { min, max, values } = &p.components[1] else {
            panic!("a spline y")
        };
        assert!(
            (min + 0.01).abs() < 1e-5 && (max - 0.03).abs() < 1e-5,
            "{min} {max}"
        );
        for (&v, &c) in values.iter().zip(&control) {
            let decoded = ScalarQuantization::Bits16.value(*min, *max, v);
            assert!((decoded - c).abs() < 1e-5, "{decoded} {c}");
        }
    }

    /// A spline lane's bounds are its control points' smallest and largest, its integers run from 0 to the top of the
    /// quantization, and the mask names the quantization; 8-bit lanes stay within a byte.
    #[test]
    fn quantization_spans_the_control_points() {
        let wave = |f: usize| {
            let t = f as f32 / 10.0;
            (t * t * t - 3.0 * t * t + 2.0 * t) * 0.5
        };
        let a = source(31, 1, |f, _| translated(wave(f), 0.0, 0.0), 0, |_, _| 0.0);
        for (quantization, top) in [
            (ScalarQuantization::Bits16, 65535u16),
            (ScalarQuantization::Bits8, 255),
        ] {
            let params = EncodeParams::uniform(
                TrackParams {
                    translation_quantization: quantization,
                    ..TrackParams::default()
                },
                1,
                0,
                false,
            );
            let (blocks, _) = encode_blocks(&a, &params).unwrap();
            let p = &blocks.blocks[0].tracks[0].position;
            assert_eq!(p.quantization, quantization);
            let ScalarData::Spline { min, max, values } = &p.components[0] else {
                panic!("a spline x")
            };
            assert!(min < max);
            assert_eq!(values.iter().min(), Some(&0));
            assert_eq!(values.iter().max(), Some(&top));
            let out = encode(&a, &params).unwrap();
            let stored = SplineBlocks::read(&out).unwrap();
            assert_eq!(stored.blocks[0].tracks[0].position.quantization, quantization);
        }
        // an empty range is 0; the top of the range is the top integer; below the range is 0
        assert_eq!(quantize(1.0, 1.0, 1.0, 65535, Flavor::Games), 0);
        assert_eq!(quantize(0.0, 2.0, 1.0, 65535, Flavor::Games), 32767);
        assert_eq!(quantize(0.0, 2.0, 2.0, 65535, Flavor::Games), 65535);
        assert_eq!(quantize(0.0, 2.0, -1.0, 255, Flavor::Games), 0);
    }

    /// 300 frames at 256 a block are two blocks sharing frame 255; at 10 a block, 34. Each block's knots end on its
    /// own last frame, and the decoded frames stay within the tolerance.
    #[test]
    fn frames_are_cut_into_blocks_that_share_their_boundary() {
        let curve = |f: usize| {
            let t = f as f32 / 299.0;
            translated(t * t * 3.0 - t, 0.5 * t, 0.0)
        };
        let a = source(300, 1, |f, _| curve(f), 0, |_, _| 0.0);
        for (per_block, count, last_frames) in [(256u32, 2usize, [255u8, 44]), (10, 34, [9, 2])] {
            let mut p = params(&a);
            p.max_frames_per_block = per_block;
            let out = encode(&a, &p).unwrap();
            let Tracks::Spline(d) = &out.tracks else {
                panic!("spline")
            };
            assert_eq!(d.num_blocks as usize, count);
            assert_eq!((d.num_frames, d.max_frames_per_block), (300, per_block));
            let blocks = SplineBlocks::read(&out).unwrap();
            let last = |b: &BlockData| {
                *b.tracks[0]
                    .position
                    .knots
                    .as_ref()
                    .unwrap()
                    .values
                    .last()
                    .unwrap()
            };
            assert_eq!(last(&blocks.blocks[0]), last_frames[0]);
            assert_eq!(last(blocks.blocks.last().unwrap()), last_frames[1]);
            assert!(worst_error(&a, &out).0 < 1.1e-3);
        }
    }

    /// The header follows the frame count and duration in f32: `frameDuration`, `blockDuration` and its inverse.
    #[test]
    fn the_header_is_computed_in_f32() {
        let a = source(40, 1, |f, _| translated(f as f32, 0.0, 0.0), 0, |_, _| 0.0);
        let out = encode(&a, &params(&a)).unwrap();
        let Tracks::Spline(d) = &out.tracks else {
            panic!("spline")
        };
        let frame = a.duration / 39.0;
        assert_eq!(d.frame_duration.to_bits(), frame.to_bits());
        assert_eq!(d.block_duration.to_bits(), (frame * 255.0).to_bits());
        assert_eq!(
            d.block_inverse_duration.to_bits(),
            (1.0 / (frame * 255.0)).to_bits()
        );
        assert_eq!(d.mask_and_quantization_size, 4);
        assert!(d.transform_offsets.is_empty() && d.float_offsets.is_empty());
        assert_eq!(
            (out.class.as_str(), out.kind),
            ("hkaSplineCompressedAnimation", AnimationType::SplineCompressed)
        );
    }

    /// H65: 16 zero bytes follow the last block when asked for and the last rotation (THREECOMP40, here absent) ends
    /// less than 3 bytes before the block's padded end; never otherwise.
    #[test]
    fn the_guard_follows_the_rule() {
        // four still tracks: 16 bytes of masks, the last (absent) rotation ending where the block does
        let a = source(5, 4, |_, _| STILL, 0, |_, _| 0.0);
        let length = |a: &Animation, p: &EncodeParams| {
            let Tracks::Spline(d) = encode(a, p).unwrap().tracks else {
                panic!("spline")
            };
            d.data.len()
        };
        let mut p = params(&a);
        assert_eq!(length(&a, &p), 16);
        p.guard = true;
        assert_eq!(length(&a, &p), 32);
        assert!(SplineBlocks::read(&encode(&a, &p).unwrap()).unwrap().guard);
        // another rotation quantization: no guard
        let mut uncompressed = p.clone();
        for t in &mut uncompressed.transform {
            t.rotation_quantization = RotationQuantization::Uncompressed;
        }
        assert_eq!(length(&a, &uncompressed), 16);
        // three tracks: the rotation ends 4 bytes before the padded end
        let three = source(5, 3, |_, _| STILL, 0, |_, _| 0.0);
        assert_eq!(
            length(&three, &EncodeParams::uniform(TrackParams::default(), 3, 0, true)),
            16
        );
    }

    fn bits(p: PackedQuaternion) -> u64 {
        let PackedQuaternion::ThreeComp40(b) = p else {
            panic!("40 bits")
        };
        u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], 0, 0, 0])
    }

    /// The packing: the largest component (the last of equal ones) is left out and named with its sign; the others are
    /// truncated toward zero in steps of `1 / (2047 sqrt 2)`; NaN packs as the games' files store it.
    #[test]
    fn quaternions_pack_as_the_games_files_store_them() {
        // the identity: three zeros at 2047, index 3, positive
        let identity = bits(pack(IDENTITY, RotationQuantization::ThreeComp40, Flavor::Games));
        assert_eq!(identity, 0x7FF | 0x7FF << 12 | 0x7FF << 24 | 3 << 36);
        // all four equal: the last is the largest
        assert_eq!(
            (bits(pack([0.5; 4], RotationQuantization::ThreeComp40, Flavor::Games)) >> 36) & 3,
            3
        );
        // the sign of the largest
        let negative = bits(pack(
            [0.0, -1.0, 0.0, 0.0],
            RotationQuantization::ThreeComp40,
            Flavor::Games,
        ));
        assert_eq!(((negative >> 36) & 3, (negative >> 38) & 1), (1, 1));
        // truncation toward zero, on both sides of zero
        let q = normalized([0.1, -0.1, 0.0, 1.0], Flavor::Games);
        let packed = bits(pack(q, RotationQuantization::ThreeComp40, Flavor::Games));
        assert_eq!(packed & 0xFFF, 2047 + (q[0] * THREE_COMP40_SCALE).trunc() as u64);
        assert_eq!(
            (packed >> 12) & 0xFFF,
            2047 - (-q[1] * THREE_COMP40_SCALE).trunc() as u64
        );
        // NaN saturates every component to the most negative 16-bit integer, index 0
        assert_eq!(
            pack([f32::NAN; 4], RotationQuantization::ThreeComp40, Flavor::Games),
            PackedQuaternion::ThreeComp40([0x00, 0x80, 0x00, 0x00, 0x80])
        );
        assert_eq!(
            pack([f32::NAN; 4], RotationQuantization::ThreeComp48, Flavor::Games),
            PackedQuaternion::ThreeComp48([0x8000, 0x8000, 0x8000])
        );
        // a zero control point normalises to NaN
        assert!(normalized([0.0; 4], Flavor::Games).iter().all(|v| v.is_nan()));
        // unit quaternions come back within a few steps of each component
        for i in 0..200 {
            let t = i as f32 * 0.37;
            let q = normalized(
                [
                    t.fract() - 0.5,
                    (t * 1.7).fract() - 0.5,
                    (t * 2.3).fract() - 0.5,
                    (t * 0.9).fract() - 0.5,
                ],
                Flavor::Games,
            );
            for (how, step) in [
                (RotationQuantization::ThreeComp40, 1.0 / THREE_COMP40_SCALE),
                (RotationQuantization::ThreeComp48, 1.0 / THREE_COMP48_SCALE),
            ] {
                let back = pack(q, how, Flavor::Games).unpack();
                assert!(spline_fit::distance(q, back) < 3.0 * step, "{q:?} {back:?}");
            }
            assert_eq!(
                pack(q, RotationQuantization::Uncompressed, Flavor::Games).unpack(),
                q
            );
        }
    }

    /// A smooth animation of several tracks comes back within the tolerance and its quantization.
    #[test]
    fn a_smooth_animation_comes_back_within_the_tolerance() {
        let a = source(
            90,
            3,
            |f, t| {
                let x = f as f32 / 89.0 + t as f32;
                QsTransform {
                    translation: [x * x * 0.2, 1.0 - x * 0.5, 0.25 * t as f32, 0.0],
                    rotation: normalized([0.1 * x, 0.3 - 0.1 * x * x, 0.05, 1.0], Flavor::Games),
                    scale: [1.0, 1.0 + 0.1 * x, 1.0, 0.0],
                }
            },
            0,
            |_, _| 0.0,
        );
        let out = encode(&a, &params(&a)).unwrap();
        let (position, rotation) = worst_error(&a, &out);
        assert!(position < 1.1e-3, "{position}");
        assert!(rotation < 1.5e-3, "{rotation}");
    }

    #[test]
    fn what_cannot_be_encoded_is_refused() {
        let good = source(4, 1, |_, _| STILL, 0, |_, _| 0.0);
        let refused = |a: &Animation, p: &EncodeParams, text: &str| {
            let e = encode(a, p).unwrap_err().to_string();
            assert!(e.contains(text), "{e}");
        };
        let mut a = good.clone();
        a.tracks = Tracks::Unread("x".into());
        refused(&a, &params(&good), "not an interleaved animation");
        let one = source(1, 1, |_, _| STILL, 0, |_, _| 0.0);
        refused(&one, &params(&one), "at least 2");
        let nan = source(
            4,
            1,
            |f, _| translated(if f == 2 { f32::NAN } else { 0.0 }, 0.0, 0.0),
            0,
            |_, _| 0.0,
        );
        refused(&nan, &params(&nan), "not finite");
        refused(
            &good,
            &EncodeParams::uniform(TrackParams::default(), 2, 0, false),
            "parameters for 2",
        );
        let mut p = params(&good);
        p.transform[0].rotation_degree = 4;
        refused(&good, &p, "rotation degree 4");
        let mut p = params(&good);
        p.transform[0].scale_tolerance = -1.0;
        refused(&good, &p, "scale tolerance");
        let two = source(4, 1, |_, _| STILL, 2, |_, _| 0.0);
        refused(&two, &params(&two), "more than one float track");
        let mut p = params(&good);
        p.max_frames_per_block = 1;
        refused(&good, &p, "frames a block");
        let mut a = good.clone();
        a.duration = 0.0;
        refused(&a, &params(&good), "duration");
    }

    /// CompressAnim's command line sets only the rotation quantization and tolerance; the rest is its default.
    #[test]
    fn compress_anim_parameters() {
        let p = TrackParams::compress_anim(RotationQuantization::ThreeComp48, 0.01);
        assert_eq!(p.rotation_quantization, RotationQuantization::ThreeComp48);
        assert_eq!(p.rotation_tolerance, 0.01);
        assert_eq!(
            (p.translation_tolerance, p.scale_tolerance, p.float_tolerance),
            (0.001, 0.001, 0.001)
        );
        assert_eq!(
            (
                p.translation_degree,
                p.rotation_degree,
                p.scale_degree,
                p.float_degree
            ),
            (3, 3, 3, 3)
        );
        assert_eq!(
            (p.translation_quantization, p.scale_quantization),
            (ScalarQuantization::Bits16, ScalarQuantization::Bits16)
        );
    }
}
