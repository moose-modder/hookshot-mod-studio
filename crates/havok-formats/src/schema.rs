//! Lookups in a tagfile's type table for the writers: where a member sits in its object, how wide an integer member is,
//! what an array holds, and the bytes of an object under construction.
//!
//! The writers (`mesh_write`, `extern_write`) put new arrays into an editable [`TagfileImage`](crate::TagfileImage), and
//! the layout of what they write comes from the file's own type table, never from a table of offsets: a file of another
//! SDK version lays its classes out its own way.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use crate::{ImageItem, ItemKind, Result, TagfileImage, Type, invalid};

/// The first value `f` finds on type `index` or the types it derives from.
fn inherited<T>(types: &[Type], mut index: usize, f: impl Fn(&Type) -> Option<T>) -> Result<Option<T>> {
    for _ in 0..types.len() {
        if index == 0 {
            return Ok(None);
        }
        let t = types
            .get(index)
            .ok_or_else(|| invalid(0, format!("type index {index} out of bounds")))?;
        if let Some(value) = f(t) {
            return Ok(Some(value));
        }
        index = t.parent;
    }
    Err(invalid(0, "cyclic inherited property"))
}

/// A member found by name: its byte offset from the start of the object that holds it, and its type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Member {
    pub offset: usize,
    pub type_index: usize,
}

/// A type table to look layouts up in.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Schema<'t> {
    pub types: &'t [Type],
}

impl<'t> Schema<'t> {
    /// The name of type `index`.
    pub fn name(&self, index: usize) -> &str {
        self.types.get(index).map_or("", |t| t.name.as_str())
    }

    /// The size in bytes of a value of type `index`.
    pub fn size_of(&self, index: usize) -> Result<usize> {
        inherited(self.types, index, |t| t.size)?
            .ok_or_else(|| invalid(0, format!("type {} has no size", self.name(index))))
    }

    /// The format word of type `index`.
    pub fn format_of(&self, index: usize) -> Result<usize> {
        let format = inherited(self.types, index, |t| t.format)?
            .ok_or_else(|| invalid(0, format!("type {} has no format", self.name(index))))?;
        usize::try_from(format).map_err(|_| invalid(0, "a format word that does not fit usize"))
    }

    /// The element type of an array or pointer type.
    pub fn subtype_of(&self, index: usize) -> Result<usize> {
        inherited(self.types, index, |t| t.subtype)?
            .filter(|&s| s != 0)
            .ok_or_else(|| invalid(0, format!("type {} has no element type", self.name(index))))
    }

    /// The width in bytes of an integer type (1, 2, 4 or 8).
    pub fn int_width(&self, index: usize) -> Result<usize> {
        let format = self.format_of(index)?;
        if ![2, 4].contains(&(format & 15)) {
            return Err(invalid(0, format!("type {} is not an integer", self.name(index))));
        }
        let bits = format >> 10;
        match bits {
            8 => Ok(1),
            16 => Ok(2),
            32 => Ok(4),
            64 => Ok(8),
            other => Err(invalid(
                0,
                format!("type {} is {other} bits wide", self.name(index)),
            )),
        }
    }

    /// The member `name` declared by `class` or a class it derives from.
    pub fn field(&self, class: usize, name: &str) -> Result<Member> {
        let mut index = class;
        for _ in 0..self.types.len() {
            if index == 0 {
                break;
            }
            let t = self
                .types
                .get(index)
                .ok_or_else(|| invalid(0, format!("type index {index} out of bounds")))?;
            if let Some(f) = t.fields.iter().find(|f| f.name == name) {
                return Ok(Member {
                    offset: f.offset,
                    type_index: f.type_index,
                });
            }
            index = t.parent;
        }
        Err(invalid(0, format!("{} has no member {name}", self.name(class))))
    }

    /// Whether `class` or a class it derives from declares the member `name`.
    pub fn has_field(&self, class: usize, name: &str) -> bool {
        self.field(class, name).is_ok()
    }

    /// The member at the end of the path `names` from `class`: the offsets of the members nested in each other add up.
    pub fn path(&self, class: usize, names: &[&str]) -> Result<Member> {
        let mut offset = 0usize;
        let mut ty = class;
        for name in names {
            let m = self.field(ty, name)?;
            offset = offset
                .checked_add(m.offset)
                .ok_or_else(|| invalid(0, "member offset overflow"))?;
            ty = m.type_index;
        }
        Ok(Member {
            offset,
            type_index: ty,
        })
    }

    /// `count` zeroed elements of type `index`.
    pub fn zeroed(&self, index: usize, count: usize) -> Result<Vec<u8>> {
        let size = self.size_of(index)?;
        let total = size
            .checked_mul(count)
            .ok_or_else(|| invalid(0, "array size overflow"))?;
        Ok(vec![0; total])
    }
}

/// Write the unsigned `value` little endian over `width` bytes at `at`; refused when it does not fit or `at` is outside.
pub(crate) fn put_uint(buf: &mut [u8], at: usize, width: usize, value: u64) -> Result<()> {
    if width > 8 {
        return Err(invalid(0, format!("an integer member of {width} bytes")));
    }
    let end = at
        .checked_add(width)
        .ok_or_else(|| invalid(0, "member offset overflow"))?;
    let slot = buf.get_mut(at..end).ok_or_else(|| {
        invalid(
            0,
            format!("a member of {width} bytes at {at} is outside its object"),
        )
    })?;
    let bytes = value.to_le_bytes();
    let (low, high) = bytes.split_at(width.min(8));
    if high.iter().any(|&b| b != 0) {
        return Err(invalid(0, format!("{value} does not fit {width} bytes")));
    }
    slot.copy_from_slice(low);
    Ok(())
}

/// Write floats little endian from `at`.
pub(crate) fn put_f32s(buf: &mut [u8], at: usize, values: &[f32]) -> Result<()> {
    for (i, v) in values.iter().enumerate() {
        let offset = i
            .checked_mul(4)
            .and_then(|o| o.checked_add(at))
            .ok_or_else(|| invalid(0, "member offset overflow"))?;
        let end = offset
            .checked_add(4)
            .ok_or_else(|| invalid(0, "member offset overflow"))?;
        buf.get_mut(offset..end)
            .ok_or_else(|| invalid(0, format!("a float at {offset} is outside its object")))?
            .copy_from_slice(&v.to_le_bytes());
    }
    Ok(())
}

/// Read the unsigned little-endian value of `width` bytes at `at`.
pub(crate) fn get_uint(buf: &[u8], at: usize, width: usize) -> Result<u64> {
    let end = at
        .checked_add(width)
        .ok_or_else(|| invalid(0, "member offset overflow"))?;
    let bytes = buf.get(at..end).ok_or_else(|| {
        invalid(
            0,
            format!("a member of {width} bytes at {at} is outside its object"),
        )
    })?;
    Ok(bytes.iter().rev().fold(0u64, |n, &b| (n << 8) | u64::from(b)))
}

impl TagfileImage {
    /// The object items whose type is named `class`, in item order.
    pub(crate) fn objects_named(&self, class: &str) -> Vec<usize> {
        self.items
            .iter()
            .enumerate()
            .filter(|(i, item)| {
                *i != 0
                    && item.kind == ItemKind::Object
                    && item.count == 1
                    && self.types.get(item.type_index).is_some_and(|t| t.name == class)
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// The item a reference slot of item `item` at byte `offset` refers to (0 for none).
    pub(crate) fn reference_at(&self, item: usize, offset: usize) -> Result<usize> {
        let data = &self
            .items
            .get(item)
            .ok_or_else(|| invalid(0, format!("no item {item}")))?
            .data;
        usize::try_from(get_uint(data, offset, 4)?)
            .map_err(|_| invalid(0, "an item index that does not fit usize"))
    }

    /// Make the array slot at byte `offset` of item `item` hold `count` elements of type `element`, whose bytes are
    /// `data`: a new run item, or no item at all for an empty array. The elements' own reference slots are set by the
    /// caller afterwards ([`TagfileImage::set_reference`]).
    pub(crate) fn set_array(
        &mut self,
        item: usize,
        offset: usize,
        element: usize,
        count: usize,
        data: Vec<u8>,
    ) -> Result<usize> {
        let target = if count == 0 {
            0
        } else {
            self.push_item(ImageItem {
                type_index: element,
                kind: ItemKind::Run,
                count,
                data,
            })
        };
        self.set_reference(item, offset, target)?;
        Ok(target)
    }

    /// Overwrite bytes of item `item` from byte `at`.
    pub(crate) fn patch(&mut self, item: usize, at: usize, bytes: &[u8]) -> Result<()> {
        let data = &mut self
            .items
            .get_mut(item)
            .ok_or_else(|| invalid(0, format!("no item {item}")))?
            .data;
        let end = at
            .checked_add(bytes.len())
            .ok_or_else(|| invalid(0, "member offset overflow"))?;
        data.get_mut(at..end)
            .ok_or_else(|| invalid(0, format!("item {item}: bytes {at}..{end} are outside its data")))?
            .copy_from_slice(bytes);
        Ok(())
    }
}
