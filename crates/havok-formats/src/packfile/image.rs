//! An editable image of a packfile, and the writer that lays it out again.
//!
//! [`Packfile::image`] turns a packfile into the objects it holds, each a tree of byte blocks: the object's body, the
//! arrays and strings its pointer slots lead to (a pointer slot with a local fixup owns the run it points at), and the
//! other objects its slots refer to (a pointer slot with a global fixup). [`PackfileImage::write`] makes the file from
//! it by the rules every file of the games follows, found on the data and held on all of it: all 490 Tony Hawk's SHRED
//! (PS3) files, version 7, and the 35 distinct packfiles of Sekiro and Elden Ring, versions 9 and 11, are read, imaged and
//! written back byte for byte.
//!
//! The rules:
//!
//! - The objects are written in the order a depth-first walk from the root finds them, each object's pointer slots taken
//!   in order of their offset, a slot that owns a run followed into the run at its place; an object already written is not
//!   written again. Every object starts on a 16-byte boundary.
//! - An object is its body, then the runs its body's slots own in slot order, each starting on a 16-byte boundary. A run is
//!   its bytes, then the runs its own slots own in slot order, each starting on a 2-byte boundary (the nested runs of the
//!   data are strings), then zeros up to the next 16-byte boundary. So a short array of structs is followed at once by the
//!   strings its elements point to, and each string after the first starts on the boundary the one before it was padded
//!   to. The exception is a run that is nothing but pointers, each to a run of its own (an array of strings): its runs
//!   follow one another from 2-byte boundaries with no padding between, and the zeros come after the last
//!   ([`Block::packed`]).
//! - The three fixup tables hold one entry per pointer slot that owns a run (local), per pointer slot that points to an
//!   object (global) and per object (virtual). Local and global entries are in the order the walk meets the slots (a
//!   run's own slots follow the slot that owns it, before the next slot of its parent's); virtual ones ascend by the
//!   object's offset. Each table is followed by `0xFFFFFFFF` words up to a 16-byte boundary (none if it ends on one).
//!   Exports and imports are empty.
//! - `__classnames__` lists the four reflection classes (`hkClass`, `hkClassMember`, `hkClassEnum`, `hkClassEnumItem`)
//!   first, then every other class in the order its first object is written, each as `signature, 0x09, name, 0`, then
//!   `0xFF` padding to 16 bytes. `__types__` holds the image's `types` in the order given, laid out like the data, or
//!   nothing. All header bytes no field names are `0xFF`.
//!
//! A file that is laid out otherwise is written in this layout, with the same objects and pointers.
use std::collections::BTreeMap;

use super::{CLASS_SIGNATURES, KNOWN, Object, Packfile, ROOT_HEADER, Rules, SECTION_FIELDS};
use crate::{Result, invalid};

/// The deepest chain of runs inside runs an image holds or writes. The data nests runs two deep (an array of structs
/// whose elements point to strings); a deeper chain is refused so that no file can make the readers or the drop of the
/// image recurse without bound.
pub const MAX_NESTING: usize = 8;
/// The reflection classes every packfile lists first, in this order.
const REFLECTION_CLASSES: [&str; 4] = ["hkClass", "hkClassMember", "hkClassEnum", "hkClassEnumItem"];
/// The alignment the runs an object's body points to start on.
pub const RUN_ALIGNMENT: usize = 16;
/// The alignment the runs a run points to start on.
pub const NESTED_RUN_ALIGNMENT: usize = 2;

/// What a pointer slot of a block points to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A run the slot owns: array elements or characters, written inside the object that holds the slot.
    Run(Block),
    /// An object of the image, by its index in [`PackfileImage::objects`].
    Object(usize),
}

/// A pointer slot of a block: `pointer_size` zero bytes at `offset` in the block's bytes, and where they point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    /// The slot's offset in the block's bytes.
    pub offset: usize,
    /// What the slot points to.
    pub target: Target,
}

/// A run of bytes with the pointer slots in it: an object's body, or an array's elements, or characters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    /// The bytes, padding included: the file's own bytes up to where the next block starts.
    pub bytes: Vec<u8>,
    /// The pointer slots in `bytes`, ascending and not overlapping.
    pub slots: Vec<Slot>,
    /// The boundary the block's first byte is written on: [`RUN_ALIGNMENT`] for what an object's body points to,
    /// [`NESTED_RUN_ALIGNMENT`] for what a run points to. Not used for a body.
    pub align: usize,
    /// Whether the block is an array of pointers to runs (its bytes are nothing but those pointers): the runs it owns are
    /// then written one after the other from a 2-byte boundary, as the strings of an array of strings are, and the zeros
    /// up to the next 16-byte boundary follow the last of them instead of each.
    pub packed: bool,
}

impl Block {
    /// A block of `bytes` with no pointer slots, for a run an object's body points to.
    pub fn run(bytes: Vec<u8>) -> Self {
        Block {
            bytes,
            slots: Vec::new(),
            align: RUN_ALIGNMENT,
            packed: false,
        }
    }
}

/// An object: a class and the block of its body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageObject {
    /// The class name.
    pub class: String,
    /// The object: body bytes and what its pointer slots lead to.
    pub block: Block,
}

/// An editable copy of a packfile; see the [module](self) for the layout [`write`](Self::write) gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackfileImage {
    /// The file version: 7, 9 or 11.
    pub version: u32,
    /// The layout rules.
    pub rules: Rules,
    /// The user tag.
    pub user_tag: u32,
    /// The contents version text.
    pub contents_version: String,
    /// The two header words at `0x38` and `0x3C`, as stored.
    pub filler: [u32; 2],
    /// The predicate region of a version 11 file, as stored.
    pub predicate_region: Vec<u8>,
    /// Every class name the file lists, with its signature: what [`write`](Self::write) takes a signature from.
    pub signatures: Vec<(String, u32)>,
    /// The objects, in any order; [`write`](Self::write) orders them.
    pub objects: Vec<ImageObject>,
    /// The objects of the `__types__` section (the class metadata of a file written with reflection; none in 517 of
    /// the 525 packfiles of the games), in the order they are written in. Their pointers lead to one another only.
    pub types: Vec<ImageObject>,
    /// The index of the root object.
    pub root: usize,
}

fn add(a: usize, b: usize) -> Result<usize> {
    a.checked_add(b)
        .ok_or_else(|| invalid(0, "an offset overflows the address space"))
}

fn align_up(value: usize, to: usize) -> Result<usize> {
    if to == 0 || !to.is_power_of_two() {
        return Err(invalid(
            0,
            format!("{to} is not a boundary a block can be aligned on"),
        ));
    }
    add(value, to.saturating_sub(1)).map(|v| v & !to.saturating_sub(1))
}

fn word(value: usize) -> Result<u32> {
    u32::try_from(value).map_err(|_| invalid(0, "the file would need an offset beyond 32 bits"))
}

impl Packfile<'_> {
    /// An editable image of the file: its objects, the arrays and strings they own and the objects they point to. The
    /// objects of the `__types__` section keep the order they are in; an object of the data nothing leads to from the root,
    /// a pointer slot with bytes in it, a run pointed at from before it or from two slots, and runs nested deeper than
    /// [`MAX_NESTING`] are refused, because writing the image would change them.
    pub fn image(&self) -> Result<PackfileImage> {
        let mut objects = Vec::with_capacity(self.objects.len());
        for object in &self.objects {
            objects.push(ImageObject {
                class: self.class_name(object).to_owned(),
                block: self.block_of(object)?,
            });
        }
        let types = match self.types()? {
            Some(view) => {
                let mut list = Vec::with_capacity(view.objects.len());
                for object in &view.objects {
                    list.push(ImageObject {
                        class: view.class_name(object).to_owned(),
                        block: view.block_of(object)?,
                    });
                }
                list
            }
            None => Vec::new(),
        };
        let root = self
            .objects
            .binary_search_by_key(&self.root, |o| o.offset)
            .map_err(|_| self.err(self.root, "the root is not an object"))?;
        let image = PackfileImage {
            version: self.version,
            rules: self.rules,
            user_tag: self.user_tag,
            contents_version: self.contents_version.clone(),
            filler: self.filler,
            predicate_region: self.predicate_region.to_vec(),
            signatures: self
                .classes
                .iter()
                .map(|c| (c.name.clone(), c.signature))
                .collect(),
            objects,
            types,
            root,
        };
        let reached = image.walk_order()?;
        if reached.len() != image.objects.len() {
            let missing = (0..image.objects.len())
                .find(|i| !reached.contains(i))
                .unwrap_or(0);
            let at = self.objects.get(missing).map_or(0, |o| o.offset);
            return Err(self.err(at, "an object that nothing leads to from the root"));
        }
        Ok(image)
    }

    /// The block tree of one object.
    fn block_of(&self, object: &Object) -> Result<Block> {
        let end = add(object.offset, object.size).map_err(|e| self.err(object.offset, e.detail))?;
        let width = self.rules.pointer_size;
        let first = self.local.partition_point(|&(src, _)| src < object.offset);
        let local: Vec<(usize, usize)> = self
            .local
            .get(first..)
            .unwrap_or_default()
            .iter()
            .copied()
            .take_while(|&(src, _)| src < end)
            .collect();
        let first = self.global.partition_point(|&(src, _)| src < object.offset);
        let global: Vec<(usize, usize)> = self
            .global
            .get(first..)
            .unwrap_or_default()
            .iter()
            .copied()
            .take_while(|&(src, _)| src < end)
            .collect();
        // the runs start where local fixups point, in file order; region 0 is the body, region i + 1 run i
        let mut starts: Vec<usize> = local.iter().map(|&(_, dst)| dst).collect();
        starts.sort_unstable();
        if starts.windows(2).any(|w| w.first() == w.get(1)) {
            return Err(self.err(object.offset, "two pointer slots of an object point to one run"));
        }
        if starts.iter().any(|&d| d <= object.offset || d >= end) {
            return Err(self.err(object.offset, "a local fixup points outside its object"));
        }
        let region_start = |region: usize| -> usize {
            match region.checked_sub(1) {
                None => object.offset,
                Some(run) => starts.get(run).copied().unwrap_or(end),
            }
        };
        let region_end = |region: usize| -> usize { starts.get(region).copied().unwrap_or(end) };
        let region_of = |src: usize| starts.partition_point(|&s| s <= src);
        // the slots of each region, in order, with where they point
        let regions = add(starts.len(), 1)?;
        let mut slots: Vec<Vec<(usize, SlotTarget)>> = vec![Vec::new(); regions];
        for &(src, dst) in &local {
            let region = region_of(src);
            let run = starts
                .binary_search(&dst)
                .map_err(|_| self.err(src, "a local fixup does not point at a run"))?;
            let child = add(run, 1)?;
            if child <= region {
                return Err(self.err(src, "a pointer slot points to a run that is laid out before it"));
            }
            slots
                .get_mut(region)
                .ok_or_else(|| self.err(src, "a pointer slot outside its object"))?
                .push((src, SlotTarget::Run(child)));
        }
        for &(src, dst) in &global {
            let target = self
                .objects
                .binary_search_by_key(&dst, |o| o.offset)
                .map_err(|_| self.err(src, "a global fixup does not point at an object"))?;
            slots
                .get_mut(region_of(src))
                .ok_or_else(|| self.err(src, "a pointer slot outside its object"))?
                .push((src, SlotTarget::Object(target)));
        }
        // build from the last region up: a run is only ever pointed at from an earlier region
        let mut built: Vec<Option<Block>> = vec![None; regions];
        for region in (0..regions).rev() {
            let start = region_start(region);
            let stop = region_end(region);
            let bytes = self.bytes(start, stop.saturating_sub(start))?.to_vec();
            let mut list = slots.get(region).cloned().unwrap_or_default();
            list.sort_unstable_by_key(|&(src, _)| src);
            let mut out = Vec::with_capacity(list.len());
            let mut previous_end = start;
            for (src, target) in list {
                let slot_end = add(src, width).map_err(|e| self.err(src, e.detail))?;
                if src < previous_end || slot_end > stop {
                    return Err(self.err(src, "a pointer slot overlaps another or leaves its block"));
                }
                previous_end = slot_end;
                if self.bytes(src, width)?.iter().any(|&b| b != 0) {
                    return Err(self.err(src, "a pointer slot holds bytes; only zeros are written"));
                }
                let target = match target {
                    SlotTarget::Object(index) => Target::Object(index),
                    SlotTarget::Run(child) => {
                        let block = built
                            .get_mut(child)
                            .and_then(Option::take)
                            .ok_or_else(|| self.err(src, "a run is pointed at twice"))?;
                        Target::Run(block)
                    }
                };
                out.push(Slot {
                    offset: src.saturating_sub(start),
                    target,
                });
            }
            if let Some(slot) = built.get_mut(region) {
                *slot = Some(Block {
                    bytes,
                    slots: out,
                    align: RUN_ALIGNMENT,
                    packed: false,
                });
            }
        }
        let mut body = built
            .into_iter()
            .next()
            .flatten()
            .ok_or_else(|| self.err(object.offset, "an object without a body"))?;
        // the nested runs start on the nested boundary, the rest on the run boundary; the depth is bounded
        set_alignments(&mut body, 0, width)
            .map_err(|depth| self.err(object.offset, format!("runs are nested {depth} deep")))?;
        Ok(body)
    }
}

/// Where a slot points while the tree is built: a region of the object (a run, counted from 1) or an object.
#[derive(Debug, Clone, Copy)]
enum SlotTarget {
    Run(usize),
    Object(usize),
}

/// Give every run the alignment of its place: [`RUN_ALIGNMENT`] under a body, [`NESTED_RUN_ALIGNMENT`] under a run; and
/// mark a run that is an array of pointers to runs as packed. Err with the depth reached when it passes
/// [`MAX_NESTING`].
fn set_alignments(body: &mut Block, depth: usize, width: usize) -> std::result::Result<(), usize> {
    if depth > MAX_NESTING {
        return Err(depth);
    }
    let align = if depth == 0 {
        RUN_ALIGNMENT
    } else {
        NESTED_RUN_ALIGNMENT
    };
    // a run of pointers and nothing else: one slot every `width` bytes from its start, each owning a run
    let tiled = depth > 0
        && !body.slots.is_empty()
        && body.bytes.len() == body.slots.len().saturating_mul(width)
        && body
            .slots
            .iter()
            .enumerate()
            .all(|(i, s)| s.offset == i.saturating_mul(width) && matches!(s.target, Target::Run(_)));
    body.packed = tiled;
    for slot in &mut body.slots {
        if let Target::Run(run) = &mut slot.target {
            run.align = align;
            set_alignments(run, depth.saturating_add(1), width)?;
        }
    }
    Ok(())
}

/// The fixups a layout produces: absolute offsets in the data section.
#[derive(Default)]
struct Fixups {
    local: Vec<(usize, usize)>,
    global: Vec<(usize, usize)>,
}

impl PackfileImage {
    /// The indices of the objects in the order the writer writes them: a depth-first walk from the root.
    fn walk_order(&self) -> Result<Vec<usize>> {
        let mut order = Vec::new();
        let mut seen = vec![false; self.objects.len()];
        let mut stack = vec![self.root];
        while let Some(index) = stack.pop() {
            let Some(flag) = seen.get_mut(index) else {
                return Err(invalid(0, format!("object {index} does not exist")));
            };
            if *flag {
                continue;
            }
            *flag = true;
            order.push(index);
            let object = self
                .objects
                .get(index)
                .ok_or_else(|| invalid(0, format!("object {index} does not exist")))?;
            let mut kids = Vec::new();
            collect_objects(&object.block, 0, &mut kids)?;
            stack.extend(kids.into_iter().rev());
        }
        Ok(order)
    }

    /// The signature of class `name`: the file's own, else the 7.1 table's.
    fn signature(&self, name: &str) -> Result<u32> {
        self.signatures
            .iter()
            .find(|(n, _)| n == name)
            .map(|&(_, s)| s)
            .or_else(|| CLASS_SIGNATURES.iter().find(|(n, _)| *n == name).map(|&(_, s)| s))
            .ok_or_else(|| invalid(0, format!("class {name} has no signature in the image")))
    }

    /// Lay the image out as a packfile; see the [module](self) for the rules.
    pub fn write(&self) -> Result<Vec<u8>> {
        let width = self.rules.pointer_size;
        let known = KNOWN
            .iter()
            .find(|k| k.version == self.version && k.rules == self.rules)
            .ok_or_else(|| {
                invalid(
                    0,
                    format!(
                        "version {} with these layout rules cannot be written",
                        self.version
                    ),
                )
            })?;
        let order = self.walk_order()?;
        let data = lay_out(&self.objects, &order, width)?;
        let types_order: Vec<usize> = (0..self.types.len()).collect();
        let types = lay_out(&self.types, &types_order, width)?;
        // the class names
        let mut classes: Vec<&str> = Vec::new();
        for name in REFLECTION_CLASSES {
            if self.signatures.iter().any(|(n, _)| n == name) {
                classes.push(name);
            }
        }
        for object in order
            .iter()
            .filter_map(|&i| self.objects.get(i))
            .chain(&self.types)
        {
            if !classes.contains(&object.class.as_str()) {
                classes.push(&object.class);
            }
        }
        let big = !self.rules.little_endian;
        let put32 =
            |out: &mut Vec<u8>, v: u32| out.extend(if big { v.to_be_bytes() } else { v.to_le_bytes() });
        let mut names: Vec<u8> = Vec::new();
        let mut name_offset: BTreeMap<&str, usize> = BTreeMap::new();
        for name in &classes {
            put32(&mut names, self.signature(name)?);
            names.push(9);
            name_offset.insert(name, names.len());
            names.extend(name.as_bytes());
            names.push(0);
        }
        let names_len = align_up(names.len(), 16)?;
        names.resize(names_len, 0xFF);
        // the fixup tables of both sections
        let tables = |laid: &Laid, objects: &[ImageObject], section: u32| -> Result<[Vec<u8>; 3]> {
            let table = |entries: Vec<Vec<u32>>| -> Vec<u8> {
                let mut out = Vec::new();
                for w in entries.into_iter().flatten() {
                    out.extend(if big { w.to_be_bytes() } else { w.to_le_bytes() });
                }
                let padded = out.len().div_ceil(16).saturating_mul(16);
                out.resize(padded, 0xFF);
                out
            };
            let mut local = Vec::new();
            for &(s, d) in &laid.local {
                local.push(vec![word(s)?, word(d)?]);
            }
            let mut global = Vec::new();
            for &(s, d) in &laid.global {
                global.push(vec![word(s)?, section, word(d)?]);
            }
            let mut virtuals = Vec::new();
            for &(start, index) in &laid.virtuals {
                let class = objects
                    .get(index)
                    .map(|o| o.class.as_str())
                    .ok_or_else(|| invalid(0, format!("object {index} does not exist")))?;
                let at = name_offset
                    .get(class)
                    .copied()
                    .ok_or_else(|| invalid(0, format!("class {class} is not listed")))?;
                virtuals.push(vec![word(start)?, 0, word(at)?]);
            }
            Ok([table(local), table(global), table(virtuals)])
        };
        let data_tables = tables(&data, &self.objects, 2)?;
        let types_tables = tables(&types, &self.types, 1)?;
        // the headers
        let sections = 3usize;
        let first_section = add(ROOT_HEADER, self.predicate_region.len())?;
        let names_start = add(first_section, sections.saturating_mul(known.stride))?;
        let types_start = add(names_start, names.len())?;
        // a section: its bytes, then the three tables; the offsets of each part from the section's start
        let offsets = |bytes: &[u8], tables: &[Vec<u8>; 3]| -> Result<[usize; 6]> {
            let [local, global, virtuals] = tables;
            let local_at = bytes.len();
            let global_at = add(local_at, local.len())?;
            let virtual_at = add(global_at, global.len())?;
            let exports_at = add(virtual_at, virtuals.len())?;
            Ok([
                local_at, global_at, virtual_at, exports_at, exports_at, exports_at,
            ])
        };
        let types_empty = self.types.is_empty();
        let types_offsets = if types_empty {
            [0; 6]
        } else {
            offsets(&types.bytes, &types_tables)?
        };
        let types_size = if types_empty { 0 } else { types_offsets[5] };
        let data_start = add(types_start, types_size)?;
        let data_offsets = offsets(&data.bytes, &data_tables)?;
        let root_offset = data
            .offset_of
            .get(&self.root)
            .copied()
            .ok_or_else(|| invalid(0, "the root object does not exist"))?;
        let root_class = self
            .objects
            .get(self.root)
            .map(|o| o.class.as_str())
            .ok_or_else(|| invalid(0, "the root object does not exist"))?;
        let root_name = name_offset
            .get(root_class)
            .copied()
            .ok_or_else(|| invalid(0, "the root class is not listed"))?;
        let mut out: Vec<u8> = Vec::new();
        out.extend([0x57, 0xE0, 0xE0, 0x57, 0x10, 0xC0, 0xC0, 0x10]);
        put32(&mut out, self.user_tag);
        put32(&mut out, self.version);
        out.extend(self.rules.bytes());
        put32(&mut out, word(sections)?);
        put32(&mut out, 2);
        put32(&mut out, word(root_offset)?);
        put32(&mut out, 0);
        put32(&mut out, word(root_name)?);
        let mut text = self.contents_version.as_bytes().to_vec();
        text.push(0);
        text.resize(16, 0xFF);
        out.extend(text);
        put32(&mut out, self.filler[0]);
        put32(&mut out, self.filler[1]);
        out.extend(&self.predicate_region);
        let section = |out: &mut Vec<u8>, tag: &str, start: usize, parts: [usize; 6]| -> Result<()> {
            let mut t = tag.as_bytes().to_vec();
            t.resize(19, 0);
            t.push(0xFF);
            out.extend(t);
            put32(out, word(start)?);
            for f in parts {
                put32(out, word(f)?);
            }
            out.resize(add(out.len(), known.stride.saturating_sub(SECTION_FIELDS))?, 0xFF);
            Ok(())
        };
        let names_end = names.len();
        section(&mut out, "__classnames__", names_start, [names_end; 6])?;
        section(&mut out, "__types__", types_start, types_offsets)?;
        section(&mut out, "__data__", data_start, data_offsets)?;
        out.extend(names);
        if !types_empty {
            out.extend(types.bytes);
            for table in types_tables {
                out.extend(table);
            }
        }
        out.extend(data.bytes);
        for table in data_tables {
            out.extend(table);
        }
        Ok(out)
    }
}

/// One section's objects laid out: the bytes and the pointers in them, as offsets in the section.
struct Laid {
    bytes: Vec<u8>,
    local: Vec<(usize, usize)>,
    global: Vec<(usize, usize)>,
    /// Each object's offset and index, ascending by offset.
    virtuals: Vec<(usize, usize)>,
    offset_of: BTreeMap<usize, usize>,
}

/// Write the objects `order` names, in that order, each on a 16-byte boundary.
fn lay_out(objects: &[ImageObject], order: &[usize], width: usize) -> Result<Laid> {
    let mut bytes: Vec<u8> = Vec::new();
    let mut fixups = Fixups::default();
    let mut offset_of: BTreeMap<usize, usize> = BTreeMap::new();
    let mut virtuals: Vec<(usize, usize)> = Vec::new();
    for &index in order {
        let object = objects
            .get(index)
            .ok_or_else(|| invalid(0, format!("object {index} does not exist")))?;
        pad_to(&mut bytes, 16)?;
        let start = bytes.len();
        offset_of.insert(index, start);
        virtuals.push((start, index));
        place(&object.block, width, &mut bytes, &mut fixups, 0, true)?;
    }
    pad_to(&mut bytes, 16)?;
    // global fixups name their target's offset now that every object has one
    let mut global = Vec::with_capacity(fixups.global.len());
    for &(src, target) in &fixups.global {
        let dst = offset_of
            .get(&target)
            .copied()
            .ok_or_else(|| invalid(0, format!("object {target} does not exist")))?;
        global.push((src, dst));
    }
    // the fixups stay in the order the walk met their slots
    let local = fixups.local;
    Ok(Laid {
        bytes,
        local,
        global,
        virtuals,
        offset_of,
    })
}

/// The indices of the objects a block and its runs point to, in the order the walk meets them.
fn collect_objects(block: &Block, depth: usize, out: &mut Vec<usize>) -> Result<()> {
    if depth > MAX_NESTING {
        return Err(invalid(
            0,
            format!("runs are nested more than {MAX_NESTING} deep"),
        ));
    }
    for slot in &block.slots {
        match &slot.target {
            Target::Object(index) => out.push(*index),
            Target::Run(run) => collect_objects(run, depth.saturating_add(1), out)?,
        }
    }
    Ok(())
}

fn pad_to(data: &mut Vec<u8>, to: usize) -> Result<()> {
    let padded = align_up(data.len(), to)?;
    data.resize(padded, 0);
    Ok(())
}

/// Write a block at the end of `data`: its bytes, then each run its slots own, then zeros to a 16-byte boundary.
fn place(
    block: &Block,
    width: usize,
    data: &mut Vec<u8>,
    fixups: &mut Fixups,
    depth: usize,
    pad_end: bool,
) -> Result<()> {
    if depth > MAX_NESTING {
        return Err(invalid(
            0,
            format!("runs are nested more than {MAX_NESTING} deep"),
        ));
    }
    let base = data.len();
    data.extend(&block.bytes);
    let mut previous_end = 0usize;
    for slot in &block.slots {
        let end = add(slot.offset, width)?;
        if slot.offset < previous_end || end > block.bytes.len() {
            return Err(invalid(
                0,
                format!(
                    "the pointer slot at byte {} overlaps another or leaves its block",
                    slot.offset
                ),
            ));
        }
        previous_end = end;
        let src = add(base, slot.offset)?;
        match &slot.target {
            Target::Object(index) => fixups.global.push((src, *index)),
            Target::Run(run) => {
                pad_to(data, run.align)?;
                let dst = data.len();
                fixups.local.push((src, dst));
                place(run, width, data, fixups, depth.saturating_add(1), !block.packed)?;
            }
        }
    }
    if pad_end { pad_to(data, 16) } else { Ok(()) }
}
