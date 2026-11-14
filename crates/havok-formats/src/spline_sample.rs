//! A decoded spline animation sampled at a time: the block and frame a time falls in, each track's components
//! evaluated on its B-spline basis (*The NURBS Book*, algorithms A2.1 and A2.2) in f64 and rounded once to f32, and a
//! skeleton posed by a binding. Decoding is [`crate::spline`]'s (a parse module); this works on its typed values.
//!
//! A [`SplineAnimation`] has public fields, so it need not come from [`SplineAnimation::parse`]: every count and index
//! used here is checked. The values hold no stored bytes, so a refusal here names offset 0.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use crate::animation::{Binding, InterleavedData, QsTransform, Skeleton};
use crate::spline::{
    Absent, Component, Fallback, FloatTrack, Knots, Rotation, SplineAnimation, Track, Vector,
};
use crate::{Result, invalid};

impl SplineAnimation {
    /// The block and the frame within it that `time` seconds is: frames count from 0 at `frameDuration` apart, a block
    /// covering `maxFramesPerBlock - 1` of them; a time outside the animation is its nearest end. Refused when
    /// `time` is not finite, or the animation has no blocks, no frames, fewer than two frames a block or no usable
    /// frame duration.
    pub fn locate(&self, time: f32) -> Result<(usize, f32)> {
        let last_block = self
            .blocks
            .len()
            .checked_sub(1)
            .ok_or_else(|| invalid(0, "the animation has no blocks"))?;
        let last_frame = self
            .num_frames
            .checked_sub(1)
            .ok_or_else(|| invalid(0, "the animation has no frames"))?;
        let per_block = self
            .max_frames_per_block
            .checked_sub(1)
            .filter(|&n| n > 0)
            .ok_or_else(|| {
                invalid(
                    0,
                    format!(
                        "expected at least 2 frames per block, found {}",
                        self.max_frames_per_block
                    ),
                )
            })?;
        if !time.is_finite() {
            return Err(invalid(0, format!("expected a finite time, found {time}")));
        }
        if !self.frame_duration.is_finite() || self.frame_duration <= 0.0 {
            return Err(invalid(
                0,
                format!(
                    "expected a positive frame duration, found {}",
                    self.frame_duration
                ),
            ));
        }
        let frame = (f64::from(time) / f64::from(self.frame_duration)).clamp(0.0, f64::from(last_frame));
        let per_block = f64::from(per_block);
        // a whole number in 0..=last_block, so the conversion is exact
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let block = ((frame / per_block).floor() as usize).min(last_block);
        // the frame within the block is rounded once to f32, as the module documentation says
        #[allow(clippy::cast_possible_truncation, clippy::arithmetic_side_effects)]
        let within = (frame - block as f64 * per_block) as f32;
        Ok((block, within))
    }

    /// A track at `time` seconds; what the track does not state is `absent`'s fallback: `reference` (the bone's
    /// reference transform) or the neutral value.
    pub fn sample(
        &self,
        track: usize,
        time: f32,
        reference: &QsTransform,
        absent: Absent,
    ) -> Result<QsTransform> {
        let (block, frame) = self.locate(time)?;
        let t = self
            .blocks
            .get(block)
            .and_then(|b| b.get(track))
            .ok_or_else(|| invalid(0, format!("no track {track}")))?;
        sample_track(t, frame, reference, absent)
    }

    /// A float track at `time` seconds.
    pub fn sample_float(&self, track: usize, time: f32) -> Result<f32> {
        let (block, frame) = self.locate(time)?;
        float_value(self, block, track, frame)
    }

    /// The block holding frame `frame` (0 to `numFrames - 1`) and the frame within it, counted exactly: a frame two
    /// blocks share is the later block's first, as [`Self::locate`] places it, and the last frame the last block's end.
    pub fn locate_frame(&self, frame: u32) -> Result<(usize, f32)> {
        let last_block = self
            .blocks
            .len()
            .checked_sub(1)
            .ok_or_else(|| invalid(0, "the animation has no blocks"))?;
        let per_block = self
            .max_frames_per_block
            .checked_sub(1)
            .filter(|&n| n > 0)
            .ok_or_else(|| invalid(0, "expected at least 2 frames per block"))?;
        if frame >= self.num_frames {
            return Err(invalid(
                0,
                format!("frame {frame} of an animation of {} frames", self.num_frames),
            ));
        }
        let block = usize::try_from(frame.checked_div(per_block).unwrap_or_default())
            .map_err(|_| invalid(0, "block index"))?
            .min(last_block);
        let first = u32::try_from(block)
            .ok()
            .and_then(|b| b.checked_mul(per_block))
            .ok_or_else(|| invalid(0, "block start"))?;
        let within = frame
            .checked_sub(first)
            .ok_or_else(|| invalid(0, "frame before its block"))?;
        // a frame within a block is below 256, exact in f32
        let within = u16::try_from(within).map_err(|_| invalid(0, "frame within a block"))?;
        Ok((block, f32::from(within)))
    }

    /// A track at frame `frame` (see [`Self::locate_frame`]); what the track does not state is `absent`'s fallback.
    pub fn sample_frame(
        &self,
        track: usize,
        frame: u32,
        reference: &QsTransform,
        absent: Absent,
    ) -> Result<QsTransform> {
        let (block, within) = self.locate_frame(frame)?;
        let t = self
            .blocks
            .get(block)
            .and_then(|b| b.get(track))
            .ok_or_else(|| invalid(0, format!("no track {track}")))?;
        sample_track(t, within, reference, absent)
    }

    /// A float track at frame `frame` (see [`Self::locate_frame`]).
    pub fn sample_float_frame(&self, track: usize, frame: u32) -> Result<f32> {
        let (block, within) = self.locate_frame(frame)?;
        float_value(self, block, track, within)
    }

    /// Every track at every frame, as an interleaved animation stores them: frame by frame, the transform tracks in
    /// order (and the float tracks likewise). `references` holds one transform a transform track, what `absent`'s
    /// [`Fallback::Reference`] stands for (the bone each drives; with [`Absent::NEUTRAL`] it is not read).
    pub fn interleaved(&self, references: &[QsTransform], absent: Absent) -> Result<InterleavedData> {
        let tracks = self.blocks.first().map_or(0, Vec::len);
        let floats = self.floats.first().map_or(0, Vec::len);
        if references.len() != tracks {
            return Err(invalid(
                0,
                format!("{} reference transforms for {tracks} tracks", references.len()),
            ));
        }
        let frames = usize::try_from(self.num_frames).map_err(|_| invalid(0, "frames"))?;
        let mut transforms = Vec::with_capacity(frames.saturating_mul(tracks));
        let mut values = Vec::with_capacity(frames.saturating_mul(floats));
        for frame in 0..self.num_frames {
            for (track, reference) in references.iter().enumerate() {
                transforms.push(self.sample_frame(track, frame, reference, absent)?);
            }
            for track in 0..floats {
                values.push(self.sample_float_frame(track, frame)?);
            }
        }
        Ok(InterleavedData {
            transforms,
            floats: values,
        })
    }

    /// Every float slot of `skeleton` at `time` seconds: the slots a float track of `binding` drives sampled (a track
    /// bound to a negative slot index drives none), the others at the skeleton's reference value. `binding` must be the
    /// one that binds this animation to `skeleton`.
    pub fn floats_at(&self, time: f32, binding: &Binding, skeleton: &Skeleton) -> Result<Vec<f32>> {
        let tracks = self.floats.first().map_or(0, Vec::len);
        if binding.float_track_to_slot.len() != tracks {
            return Err(invalid(
                0,
                format!(
                    "the binding has {} float tracks, the animation {tracks}",
                    binding.float_track_to_slot.len()
                ),
            ));
        }
        let mut floats = skeleton.reference_floats.clone();
        for (track, &slot) in binding.float_track_to_slot.iter().enumerate() {
            // a negative slot index is a track that drives no slot
            if slot < 0 {
                continue;
            }
            let outside = || {
                invalid(
                    0,
                    format!(
                        "float track {track} drives slot {slot}, outside the skeleton's {} slots",
                        skeleton.reference_floats.len()
                    ),
                )
            };
            let slot = usize::try_from(slot).map_err(|_| outside())?;
            let value = self.sample_float(track, time)?;
            *floats.get_mut(slot).ok_or_else(outside)? = value;
        }
        Ok(floats)
    }

    /// Every bone of `skeleton` at `time` seconds: the bones a track of `binding` drives sampled (a track bound to a
    /// negative bone index drives none), the others at their reference transform. `binding` must be the one that binds this animation to `skeleton`.
    pub fn pose_at(
        &self,
        time: f32,
        binding: &Binding,
        skeleton: &Skeleton,
        absent: Absent,
    ) -> Result<Vec<QsTransform>> {
        let tracks = self.blocks.first().map_or(0, Vec::len);
        if binding.transform_track_to_bone.len() != tracks {
            return Err(invalid(
                0,
                format!(
                    "the binding has {} tracks, the animation {tracks}",
                    binding.transform_track_to_bone.len()
                ),
            ));
        }
        let mut pose = skeleton.reference_pose.clone();
        for (track, &bone) in binding.transform_track_to_bone.iter().enumerate() {
            // a negative bone index is a track that drives no bone
            if bone < 0 {
                continue;
            }
            let outside = || {
                invalid(
                    0,
                    format!(
                        "track {track} drives bone {bone}, outside the skeleton's {} bones",
                        skeleton.reference_pose.len()
                    ),
                )
            };
            let bone = usize::try_from(bone).map_err(|_| outside())?;
            let reference = skeleton.reference_pose.get(bone).ok_or_else(outside)?;
            let sampled = self.sample(track, time, reference, absent)?;
            *pose.get_mut(bone).ok_or_else(outside)? = sampled;
        }
        Ok(pose)
    }
}

/// Float track `track` of block `block` at `frame` within the block.
fn float_value(a: &SplineAnimation, block: usize, track: usize, frame: f32) -> Result<f32> {
    let t = a
        .floats
        .get(block)
        .and_then(|b| b.get(track))
        .ok_or_else(|| invalid(0, format!("no float track {track}")))?;
    match t {
        FloatTrack::Static(v) => Ok(*v),
        FloatTrack::Spline { knots, points } => spline_value(knots, points, frame),
    }
}

fn sample_track(t: &Track, frame: f32, reference: &QsTransform, absent: Absent) -> Result<QsTransform> {
    let position = sample_vector(
        &t.position,
        frame,
        match absent.position {
            Fallback::Reference => reference.translation,
            Fallback::Neutral => [0.0; 4],
        },
    )?;
    let scale = sample_vector(
        &t.scale,
        frame,
        match absent.scale {
            Fallback::Reference => reference.scale,
            Fallback::Neutral => [1.0, 1.0, 1.0, 0.0],
        },
    )?;
    let rotation = match &t.rotation {
        Rotation::Absent => match absent.rotation {
            Fallback::Reference => reference.rotation,
            Fallback::Neutral => [0.0, 0.0, 0.0, 1.0],
        },
        Rotation::Static(q) if q.iter().all(|v| v.is_finite()) => *q,
        Rotation::Static(_) => {
            return Err(invalid(0, "a static rotation is stored as non-finite floats"));
        }
        Rotation::Spline { knots, points } => {
            let at = evaluate(knots, frame, points.len())?;
            let mut q = [0.0f64; 4];
            for (p, w) in &at {
                let c = points
                    .get(*p)
                    .ok_or_else(|| invalid(0, format!("control point {p} of {} is missing", points.len())))?;
                for (sum, component) in q.iter_mut().zip(c) {
                    *sum += f64::from(*component) * w;
                }
            }
            let norm = q.iter().map(|v| v * v).sum::<f64>().sqrt();
            if !norm.is_finite() {
                return Err(invalid(
                    0,
                    "a rotation spline blends a control point that is stored as non-finite floats",
                ));
            }
            if norm == 0.0 {
                return Err(invalid(0, "a rotation spline blends to no rotation"));
            }
            // rounded once to f32
            #[allow(clippy::cast_possible_truncation)]
            q.map(|v| (v / norm) as f32)
        }
    };
    Ok(QsTransform {
        translation: position,
        rotation,
        scale,
    })
}

/// The slot `index` of a basis vector, which the algorithm sizes `degree + 1`.
fn slot(values: &mut [f64], index: usize) -> Result<&mut f64> {
    let len = values.len();
    values
        .get_mut(index)
        .ok_or_else(|| invalid(0, format!("basis index {index} outside {len} entries")))
}

/// The value at `index` of a basis vector (see [`slot`]).
fn value(values: &[f64], index: usize) -> Result<f64> {
    values
        .get(index)
        .copied()
        .ok_or_else(|| invalid(0, format!("basis index {index} outside {} entries", values.len())))
}

/// The control points and weights a B-spline of `knots` over `points` control points has at `frame`
/// (algorithms A2.1 and A2.2 of The NURBS Book): the `degree + 1` points of the knot span the frame lies in. A frame
/// outside the knots is the nearest end. Refused when the knots are too few for `points` and the degree, or fall
/// between the first and last the evaluation reads.
fn evaluate(knots: &Knots, frame: f32, points: usize) -> Result<Vec<(usize, f64)>> {
    let p = usize::from(knots.degree);
    let k = |i: usize| -> Result<f64> {
        knots.values.get(i).map(|&v| f64::from(v)).ok_or_else(|| {
            invalid(
                0,
                format!(
                    "knot {i} outside the {} knots (degree {p}, {points} points)",
                    knots.values.len()
                ),
            )
        })
    };
    let (first, last) = (k(p)?, k(points)?);
    if first > last {
        return Err(invalid(
            0,
            format!("expected knots that do not decrease: knot {p} is {first}, knot {points} is {last}"),
        ));
    }
    let u = f64::from(frame).clamp(first, last);
    // the span: the last i in p..points with knots[i] <= u (the last span holds the end)
    let mut span = p;
    while span.saturating_add(1) < points && k(span.saturating_add(1))? <= u {
        span = span.saturating_add(1);
    }
    let width = p.saturating_add(1);
    let mut n = vec![0.0f64; width];
    let mut left = vec![0.0f64; width];
    let mut right = vec![0.0f64; width];
    *slot(&mut n, 0)? = 1.0;
    // `span >= p >= j` and `r < j`, so none of the differences below is negative
    for j in 1..=p {
        *slot(&mut left, j)? = u - k(span.saturating_add(1).saturating_sub(j))?;
        *slot(&mut right, j)? = k(span.saturating_add(j))? - u;
        let mut saved = 0.0;
        for r in 0..j {
            let right_r = value(&right, r.saturating_add(1))?;
            let left_r = value(&left, j.saturating_sub(r))?;
            let denominator = right_r + left_r;
            let temp = if denominator == 0.0 {
                0.0
            } else {
                value(&n, r)? / denominator
            };
            *slot(&mut n, r)? = saved + right_r * temp;
            saved = left_r * temp;
        }
        *slot(&mut n, j)? = saved;
    }
    let first_point = span.saturating_sub(p);
    Ok(n.into_iter()
        .enumerate()
        .map(|(i, weight)| (first_point.saturating_add(i), weight))
        .collect())
}

/// The value at `frame` of a spline of scalar control points.
fn spline_value(knots: &Knots, points: &[f32], frame: f32) -> Result<f32> {
    let mut sum = 0.0;
    for (p, w) in evaluate(knots, frame, points.len())? {
        let point = points
            .get(p)
            .ok_or_else(|| invalid(0, format!("control point {p} of {} is missing", points.len())))?;
        sum += f64::from(*point) * w;
    }
    // rounded once to f32
    #[allow(clippy::cast_possible_truncation)]
    let rounded = sum as f32;
    Ok(rounded)
}

fn sample_component(c: &Component, knots: Option<&Knots>, frame: f32, default: f32) -> Result<f32> {
    Ok(match c {
        Component::Absent => default,
        Component::Static(v) => *v,
        Component::Spline(points) => {
            let knots = knots.ok_or_else(|| invalid(0, "a spline component without knots"))?;
            spline_value(knots, points, frame)?
        }
    })
}

fn sample_vector(v: &Vector, frame: f32, default: [f32; 4]) -> Result<[f32; 4]> {
    let [dx, dy, dz, dw] = default;
    Ok([
        sample_component(&v.x, v.knots.as_ref(), frame, dx)?,
        sample_component(&v.y, v.knots.as_ref(), frame, dy)?,
        sample_component(&v.z, v.knots.as_ref(), frame, dz)?,
        dw,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::QsTransform;

    fn still() -> Track {
        let none = || Vector {
            knots: None,
            x: Component::Absent,
            y: Component::Absent,
            z: Component::Absent,
        };
        Track {
            position: none(),
            rotation: Rotation::Absent,
            scale: none(),
        }
    }

    /// A hand-built animation (every field is public): `blocks` blocks of one track, with the given counts.
    fn built(blocks: usize, num_frames: u32, max_frames_per_block: u32) -> SplineAnimation {
        SplineAnimation {
            blocks: vec![vec![still()]; blocks],
            floats: vec![Vec::new(); blocks],
            unreadable: Vec::new(),
            duration: 1.0,
            num_frames,
            max_frames_per_block,
            frame_duration: 1.0,
        }
    }

    const REFERENCE: QsTransform = QsTransform {
        translation: [0.0; 4],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0, 1.0, 1.0, 0.0],
    };

    #[test]
    fn locate_finds_the_block_and_the_frame_within_it() {
        let a = built(3, 9, 4);
        assert_eq!(a.locate(0.0).unwrap(), (0, 0.0));
        assert_eq!(a.locate(4.0).unwrap(), (1, 1.0));
        assert_eq!(a.locate(100.0).unwrap(), (2, 2.0));
        assert_eq!(a.locate(-3.0).unwrap(), (0, 0.0));
    }

    #[test]
    fn locate_refuses_counts_that_cannot_locate_anything() {
        for (a, expected) in [
            (built(0, 9, 4), "no blocks"),
            (built(1, 0, 4), "no frames"),
            (built(1, 9, 1), "frames per block"),
            (built(1, 9, 0), "frames per block"),
        ] {
            let error = a.locate(1.0).unwrap_err();
            assert!(error.detail.contains(expected), "{error}");
        }
        let mut a = built(1, 9, 4);
        a.frame_duration = 0.0;
        assert!(a.locate(1.0).is_err());
        assert!(built(1, 9, 4).locate(f32::NAN).is_err());
    }

    #[test]
    fn sampling_a_hand_built_animation_with_no_blocks_is_an_error() {
        let a = built(0, 9, 4);
        assert!(a.sample(0, 1.0, &REFERENCE, Absent::NEUTRAL).is_err());
    }

    #[test]
    fn decreasing_knots_are_an_error() {
        let mut a = built(1, 9, 4);
        let track = a.blocks.first_mut().and_then(|b| b.first_mut()).unwrap();
        // degree 1 over two points: the clamp bounds are knots 1 and 2, which fall from 9 to 0
        track.rotation = Rotation::Spline {
            knots: Knots {
                degree: 1,
                values: vec![9, 9, 0, 0],
            },
            points: vec![[0.0, 0.0, 0.0, 1.0]; 2],
        };
        let error = a.sample(0, 1.0, &REFERENCE, Absent::NEUTRAL).unwrap_err();
        assert!(error.detail.contains("knots"), "{error}");
    }

    #[test]
    fn a_bone_outside_the_skeleton_is_an_error() {
        let a = built(1, 9, 4);
        let skeleton = Skeleton {
            name: String::new(),
            parents: vec![-1],
            bones: vec![crate::animation::Bone {
                name: String::new(),
                lock_translation: false,
            }],
            reference_pose: vec![REFERENCE],
            reference_floats: Vec::new(),
            float_slots: Vec::new(),
        };
        let binding = Binding {
            original_skeleton_name: String::new(),
            animation: None,
            transform_track_to_bone: vec![1],
            float_track_to_slot: Vec::new(),
            partition_indices: Vec::new(),
            blend_hint: 0,
        };
        let error = a.pose_at(0.0, &binding, &skeleton, Absent::NEUTRAL).unwrap_err();
        assert!(error.detail.contains("bone 1"), "{error}");
    }
}
