//! The Havok animation container of a tagfile (`hkaAnimationContainer`, the class the skeleton and animation files of
//! Sekiro and Elden Ring hold as their one named variant), as typed values: skeletons, animations (their headers and, for a spline
//! compressed one, the compressed data as stored) and the bindings that name which bone each track drives.
//!
//! This is reading: every number is the file's. How a spline compressed animation decodes to bone transforms at a time
//! is not here (its data is kept as stored). Fields are read by the names of the class metadata the file carries
//! (`hkaSkeleton`, `hkaBone`, `hkQsTransform`, `hkaSplineCompressedAnimation`, `hkaDefaultAnimatedReferenceFrame`,
//! `hkaAnimationBinding`); a class or field the file does not have is an error naming it, never a default.

pub use crate::animation_write::replace_animation;
use crate::enums::hka::BlendHint;
use crate::{Result, Tagfile, Value, invalid};

/// `hkQsTransform`: translation, rotation (a quaternion, `x y z w`) and scale, each four floats as stored (the fourth
/// of a translation or scale is padding).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QsTransform {
    /// `translation`: `x y z` and a padding lane.
    pub translation: [f32; 4],
    /// `rotation`: a unit quaternion `x y z w`.
    pub rotation: [f32; 4],
    /// `scale`: `x y z` and a padding lane.
    pub scale: [f32; 4],
}

/// `hkaBone`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bone {
    /// The bone's name.
    pub name: String,
    /// Whether the bone's translation is locked.
    pub lock_translation: bool,
}

/// `hkaSkeleton`.
#[derive(Clone, Debug, PartialEq)]
pub struct Skeleton {
    /// `name`.
    pub name: String,
    /// One a bone: the parent's index, or -1 for a root.
    pub parents: Vec<i16>,
    /// `bones`: one entry a bone, in the order `parents` and `reference_pose` follow.
    pub bones: Vec<Bone>,
    /// One a bone: its local transform at rest.
    pub reference_pose: Vec<QsTransform>,
    /// `referenceFloats`: the rest value of each float slot.
    pub reference_floats: Vec<f32>,
    /// `floatSlots`: the name of each float slot, which the float tracks of a binding drive.
    pub float_slots: Vec<String>,
}

/// `hkaAnimation::AnimationType`, as the file's `type` field, numbered as the 2014 and later SDKs number it (the
/// tagfiles this crate reads); older SDKs number it differently (spline compressed is 5 in 2010). The class name,
/// not this value, decides how an animation is read.
///
/// Over every distinct tagfile, each class stores one value in all its animations: `hkaInterleavedUncompressedAnimation`
/// 1 (Sekiro 2,443, Elden Ring 153), `hkaSplineCompressedAnimation` 3 (Sekiro 11,571, Elden Ring 48,628) and
/// `hkaQuantizedAnimation` 4 (Sekiro 431, Elden Ring none). The values 0, 2, 5 and 6 occur in no file. The names are
/// HKLib's, from its 2018.1 classes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimationType {
    /// Value 0: unknown.
    Unknown,
    /// Value 1: `hkaInterleavedUncompressedAnimation`.
    Interleaved,
    /// Value 2: `hkaMirroredAnimation`.
    Mirrored,
    /// Value 3: `hkaSplineCompressedAnimation`.
    SplineCompressed,
    /// Value 4: `hkaQuantizedAnimation`.
    Quantized,
    /// Value 5: `hkaPredictiveCompressedAnimation`.
    Predictive,
    /// Value 6: `hkaReferencePoseAnimation`, the reference pose as an animation.
    ReferencePose,
    /// A value the SDK's enumeration does not have.
    Other(u32),
}

impl AnimationType {
    fn of(value: u32) -> AnimationType {
        match value {
            0 => AnimationType::Unknown,
            1 => AnimationType::Interleaved,
            2 => AnimationType::Mirrored,
            3 => AnimationType::SplineCompressed,
            4 => AnimationType::Quantized,
            5 => AnimationType::Predictive,
            6 => AnimationType::ReferencePose,
            other => AnimationType::Other(other),
        }
    }
}

/// `hkaDefaultAnimatedReferenceFrame`: the motion of the animation's reference frame (its root motion), sampled.
///
/// Its `frameType` member is not read: Sekiro's type tables mark it as a member that is not serialised (the member
/// flags 0x21 where a stored member has 0x20), and it is 0 in all 27,470 reference frames of Sekiro (6,864) and Elden
/// Ring (20,606), so it carries no data.
#[derive(Clone, Debug, PartialEq)]
pub struct ReferenceFrame {
    /// `up`: the up axis, `x y z` and a padding lane.
    pub up: [f32; 4],
    /// `forward`: the forward axis, `x y z` and a padding lane.
    pub forward: [f32; 4],
    /// `duration` in seconds.
    pub duration: f32,
    /// `x y z` and the rotation about `up` in the fourth, one a sample over `duration`.
    pub samples: Vec<[f32; 4]>,
}

/// What `hkaSplineCompressedAnimation` stores besides its header: the blocks of compressed tracks, as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct SplineData {
    /// `numFrames`: the frames of the whole animation.
    pub num_frames: u32,
    /// `numBlocks`: how many blocks the frames are cut into.
    pub num_blocks: u32,
    /// `maxFramesPerBlock`: the most frames one block holds.
    pub max_frames_per_block: u32,
    /// `maskAndQuantizationSize`, as stored; [`crate::spline`] does not use it.
    pub mask_and_quantization_size: u32,
    /// `blockDuration`: seconds one block spans.
    pub block_duration: f32,
    /// `blockInverseDuration`, as stored (nominally the reciprocal of `block_duration`).
    pub block_inverse_duration: f32,
    /// `frameDuration`: seconds between frames.
    pub frame_duration: f32,
    /// `blockOffsets`: the byte offset in `data` at which each of the `num_blocks` blocks starts (16-byte aligned).
    pub block_offsets: Vec<u32>,
    /// `floatBlockOffsets`: for each block, the byte offset of its float data counted from the block's own start.
    pub float_block_offsets: Vec<u32>,
    /// `transformOffsets`, as stored; [`crate::spline`] does not use it.
    pub transform_offsets: Vec<u32>,
    /// `floatOffsets`, as stored; [`crate::spline`] does not use it.
    pub float_offsets: Vec<u32>,
    /// `data`: the compressed bytes of every block, unmodified.
    pub data: Vec<u8>,
    /// `endian`: 0 for little-endian `data`; [`crate::spline`] refuses any other value.
    pub endian: i32,
}

/// `hkaInterleavedUncompressedAnimation`: the transforms of every track at every sample, track-major within a sample.
#[derive(Clone, Debug, PartialEq)]
pub struct InterleavedData {
    /// `transforms`: `transform_tracks` transforms for each sample.
    pub transforms: Vec<QsTransform>,
    /// `floats`: `float_tracks` values for each sample.
    pub floats: Vec<f32>,
}

/// How an animation stores its tracks.
#[derive(Clone, Debug, PartialEq)]
pub enum Tracks {
    /// A `hkaSplineCompressedAnimation`; see [`crate::spline`] to decode it.
    Spline(SplineData),
    /// A `hkaInterleavedUncompressedAnimation`.
    Interleaved(InterleavedData),
    /// A class this reader does not read the tracks of (its name): the header is still read.
    Unread(String),
}

/// `hkaAnimation` and the class that derives it.
#[derive(Clone, Debug, PartialEq)]
pub struct Animation {
    /// The class of the object (`hkaSplineCompressedAnimation`).
    pub class: String,
    /// `type`.
    pub kind: AnimationType,
    /// `duration` in seconds.
    pub duration: f32,
    /// `numberOfTransformTracks`.
    pub transform_tracks: usize,
    /// `numberOfFloatTracks`.
    pub float_tracks: usize,
    /// `extractedMotion`: the root motion, or `None` when the pointer is null.
    pub extracted_motion: Option<ReferenceFrame>,
    /// The name of each annotation track (one a transform track).
    pub annotation_tracks: Vec<String>,
    /// The tracks, by the object's class.
    pub tracks: Tracks,
}

/// `hkaAnimationBinding`: which bone of a skeleton each track of an animation drives.
#[derive(Clone, Debug, PartialEq)]
pub struct Binding {
    /// `originalSkeletonName`: the name of the skeleton the animation was authored for.
    pub original_skeleton_name: String,
    /// Index into [`Container::animations`] of the animation it binds, when the container holds it.
    pub animation: Option<usize>,
    /// `transformTrackToBoneIndices`: the index into [`Skeleton::bones`] each transform track drives.
    pub transform_track_to_bone: Vec<i16>,
    /// `floatTrackToFloatSlotIndices`: the index into [`Skeleton::float_slots`] each float track drives.
    pub float_track_to_slot: Vec<i16>,
    /// `partitionIndices`, as stored.
    pub partition_indices: Vec<i16>,
    /// `blendHint`, as stored: 0, 1 or 2 (see [`Binding::blend`]).
    pub blend_hint: u32,
}

impl Binding {
    /// The blend hint, named; `None` for a stored number the enumeration does not name. Which composition 1 and 2 mean
    /// is not told by the files (see [`BlendHint`]).
    pub fn blend(&self) -> Option<BlendHint> {
        BlendHint::from_value(i64::from(self.blend_hint))
    }
}

/// `hkaAnimationContainer`.
#[derive(Clone, Debug, PartialEq)]
pub struct Container {
    /// `skeletons`.
    pub skeletons: Vec<Skeleton>,
    /// `animations`.
    pub animations: Vec<Animation>,
    /// `bindings`.
    pub bindings: Vec<Binding>,
}

/// The root's one named variant of class `hkaAnimationContainer`.
pub(crate) fn named<'t, 'a>(tag: &'t Tagfile<'a>) -> Result<Value<'t, 'a>> {
    let root = tag.root()?;
    let variants = root.field("namedVariants")?.array()?;
    for i in 0..variants.len() {
        let variant = variants.get(i)?;
        if variant.field("className")?.string()? == "hkaAnimationContainer" {
            return variant.field("variant")?.dereference();
        }
    }
    Err(invalid(0, "no hkaAnimationContainer in the root container"))
}

fn qs(v: Value<'_, '_>) -> Result<QsTransform> {
    Ok(QsTransform {
        translation: v.field("translation")?.vector::<4>()?,
        rotation: v.field("rotation")?.vector::<4>()?,
        scale: v.field("scale")?.vector::<4>()?,
    })
}

fn list<'t, 'a, T>(v: Value<'t, 'a>, each: impl Fn(Value<'t, 'a>) -> Result<T>) -> Result<Vec<T>> {
    let a = v.array()?;
    (0..a.len()).map(|i| each(a.get(i)?)).collect()
}

fn i16s(v: Value<'_, '_>) -> Result<Vec<i16>> {
    list(v, |e| {
        i16::try_from(e.signed()?).map_err(|_| invalid(e.at(), "track index exceeds i16"))
    })
}

fn u32s(v: Value<'_, '_>) -> Result<Vec<u32>> {
    list(v, |e| {
        u32::try_from(e.unsigned()?).map_err(|_| invalid(e.at(), "offset exceeds u32"))
    })
}

fn count(v: Value<'_, '_>) -> Result<usize> {
    usize::try_from(v.signed()?).map_err(|_| invalid(v.at(), "negative count"))
}

/// An integer field that is not negative, as u32.
fn small(v: Value<'_, '_>) -> Result<u32> {
    u32::try_from(v.signed()?).map_err(|_| invalid(v.at(), "value outside u32"))
}

pub(crate) fn skeleton(v: Value<'_, '_>) -> Result<Skeleton> {
    let parents = i16s(v.field("parentIndices")?)?;
    let bones = list(v.field("bones")?, |b| {
        Ok(Bone {
            name: b.field("name")?.string()?,
            lock_translation: b.field("lockTranslation")?.unsigned()? != 0,
        })
    })?;
    let reference_pose = list(v.field("referencePose")?, qs)?;
    if parents.len() != bones.len() || reference_pose.len() != bones.len() {
        return Err(invalid(
            v.at(),
            format!(
                "{} bones, {} parents, {} reference transforms",
                bones.len(),
                parents.len(),
                reference_pose.len()
            ),
        ));
    }
    Ok(Skeleton {
        name: v.field("name")?.string()?,
        parents,
        bones,
        reference_pose,
        reference_floats: list(v.field("referenceFloats")?, |e| e.float())?,
        float_slots: list(v.field("floatSlots")?, |e| e.string())?,
    })
}

fn reference_frame(v: Value<'_, '_>) -> Result<ReferenceFrame> {
    Ok(ReferenceFrame {
        up: v.field("up")?.vector::<4>()?,
        forward: v.field("forward")?.vector::<4>()?,
        duration: v.field("duration")?.float()?,
        samples: list(v.field("referenceFrameSamples")?, |e| e.vector::<4>())?,
    })
}

fn animation(v: Value<'_, '_>) -> Result<Animation> {
    let class = v.type_name().to_owned();
    let tracks = match class.as_str() {
        "hkaSplineCompressedAnimation" => Tracks::Spline(SplineData {
            num_frames: small(v.field("numFrames")?)?,
            num_blocks: small(v.field("numBlocks")?)?,
            max_frames_per_block: small(v.field("maxFramesPerBlock")?)?,
            mask_and_quantization_size: small(v.field("maskAndQuantizationSize")?)?,
            block_duration: v.field("blockDuration")?.float()?,
            block_inverse_duration: v.field("blockInverseDuration")?.float()?,
            frame_duration: v.field("frameDuration")?.float()?,
            block_offsets: u32s(v.field("blockOffsets")?)?,
            float_block_offsets: u32s(v.field("floatBlockOffsets")?)?,
            transform_offsets: u32s(v.field("transformOffsets")?)?,
            float_offsets: u32s(v.field("floatOffsets")?)?,
            data: v.field("data")?.array()?.raw()?.to_vec(),
            endian: i32::try_from(v.field("endian")?.signed()?).map_err(|_| invalid(v.at(), "endian"))?,
        }),
        "hkaInterleavedUncompressedAnimation" => Tracks::Interleaved(InterleavedData {
            transforms: list(v.field("transforms")?, qs)?,
            floats: list(v.field("floats")?, |e| e.float())?,
        }),
        other => Tracks::Unread(other.to_owned()),
    };
    Ok(Animation {
        kind: AnimationType::of(small(v.field("type")?)?),
        duration: v.field("duration")?.float()?,
        transform_tracks: count(v.field("numberOfTransformTracks")?)?,
        float_tracks: count(v.field("numberOfFloatTracks")?)?,
        extracted_motion: v
            .field("extractedMotion")?
            .pointer()?
            .map(reference_frame)
            .transpose()?,
        annotation_tracks: list(v.field("annotationTracks")?, |t| t.field("trackName")?.string())?,
        class,
        tracks,
    })
}

/// The animation container of `tag` (the one named variant of class `hkaAnimationContainer` its root holds).
pub fn container(tag: &Tagfile) -> Result<Container> {
    let c = named(tag)?;
    let skeletons = list(c.field("skeletons")?, |p| skeleton(p.dereference()?))?;
    let animation_values = list(c.field("animations")?, |p| p.dereference())?;
    let animations = animation_values
        .iter()
        .map(|v| animation(*v))
        .collect::<Result<Vec<_>>>()?;
    let bindings = list(c.field("bindings")?, |p| {
        let b = p.dereference()?;
        let target = b.field("animation")?.pointer()?;
        Ok(Binding {
            original_skeleton_name: b.field("originalSkeletonName")?.string()?,
            animation: target.and_then(|t| {
                animation_values
                    .iter()
                    .position(|a| a.offset == t.offset && a.type_index == t.type_index)
            }),
            transform_track_to_bone: i16s(b.field("transformTrackToBoneIndices")?)?,
            float_track_to_slot: i16s(b.field("floatTrackToFloatSlotIndices")?)?,
            partition_indices: i16s(b.field("partitionIndices")?)?,
            blend_hint: small(b.field("blendHint")?)?,
        })
    })?;
    Ok(Container {
        skeletons,
        animations,
        bindings,
    })
}
