#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use crate::{Result, Tagfile, invalid};

/// The format word of `hkHalf16`, the 16-bit float type of the games' tagfiles: kind 5 (a real), 16 bits wide. The
/// distinct type tables of Sekiro (229 tables, 56 types in them) and Elden Ring (875 tables, 215 types) hold it
/// beside `float` (`0x174605`), the one other real format they store: no table has a double or any other width.
pub const HALF_FORMAT: usize = 0x7_4605;

/// A typed view into DATA. References remain indices, so cyclic object graphs
/// do not cause recursive allocations or stack overflow during parsing.
#[derive(Debug, Clone, Copy)]
pub struct Value<'t, 'a> {
    pub(crate) tag: &'t Tagfile<'a>,
    /// The value's type in [`Tagfile::types`].
    pub type_index: usize,
    /// The value's byte offset in DATA.
    pub offset: usize,
}

/// A run of values of one type: an item of the tagfile, an array member, or a fixed-size array.
#[derive(Debug, Clone, Copy)]
pub struct Array<'t, 'a> {
    tag: &'t Tagfile<'a>,
    type_index: usize,
    offset: usize,
    count: usize,
    stride: usize,
}
impl<'a> Tagfile<'a> {
    /// The elements of item `index` of the item table. An index the table lacks is refused at the table.
    pub fn item(&self, index: usize) -> Result<Array<'_, 'a>> {
        self.item_referenced(index, self.items_offset)
    }
    /// Like [`item`](Self::item) for an index stored at file offset `at`, which is where a refusal is reported.
    fn item_referenced(&self, index: usize, at: usize) -> Result<Array<'_, 'a>> {
        let item = self.items.get(index).ok_or_else(|| {
            invalid(
                at,
                format!(
                    "item index {index} out of bounds: the file has {} items",
                    self.items.len()
                ),
            )
        })?;
        let stride = self.stride(item.type_index)?;
        self.extent(stride, item.count)?;
        Ok(Array {
            tag: self,
            type_index: item.type_index,
            offset: item.offset,
            count: item.count,
            stride,
        })
    }
    /// The root object (the first element of item 1).
    pub fn root(&self) -> Result<Value<'_, 'a>> {
        self.item(1)?.get(0)
    }
}
impl<'t, 'a> Array<'t, 'a> {
    /// The number of elements.
    pub fn len(self) -> usize {
        self.count
    }
    /// Whether there are no elements.
    pub fn is_empty(self) -> bool {
        self.count == 0
    }
    /// The bytes of an array of bytes, as stored.
    pub fn raw(self) -> Result<&'a [u8]> {
        if self.stride != 1 {
            return Err(invalid(
                self.tag.at(self.offset),
                format!(
                    "expected an array of bytes, not elements of {} bytes",
                    self.stride
                ),
            ));
        }
        self.tag.bytes(self.offset, self.count)
    }
    /// The bytes of all the elements, as stored.
    pub(crate) fn element_bytes(self) -> Result<&'a [u8]> {
        if self.count == 0 {
            return Ok(&[]);
        }
        let size = self.tag.extent(self.stride, self.count)?;
        self.tag.bytes(self.offset, size)
    }
    /// Element `index`; an error when it is out of bounds or runs past DATA.
    pub fn get(self, index: usize) -> Result<Value<'t, 'a>> {
        let offset = index
            .checked_mul(self.stride)
            .and_then(|n| self.offset.checked_add(n))
            .ok_or_else(|| invalid(self.tag.at(self.offset), "array element offset overflows"))?;
        if index >= self.count {
            return Err(invalid(
                self.tag.at(offset),
                format!(
                    "array index {index} out of bounds: the array has {} elements",
                    self.count
                ),
            ));
        }
        self.tag.bytes(offset, self.stride)?;
        Ok(Value {
            tag: self.tag,
            type_index: self.type_index,
            offset,
        })
    }
}
impl<'t, 'a> Value<'t, 'a> {
    /// The file offset of the value, for refusals.
    pub(crate) fn at(self) -> usize {
        self.tag.at(self.offset)
    }
    /// The name of the value's type (a value's type index is one the tagfile checked: its items', fields' and
    /// array elements').
    pub fn type_name(self) -> &'t str {
        self.tag
            .types
            .get(self.type_index)
            .map_or("", |t| t.name.as_str())
    }
    /// The type's format word (see [`Tagfile::format`]).
    pub fn format(self) -> Result<usize> {
        self.tag.format(self.type_index)
    }
    /// Whether the value's class (or a class it derives from) declares a field `name`.
    pub fn has_field(self, name: &str) -> bool {
        let mut index = self.type_index;
        while let Some(t) = self.tag.types.get(index).filter(|_| index != 0) {
            if t.fields.iter().any(|f| f.name == name) {
                return true;
            }
            index = t.parent;
        }
        false
    }
    /// The member `name` of a class value, looked up on the class and then its parents. Refused when the value is not
    /// a class, has no such member, or the member does not fit the class's layout.
    pub fn field(self, name: &str) -> Result<Self> {
        if self.format()? & 15 != 7 {
            return Err(invalid(
                self.at(),
                format!("field {name} needs a class value, not a {}", self.type_name()),
            ));
        }
        let mut index = self.type_index;
        while let Some(t) = self.tag.types.get(index).filter(|_| index != 0) {
            if let Some(f) = t.fields.iter().find(|f| f.name == name) {
                let offset = self
                    .offset
                    .checked_add(f.offset)
                    .ok_or_else(|| invalid(self.at(), "field offset overflow"))?;
                let size = self.tag.inherited(f.type_index, |t| t.size)?;
                let parent_size = self.tag.inherited(self.type_index, |t| t.size)?;
                if f.offset.checked_add(size).is_none_or(|n| n > parent_size) {
                    return Err(invalid(
                        self.tag.at(offset),
                        format!(
                            "field {name} of {size} bytes at {:#x} exceeds the class size {parent_size}",
                            f.offset
                        ),
                    ));
                }
                self.tag.bytes(offset, size)?;
                return Ok(Self {
                    tag: self.tag,
                    type_index: f.type_index,
                    offset,
                });
            }
            index = t.parent;
        }
        Err(invalid(
            self.at(),
            format!("{} has no field {name}", self.type_name()),
        ))
    }
    /// The item index a pointer, string or array slot stores. A slot is the index in its first four bytes and zeros
    /// in the rest (an `hkArray`'s size and capacity words included, which the loader fills in from the item):
    /// every slot of Sekiro's and Elden Ring's tagfiles is that, and any other is refused.
    fn reference_index(self) -> Result<usize> {
        let size = self.tag.stride(self.type_index)?;
        let slot = self.tag.bytes(self.offset, size)?;
        let Some((index, rest)) = slot.split_first_chunk::<4>() else {
            return Err(invalid(
                self.at(),
                format!(
                    "a {} slot of {size} bytes cannot hold an item index",
                    self.type_name()
                ),
            ));
        };
        if rest.iter().any(|&b| b != 0) {
            return Err(invalid(
                self.at().saturating_add(4),
                format!(
                    "bytes 4..{size} of a {} slot are not zero; only the item index is stored",
                    self.type_name()
                ),
            ));
        }
        Ok(u32::from_le_bytes(*index) as usize)
    }
    /// The object a pointer value refers to; `None` for a null pointer.
    pub fn pointer(self) -> Result<Option<Self>> {
        if self.format()? & 15 != 6 {
            return Err(invalid(
                self.at(),
                format!("expected a pointer, not a {}", self.type_name()),
            ));
        }
        let index = self.reference_index()?;
        if index == 0 {
            Ok(None)
        } else {
            self.tag.item_referenced(index, self.at())?.get(0).map(Some)
        }
    }
    /// Like [`pointer`](Self::pointer), but a null pointer is an error.
    pub fn dereference(self) -> Result<Self> {
        self.pointer()?
            .ok_or_else(|| invalid(self.at(), "expected a pointer to an object, found a null pointer"))
    }
    /// A tagfile string reference, bounded by its character ITEM.
    pub fn string(self) -> Result<String> {
        if self.format()? & 15 != 3 {
            return Err(invalid(
                self.at(),
                format!("expected a string, not a {}", self.type_name()),
            ));
        }
        let index = self.reference_index()?;
        if index == 0 {
            return Ok(String::new());
        }
        let item = self.tag.item_referenced(index, self.at())?;
        let mut bytes = Vec::new();
        for i in 0..item.len() {
            let byte = u8::try_from(item.get(i)?.unsigned()?)
                .map_err(|_| invalid(self.at(), "expected a string of bytes: a character exceeds 255"))?;
            if byte == 0 {
                return String::from_utf8(bytes).map_err(|_| invalid(self.at(), "expected a UTF-8 string"));
            }
            bytes.push(byte);
        }
        Err(invalid(self.at(), "expected a string ending in a zero byte"))
    }
    /// The size in bytes of one element of an array value, as the element type's metadata gives it: the same whether
    /// the array stores elements or is empty.
    pub fn element_size(self) -> Result<usize> {
        if self.format()? & 15 != 8 {
            return Err(invalid(
                self.at(),
                format!("expected an array, not a {}", self.type_name()),
            ));
        }
        let element = self.tag.inherited(self.type_index, |t| t.subtype)?;
        if element == 0 {
            return Err(invalid(self.at(), "expected an array with an element type"));
        }
        self.tag.stride(element)
    }
    /// The elements of an array value (a stored array or a fixed-size one).
    pub fn array(self) -> Result<Array<'t, 'a>> {
        let format = self.format()?;
        if format & 15 != 8 {
            return Err(invalid(
                self.at(),
                format!("expected an array, not a {}", self.type_name()),
            ));
        }
        if format & 32 == 0 {
            return self.tag.item_referenced(self.reference_index()?, self.at());
        }
        let type_index = self.tag.inherited(self.type_index, |t| t.subtype)?;
        if type_index == 0 {
            return Err(invalid(self.at(), "expected an array with an element type"));
        }
        let count = format >> 8;
        let stride = self.tag.inherited(type_index, |t| t.size)?;
        let size = self
            .tag
            .extent(stride, count)
            .map_err(|e| invalid(self.at(), e.detail))?;
        self.tag.bytes(self.offset, size)?;
        Ok(Array {
            tag: self.tag,
            type_index,
            offset: self.offset,
            count,
            stride,
        })
    }
    /// Integer bits without sign extension, preserving the stored representation.
    pub fn unsigned(self) -> Result<u64> {
        let format = self.format()?;
        if ![2, 4].contains(&(format & 15)) {
            return Err(invalid(
                self.at(),
                format!("expected an integer, not a {}", self.type_name()),
            ));
        }
        let bits = format >> 10;
        if ![8, 16, 32, 64].contains(&bits) {
            return Err(invalid(
                self.at(),
                format!("expected an integer of 8, 16, 32 or 64 bits, not {bits}"),
            ));
        }
        let bytes = self.tag.bytes(self.offset, bits / 8)?;
        Ok(bytes.iter().rev().fold(0u64, |n, &b| (n << 8) | u64::from(b)))
    }
    /// An integer, sign-extended when its type is signed.
    pub fn signed(self) -> Result<i64> {
        let format = self.format()?;
        let bits = format >> 10;
        let value = self.unsigned()?;
        if format & 512 == 0 {
            return i64::try_from(value).map_err(|_| invalid(self.at(), "expected a value that fits i64"));
        }
        // sign-extended from its width: 8, 16, 32 or 64 bits, the widths `unsigned` reads
        let shift = match bits {
            8 => 56,
            16 => 48,
            32 => 32,
            _ => 0,
        };
        Ok((value.wrapping_shl(shift) as i64).wrapping_shr(shift))
    }
    /// Stored float32, including nonfinite values in unused SIMD lanes.
    /// Geometry consumers must validate the components they actually use.
    pub fn float(self) -> Result<f32> {
        if self.format()? != 0x174605 {
            return Err(invalid(
                self.at(),
                format!("expected a float32, not a {}", self.type_name()),
            ));
        }
        let value = f32::from_le_bytes(self.tag.array(self.offset)?);
        Ok(value)
    }
    /// A stored `hkHalf16` (format word [`HALF_FORMAT`]) as the `f32` it stands for. Havok's half is the top 16 bits of
    /// an `f32` (sign, the 8 exponent bits and 7 mantissa bits), not an IEEE binary16: the files agree, see
    /// [`HALF_FORMAT`].
    pub fn half(self) -> Result<f32> {
        if self.format()? != HALF_FORMAT {
            return Err(invalid(
                self.at(),
                format!("expected a half float, not a {}", self.type_name()),
            ));
        }
        let [low, high] = self.tag.array(self.offset)?;
        Ok(f32::from_bits(u32::from(u16::from_le_bytes([low, high])) << 16))
    }
    /// An array of exactly `N` floats.
    pub fn vector<const N: usize>(self) -> Result<[f32; N]> {
        let a = self.array()?;
        if a.len() != N {
            return Err(invalid(
                self.at(),
                format!("expected a vector of {N} floats, found {}", a.len()),
            ));
        }
        let mut result = [0.; N];
        for (i, x) in result.iter_mut().enumerate() {
            *x = a.get(i)?.float()?;
        }
        Ok(result)
    }
}
