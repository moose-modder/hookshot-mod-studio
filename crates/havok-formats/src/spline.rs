//! `hkaSplineCompressedAnimation` decoded: the blocks of compressed tracks its `data` holds as typed values
//! ([`SplineAnimation::parse`]) and each track sampled at a time ([`SplineAnimation::pose_at`]) or at every frame
//! ([`SplineAnimation::interleaved`]). Written too: the stored values of the blocks, kept exactly and laid out again
//! ([`SplineBlocks`]), and an interleaved animation compressed ([`encode`]).
//!
//! The layout (the block, mask and track structure, the quantizations, the knot vectors, the spans of time a block
//! covers) was worked out from the open-source readers Smithbox's `SplineCompressedAnimation.cs` and
//! `NewHavokAnimation_SplineCompressed.cs` (an adaptation of HavokLib's `hkaSplineDecompressor`, GPL-3.0) and from
//! game files; no code is taken from either. The spline arithmetic is the standard B-spline basis (Piegl and Tiller,
//! *The NURBS Book*, algorithms A2.1 and A2.2), written in f64 and rounded once to f32.
//!
//! # The data
//!
//! `data` is `numBlocks` blocks at `blockOffsets`, each 16-byte aligned, a block covering `maxFramesPerBlock - 1`
//! frames (adjacent blocks share their boundary frame). A block is, for each transform track in order:
//!
//! - a 4-byte mask: the quantizations (bits 0-1 the position scalars', 2-5 the rotation's, 6-7 the scale scalars'),
//!   then the position, rotation and scale flags, each byte holding in its low nibble the components that are
//!   static (x y z w) and in its high nibble those that are splines;
//! - after all the masks, padding to 4; then for each track a position, a rotation and a scale, each followed by
//!   padding to 4: a vector with a spline component is a spline track (a count, a degree, the knot bytes, padding to 4,
//!   each spline component's bounds and each static component's value as f32, then the control points a point at a
//!   time with the spline components in x y z order, each a quantized scalar between its bounds), a vector with only
//!   static components is those f32s; a rotation with a spline flag is a spline track (a count, a degree, the knots,
//!   padding to the quantization's alignment, the quantized quaternions), one with a static flag is one quantized
//!   quaternion at that alignment;
//! - the float tracks (below), and padding to 16.
//!
//! A component with neither flag is absent, and the caller states what stands in for it ([`Absent`]). Measured on
//! Elden Ring's character animations (1,122,267 tracks of 8,888 animations), an absent rotation is on a bone whose
//! reference rotation is not the identity in 34% of the cases against 77% of all tracks, and an absent position
//! component on a bone whose reference component is not zero in 41%, 4% and 9% of the cases (x, y, z) against 93%, 36%
//! and 44% of all, so the encoder left out zero and the identity, not the reference. A pose read from the running
//! game's memory agreed with the neutral decoding on the bones where the two fallbacks differ (12 of 12 closer to
//! neutral; errors 1e-6 to 1e-5 against 5e-5 to 3e-4 for the reference). Files of a second game (611,903 tracks of 5,843
//! animations) point the same way, more weakly, and were not compared in the running game: of its absent components, the
//! share on a bone whose reference is not neutral is 51.6% (rotation), 59.9%, 6.5% and 19.7% (position x, y, z) against
//! 85.6%, 93.1%, 33.6% and 47.2% of all tracks. So the neutral value is the better default, but the choice is the
//! caller's.
//! A track bound to a negative bone index drives no bone.
//!
//! # Float tracks
//!
//! The mask area is `4 * transform tracks + float tracks` bytes padded to 4: after the transform masks, one mask byte
//! for each float track. The float data begins where the transform data ends (`floatBlockOffsets`) and holds each float
//! track as its mask byte says. The cutscene binders of both games have float tracks, 66 animations in Sekiro and 542 in
//! Elden Ring (1,110 blocks in all), each with exactly one, the field of view of its camera (the binding drives the one
//! float slot `camNNNN_FOV` of the cutscene skeleton, whose reference value is 0). Two mask bytes occur:
//!
//! - `0x03`, static (946 blocks): one f32, for the whole block.
//! - `0x12`, a spline (164 blocks): a count, a degree and the knots as a transform spline has them, padding to 4, the
//!   lowest and highest value as f32, then the 16-bit control points, each a value between the two scaled by
//!   `1 / 65535` (the largest control point of every one of the 164 blocks is 65534 or 65535, never above).
//!
//! Bit `0x01` of the mask is the static flag, `0x10` the spline flag and `0x02` the 16-bit quantization; no other mask
//! byte occurs, so no other is read, nor is more than one float track an animation (what pads between two float tracks is
//! not shown). The float data ends where the block's padding to 16 reaches the next block: none of the 1,110 blocks is
//! followed by more bytes than that, so the 16 bytes of the [`SplineAnimation::parse`] rule are not accepted after a
//! block with float tracks. Neighbouring blocks agree on the value at their shared frame to within 8e-6 (a value of a
//! field of view in radians, 0.12 to 1.75 over the 1,110 blocks).
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use crate::animation::{Animation, Tracks};
use crate::{Result, invalid};

pub use crate::spline_data::{
    BlockData, FloatData, Knots, PackedQuaternion, QuaternionData, RotationData, RotationQuantization,
    ScalarData, ScalarQuantization, SplineBlocks, TrackData, VectorData, WrittenBlocks,
};
pub use crate::spline_encode::{EncodeParams, Flavor, TrackParams, encode, encode_blocks};

/// One component of a vector: stated by the mask or not.
#[derive(Clone, Debug, PartialEq)]
pub enum Component {
    /// Neither static nor spline: the caller's default stands.
    Absent,
    /// A constant, stored as an f32.
    Static(f32),
    /// The control points (decoded to f32), evaluated with the track's knots.
    Spline(Vec<f32>),
}

/// A position or a scale.
#[derive(Clone, Debug, PartialEq)]
pub struct Vector {
    /// Present when any component is a spline.
    pub knots: Option<Knots>,
    /// The x component.
    pub x: Component,
    /// The y component.
    pub y: Component,
    /// The z component.
    pub z: Component,
}

/// A rotation, by what the track's mask states of it.
#[derive(Clone, Debug, PartialEq)]
pub enum Rotation {
    /// Neither flag set: the caller's [`Fallback`] stands (neutral is the identity).
    Absent,
    /// A quaternion `x y z w`, as decoded from its quantization.
    Static([f32; 4]),
    /// Control points evaluated with knots.
    Spline {
        /// The knots the points are evaluated with.
        knots: Knots,
        /// The control points, each a quaternion `x y z w` decoded from its quantization.
        points: Vec<[f32; 4]>,
    },
}

/// What stands in for a component a track does not state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fallback {
    /// Zero for a position, the identity for a rotation, one for a scale.
    Neutral,
    /// The bone's reference transform.
    Reference,
}

/// The fallback for each kind of component; the caller states it (see the module documentation for what the measured
/// files say).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Absent {
    /// What stands in for an absent position component.
    pub position: Fallback,
    /// What stands in for an absent rotation.
    pub rotation: Fallback,
    /// What stands in for an absent scale component.
    pub scale: Fallback,
}

impl Absent {
    /// Zero, identity and one.
    pub const NEUTRAL: Absent = Absent {
        position: Fallback::Neutral,
        rotation: Fallback::Neutral,
        scale: Fallback::Neutral,
    };
    /// The reference transform for all three.
    pub const REFERENCE: Absent = Absent {
        position: Fallback::Reference,
        rotation: Fallback::Reference,
        scale: Fallback::Reference,
    };
}

/// One transform track of one block.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    /// The position, `xyz`.
    pub position: Vector,
    /// The rotation.
    pub rotation: Rotation,
    /// The scale, `xyz`.
    pub scale: Vector,
}

/// One float track of one block (a camera's field of view in the cutscene binders).
#[derive(Clone, Debug, PartialEq)]
pub enum FloatTrack {
    /// One value for the whole block.
    Static(f32),
    /// Control points (decoded to f32), evaluated with the track's knots.
    Spline {
        /// The knots the points are evaluated with.
        knots: Knots,
        /// The control points.
        points: Vec<f32>,
    },
}

/// Something a track stores that is NaN or infinite, which no value can be decoded from.
///
/// - A vector component (a position or scale axis) whose stored value, or the range its quantised control points are
///   scaled between, is not finite (Elden Ring's `c2030/a001_009000` has one, the scale of the leaf bone `R_Egg`, all
///   six stored bounds NaN). Its control points are still skipped, and the component is [`Component::Absent`], so the
///   caller's `absent` stands for it. `point` is `None`.
/// - An uncompressed rotation with a non-finite float: the file stores it, so it is kept as stored in [`Rotation`]
///   (two Elden Ring cutscene animations, `s12_05_0020`'s `cut0040` and `cut0060`, each have one control point of four
///   quiet NaNs in the rotation spline of a wing bone, of 249 and 256 control points). `part` is `rotation` and `point` the
///   control point, or `None` for a static rotation. Sampling where such a point counts is an error, not a rotation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unreadable {
    /// Index of the block.
    pub block: usize,
    /// Index of the transform track within the block.
    pub track: usize,
    /// `position x`, `scale z`, `rotation`, ...
    pub part: &'static str,
    /// The control point of a rotation spline; `None` for a vector component and for a static rotation.
    pub point: Option<usize>,
}

/// A decoded `hkaSplineCompressedAnimation`.
#[derive(Clone, Debug, PartialEq)]
pub struct SplineAnimation {
    /// One list of transform tracks for each block.
    pub blocks: Vec<Vec<Track>>,
    /// One list of float tracks for each block (empty lists when the animation has none).
    pub floats: Vec<Vec<FloatTrack>>,
    /// The vector components whose stored value or quantisation range is not finite, which [`Self::sample`] takes as
    /// absent, and the uncompressed rotations that store a non-finite float, which it refuses where they count.
    pub unreadable: Vec<Unreadable>,
    /// The animation's `duration` in seconds.
    pub duration: f32,
    /// `numFrames`.
    pub num_frames: u32,
    /// `maxFramesPerBlock`.
    pub max_frames_per_block: u32,
    /// `frameDuration` in seconds.
    pub frame_duration: f32,
}

impl SplineAnimation {
    /// Decode the blocks of `animation` (which must be a spline compressed one, little endian, with at most one float
    /// track). Each block must end where the data says the next thing begins: the float data at `floatBlockOffsets`,
    /// then, after padding to 16, the next block or the end of the data. The stored values are
    /// [`SplineBlocks::read`]'s; this decodes them ([`SplineAnimation::decode`]).
    pub fn parse(animation: &Animation) -> Result<SplineAnimation> {
        let Tracks::Spline(s) = &animation.tracks else {
            return Err(invalid(
                0,
                format!("{} is not a spline compressed animation", animation.class),
            ));
        };
        let stored = SplineBlocks::read(animation)?;
        Ok(SplineAnimation::decode(
            &stored,
            animation.duration,
            s.num_frames,
            s.max_frames_per_block,
            s.frame_duration,
        ))
    }

    /// The decoded values of stored blocks, with the header values a sample needs: control points scaled between
    /// their bounds, quaternions unpacked. A non-finite static value or spline bound of a vector component makes that
    /// component absent, and a non-finite quaternion is kept; both are named in [`SplineAnimation::unreadable`].
    pub fn decode(
        stored: &SplineBlocks,
        duration: f32,
        num_frames: u32,
        max_frames_per_block: u32,
        frame_duration: f32,
    ) -> SplineAnimation {
        let mut unreadable = Vec::new();
        let mut blocks = Vec::with_capacity(stored.blocks.len());
        let mut floats = Vec::with_capacity(stored.blocks.len());
        for (b, block) in stored.blocks.iter().enumerate() {
            let mut tracks = Vec::with_capacity(block.tracks.len());
            for (t, track) in block.tracks.iter().enumerate() {
                let mut name = |part: &'static str, point: Option<usize>| {
                    unreadable.push(Unreadable {
                        block: b,
                        track: t,
                        part,
                        point,
                    });
                };
                let position = vector(
                    &track.position,
                    ["position x", "position y", "position z"],
                    &mut name,
                );
                let rotation = rotation(&track.rotation, &mut name);
                let scale = vector(&track.scale, ["scale x", "scale y", "scale z"], &mut name);
                tracks.push(Track {
                    position,
                    rotation,
                    scale,
                });
            }
            blocks.push(tracks);
            floats.push(block.floats.iter().map(float_track).collect());
        }
        SplineAnimation {
            blocks,
            floats,
            unreadable,
            duration,
            num_frames,
            max_frames_per_block,
            frame_duration,
        }
    }
}

/// A stored vector decoded: a non-finite static value or spline bound makes its component absent, named by `bad`.
fn vector(
    v: &VectorData,
    parts: [&'static str; 3],
    bad: &mut impl FnMut(&'static str, Option<usize>),
) -> Vector {
    let mut decoded = [Component::Absent, Component::Absent, Component::Absent];
    for ((out, stored), part) in decoded.iter_mut().zip(&v.components).zip(parts) {
        *out = match stored {
            ScalarData::Static(value) if value.is_finite() => Component::Static(*value),
            ScalarData::Spline { min, max, values } if min.is_finite() && max.is_finite() => {
                Component::Spline(
                    values
                        .iter()
                        .map(|&q| v.quantization.value(*min, *max, q))
                        .collect(),
                )
            }
            ScalarData::Static(_) | ScalarData::Spline { .. } => {
                bad(part, None);
                Component::Absent
            }
            ScalarData::Absent => Component::Absent,
        };
    }
    let [x, y, z] = decoded;
    Vector {
        knots: v.knots.clone(),
        x,
        y,
        z,
    }
}

/// A stored rotation decoded. A quaternion stored as non-finite floats (only an uncompressed one can be) is kept as
/// stored and named by `bad`: the control point of a spline, or `None` for a static rotation (see [`Unreadable`]).
fn rotation(r: &RotationData, bad: &mut impl FnMut(&'static str, Option<usize>)) -> Rotation {
    match &r.value {
        QuaternionData::Absent => Rotation::Absent,
        QuaternionData::Static(q) => {
            let q = q.unpack();
            if !q.iter().all(|v| v.is_finite()) {
                bad("rotation", None);
            }
            Rotation::Static(q)
        }
        QuaternionData::Spline { knots, points } => {
            let points: Vec<[f32; 4]> = points.iter().map(PackedQuaternion::unpack).collect();
            for (i, p) in points.iter().enumerate() {
                if !p.iter().all(|v| v.is_finite()) {
                    bad("rotation", Some(i));
                }
            }
            Rotation::Spline {
                knots: knots.clone(),
                points,
            }
        }
    }
}

fn float_track(f: &FloatData) -> FloatTrack {
    match f {
        FloatData::Static(v) => FloatTrack::Static(*v),
        FloatData::Spline {
            knots,
            min,
            max,
            values,
        } => FloatTrack::Spline {
            knots: knots.clone(),
            points: values
                .iter()
                .map(|&q| ScalarQuantization::Bits16.value(*min, *max, q))
                .collect(),
        },
    }
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    reason = "test fixtures built by hand"
)]
mod tests {
    use super::*;
    use crate::animation::{AnimationType, QsTransform, SplineData};
    use crate::spline_data::{THREE_COMP40_STEP, THREE_COMP48_STEP};

    /// Positions and scales fall back to the reference, rotations to the identity.
    const MIXED: Absent = Absent {
        position: Fallback::Reference,
        rotation: Fallback::Neutral,
        scale: Fallback::Reference,
    };

    #[test]
    fn three_component_steps_are_the_nearest_f32_to_their_exact_values() {
        for (step, levels) in [(THREE_COMP48_STEP, 16383.0), (THREE_COMP40_STEP, 2047.0)] {
            let exact = 1.0 / (levels * std::f64::consts::SQRT_2);
            let below = f64::from(f32::from_bits(step.to_bits() - 1));
            let above = f64::from(f32::from_bits(step.to_bits() + 1));
            let error = (f64::from(step) - exact).abs();
            assert!(
                error <= (below - exact).abs() && error <= (above - exact).abs(),
                "{step}"
            );
        }
    }

    const IDENTITY: QsTransform = QsTransform {
        translation: [1.0, 2.0, 3.0, 0.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0, 1.0, 1.0, 0.0],
    };

    fn animation(
        data: Vec<u8>,
        tracks: usize,
        blocks: &[(u32, u32)],
        per_block: u32,
        frames: u32,
    ) -> Animation {
        Animation {
            class: "hkaSplineCompressedAnimation".into(),
            kind: AnimationType::SplineCompressed,
            duration: (frames - 1) as f32,
            transform_tracks: tracks,
            float_tracks: 0,
            extracted_motion: None,
            annotation_tracks: vec![String::new(); tracks],
            tracks: Tracks::Spline(SplineData {
                num_frames: frames,
                num_blocks: blocks.len() as u32,
                max_frames_per_block: per_block,
                mask_and_quantization_size: (tracks * 4) as u32,
                block_duration: (per_block - 1) as f32,
                block_inverse_duration: 1.0 / (per_block - 1) as f32,
                frame_duration: 1.0,
                block_offsets: blocks.iter().map(|b| b.0).collect(),
                float_block_offsets: blocks.iter().map(|b| b.1).collect(),
                transform_offsets: vec![],
                float_offsets: vec![],
                data,
                endian: 0,
            }),
        }
    }

    fn f(v: f32) -> [u8; 4] {
        v.to_le_bytes()
    }

    fn pad(data: &mut Vec<u8>, align: usize) {
        while !data.len().is_multiple_of(align) {
            data.push(0);
        }
    }

    /// One block of one track with a static position (x and z), an uncompressed static rotation and no scale.
    fn static_block(x: f32, z: f32) -> Vec<u8> {
        // quantization: rotation 5 (uncompressed); position x z static; rotation static; scale absent
        let mut d = vec![5 << 2, 0x01 | 0x04, 0x01, 0x00];
        d.extend(f(x));
        d.extend(f(z));
        pad(&mut d, 4);
        d.extend([f(0.0), f(1.0), f(0.0), f(0.0)].concat());
        d
    }

    #[test]
    fn a_static_track_states_what_the_mask_names_and_the_reference_stands_for_the_rest() {
        let mut d = static_block(10.0, 30.0);
        let end = d.len() as u32;
        pad(&mut d, 16);
        let a = animation(d, 1, &[(0, end)], 256, 2);
        let s = SplineAnimation::parse(&a).unwrap();
        let t = s.sample(0, 0.0, &IDENTITY, MIXED).unwrap();
        // x and z from the file, y from the reference; the rotation as stored; the scale the reference's
        assert_eq!(t.translation, [10.0, 2.0, 30.0, 0.0]);
        assert_eq!(t.rotation, [0.0, 1.0, 0.0, 0.0]);
        assert_eq!(t.scale, [1.0, 1.0, 1.0, 0.0]);
    }

    /// A stored value that is NaN is that component's own: it reads as absent (the caller's `absent` stands for it) and
    /// is named in `unreadable`, where it used to refuse the whole animation (Elden Ring's `c2030/a001_009000`).
    #[test]
    fn a_nan_component_is_read_as_absent_and_named() {
        let mut d = static_block(f32::NAN, 5.0);
        let end = d.len() as u32;
        pad(&mut d, 16);
        let a = animation(d, 1, &[(0, end)], 256, 2);
        let s = SplineAnimation::parse(&a).unwrap();
        assert_eq!(
            s.unreadable,
            [Unreadable {
                block: 0,
                track: 0,
                part: "position x",
                point: None
            }]
        );
        let t = s.sample(0, 0.0, &IDENTITY, Absent::NEUTRAL).unwrap();
        // x absent (neutral zero), z as stored
        assert_eq!(t.translation, [0.0, 0.0, 5.0, 0.0]);
        // an infinite one too, and a finite animation has none
        let mut d = static_block(f32::INFINITY, 0.0);
        let end = d.len() as u32;
        pad(&mut d, 16);
        assert_eq!(
            SplineAnimation::parse(&animation(d, 1, &[(0, end)], 256, 2))
                .unwrap()
                .unreadable
                .len(),
            1
        );
        let mut d = static_block(1.0, 2.0);
        let end = d.len() as u32;
        pad(&mut d, 16);
        assert!(
            SplineAnimation::parse(&animation(d, 1, &[(0, end)], 256, 2))
                .unwrap()
                .unreadable
                .is_empty()
        );
    }

    /// A block that ends elsewhere than the data says is refused naming both places.
    #[test]
    fn a_block_must_end_where_the_float_data_begins() {
        let mut d = static_block(0.0, 0.0);
        let end = d.len() as u32;
        pad(&mut d, 16);
        let a = animation(d.clone(), 1, &[(0, end + 4)], 256, 2);
        let e = SplineAnimation::parse(&a).unwrap_err().to_string();
        assert!(e.contains("float data begins"), "{e}");
        let a = animation(d, 1, &[(0, end)], 256, 2);
        assert!(SplineAnimation::parse(&a).is_ok());
    }

    /// `tracks` tracks whose masks state nothing but the rotation quantization `code`: a block of `4 * tracks` bytes.
    fn empty_tracks(tracks: usize, code: u8) -> Vec<u8> {
        (0..tracks).flat_map(|_| [code << 2, 0, 0, 0]).collect()
    }

    /// The animation of one block of `block` (its float data at the block's end) followed by `after`, and one more
    /// block of the same tracks after it when `second` is set.
    fn with_trailing(block: &[u8], after: &[u8], second: bool, tracks: usize) -> Result<SplineAnimation> {
        let mut d = block.to_vec();
        d.extend(after);
        let end = block.len() as u32;
        let mut blocks = vec![(0, end)];
        if second {
            blocks.push((d.len() as u32, end));
            d.extend(block);
        }
        let frames = 2 + second as u32 * 2;
        SplineAnimation::parse(&animation(d, tracks, &blocks, 3, frames))
    }

    /// The 16 zero bytes the 2018.1 encoder adds after a last block whose last track's rotation is 40 bits and ends
    /// within two bytes of the block's padded end, and the cases of the rule that has them accepted (nothing else is).
    #[test]
    fn sixteen_zero_bytes_are_accepted_after_a_last_block_by_the_rule_and_nowhere_else() {
        // four tracks of absent components: the masks fill 16 bytes, the (absent) last rotation ends where the block
        // does, on a 16-byte boundary
        let code1 = empty_tracks(4, 1);
        assert!(
            with_trailing(&code1, &[], false, 4).is_ok(),
            "the bytes are not required"
        );
        assert!(with_trailing(&code1, &[0; 16], false, 4).is_ok());
        // the same bytes in the middle of the animation, or a last rotation of another quantization, or less or
        // more or other bytes
        assert!(
            with_trailing(&code1, &[0; 16], true, 4).is_err(),
            "not a last block"
        );
        for code in [2, 5] {
            assert!(
                with_trailing(&empty_tracks(4, code), &[0; 16], false, 4).is_err(),
                "code {code}"
            );
        }
        for after in [vec![0u8; 8], vec![0u8; 32], {
            let mut v = vec![0u8; 16];
            v[5] = 1;
            v
        }] {
            assert!(with_trailing(&code1, &after, false, 4).is_err(), "{after:?}");
        }
        // three tracks: the masks end at 12 and the block pads to 16, four bytes past the last rotation's end
        assert!(
            with_trailing(&empty_tracks(3, 1), &[0; 16], false, 3).is_err(),
            "the rotation ends 4 bytes before the padded end"
        );
        // only the last track's rotation counts
        let mut mixed = empty_tracks(4, 5);
        mixed[0] = 1 << 2;
        assert!(with_trailing(&mixed, &[0; 16], false, 4).is_err());
    }

    /// The rule counts the end of a rotation that is stored: a position x, then a 3-point spline rotation of 40-bit
    /// quaternions (23 bytes) ends at 31, one byte before the padded end 32.
    #[test]
    fn a_stored_last_rotation_ends_the_rule_where_it_ends() {
        let mut d = vec![1 << 2, 0x01, 0x10, 0x00];
        d.extend(f(1.0)); // position x, static
        d.extend(2i16.to_le_bytes());
        d.push(1);
        d.extend([0, 0, 1, 2, 2]); // 3 points + 1 + 1 knots
        d.extend([0x7F; 15]); // 3 quaternions of 5 bytes
        assert_eq!(d.len(), 31);
        let end = d.len() as u32;
        pad(&mut d, 4);
        assert_eq!(d.len() as u32, 32);
        let mut with_extra = d.clone();
        with_extra.extend([0; 16]);
        for data in [d, with_extra] {
            let a = animation(data, 1, &[(0, end + 1)], 256, 2);
            assert!(SplineAnimation::parse(&a).is_ok());
        }
    }

    /// The header's mask area is the masks of the tracks, padded to 4.
    #[test]
    fn the_mask_size_the_header_states_must_be_the_tracks() {
        let mut d = static_block(1.0, 2.0);
        let end = d.len() as u32;
        pad(&mut d, 16);
        for stated in [0u32, 3, 8] {
            let mut a = animation(d.clone(), 1, &[(0, end)], 256, 2);
            if let Tracks::Spline(s) = &mut a.tracks {
                s.mask_and_quantization_size = stated;
            }
            let e = SplineAnimation::parse(&a).unwrap_err().to_string();
            assert!(e.contains("maskAndQuantizationSize"), "{e}");
        }
        assert!(SplineAnimation::parse(&animation(d, 1, &[(0, end)], 256, 2)).is_ok());
    }

    /// A position spline of `degree` over `points` control points: x a spline, nothing else.
    fn degree_block(degree: u8, points: usize) -> Vec<u8> {
        let mut d = vec![1 << 2, 0x10, 0x00, 0x00];
        d.extend(((points - 1) as i16).to_le_bytes());
        d.push(degree);
        d.extend(vec![0; points + degree as usize + 1]); // knots
        pad(&mut d, 4);
        d.extend(f(0.0));
        d.extend(f(1.0)); // x bounds
        d.extend(vec![0; points]); // control points
        pad(&mut d, 4);
        d
    }

    /// Degrees 1, 2 and 3 are the SDK's; no spline track of the games' files has another.
    #[test]
    fn a_spline_track_has_degree_one_to_three() {
        for (degree, ok) in [
            (0, false),
            (1, true),
            (2, true),
            (3, true),
            (4, false),
            (5, false),
        ] {
            let mut d = degree_block(degree, 6);
            let end = d.len() as u32;
            pad(&mut d, 16);
            let a = animation(d, 1, &[(0, end)], 256, 2);
            let result = SplineAnimation::parse(&a);
            assert_eq!(result.is_ok(), ok, "degree {degree}: {result:?}");
            if !ok {
                assert!(
                    result
                        .unwrap_err()
                        .to_string()
                        .contains(&format!("degree {degree}")),
                    "degree {degree}"
                );
            }
        }
    }

    /// A position spline of degree 1 over three control points, x 8-bit between 0 and 10, y static 7.5, z absent: the
    /// control points 0, 10, 0 at knots [0 0 5 10 10] are a ramp up over frames 0 to 5 and down over 5 to 10.
    #[test]
    fn a_spline_position_is_evaluated_between_its_knots() {
        // the position flags: x spline, y static, z nothing; the rotation (absent) is quantized as code 1
        let mut d = vec![1 << 2, 0x10 | 0x02, 0x00, 0x00];
        pad(&mut d, 4);
        d.extend(2i16.to_le_bytes());
        d.push(1);
        d.extend([0, 0, 5, 10, 10]);
        pad(&mut d, 4);
        d.extend(f(0.0));
        d.extend(f(10.0)); // x bounds
        d.extend(f(7.5)); // y static (the y flag is static)
        d.extend([0, 255, 0]); // x control points
        pad(&mut d, 4);
        let end = d.len() as u32;
        pad(&mut d, 16);
        let a = animation(d, 1, &[(0, end)], 256, 11);
        let s = SplineAnimation::parse(&a).unwrap();
        let at = |t: f32| s.sample(0, t, &IDENTITY, MIXED).unwrap().translation;
        assert_eq!(at(0.0), [0.0, 7.5, 3.0, 0.0]);
        assert_eq!(at(2.5), [5.0, 7.5, 3.0, 0.0]);
        assert_eq!(at(5.0), [10.0, 7.5, 3.0, 0.0]);
        assert_eq!(at(7.5), [5.0, 7.5, 3.0, 0.0]);
        assert_eq!(at(10.0), [0.0, 7.5, 3.0, 0.0]);
        // past the end is the end
        assert_eq!(at(99.0), [0.0, 7.5, 3.0, 0.0]);
    }

    /// The identity in three 12-bit components and the index 3 (w), and in three 15-bit ones, as the formulas state
    /// them.
    #[test]
    fn the_quantized_quaternions_decode_by_their_bit_layouts() {
        let c: u64 = 0x7FF | (0x7FF << 12) | (0x7FF << 24) | (3 << 36);
        let five = |c: u64| PackedQuaternion::ThreeComp40(c.to_le_bytes()[..5].try_into().unwrap());
        assert_eq!(five(c).unpack(), [0.0, 0.0, 0.0, 1.0]);
        // the sign bit makes w -1
        assert_eq!(five(c | (1 << 38)).unpack(), [0.0, 0.0, 0.0, -1.0]);
        // 15-bit components at 0x3FFF are zero: the index is bit 15 of x and of y (x low, y high); index 0 is x
        assert_eq!(
            PackedQuaternion::ThreeComp48([0x3FFF, 0x3FFF, 0x3FFF]).unpack(),
            [1.0, 0.0, 0.0, 0.0]
        );
        // both bits set is index 3 (w); the sign bit of z negates it
        assert_eq!(
            PackedQuaternion::ThreeComp48([0xBFFF, 0xBFFF, 0xBFFF]).unpack(),
            [0.0, 0.0, 0.0, -1.0]
        );
    }

    /// Two blocks of two frames each (maxFramesPerBlock 3 gives 2 frames a block): a time picks the block that covers
    /// it, and the last frame is the last block's end.
    #[test]
    fn a_time_is_located_in_its_block() {
        let mut d = static_block(1.0, 0.0);
        let first_end = d.len() as u32;
        pad(&mut d, 16);
        let second = d.len() as u32;
        d.extend(static_block(2.0, 0.0));
        pad(&mut d, 16);
        // each block's float offset counts from the block's own start
        let a = animation(d, 1, &[(0, first_end), (second, first_end)], 3, 5);
        let s = SplineAnimation::parse(&a).unwrap();
        assert_eq!(s.locate(0.5).unwrap(), (0, 0.5));
        assert_eq!(s.locate(1.9).unwrap(), (0, 1.9));
        assert_eq!(s.locate(2.0).unwrap(), (1, 0.0));
        assert_eq!(s.locate(4.0).unwrap(), (1, 2.0));
        assert_eq!(s.locate(99.0).unwrap(), (1, 2.0));
        assert_eq!(s.sample(0, 1.5, &IDENTITY, MIXED).unwrap().translation[0], 1.0);
        assert_eq!(s.sample(0, 3.0, &IDENTITY, MIXED).unwrap().translation[0], 2.0);
    }

    #[test]
    fn what_is_not_read_is_refused_by_name() {
        // rotation quantizations 0 (POLAR32), 3 (THREECOMP24) and 4 (STRAIGHT16)
        for (code, name) in [(0u8, "POLAR32"), (3, "THREECOMP24"), (4, "STRAIGHT16")] {
            let mut d = vec![code << 2, 0, 0x01, 0];
            d.extend([0, 0, 0, 0]);
            pad(&mut d, 4);
            let end = d.len() as u32;
            pad(&mut d, 16);
            let a = animation(d, 1, &[(0, end)], 256, 2);
            let e = SplineAnimation::parse(&a).unwrap_err().to_string();
            assert!(e.contains(name), "{e}");
        }
        // two float tracks: what pads between them is not shown
        let mut a = animation(vec![0; 16], 1, &[(0, 4)], 256, 2);
        a.float_tracks = 2;
        assert!(
            SplineAnimation::parse(&a)
                .unwrap_err()
                .to_string()
                .contains("2 float tracks")
        );
    }

    /// One block of one transform track (a static position x, an uncompressed static rotation, no scale) and one float
    /// track whose mask byte is `mask` and data `float`. Returns the data (not padded to 16) and the float data's
    /// offset.
    fn float_block(mask: u8, float: &[u8]) -> (Vec<u8>, u32) {
        // the mask area is the transform mask and the float mask byte, padded to 4: 8 bytes
        let mut d = vec![5 << 2, 0x01, 0x01, 0x00, mask, 0, 0, 0];
        d.extend(f(4.0)); // position x
        d.extend([f(0.0), f(0.0), f(0.0), f(1.0)].concat()); // rotation
        let at = d.len() as u32;
        d.extend(float);
        (d, at)
    }

    /// An animation of `blocks` (each its data from `float_block`, padded to 16), with one float track.
    fn float_animation(blocks: &[(u8, Vec<u8>)], frames: u32) -> Animation {
        let mut data = Vec::new();
        let mut offsets = Vec::new();
        for (mask, float) in blocks {
            let (d, at) = float_block(*mask, float);
            offsets.push((data.len() as u32, at));
            data.extend(d);
            pad(&mut data, 16);
        }
        let mut a = animation(data, 1, &offsets, 3, frames);
        a.float_tracks = 1;
        if let Tracks::Spline(s) = &mut a.tracks {
            s.mask_and_quantization_size = 8;
        }
        a
    }

    /// A float spline (mask `0x12`) of 16-bit control points between `low` and `high`, degree 1, knots 0 0 5 10 10.
    fn float_spline(low: f32, high: f32, points: [u16; 3]) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend(2i16.to_le_bytes());
        d.push(1);
        d.extend([0, 0, 5, 10, 10]);
        pad(&mut d, 4);
        d.extend(f(low));
        d.extend(f(high));
        d.extend(points.iter().flat_map(|p| p.to_le_bytes()));
        d
    }

    #[test]
    fn a_static_float_track_is_one_value_a_block() {
        let a = float_animation(&[(0x03, f(0.75).to_vec())], 2);
        let s = SplineAnimation::parse(&a).unwrap();
        assert_eq!(s.floats, [[FloatTrack::Static(0.75)]]);
        // the transform tracks are read as without the float track
        assert_eq!(s.blocks.len(), 1);
        assert_eq!(s.sample_float(0, 0.0).unwrap(), 0.75);
        assert_eq!(s.sample_float(0, 99.0).unwrap(), 0.75);
        assert!(s.sample_float(1, 0.0).is_err());
        let t = s.sample(0, 0.0, &IDENTITY, MIXED).unwrap();
        assert_eq!(t.translation, [4.0, 2.0, 3.0, 0.0]);
    }

    /// Control points 0, 65535, 0 between 1 and 3 are the values 1, 3, 1: a ramp up over frames 0 to 5 and down over 5
    /// to 10, the 16-bit scale being 1 / 65535.
    #[test]
    fn a_float_spline_is_16_bit_control_points_between_its_bounds() {
        let a = float_animation(&[(0x12, float_spline(1.0, 3.0, [0, 65535, 0]))], 11);
        let s = SplineAnimation::parse(&a).unwrap();
        let Some(FloatTrack::Spline { knots, points }) = s.floats.first().and_then(|b| b.first()) else {
            panic!("{:?}", s.floats);
        };
        assert_eq!(knots.degree, 1);
        assert_eq!(knots.values, [0, 0, 5, 10, 10]);
        assert_eq!(points, &[1.0, 3.0, 1.0]);
        let at = |t: f32| s.sample_float(0, t).unwrap();
        assert_eq!(
            (at(0.0), at(2.5), at(5.0), at(7.5), at(10.0)),
            (1.0, 2.0, 3.0, 2.0, 1.0)
        );
        // a control point is a 65535th of the range
        let a = float_animation(&[(0x12, float_spline(0.0, 65535.0, [1, 2, 65534]))], 11);
        let s = SplineAnimation::parse(&a).unwrap();
        assert_eq!(
            s.floats,
            [[FloatTrack::Spline {
                knots: Knots {
                    degree: 1,
                    values: vec![0, 0, 5, 10, 10]
                },
                points: vec![1.0, 2.0, 65534.0]
            }]]
        );
    }

    /// Two blocks (maxFramesPerBlock 3 gives two frames a block) each with its float track; a time picks the block.
    #[test]
    fn the_float_track_of_the_block_a_time_is_in_answers() {
        let a = float_animation(&[(0x03, f(1.0).to_vec()), (0x03, f(2.0).to_vec())], 5);
        let s = SplineAnimation::parse(&a).unwrap();
        assert_eq!(s.floats.len(), 2);
        assert_eq!(s.sample_float(0, 1.5).unwrap(), 1.0);
        assert_eq!(s.sample_float(0, 3.0).unwrap(), 2.0);
    }

    /// Only the two mask bytes the games' files have are read, each as the file lays it out.
    #[test]
    fn a_float_mask_byte_other_than_the_two_that_occur_is_refused() {
        for mask in [0x00u8, 0x01, 0x02, 0x10, 0x11, 0x13, 0x22, 0x32, 0x83] {
            let a = float_animation(&[(mask, f(1.0).to_vec())], 2);
            let e = SplineAnimation::parse(&a).unwrap_err().to_string();
            assert!(
                e.contains("float track 0") && e.contains(&format!("{mask:#04x}")),
                "{mask:#04x}: {e}"
            );
        }
    }

    #[test]
    fn a_float_track_stores_finite_values_and_ranges() {
        for float in [f(f32::NAN).to_vec(), f(f32::INFINITY).to_vec()] {
            let a = float_animation(&[(0x03, float)], 2);
            let e = SplineAnimation::parse(&a).unwrap_err().to_string();
            assert!(e.contains("not finite"), "{e}");
        }
        for (low, high) in [(f32::NAN, 1.0), (0.0, f32::INFINITY)] {
            let a = float_animation(&[(0x12, float_spline(low, high, [0, 1, 2]))], 11);
            let e = SplineAnimation::parse(&a).unwrap_err().to_string();
            assert!(e.contains("not finite"), "{e}");
        }
    }

    /// The header's mask area is `4 * transform tracks + float tracks`, padded to 4.
    #[test]
    fn the_mask_size_counts_the_float_tracks() {
        for (stated, ok) in [(4u32, false), (5, false), (8, true), (12, false)] {
            let mut a = float_animation(&[(0x03, f(1.0).to_vec())], 2);
            if let Tracks::Spline(s) = &mut a.tracks {
                s.mask_and_quantization_size = stated;
            }
            assert_eq!(SplineAnimation::parse(&a).is_ok(), ok, "{stated}");
        }
        // five masks of float tracks would be 8 bytes too, but a track count is the animation's
        let mut a = float_animation(&[(0x03, f(1.0).to_vec())], 2);
        a.float_tracks = 0;
        let e = SplineAnimation::parse(&a).unwrap_err().to_string();
        assert!(e.contains("maskAndQuantizationSize 8 disagrees"), "{e}");
    }

    /// The float data ends where the padding to 16 reaches the next block: more bytes are refused, the 16 zero bytes a
    /// 40-bit last rotation can have after a block included (none of the 1,110 blocks with float tracks has them).
    #[test]
    fn nothing_follows_float_data_but_its_padding() {
        let mut a = float_animation(&[(0x03, f(1.0).to_vec())], 2);
        assert!(SplineAnimation::parse(&a).is_ok());
        if let Tracks::Spline(s) = &mut a.tracks {
            s.data.extend([0; 16]);
        }
        let e = SplineAnimation::parse(&a).unwrap_err().to_string();
        assert!(e.contains("the next begins"), "{e}");
    }

    /// The sampled pose of a skeleton's float slots: a track drives the slot the binding names, the others stay at
    /// their reference value.
    #[test]
    fn floats_at_drives_the_slot_a_binding_names() {
        use crate::animation::{Binding, Skeleton};
        let a = float_animation(&[(0x03, f(0.75).to_vec())], 2);
        let s = SplineAnimation::parse(&a).unwrap();
        let skeleton = Skeleton {
            name: String::new(),
            parents: vec![-1],
            bones: Vec::new(),
            reference_pose: vec![IDENTITY],
            reference_floats: vec![0.0, 9.0],
            float_slots: vec!["a".into(), "b".into()],
        };
        let binding = |slot| Binding {
            original_skeleton_name: String::new(),
            animation: None,
            transform_track_to_bone: vec![0],
            float_track_to_slot: vec![slot],
            partition_indices: Vec::new(),
            blend_hint: 0,
        };
        assert_eq!(s.floats_at(0.0, &binding(0), &skeleton).unwrap(), [0.75, 9.0]);
        assert_eq!(s.floats_at(0.0, &binding(1), &skeleton).unwrap(), [0.0, 0.75]);
        assert_eq!(s.floats_at(0.0, &binding(-1), &skeleton).unwrap(), [0.0, 9.0]);
        assert!(s.floats_at(0.0, &binding(2), &skeleton).is_err());
        let mut none = binding(0);
        none.float_track_to_slot.clear();
        assert!(s.floats_at(0.0, &none, &skeleton).is_err());
    }

    /// An uncompressed rotation spline of three control points (degree 1, knots 0 0 5 10 10) whose point `bad` (if any)
    /// is four NaNs, as the games' files store one.
    fn rotation_spline(bad: Option<usize>) -> Animation {
        let mut d = vec![5 << 2, 0, 0x10, 0];
        d.extend(2i16.to_le_bytes());
        d.push(1);
        d.extend([0, 0, 5, 10, 10]);
        pad(&mut d, 4);
        for i in 0..3 {
            let q = if bad == Some(i) {
                [f32::NAN; 4]
            } else {
                [0.0, 0.0, 0.0, 1.0]
            };
            d.extend(q.iter().flat_map(|v| f(*v)));
        }
        let end = d.len() as u32;
        pad(&mut d, 16);
        animation(d, 1, &[(0, end)], 256, 11)
    }

    /// The two Elden Ring cutscene animations whose wing bone stores a control point of four NaNs are data, not a
    /// misread: the point is kept as stored and named, and sampling refuses only where it counts.
    #[test]
    fn a_non_finite_rotation_control_point_is_stored_data() {
        let s = SplineAnimation::parse(&rotation_spline(Some(2))).unwrap();
        assert_eq!(
            s.unreadable,
            [Unreadable {
                block: 0,
                track: 0,
                part: "rotation",
                point: Some(2)
            }]
        );
        let Rotation::Spline { points, .. } = &s.blocks[0][0].rotation else {
            panic!("not a spline");
        };
        assert!(points[2].iter().all(|v| v.is_nan()));
        assert_eq!(points[1], [0.0, 0.0, 0.0, 1.0]);
        // frames 0 to 5 blend points 0 and 1; frames 5 to 10 blend 1 and 2
        assert!(s.sample(0, 2.5, &IDENTITY, MIXED).is_ok());
        let e = s.sample(0, 7.5, &IDENTITY, MIXED).unwrap_err().to_string();
        assert!(e.contains("non-finite"), "{e}");
        // a rotation spline of finite points names nothing
        assert!(
            SplineAnimation::parse(&rotation_spline(None))
                .unwrap()
                .unreadable
                .is_empty()
        );
        // an infinite float is not finite either, and a static rotation is named without a point
        let mut d = vec![5 << 2, 0, 0x01, 0];
        d.extend([f32::INFINITY, 0.0, 0.0, 1.0].iter().flat_map(|v| f(*v)));
        let end = d.len() as u32;
        pad(&mut d, 16);
        let s = SplineAnimation::parse(&animation(d, 1, &[(0, end)], 256, 2)).unwrap();
        assert_eq!(
            s.unreadable,
            [Unreadable {
                block: 0,
                track: 0,
                part: "rotation",
                point: None
            }]
        );
        assert!(s.sample(0, 0.0, &IDENTITY, MIXED).is_err());
    }
}
