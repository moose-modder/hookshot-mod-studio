#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
//! Writing tagfiles: an editable copy of a [`Tagfile`] ([`TagfileImage`]) and the writer that emits it.
//!
//! A tagfile is a type table and a graph of items: the root object, and what its pointers, strings and arrays refer
//! to. The writer keeps the type table as it was read (`TYPE`, or the `TCRF` reference to a compendium: the bytes
//! are not rebuilt) and emits `DATA`, `ITEM` and `PTCH` from the items. It does not copy the input's item order or
//! layout: it derives both from the graph, by the rules below, which the stock files of Sekiro (2016.2) and Elden
//! Ring (2018.1) follow without exception, so a file read and written again comes back byte for byte.
//!
//! **Order.** Items are numbered in the order a walk from the root finds them, and `DATA` holds them in an order of
//! the same walk:
//!
//! - The walk takes *objects* (items of kind `0x10`, what pointers refer to) from a first-in first-out queue,
//!   starting with the root. An object is laid out when it is taken. Then the *runs* (kind `0x20`: arrays and
//!   strings) it refers to are laid out, level by level: all the runs the object refers to, then all the runs those
//!   refer to, and so on. Objects found on the way join the queue.
//! - Whenever an item is processed, the references in it that name an item not yet numbered are numbered in this
//!   order: every reference that is not a string, then the strings; within each, by the reference's offset in one
//!   element of the item's type, then by the element. A run is laid out in the order of its numbers.
//!
//! **Alignment.** The root is at 0; an object starts at a multiple of `max(2, its type's alignment)`; a run that an
//! array refers to at a multiple of 16, and one that a string refers to at a multiple of 2. Padding is zero, and
//! `DATA` ends on a multiple of 16.
//!
//! **Patches.** `PTCH` lists, for every non-zero reference, the `DATA` offset of its slot under the type of the
//! slot: one record a type, the types ascending, the offsets ascending. A null reference is not listed, and a file
//! without references has no `PTCH` section.
//!
//! A reference slot is the item's index in its first four bytes and zeros after them, whatever its width.
use std::collections::VecDeque;
use std::rc::Rc;

use crate::{ItemKind, Result, Tagfile, Type, invalid};

/// What a reference slot refers to by its kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlotKind {
    /// A pointer: one object.
    Pointer,
    /// A string: the characters of a run.
    String,
    /// An array: the elements of a run.
    Array,
}

/// A reference slot of a type, at an offset in one element of it.
#[derive(Debug, Clone, Copy)]
struct Slot {
    offset: usize,
    kind: SlotKind,
    /// The slot's own type (the pointer, string or array type), the type its `PTCH` record is listed under.
    type_index: usize,
    /// The slot's width in bytes: the item index and zeros.
    size: usize,
}

/// A reference stored in an item, and where it is.
#[derive(Debug, Clone, Copy)]
struct Reference {
    target: usize,
    slot: Slot,
    /// The slot's offset in the item's data.
    position: usize,
    /// The order references are numbered in: strings last, then by the slot's offset in its element, then by element.
    key: (bool, usize, usize),
}

/// The deepest nesting of types the writer follows.
const MAX_TYPE_DEPTH: usize = 128;

fn overflow() -> crate::Error {
    invalid(0, "a size or offset overflows")
}

fn add(a: usize, b: usize) -> Result<usize> {
    a.checked_add(b).ok_or_else(overflow)
}

fn mul(a: usize, b: usize) -> Result<usize> {
    a.checked_mul(b).ok_or_else(overflow)
}

fn word(value: usize) -> Result<u32> {
    u32::try_from(value).map_err(|_| invalid(0, format!("{value} does not fit a 32-bit field")))
}

/// The first value `f` finds on type `index` or the types it derives from; the default at the null type.
fn inherited<T: Default>(types: &[Type], mut index: usize, f: impl Fn(&Type) -> Option<T>) -> Result<T> {
    for _ in 0..types.len() {
        if index == 0 {
            return Ok(T::default());
        }
        let t = types
            .get(index)
            .ok_or_else(|| invalid(0, format!("type index {index} out of bounds")))?;
        if let Some(value) = f(t) {
            return Ok(value);
        }
        index = t.parent;
    }
    Err(invalid(0, "cyclic inherited property"))
}

fn stride(types: &[Type], index: usize) -> Result<usize> {
    inherited(types, index, |t| t.size)
}

/// The reference slots of each type, found once.
struct Layouts<'t> {
    types: &'t [Type],
    memo: Vec<Option<Rc<[Slot]>>>,
}

impl<'t> Layouts<'t> {
    fn new(types: &'t [Type]) -> Self {
        Self {
            types,
            memo: vec![None; types.len()],
        }
    }

    /// The reference slots inside one value of type `index`, ascending by offset: its pointers, strings and arrays,
    /// and those of the members of a class, of its parents' members and of the elements of a fixed-size array.
    fn slots(&mut self, index: usize, depth: usize) -> Result<Rc<[Slot]>> {
        if let Some(Some(done)) = self.memo.get(index) {
            return Ok(Rc::clone(done));
        }
        if depth > MAX_TYPE_DEPTH {
            return Err(invalid(0, "types nest deeper than the writer follows"));
        }
        let types = self.types;
        let format = usize::try_from(inherited(types, index, |t| t.format)?)
            .map_err(|_| invalid(0, "type format exceeds the address space"))?;
        let mut slots: Vec<Slot> = Vec::new();
        let own = |kind| -> Result<Slot> {
            Ok(Slot {
                offset: 0,
                kind,
                type_index: index,
                size: stride(types, index)?,
            })
        };
        match format & 15 {
            7 => {
                // the members of the root class first, then each class down to this one
                let mut chain = Vec::new();
                let mut at = index;
                while at != 0 {
                    if chain.len() > types.len() {
                        return Err(invalid(0, "cyclic inherited property"));
                    }
                    chain.push(at);
                    at = types
                        .get(at)
                        .ok_or_else(|| invalid(0, format!("type index {at} out of bounds")))?
                        .parent;
                }
                for &t in chain.iter().rev() {
                    let Some(ty) = types.get(t) else { continue };
                    for f in &ty.fields {
                        for s in self.slots(f.type_index, add(depth, 1)?)?.iter() {
                            slots.push(Slot {
                                offset: add(f.offset, s.offset)?,
                                ..*s
                            });
                        }
                    }
                }
            }
            6 => slots.push(own(SlotKind::Pointer)?),
            3 => slots.push(own(SlotKind::String)?),
            8 if format & 32 == 0 => slots.push(own(SlotKind::Array)?),
            8 => {
                let element = inherited(types, index, |t| t.subtype)?;
                let count = format >> 8;
                let size = stride(types, element)?;
                let inner = self.slots(element, add(depth, 1)?)?;
                if !inner.is_empty() {
                    for i in 0..count {
                        for s in inner.iter() {
                            slots.push(Slot {
                                offset: add(mul(i, size)?, s.offset)?,
                                ..*s
                            });
                        }
                    }
                }
            }
            _ => {}
        }
        slots.sort_by_key(|s| s.offset);
        let slots: Rc<[Slot]> = slots.into();
        if let Some(entry) = self.memo.get_mut(index) {
            *entry = Some(Rc::clone(&slots));
        }
        Ok(slots)
    }
}

/// One item of a [`TagfileImage`]: `count` elements of one type, stored as the bytes a tagfile holds for them.
///
/// The bytes of the elements are `count` times the type's size long. A reference slot (a pointer, string or array)
/// holds the index of the item it refers to in [`TagfileImage::items`], in its first four bytes, and zeros after
/// them; 0 is a null reference.
#[derive(Debug, Clone)]
pub struct ImageItem {
    /// The type of one element, an index into [`TagfileImage::types`] (never 0 after the null item).
    pub type_index: usize,
    /// An object (one, what a pointer refers to) or a run (what an array or string refers to).
    pub kind: ItemKind,
    /// The number of elements.
    pub count: usize,
    /// The bytes of the elements.
    pub data: Vec<u8>,
}

/// An owned, editable tagfile: the type table as read and the items with their bytes. [`TagfileImage::write`] emits
/// it as a tagfile, in the order and layout the game's own files have.
///
/// Edit values by changing [`ImageItem::data`]; change structure by pointing a reference slot at another item
/// ([`TagfileImage::set_reference`]), adding items ([`TagfileImage::push_item`]) or changing an item's count and bytes
/// together. Item 1 stays the root object. The writer renumbers the items, so the order they are in here does not
/// matter; an item nothing refers to (from the root on) is not written.
#[derive(Debug, Clone)]
pub struct TagfileImage {
    /// The SDK version string, as in [`Tagfile::version`].
    pub version: [u8; 8],
    /// The type table, index 0 the null type. The table cannot be changed here: the file's own `TYPE` section, or its
    /// `TCRF` reference to a compendium, is written as it was read.
    pub types: Vec<Type>,
    /// The items, from the null item at index 0 (no type, no bytes) and the root at index 1.
    pub items: Vec<ImageItem>,
    type_section: Vec<u8>,
}

impl Tagfile<'_> {
    /// An editable copy of the file: its type table and every item's bytes.
    pub fn image(&self) -> Result<TagfileImage> {
        let mut items = Vec::with_capacity(self.items.len());
        for (i, item) in self.items.iter().enumerate() {
            let data = if i == 0 {
                Vec::new()
            } else {
                let size = self.extent(self.stride(item.type_index)?, item.count)?;
                self.bytes(item.offset, size)?.to_vec()
            };
            items.push(ImageItem {
                type_index: item.type_index,
                kind: item.kind,
                count: item.count,
                data,
            });
        }
        Ok(TagfileImage {
            version: self.version,
            types: self.types.clone(),
            items,
            type_section: self.type_section().to_vec(),
        })
    }
}

/// What the walk from the root decided about the items.
struct Plan {
    /// The old index of each new item, from the null item.
    order: Vec<usize>,
    /// The old index of each item in the order `DATA` holds them.
    placed: Vec<usize>,
    /// The new index of each old item; 0 for one the walk did not reach.
    number: Vec<usize>,
    /// The kind of the slot that first referred to each old item.
    found_by: Vec<Option<SlotKind>>,
}

impl TagfileImage {
    /// Set the reference slot at byte `offset` of item `item` to refer to item `target` (0 for none): the index in the
    /// first four bytes, zeros after them up to the slot's width.
    pub fn set_reference(&mut self, item: usize, offset: usize, target: usize) -> Result<()> {
        let width = self.slot_at(item, offset)?.size;
        let end = add(offset, width)?;
        let value = word(target)?;
        let slot = self
            .items
            .get_mut(item)
            .and_then(|i| i.data.get_mut(offset..end))
            .ok_or_else(|| invalid(0, format!("item {item}: byte {offset} is outside its data")))?;
        slot.fill(0);
        for (to, from) in slot.iter_mut().zip(value.to_le_bytes()) {
            *to = from;
        }
        Ok(())
    }

    /// Make the string slot at byte `offset` of item `item` read `text`: a new run of `char` holds it, and the slot
    /// refers to that. Returns the new item's index. The characters it replaces stay in the image until a write
    /// leaves them out (nothing refers to them).
    pub fn set_string(&mut self, item: usize, offset: usize, text: &str) -> Result<usize> {
        if self.slot_at(item, offset)?.kind != SlotKind::String {
            return Err(invalid(
                0,
                format!("item {item}: the slot at byte {offset} is not a string"),
            ));
        }
        let char_type = self
            .types
            .iter()
            .position(|t| t.name == "char")
            .ok_or_else(|| invalid(0, "the type table has no char type"))?;
        let mut data = text.as_bytes().to_vec();
        data.push(0);
        let target = self.push_item(ImageItem {
            type_index: char_type,
            kind: ItemKind::Run,
            count: data.len(),
            data,
        });
        self.set_reference(item, offset, target)?;
        Ok(target)
    }

    /// The reference slot at byte `offset` of item `item`, counted from the start of the item's data.
    fn slot_at(&self, item: usize, offset: usize) -> Result<Slot> {
        let none = || invalid(0, format!("item {item} has no reference slot at byte {offset}"));
        let it = self.items.get(item).ok_or_else(none)?;
        let size = stride(&self.types, it.type_index)?;
        let slots = Layouts::new(&self.types).slots(it.type_index, 0)?;
        let within = offset.checked_rem(size).ok_or_else(none)?;
        slots
            .iter()
            .find(|s| s.offset == within)
            .copied()
            .ok_or_else(none)
    }

    /// Add an item and return its index. Nothing is written for it until a reference slot refers to it.
    pub fn push_item(&mut self, item: ImageItem) -> usize {
        self.items.push(item);
        self.items.len().saturating_sub(1)
    }

    /// The references an item stores, in the order they are numbered. Refused: bytes of the wrong length, a slot that
    /// holds more than an index, an index outside the items.
    fn references(&self, layouts: &mut Layouts<'_>, index: usize) -> Result<Vec<Reference>> {
        let item = self
            .items
            .get(index)
            .ok_or_else(|| invalid(0, format!("no item {index}")))?;
        let size = stride(&self.types, item.type_index)?;
        let want = mul(size, item.count)?;
        if item.data.len() != want {
            return Err(invalid(
                0,
                format!(
                    "item {index}: {} bytes for {} elements of {size}",
                    item.data.len(),
                    item.count
                ),
            ));
        }
        let slots = layouts.slots(item.type_index, 0)?;
        let mut references = Vec::new();
        if slots.is_empty() || size == 0 {
            return Ok(references);
        }
        for element in 0..item.count {
            let base = mul(element, size)?;
            for &slot in slots.iter() {
                let position = add(base, slot.offset)?;
                let bytes = item
                    .data
                    .get(position..add(position, slot.size)?)
                    .ok_or_else(|| invalid(0, format!("item {index}: a slot runs past its data")))?;
                let (head, rest) = bytes
                    .split_first_chunk::<4>()
                    .ok_or_else(|| invalid(0, format!("item {index}: a slot of {} bytes", slot.size)))?;
                if rest.iter().any(|&b| b != 0) {
                    return Err(invalid(
                        0,
                        format!("item {index}: bytes 4.. of the slot at {position} are not zero"),
                    ));
                }
                let target = u32::from_le_bytes(*head) as usize;
                if target == 0 {
                    continue;
                }
                if target >= self.items.len() {
                    return Err(invalid(
                        0,
                        format!("item {index} refers to item {target} of {}", self.items.len()),
                    ));
                }
                references.push(Reference {
                    target,
                    slot,
                    position,
                    key: (slot.kind == SlotKind::String, slot.offset, element),
                });
            }
        }
        references.sort_by_key(|r| r.key);
        Ok(references)
    }

    /// Walk the graph from the root, deciding the new numbers and the layout order.
    fn plan(&self, layouts: &mut Layouts<'_>) -> Result<Plan> {
        let n = self.items.len();
        let root = self
            .items
            .get(1)
            .ok_or_else(|| invalid(0, "the image has no root item"))?;
        if root.kind != ItemKind::Object || root.count != 1 {
            return Err(invalid(0, "item 1 is not a single object"));
        }
        let mut number = vec![0usize; n];
        let mut found_by = vec![None; n];
        let mut order = vec![0usize, 1];
        let mut placed = vec![1usize];
        if let Some(first) = number.get_mut(1) {
            *first = 1;
        }
        let mut objects = VecDeque::from([1usize]);
        while let Some(object) = objects.pop_front() {
            if object != 1 {
                placed.push(object);
            }
            let mut level = vec![object];
            while !level.is_empty() {
                let mut next = Vec::new();
                for &x in &level {
                    let mut runs = Vec::new();
                    for r in self.references(layouts, x)? {
                        let Some(done) = number.get_mut(r.target) else {
                            continue;
                        };
                        if *done != 0 {
                            continue;
                        }
                        order.push(r.target);
                        *done = order.len().saturating_sub(1);
                        if let Some(by) = found_by.get_mut(r.target) {
                            *by = Some(r.slot.kind);
                        }
                        match self.items.get(r.target).map(|i| i.kind) {
                            Some(ItemKind::Object) => objects.push_back(r.target),
                            Some(ItemKind::Run) => runs.push(r.target),
                            _ => return Err(invalid(0, format!("item {} is the null item", r.target))),
                        }
                    }
                    // the runs were numbered in this order, which is the order they are laid out in
                    placed.extend(&runs);
                    next.extend(runs);
                }
                level = next;
            }
        }
        Ok(Plan {
            order,
            placed,
            number,
            found_by,
        })
    }

    /// Write the image as a tagfile.
    ///
    /// The type table is written as read; the items are numbered and laid out as the module documentation says. The
    /// output of a file read as it is, and not edited, is that file's bytes.
    pub fn write(&self) -> Result<Vec<u8>> {
        let mut layouts = Layouts::new(&self.types);
        let plan = self.plan(&mut layouts)?;

        let mut data: Vec<u8> = Vec::new();
        let mut offsets = vec![0usize; self.items.len()];
        let mut patches: Vec<(usize, usize)> = Vec::new();
        for &old in &plan.placed {
            let item = self
                .items
                .get(old)
                .ok_or_else(|| invalid(0, format!("no item {old}")))?;
            let alignment = match (item.kind, plan.found_by.get(old).copied().flatten()) {
                (ItemKind::Run, Some(SlotKind::Array)) => 16,
                (ItemKind::Run, Some(SlotKind::String)) => 2,
                _ => inherited(&self.types, item.type_index, |t| t.alignment)?.max(2),
            };
            let start = add(data.len(), alignment.saturating_sub(1))?
                .checked_div(alignment)
                .and_then(|n| n.checked_mul(alignment))
                .ok_or_else(overflow)?;
            data.resize(start, 0);
            if let Some(at) = offsets.get_mut(old) {
                *at = start;
            }
            let mut bytes = item.data.clone();
            for r in self.references(&mut layouts, old)? {
                let new = word(plan.number.get(r.target).copied().unwrap_or_default())?;
                let slot = bytes
                    .get_mut(r.position..add(r.position, 4)?)
                    .ok_or_else(|| invalid(0, "a slot runs past its data"))?;
                slot.copy_from_slice(&new.to_le_bytes());
                patches.push((r.slot.type_index, add(start, r.position)?));
            }
            data.extend(bytes);
        }
        let length = add(data.len(), 15)?
            .checked_div(16)
            .and_then(|n| n.checked_mul(16))
            .ok_or_else(overflow)?;
        data.resize(length, 0);

        let mut items = vec![0u8; 12];
        for &old in plan.order.iter().skip(1) {
            let item = self
                .items
                .get(old)
                .ok_or_else(|| invalid(0, format!("no item {old}")))?;
            let kind: u32 = match item.kind {
                ItemKind::Object => 0x10,
                ItemKind::Run => 0x20,
                ItemKind::Null => return Err(invalid(0, "the null item cannot be written")),
            };
            let type_index = word(item.type_index)?;
            if type_index >= 1 << 24 {
                return Err(invalid(
                    0,
                    format!("type index {type_index} does not fit 24 bits"),
                ));
            }
            items.extend((type_index | kind << 24).to_le_bytes());
            items.extend(word(offsets.get(old).copied().unwrap_or_default())?.to_le_bytes());
            items.extend(word(item.count)?.to_le_bytes());
        }
        patches.sort_unstable();
        let mut patch_table = Vec::new();
        let mut at = 0;
        while let Some(&(type_index, _)) = patches.get(at) {
            let run = patches
                .get(at..)
                .unwrap_or_default()
                .iter()
                .take_while(|p| p.0 == type_index)
                .count();
            patch_table.extend(word(type_index)?.to_le_bytes());
            patch_table.extend(word(run)?.to_le_bytes());
            for &(_, offset) in patches.get(at..add(at, run)?).unwrap_or_default() {
                patch_table.extend(word(offset)?.to_le_bytes());
            }
            at = add(at, run)?;
        }

        let mut index = section(*b"ITEM", true, &items)?;
        if !patch_table.is_empty() {
            index.extend(section(*b"PTCH", true, &patch_table)?);
        }
        let mut root = section(*b"SDKV", true, &self.version)?;
        root.extend(section(*b"DATA", true, &data)?);
        root.extend(&self.type_section);
        root.extend(section(*b"INDX", false, &index)?);
        section(*b"TAG0", false, &root)
    }
}

/// A section: its length (the header's eight bytes with it) and, on a leaf, the flag in the top bits.
fn section(signature: [u8; 4], leaf: bool, payload: &[u8]) -> Result<Vec<u8>> {
    let size = word(add(payload.len(), 8)?)?;
    if size >= 1 << 30 {
        return Err(invalid(0, "a section of 1 GiB or more"));
    }
    let mut out = (size | if leaf { 1 << 30 } else { 0 }).to_be_bytes().to_vec();
    out.extend(signature);
    out.extend(payload);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_section_is_its_length_and_signature() {
        let s = section(*b"SDKV", true, b"20160200").expect("section");
        assert_eq!(s, b"\x40\0\0\x10SDKV20160200");
        let s = section(*b"INDX", false, b"").expect("section");
        assert_eq!(s, b"\0\0\0\x08INDX");
    }
}
