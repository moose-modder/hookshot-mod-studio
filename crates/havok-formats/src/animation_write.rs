//! An animation written into an animation file: the file's own animation object given new values, everything else
//! of the file kept ([`replace_animation`]). The file is the donor of every type (its type section, or its
//! compendium's, is written as it was read), so the new animation must be of the class the file stores there.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use crate::animation::{Animation, QsTransform, Tracks, named};
use crate::{ImageItem, ItemKind, Result, Tagfile, TagfileImage, Value, invalid};

/// Where a value of the file lives in an image: the item and the byte within it.
fn place(tag: &Tagfile, v: Value) -> Result<(usize, usize)> {
    tag.item_at(v.offset)
        .ok_or_else(|| invalid(v.offset, format!("{} lies between items", v.type_name())))
}

/// The bytes of `image` item `item` from `at`, `size` long.
fn bytes_mut(image: &mut TagfileImage, item: usize, at: usize, size: usize) -> Result<&mut [u8]> {
    let end = at
        .checked_add(size)
        .ok_or_else(|| invalid(0, "a field past the item"))?;
    image
        .items
        .get_mut(item)
        .and_then(|i| i.data.get_mut(at..end))
        .ok_or_else(|| invalid(0, format!("item {item}: bytes {at}..{end} are outside it")))
}

/// Set integer field `name` of `object` to `value`, at the width its type stores.
fn set_integer(tag: &Tagfile, image: &mut TagfileImage, object: Value, name: &str, value: i64) -> Result<()> {
    let v = object.field(name)?;
    let format = v.format()?;
    let bits = format.checked_shr(10).unwrap_or_default();
    if ![2, 4].contains(&(format & 15)) || ![8, 16, 32, 64].contains(&bits) {
        return Err(invalid(v.offset, format!("{name} is not an integer field")));
    }
    let size = bits / 8;
    let signed = format & 512 != 0;
    let fits = if signed {
        bits == 64 || (value >> (bits.saturating_sub(1))) == 0 || (value >> (bits.saturating_sub(1))) == -1
    } else {
        value >= 0 && (bits == 64 || (value >> bits) == 0)
    };
    if !fits {
        return Err(invalid(
            v.offset,
            format!("{name} = {value} does not fit {bits} bits"),
        ));
    }
    let (item, at) = place(tag, v)?;
    let le = value.to_le_bytes();
    bytes_mut(image, item, at, size)?.copy_from_slice(le.get(..size).unwrap_or_default());
    Ok(())
}

/// Set float field `name` of `object` to `value`.
fn set_float(tag: &Tagfile, image: &mut TagfileImage, object: Value, name: &str, value: f32) -> Result<()> {
    let v = object.field(name)?;
    v.float()?;
    let (item, at) = place(tag, v)?;
    bytes_mut(image, item, at, 4)?.copy_from_slice(&value.to_le_bytes());
    Ok(())
}

/// Point array field `name` of `object` at a new run of `count` elements stored as `bytes` (each element of the
/// size the array's element type stores), or at nothing when it is empty.
fn set_array(
    tag: &Tagfile,
    image: &mut TagfileImage,
    object: Value,
    name: &str,
    count: usize,
    bytes: Vec<u8>,
) -> Result<()> {
    let v = object.field(name)?;
    let (item, at) = place(tag, v)?;
    // the element type: the one the stored run has, or for an empty array the array type's own
    let stored = bytes_mut(image, item, at, 4)?;
    let run = usize::try_from(u32::from_le_bytes([
        stored.first().copied().unwrap_or_default(),
        stored.get(1).copied().unwrap_or_default(),
        stored.get(2).copied().unwrap_or_default(),
        stored.get(3).copied().unwrap_or_default(),
    ]))
    .map_err(|_| invalid(v.offset, "an item index beyond usize"))?;
    let element = match image.items.get(run) {
        Some(stored) if run != 0 => stored.type_index,
        _ => tag.inherited(v.type_index, |t| t.subtype)?,
    };
    if element == 0 {
        return Err(invalid(
            v.offset,
            format!("{name} is not an array of a known type"),
        ));
    }
    let stride = tag.stride(element)?;
    if stride.checked_mul(count) != Some(bytes.len()) {
        return Err(invalid(
            v.offset,
            format!(
                "{name}: {count} elements of {stride} bytes are not {} bytes",
                bytes.len()
            ),
        ));
    }
    let target = if count == 0 {
        0
    } else {
        image.push_item(ImageItem {
            type_index: element,
            kind: ItemKind::Run,
            count,
            data: bytes,
        })
    };
    image.set_reference(item, at, target)
}

fn words(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn transforms(values: &[QsTransform]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|t| t.translation.into_iter().chain(t.rotation).chain(t.scale))
        .flat_map(f32::to_le_bytes)
        .collect()
}

/// `tag` with animation `index` of its animation container replaced by `animation`, as an image to write
/// ([`TagfileImage::write`]). The stored animation must be of `animation`'s class (`hkaSplineCompressedAnimation` or
/// `hkaInterleavedUncompressedAnimation`) and have its track counts: the bindings, annotation tracks and the type table
/// are the file's and stay as they are, as does the extracted motion. Its duration and every field of its class are
/// `animation`'s.
pub fn replace_animation(tag: &Tagfile, index: usize, animation: &Animation) -> Result<TagfileImage> {
    let container = named(tag)?;
    let animations = container.field("animations")?.array()?;
    let object = animations.get(index)?.dereference()?;
    if object.type_name() != animation.class {
        return Err(invalid(
            object.offset,
            format!(
                "animation {index} is a {}, not a {}: a file can only store the classes its type table has",
                object.type_name(),
                animation.class
            ),
        ));
    }
    let tracks = |name: &str| -> Result<i64> { object.field(name)?.signed() };
    let stored = (tracks("numberOfTransformTracks")?, tracks("numberOfFloatTracks")?);
    let given = (
        i64::try_from(animation.transform_tracks).map_err(|_| invalid(0, "track count"))?,
        i64::try_from(animation.float_tracks).map_err(|_| invalid(0, "track count"))?,
    );
    if stored != given {
        return Err(invalid(
            object.offset,
            format!(
                "animation {index} has {} transform and {} float tracks, the new one {} and {}: the bindings are the file's",
                stored.0, stored.1, given.0, given.1
            ),
        ));
    }
    let mut image = tag.image()?;
    set_float(tag, &mut image, object, "duration", animation.duration)?;
    match &animation.tracks {
        Tracks::Spline(s) => {
            for (name, value) in [
                ("numFrames", s.num_frames),
                ("numBlocks", s.num_blocks),
                ("maxFramesPerBlock", s.max_frames_per_block),
                ("maskAndQuantizationSize", s.mask_and_quantization_size),
            ] {
                set_integer(tag, &mut image, object, name, i64::from(value))?;
            }
            set_integer(tag, &mut image, object, "endian", i64::from(s.endian))?;
            for (name, value) in [
                ("blockDuration", s.block_duration),
                ("blockInverseDuration", s.block_inverse_duration),
                ("frameDuration", s.frame_duration),
            ] {
                set_float(tag, &mut image, object, name, value)?;
            }
            for (name, values) in [
                ("blockOffsets", &s.block_offsets),
                ("floatBlockOffsets", &s.float_block_offsets),
                ("transformOffsets", &s.transform_offsets),
                ("floatOffsets", &s.float_offsets),
            ] {
                set_array(tag, &mut image, object, name, values.len(), words(values))?;
            }
            set_array(tag, &mut image, object, "data", s.data.len(), s.data.clone())?;
        }
        Tracks::Interleaved(d) => {
            set_array(
                tag,
                &mut image,
                object,
                "transforms",
                d.transforms.len(),
                transforms(&d.transforms),
            )?;
            let floats = d.floats.iter().flat_map(|v| v.to_le_bytes()).collect();
            set_array(tag, &mut image, object, "floats", d.floats.len(), floats)?;
        }
        Tracks::Unread(class) => {
            return Err(invalid(0, format!("a {class} is not written")));
        }
    }
    Ok(image)
}
