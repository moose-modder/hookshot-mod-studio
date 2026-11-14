#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use crate::types::{Cursor, read_types, required};
use crate::{Compendium, Result, Type, invalid, sections};

/// SDK versions whose tagfile layout this reader knows: 2015.1, 2016.2 and 2018.1. All store the same section tree and
/// the same item and patch tables; class layouts come from each file's own type metadata, and they differ between the
/// SDKs (a 2015.1 `hkReferencedObject` is 16 bytes, a 2016.2 one 24). Sekiro holds one 2015.1 file, an animation clip;
/// its other tagfiles are 2016.2, and Elden Ring's are 2018.1 beside 42 of 2016.2.
pub const SDK_VERSIONS: [[u8; 8]; 3] = [*b"20150100", *b"20160200", *b"20180100"];

/// What an item holds, from the top nibble of the byte stored beside its type index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    /// The null item, item 0.
    Null,
    /// Byte `0x10`: one object, the root or what a pointer refers to (a count of 1 in every item of both installs).
    Object,
    /// Byte `0x20`: a run of elements, what an array or a string refers to (any count).
    Run,
}

/// One entry of the tagfile's item table: `count` consecutive elements of one type at `offset` in DATA. Item 0 is
/// the null item; item 1 holds the root object.
#[derive(Debug, Clone, Copy)]
pub struct Item {
    /// Index into [`Tagfile::types`] (one-based; never 0 for an item after the null item).
    pub type_index: usize,
    /// What the item holds. The byte stored beside the type index is `0x10` or `0x20` on every item of Sekiro's
    /// 2,831,175 and Elden Ring's 7,330,933 and `0` on the null item; the low nibble (flags) is `0` in all of them,
    /// and any other byte is refused.
    pub kind: ItemKind,
    /// Byte offset of the first element in DATA.
    pub offset: usize,
    /// Number of elements.
    pub count: usize,
}

/// A parsed `TAG0` tagfile: its types, its item table and a borrow of its DATA section.
///
/// Objects are read lazily through [`Tagfile::root`], [`Tagfile::item`] and the [`crate::Value`] and [`crate::Array`] views they
/// return; nothing is decoded until asked for.
#[derive(Debug)]
pub struct Tagfile<'a> {
    /// The SDK version string as stored, e.g. `20160200`; one of [`SDK_VERSIONS`].
    pub version: [u8; 8],
    /// The type table. Index 0 is the null type; the file's own, or the compendium's.
    pub types: Vec<Type>,
    /// The item table, from the null item at index 0.
    pub items: Vec<Item>,
    /// The PTCH section: per type index, the DATA offsets of the stored item references of that type. Empty when the
    /// file has no PTCH section.
    pub patches: Vec<(usize, Vec<usize>)>,
    data: &'a [u8],
    /// The type table's section as stored, its header included (`TYPE`, or the `TCRF` reference to a compendium).
    type_section: &'a [u8],
    /// The file offset of DATA's first byte.
    data_offset: usize,
    /// The file offset of the item table (`ITEM`), where an item index is looked up.
    pub(crate) items_offset: usize,
    /// The file offset of the type table (`TYPE` or `TCRF`), where a type index is looked up.
    types_offset: usize,
}

impl<'a> Tagfile<'a> {
    /// Parse and check the file's sections, types, item table and patch table. A file whose types are stored in an
    /// external compendium (`TCRF`) needs `compendium`; one that carries its own (`TYPE`) ignores it. Item extents
    /// and patch references are checked against DATA, so later reads cannot leave it.
    pub fn read(bytes: &'a [u8], compendium: Option<&Compendium>) -> Result<Self> {
        let roots = sections(bytes)?;
        let &[root] = roots.as_slice() else {
            return Err(invalid(0, "expected one TAG0 root"));
        };
        if root.signature != *b"TAG0" {
            return Err(invalid(0, "expected one TAG0 root"));
        }
        let children = root.children()?;
        for s in &children {
            if ![*b"SDKV", *b"DATA", *b"TYPE", *b"TCRF", *b"INDX"].contains(&s.signature) {
                return Err(invalid(s.offset, "unknown tagfile section"));
            }
        }
        let sdk = required(&children, &[*b"SDKV"])?;
        let version: [u8; 8] = sdk
            .payload
            .try_into()
            .map_err(|_| invalid(sdk.offset, "invalid SDK version"))?;
        if !SDK_VERSIONS.contains(&version) {
            return Err(invalid(sdk.offset, "unsupported SDK version"));
        }
        let ts = required(&children, &[*b"TYPE", *b"TCRF"])?;
        let types = if ts.signature == *b"TYPE" {
            read_types(ts)?
        } else {
            let mut c = Cursor::new(ts);
            let id: [u8; 8] = c.array()?;
            c.padding()?;
            let cmp = compendium.ok_or_else(|| invalid(ts.offset, "external compendium required"))?;
            if !cmp.ids.contains(&id) {
                return Err(invalid(ts.offset, "compendium ID mismatch"));
            }
            cmp.types.clone()
        };
        let data = required(&children, &[*b"DATA"])?;
        let indices = required(&children, &[*b"INDX"])?.children()?;
        for s in &indices {
            if ![*b"ITEM", *b"PTCH"].contains(&s.signature) {
                return Err(invalid(s.offset, "unknown index section"));
            }
        }
        let table = required(&indices, &[*b"ITEM"])?;
        if table.payload.len() % 12 != 0 {
            return Err(invalid(table.offset, "partial item record"));
        }
        let mut items = Vec::new();
        for (i, &[t0, t1, t2, kind, o0, o1, o2, o3, n0, n1, n2, n3]) in
            table.payload.as_chunks::<12>().0.iter().enumerate()
        {
            // a little-endian word: the type index in the low 24 bits, the kind in the top byte
            let type_index = u32::from_le_bytes([t0, t1, t2, 0]) as usize;
            if type_index >= types.len() {
                return Err(invalid(table.offset, "item type out of bounds"));
            }
            let kind = match (i, kind) {
                (0, 0) => ItemKind::Null,
                (1.., 0x10) => ItemKind::Object,
                (1.., 0x20) => ItemKind::Run,
                _ => {
                    return Err(invalid(
                        // the record's fourth byte: 8 bytes of section header, 12 bytes a record
                        table
                            .offset
                            .saturating_add(i.saturating_mul(12))
                            .saturating_add(11),
                        format!(
                            "item {i}: kind byte {kind:#x}; only 0x10 (object) and 0x20 (run) are stored, and 0 on the null item"
                        ),
                    ));
                }
            };
            items.push(Item {
                type_index,
                kind,
                offset: u32::from_le_bytes([o0, o1, o2, o3]) as usize,
                count: u32::from_le_bytes([n0, n1, n2, n3]) as usize,
            });
        }
        // the null item first (type, count and offset 0), and at least the root after it
        let null = items.first().map(|i| (i.type_index, i.count, i.offset));
        if items.len() < 2 || null != Some((0, 0, 0)) {
            return Err(invalid(table.offset, "missing null/root item"));
        }
        let mut patches = Vec::new();
        let patch_sections = indices
            .iter()
            .filter(|s| s.signature == *b"PTCH")
            .collect::<Vec<_>>();
        if patch_sections.len() > 1 {
            return Err(invalid(table.offset, "duplicate patch section"));
        }
        if let Some(&ps) = patch_sections.first() {
            let mut c = Cursor::new(*ps);
            let mut previous_type = 0;
            while c.position < c.bytes.len() {
                let ti = c.u32_le()? as usize;
                let count = c.u32_le()? as usize;
                if ti == 0 || ti >= types.len() {
                    return Err(c.error("patch type out of bounds"));
                }
                // one record a type, the types ascending (type 0 is not patched, so 0 starts the order)
                if ti <= previous_type {
                    return Err(c.error(format!(
                        "patch record of type {ti} after type {previous_type}: the records ascend by type"
                    )));
                }
                previous_type = ti;
                let mut offsets: Vec<usize> = Vec::new();
                for _ in 0..count {
                    let offset = c.u32_le()? as usize;
                    if offsets.last().is_some_and(|&last| offset <= last) {
                        return Err(c.error(format!(
                            "patch offset {offset:#x} after {:#x}: the offsets of a record ascend strictly",
                            offsets.last().copied().unwrap_or_default()
                        )));
                    }
                    let index = data
                        .payload
                        .get(offset..)
                        .and_then(<[u8]>::first_chunk::<4>)
                        .map(|&b| u32::from_le_bytes(b) as usize)
                        .ok_or_else(|| c.error("patch exceeds DATA"))?;
                    if index >= items.len() {
                        return Err(c.error("patched item reference out of bounds"));
                    }
                    offsets.push(offset);
                }
                patches.push((ti, offsets));
            }
        }
        let result = Self {
            version,
            types,
            items,
            patches,
            data: data.payload,
            type_section: bytes
                .get(ts.offset..ts.offset.saturating_add(8).saturating_add(ts.payload.len()))
                .unwrap_or_default(),
            data_offset: data.offset.saturating_add(8),
            items_offset: table.offset,
            types_offset: ts.offset,
        };
        for (i, item) in result.items.iter().enumerate().skip(1) {
            if item.type_index == 0 {
                return Err(invalid(table.offset, format!("null type in item {i}")));
            }
            let stride = result.stride(item.type_index)?;
            let size = result
                .extent(stride, item.count)
                .map_err(|e| invalid(table.offset, format!("item {i}: {}", e.detail)))?;
            result.bytes(item.offset, size)?;
        }
        Ok(result)
    }
    /// The type table's section as the file stores it, header included: `TYPE` (the file's own types), or `TCRF`
    /// (the id of the compendium that holds them).
    pub(crate) fn type_section(&self) -> &'a [u8] {
        self.type_section
    }
    /// The item that holds the DATA byte at `offset`, and the byte's position in it; `None` between items.
    /// A linear scan of the item table.
    pub fn item_at(&self, offset: usize) -> Option<(usize, usize)> {
        self.items.iter().enumerate().skip(1).find_map(|(i, item)| {
            let stride = self.stride(item.type_index).ok()?;
            let end = item.offset.checked_add(stride.checked_mul(item.count)?)?;
            (item.offset..end)
                .contains(&offset)
                .then(|| (i, offset.saturating_sub(item.offset)))
        })
    }
    /// The first value `f` finds on type `index` or the types it derives from; the default (0) at the null type.
    pub fn inherited<T: Default>(&self, mut index: usize, f: impl Fn(&Type) -> Option<T>) -> Result<T> {
        for _ in 0..self.types.len() {
            if index == 0 {
                return Ok(T::default());
            }
            let t = self.types.get(index).ok_or_else(|| {
                invalid(
                    self.types_offset,
                    format!(
                        "type index {index} out of bounds: the file has {} types",
                        self.types.len()
                    ),
                )
            })?;
            if let Some(value) = f(t) {
                return Ok(value);
            }
            index = t.parent;
        }
        Err(invalid(self.types_offset, "cyclic inherited property"))
    }
    /// The `format` word of a type or the nearest type it derives from (low four bits: the kind, e.g. 7 class, 6
    /// pointer, 8 array, 3 string; the rest per kind).
    pub fn format(&self, index: usize) -> Result<usize> {
        let format: u64 = self.inherited(index, |t| t.format)?;
        usize::try_from(format)
            .map_err(|_| invalid(self.types_offset, "type format exceeds the address space"))
    }
    /// The stored size in bytes of one element of the type (its own or the nearest parent's).
    pub fn stride(&self, index: usize) -> Result<usize> {
        // DATA retains the reflected element width, including references: an eight-byte
        // pointer stores its ITEM index in the low word and a zero high word. Advancing
        // four bytes makes the second array element read that padding as a null pointer.
        // HKLib's PointerFormatHandler and IndexItem read UInt64 per element (2018.1).
        self.inherited(index, |t| t.size)
    }
    /// The file offset of the byte at `offset` in DATA, for errors: [`Value::offset`](crate::Value) and the item
    /// offsets are DATA's, a refusal names the file's.
    pub(crate) fn at(&self, offset: usize) -> usize {
        self.data_offset.saturating_add(offset)
    }
    /// The bytes `count` elements of `stride` bytes take. Elements of no size take none however many there are, so
    /// the extent bounds nothing that is sized from the count; more than one such element is refused.
    pub(crate) fn extent(&self, stride: usize, count: usize) -> Result<usize> {
        if stride == 0 && count > 1 {
            return Err(invalid(
                self.data_offset,
                format!("{count} zero-size elements: an array of elements with no size is not stored"),
            ));
        }
        stride.checked_mul(count).ok_or_else(|| {
            invalid(
                self.data_offset,
                format!("{count} elements of {stride} bytes overflow"),
            )
        })
    }
    /// `size` bytes of DATA at `offset`; an error naming the offset when they run past it.
    pub fn bytes(&self, offset: usize, size: usize) -> Result<&'a [u8]> {
        let end = offset.checked_add(size).ok_or_else(|| {
            invalid(
                self.at(offset),
                format!("{size} bytes at {offset:#x} of DATA overflow"),
            )
        })?;
        self.data.get(offset..end).ok_or_else(|| {
            invalid(
                self.at(offset),
                format!(
                    "{size} bytes at {offset:#x} run past the end of DATA ({:#x} bytes)",
                    self.data.len()
                ),
            )
        })
    }
    /// The `N` bytes at `offset` in DATA.
    pub(crate) fn array<const N: usize>(&self, offset: usize) -> Result<[u8; N]> {
        self.bytes(offset, N)?
            .try_into()
            .map_err(|_| invalid(self.at(offset), format!("expected {N} bytes of DATA")))
    }
}
