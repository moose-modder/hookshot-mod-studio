#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use crate::{Error, Result, Section, invalid, sections};

/// A type from a tagfile's (or compendium's) type table.
///
/// Optional properties are `None` when the type does not store them; [`Tagfile::inherited`](crate::Tagfile::inherited)
/// looks one up through the parents.
#[derive(Debug, Clone, Default)]
pub struct Type {
    /// The class or type name, template arguments not included (see `parameters`).
    pub name: String,
    /// Index of the parent type; 0 for none.
    pub parent: usize,
    /// Which optional properties follow in the file (a bit set, below 128): 1 the format word, 2 the subtype, 4 the
    /// version, 8 the size and alignment words, 16 a type-flags word ([`Type::type_flags`]), 32 the members, 64 the
    /// interfaces.
    pub flags: u64,
    /// The format word: the kind in the low four bits (7 class, 6 pointer, 8 array, 3 string, 2 and 4 integers) and
    /// kind-specific bits above.
    pub format: Option<u64>,
    /// The element type of an array or pointer, or the type a template wraps.
    pub subtype: Option<usize>,
    /// The class version.
    pub version: Option<u64>,
    /// Size in bytes.
    pub size: Option<usize>,
    /// Alignment in bytes: the low 12 bits of the stored alignment word.
    pub alignment: Option<usize>,
    /// The requested alignment: bits 12 to 15 of the stored alignment word. 0 in every type of both installs but
    /// two: `hkCompressedMassProperties` (3, on an 8-byte alignment) and `hkpConstraintAtom` (15, on a 16-byte one).
    pub requested_alignment: Option<u8>,
    /// The type's flags word, stored when presence bit 16 of [`Type::flags`] is set.
    pub type_flags: Option<TypeFlags>,
    /// Template parameters: a name and a value (a type index when the name starts with `t`).
    pub parameters: Vec<(String, u64)>,
    /// The members the class declares itself (inherited ones are on its parents).
    pub fields: Vec<Field>,
    /// Reflection properties the type declares beside its fields: counted, their records are not serialized.
    pub properties: usize,
    /// Implemented interfaces: a type index and the stored flags.
    pub interfaces: Vec<(usize, u64)>,
    /// The type's hash, from the THSH section when the file has one.
    pub hash: Option<u32>,
}

/// A type's flags word, a bit set. The distinct type tables of Sekiro (229 tables, 84,566 types) and Elden Ring (875
/// tables, 373,202 types) store the values 0, 1, 3, 16, 64, 144, 147 and 256 only; the union of their bits is
/// [`TypeFlags::SEEN`]. The bits are named as HKLib, a public reader of the 2018 SDK, names them; a bit outside
/// [`TypeFlags::SEEN`] is kept as stored.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct TypeFlags(pub u32);

impl TypeFlags {
    /// Bit 0 (value 1): not serialisable. Stored on `void`, `hkPtrAndInt` and the `hcl` base types.
    pub const NOT_SERIALIZABLE: u32 = 1;
    /// Bit 1 (value 2): abstract. Stored with 1 on the `hcl` base types (value 3).
    pub const ABSTRACT: u32 = 2;
    /// Bit 4 (value 16): an interface. Stored on `hkContainerHeapAllocator` and `hkHashMap`.
    pub const INTERFACE: u32 = 16;
    /// Bit 6 (value 64): a weak pointer. Stored on `hkViewPtr`, and on `T*` in Elden Ring's tables.
    pub const WEAK_POINTER: u32 = 64;
    /// Bit 7 (value 128): the class has its own virtual table. Stored on `hkBaseObject` (144 with the interface bit)
    /// and on `hclSetupMeshSection` and `hkbVerifiable` (147).
    pub const OWN_VTABLE: u32 = 128;
    /// Bit 8 (value 256): an attribute class. Stored on `hkbRoleAttribute`.
    pub const ATTRIBUTE: u32 = 256;
    /// The union of the bits any type of either install stores.
    pub const SEEN: u32 = 0x1D3;

    /// Whether every bit of `mask` is set.
    pub fn contains(self, mask: u32) -> bool {
        self.0 & mask == mask
    }
    /// The bits no type of either install stores, as stored.
    pub fn unseen(self) -> u32 {
        self.0 & !Self::SEEN
    }
}

/// A member's declaration flags (HKLib's `hkReflect::Decl::DeclFlagBits` of the 2018 SDK), a bit set: the low bits
/// say whether the member is stored and who may reach it in the class, bit 5 that it is a data field. The type tables
/// of every distinct tagfile of Sekiro and Elden Ring store the values `0x20` to `0x25` only, so only bits 0, 1, 2 and
/// 5 occur. Counting each (class, member, flags) once: Sekiro has 1,925 (`0x20` public 1,506, `0x21` public and not
/// serialisable 43, `0x22` protected 110, `0x23` protected and not serialisable 80, `0x24` private 77, `0x25` private
/// and not serialisable 109) and Elden Ring 2,210 (1,742, 46, 132, 93, 77 and 120).
///
/// The names of bits 1 and 2 are checked against class headers the tables do not come from. Of the 196 (class, member,
/// flags) combinations in the tables of either game whose class and member (the header's `m_name`) the Havok 6.6
/// headers declare as well (the oldest SDK whose headers are public): the 3 the headers declare `protected:` all store
/// `0x22`; of the 17 the headers declare `private:`, 16 store `0x24` and one `0x22` (`hkFlags::storage`, which another
/// instance of the template stores as `0x24`); of the 176 the headers declare `public:`, 174 store neither access bit
/// and two do (`hkxVertexFloatDataChannel::perVertexFloats`, `0x22`, and `hkReferencedObject::memSizeAndFlags`, `0x25`).
/// The opposite assignment of the two bits would fail on 19 of the 20 non-public combinations. Bit 0 is checked by the
/// data itself (see [`DeclFlags::NOT_SERIALIZABLE`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct DeclFlags(pub u32);

/// Who may reach a member in its class, from [`DeclFlags::access`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeclAccess {
    /// Neither access bit: a public member.
    Public,
    /// Bit 1 (`DECL_PROTECTED`).
    Protected,
    /// Bit 2 (`DECL_PRIVATE`).
    Private,
}

impl DeclFlags {
    /// Bit 0 (value 1, `DECL_NOT_SERIALIZABLE`): the member is declared but its value is not written. The type
    /// tables keep its offset; the bytes there are zero every time they are read: Sekiro 183 (class, member) pairs read
    /// 1,737,907 times, Elden Ring 205 pairs read 13,901,700 times (in a run of structs, its first 16 elements), none
    /// nonzero.
    pub const NOT_SERIALIZABLE: u32 = 1;
    /// Bit 1 (value 2, `DECL_PROTECTED`): the member is protected in its class. Stored with `0x20` (`0x22`) or with
    /// bit 0 (`0x23`); never with the private bit.
    pub const PROTECTED: u32 = 2;
    /// Bit 2 (value 4, `DECL_PRIVATE`): the member is private in its class. Stored as `0x24` and `0x25`; never with the
    /// protected bit.
    pub const PRIVATE: u32 = 4;
    /// Bit 3 (value 8, `DECL_HIDDEN`): in no type table.
    pub const HIDDEN: u32 = 8;
    /// Bit 4 (value 16, `DECL_PROPERTY_FIELD`): in no type table.
    pub const PROPERTY_FIELD: u32 = 16;
    /// Bit 5 (value 32, `DECL_DATA_FIELD`): set on every member record of both installs.
    pub const DATA_FIELD: u32 = 32;
    /// Bit 6 (value 64, `DECL_CUSTOM_SETTER`): in no type table.
    pub const CUSTOM_SETTER: u32 = 64;
    /// The union of the bits any member record of either install stores.
    pub const SEEN: u32 = 0x27;

    /// Whether every bit of `mask` is set.
    pub fn contains(self, mask: u32) -> bool {
        self.0 & mask == mask
    }
    /// The bits no member record of either install stores, as stored.
    pub fn unseen(self) -> u32 {
        self.0 & !Self::SEEN
    }
    /// Whether the member's value is written to the file (bit 0 clear).
    pub fn serialized(self) -> bool {
        !self.contains(Self::NOT_SERIALIZABLE)
    }
    /// The member's access in its class; `None` when both access bits are set (no record of either install has
    /// them).
    pub fn access(self) -> Option<DeclAccess> {
        match (self.contains(Self::PROTECTED), self.contains(Self::PRIVATE)) {
            (false, false) => Some(DeclAccess::Public),
            (true, false) => Some(DeclAccess::Protected),
            (false, true) => Some(DeclAccess::Private),
            (true, true) => None,
        }
    }
}

/// A member of a class.
#[derive(Debug, Clone)]
pub struct Field {
    /// The member's name.
    pub name: String,
    /// The member's stored flags, kept whole; [`Field::decl_flags`] names the bits. The 816,249 member records of the
    /// distinct type tables of Sekiro (149,035) and Elden Ring (667,214) store `0x20` to `0x25`: `0x20` is set on every
    /// record, and with it every combination of `0x01`, `0x02` and `0x04` occurs but `0x02` together with `0x04`. A
    /// record without `0x20`, or with a bit outside `0x27`, is refused.
    pub flags: u64,
    /// Byte offset within the object.
    pub offset: usize,
    /// The member's type.
    pub type_index: usize,
}

impl Field {
    /// The member's declaration flags, named. The type reader refuses a record with a bit outside
    /// [`DeclFlags::SEEN`], so the conversion keeps every bit of a field it returned.
    pub fn decl_flags(&self) -> DeclFlags {
        DeclFlags(u32::try_from(self.flags).unwrap_or(u32::MAX))
    }
}

/// A `TCM0` type compendium: the type table shared by several tagfiles, which name it by an id.
#[derive(Debug, Clone)]
pub struct Compendium {
    /// The ids of the type tables the compendium holds.
    pub ids: Vec<[u8; 8]>,
    /// Index zero is a null placeholder.
    pub types: Vec<Type>,
}

pub(crate) struct Cursor<'a> {
    pub bytes: &'a [u8],
    pub position: usize,
    pub base: usize,
}
impl<'a> Cursor<'a> {
    pub fn new(section: Section<'a>) -> Self {
        Self {
            bytes: section.payload,
            position: 0,
            base: section.offset.saturating_add(8),
        }
    }
    pub fn error(&self, detail: impl Into<String>) -> Error {
        invalid(self.base.saturating_add(self.position), detail)
    }
    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(n)
            .ok_or_else(|| self.error("length overflow"))?;
        let b = self
            .bytes
            .get(self.position..end)
            .ok_or_else(|| self.error("short input"))?;
        self.position = end;
        Ok(b)
    }
    /// The next `N` bytes.
    pub fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let b = self.take(N)?;
        b.try_into().map_err(|_| self.error("short input"))
    }
    pub fn u32_le(&mut self) -> Result<u32> {
        self.array().map(u32::from_le_bytes)
    }
    pub fn packed(&mut self) -> Result<u64> {
        let [first] = self.array()?;
        let (remaining, mask) = match first {
            0..=0x7f => (0, 0x7f),
            0x80..=0xbf => (1, 0x3f),
            0xc0..=0xdf => (2, 0x1f),
            0xe0..=0xe7 => (3, 7),
            0xe8..=0xef => (4, 7),
            0xf0..=0xf7 => (7, 7),
            0xf8 => (5, 0),
            0xf9 => (8, 0),
            _ => return Err(self.error("reserved packed integer prefix")),
        };
        let mut value = (first & mask) as u64;
        for &byte in self.take(remaining)? {
            value = (value << 8) | u64::from(byte);
        }
        Ok(value)
    }
    pub fn index(&mut self) -> Result<usize> {
        usize::try_from(self.packed()?).map_err(|_| self.error("index exceeds address space"))
    }
    pub fn padding(&mut self) -> Result<()> {
        let rest = self
            .bytes
            .get(self.position..)
            .ok_or_else(|| self.error("position past the end"))?;
        if rest.iter().any(|&b| b != 0) {
            return Err(self.error("nonzero trailing bytes"));
        }
        self.position = self.bytes.len();
        Ok(())
    }
}

pub(crate) fn required<'a>(children: &[Section<'a>], names: &[[u8; 4]]) -> Result<Section<'a>> {
    let mut found = children.iter().filter(|s| names.contains(&s.signature));
    let result = *found
        .next()
        .ok_or_else(|| invalid(0, format!("missing section {:?}", names)))?;
    if found.next().is_some() {
        return Err(invalid(result.offset, "duplicate section"));
    }
    Ok(result)
}
fn strings(section: Section<'_>) -> Result<Vec<String>> {
    // the table's strings are the NUL-terminated runs: the empty run after the last NUL is no string, so an index
    // equal to the table's length is out of bounds, not an empty name
    let Some((0, strings)) = section.payload.split_last() else {
        return Err(invalid(section.offset, "unterminated string table"));
    };
    strings
        .split(|&b| b == 0)
        .map(|b| {
            String::from_utf8(b.to_vec()).map_err(|_| invalid(section.offset, "invalid UTF-8 string table"))
        })
        .collect()
}
fn string(table: &[String], c: &mut Cursor<'_>) -> Result<String> {
    let index = c.index()?;
    table
        .get(index)
        .cloned()
        .ok_or_else(|| c.error("string index out of bounds"))
}

impl Compendium {
    /// Parse and check a `TCM0` file.
    pub fn read(bytes: &[u8]) -> Result<Self> {
        let roots = sections(bytes)?;
        let &[root] = roots.as_slice() else {
            return Err(invalid(0, "expected one TCM0 root"));
        };
        if root.signature != *b"TCM0" {
            return Err(invalid(0, "expected one TCM0 root"));
        }
        let children = root.children()?;
        for s in &children {
            if ![*b"TCID", *b"TYPE"].contains(&s.signature) {
                return Err(invalid(s.offset, "unknown compendium section"));
            }
        }
        let ids = required(&children, &[*b"TCID"])?;
        if ids.payload.is_empty() || ids.payload.len() % 8 != 0 {
            return Err(invalid(ids.offset, "invalid compendium IDs"));
        }
        let ids = ids.payload.as_chunks::<8>().0.to_vec();
        let types = read_types(required(&children, &[*b"TYPE"])?)?;
        Ok(Self { ids, types })
    }
}

pub(crate) fn read_types(section: Section<'_>) -> Result<Vec<Type>> {
    let children = section.children()?;
    for s in &children {
        if ![
            *b"TPTR", *b"TSTR", *b"TST1", *b"TNAM", *b"TNA1", *b"FSTR", *b"FST1", *b"TBOD", *b"TBDY",
            *b"THSH", *b"TPAD",
        ]
        .contains(&s.signature)
        {
            return Err(invalid(s.offset, "unknown type section"));
        }
        if [*b"TPTR", *b"TPAD"].contains(&s.signature) && s.payload.iter().any(|&b| b != 0) {
            return Err(invalid(s.offset, "nonzero type padding or pointer table"));
        }
    }
    let names = strings(required(&children, &[*b"TSTR", *b"TST1"])?)?;
    let field_names = strings(required(&children, &[*b"FSTR", *b"FST1"])?)?;
    let mut c = Cursor::new(required(&children, &[*b"TNAM", *b"TNA1"])?);
    let count = c.index()?;
    if count == 0 || count > c.bytes.len() {
        return Err(c.error("invalid type count"));
    }
    let mut types = vec![Type::default(); count];
    for ty in types.iter_mut().skip(1) {
        ty.name = string(&names, &mut c)?;
        let n = c.index()?;
        if n > c.bytes.len() / 2 {
            return Err(c.error("invalid template count"));
        }
        for _ in 0..n {
            ty.parameters.push((string(&names, &mut c)?, c.packed()?));
        }
    }
    c.padding()?;
    let mut c = Cursor::new(required(&children, &[*b"TBOD", *b"TBDY"])?);
    let mut seen = vec![false; count];
    while c.position < c.bytes.len() {
        let index = c.index()?;
        if index == 0 {
            continue;
        }
        let (Some(seen), Some(t)) = (seen.get_mut(index), types.get_mut(index)) else {
            return Err(c.error("invalid or duplicate type body"));
        };
        if std::mem::replace(seen, true) {
            return Err(c.error("invalid or duplicate type body"));
        }
        t.parent = c.index()?;
        t.flags = c.packed()?;
        if t.flags & !127 != 0 {
            return Err(c.error("unknown type flags"));
        }
        if t.flags & 1 != 0 {
            t.format = Some(c.packed()?);
        }
        if t.flags & 2 != 0 {
            t.subtype = Some(c.index()?);
        }
        if t.flags & 4 != 0 {
            t.version = Some(c.packed()?);
        }
        if t.flags & 8 != 0 {
            t.size = Some(c.index()?);
            // The alignment word: the alignment in its low 12 bits, a requested alignment in bits 12 to 15. Nothing
            // above is stored (no type of either install has a word of 16 bits or more).
            let word = c.packed()?;
            if word >> 16 != 0 {
                return Err(c.error(format!("alignment word {word:#x} has bits above 15")));
            }
            t.alignment = Some(usize::from(low16(word) & 0xFFF));
            t.requested_alignment =
                Some(u8::try_from(low16(word) >> 12).map_err(|_| c.error("alignment word"))?);
        }
        if t.flags & 16 != 0 {
            let flags = u32::try_from(c.packed()?).map_err(|_| c.error("type flags exceed 32 bits"))?;
            t.type_flags = Some(TypeFlags(flags));
        }
        if t.flags & 32 != 0 {
            // The low 16 bits count the field records that follow; the high 16 bits count reflection properties,
            // which store no records here (as HKLib's 2018 reader reads the same word).
            let counts = c.packed()?;
            t.properties = usize::try_from(counts >> 16).map_err(|_| c.error("property count overflow"))?;
            let n = (counts & 0xffff) as usize;
            if n > c.bytes.len() / 4 {
                return Err(c.error("unsupported or invalid member count"));
            }
            for _ in 0..n {
                let name = string(&field_names, &mut c)?;
                let flags = c.packed()?;
                if flags & FIELD_FLAGS_PRESENT == 0 || flags & !FIELD_FLAGS_KNOWN != 0 {
                    return Err(c.error(format!(
                        "member {name:?} has flags {flags:#x}; every member record has {FIELD_FLAGS_PRESENT:#x} set and none sets a bit outside {FIELD_FLAGS_KNOWN:#x}"
                    )));
                }
                t.fields.push(Field {
                    name,
                    flags,
                    offset: c.index()?,
                    type_index: c.index()?,
                });
            }
        }
        if t.flags & 64 != 0 {
            let n = c.index()?;
            if n > c.bytes.len() / 2 {
                return Err(c.error("invalid interface count"));
            }
            for _ in 0..n {
                t.interfaces.push((c.index()?, c.packed()?));
            }
        }
    }
    if let Some(&hashes) = children.iter().find(|s| s.signature == *b"THSH") {
        let mut c = Cursor::new(hashes);
        let n = c.index()?;
        for _ in 0..n {
            let i = c.index()?;
            let hash = c.u32_le()?;
            match types.get_mut(i) {
                Some(t) if i != 0 && t.hash.is_none() => t.hash = Some(hash),
                _ => return Err(c.error("invalid type hash index")),
            }
        }
        c.padding()?;
    }
    for (i, t) in types.iter().enumerate().skip(1) {
        if t.parameters
            .iter()
            .any(|(name, value)| name.starts_with('t') && *value >= count as u64)
            || t.parent >= count
            || t.subtype.is_some_and(|s| s >= count)
            || t.fields
                .iter()
                .any(|f| f.type_index == 0 || f.type_index >= count)
            || t.interfaces.iter().any(|&(j, _)| j == 0 || j >= count)
        {
            return Err(invalid(section.offset, format!("invalid type reference in {i}")));
        }
    }
    if let Some(i) = parent_cycle(&types) {
        return Err(invalid(section.offset, format!("type {i} inherits from itself")));
    }
    if let Some((i, kind)) = subtype_on_wrong_kind(&types) {
        return Err(invalid(
            section.offset,
            format!("type {i} stores a subtype but is of kind {kind}, not a pointer (6) or an array (8)"),
        ));
    }
    check_hashes(&types, section.offset)?;
    Ok(types)
}

/// The low 16 bits of `v`.
#[allow(
    clippy::cast_possible_truncation,
    reason = "the low bits are what is asked for"
)]
fn low16(v: u64) -> u16 {
    (v & 0xFFFF) as u16
}

/// The low 32 bits of `v`: the number a type hash takes of a stored value.
#[allow(
    clippy::cast_possible_truncation,
    reason = "the low bits are what is asked for"
)]
fn low32(v: u64) -> u32 {
    (v & 0xFFFF_FFFF) as u32
}

/// The CRC-32 table of the reflected polynomial 0xEDB88320.
#[allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    reason = "a constant table, built from indices below 256"
)]
const CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut h = i as u32;
        let mut bit = 0;
        while bit < 8 {
            h = (h >> 1) ^ (0xEDB8_8320 & 0u32.wrapping_sub(h & 1));
            bit += 1;
        }
        table[i] = h;
        i += 1;
    }
    table
};

/// The IEEE CRC-32 of `bytes` continued from `crc`, a finished CRC-32 (0 for none): the chain a type hash is.
fn crc32_continue(crc: u32, bytes: &[u8]) -> u32 {
    let mut h = !crc;
    for &b in bytes {
        let index = ((h ^ u32::from(b)) & 0xFF) as usize;
        h = (h >> 8) ^ CRC_TABLE.get(index).copied().unwrap_or_default();
    }
    !h
}

/// The most template parameters, nested ones included, that one type's hash walks. A type of the games has a handful;
/// the bound stops a table whose parameters name each other from running away.
const TEMPLATE_STEPS: usize = 1 << 12;

/// The chained CRC-32 the THSH section stores for a type, built from the type's description:
///
/// 1. the hash of the parent (0 for none);
/// 2. with presence bit 64, for each interface the CRC of the interface type's name and the stored word;
/// 3. with presence bit 1, the format word, and for an array (kind 8) whose format has `(format >> 5) & 3 == 1` the
///    hash of its subtype;
/// 4. the type's name;
/// 5. its template parameters in order: a type parameter (its name starts with `t`) is the CRC of that type's name and
///    then, the same way, of its own parameters; a value parameter is the stored value;
/// 6. with presence bit 4, the version;
/// 7. with presence bit 8, `size | alignment word << 16`, the alignment word being [`Type::alignment`] with
///    [`Type::requested_alignment`] in bits 12 to 15;
/// 8. with presence bit 16, the type flags;
/// 9. with presence bit 32, for every member of the type and of its parents, the root's first: the name, the offset
///    and the hash of the member's type.
///
/// Every number is a little-endian u32 (the low 32 bits of the stored value), names are their bytes without a
/// terminator. A census of the games' tagfiles and compendia reproduced every stored hash with this recipe (Sekiro
/// 20,180 of 20,180, Elden Ring 90,033 of 90,033), so a hash it does not reproduce names a type whose table was
/// read wrongly.
struct Hashes<'t> {
    types: &'t [Type],
    state: Vec<HashState>,
}

/// Where a type's hash is: a type a cycle comes back to while it is being hashed counts as 0.
#[derive(Clone, Copy)]
enum HashState {
    Unvisited,
    Active,
    Done(u32),
}

impl<'t> Hashes<'t> {
    fn new(types: &'t [Type]) -> Self {
        Hashes {
            types,
            state: vec![HashState::Unvisited; types.len()],
        }
    }

    /// The hash of a type already hashed; 0 for the null type and for one that is still being hashed.
    fn done(&self, index: usize) -> u32 {
        match self.state.get(index) {
            Some(HashState::Done(h)) => *h,
            _ => 0,
        }
    }

    /// Start hashing `index` if it has not been: mark it and list the types whose hashes it needs first, in the order
    /// the recipe asks for them.
    fn open(&mut self, index: usize) -> Option<(usize, Vec<usize>)> {
        let state = self.state.get_mut(index)?;
        if !matches!(state, HashState::Unvisited) {
            return None;
        }
        *state = HashState::Active;
        let t = self.types.get(index)?;
        let mut needs = Vec::new();
        if t.parent != 0 {
            needs.push(t.parent);
        }
        if let Some(subtype) = t.subtype
            && t.format.is_some_and(|f| f & 15 == 8 && (f >> 5) & 3 == 1)
        {
            needs.push(subtype);
        }
        if t.flags & 32 != 0 {
            needs.extend(t.fields.iter().map(|f| f.type_index));
        }
        // popped from the end
        needs.reverse();
        Some((index, needs))
    }

    /// The hash of type `index`, and of the types it needs; without recursion, as a table may chain them deep.
    fn hash(&mut self, index: usize, at: usize) -> Result<u32> {
        let mut stack: Vec<(usize, Vec<usize>)> = self.open(index).into_iter().collect();
        while let Some((current, needs)) = stack.last_mut() {
            if let Some(next) = needs.pop() {
                if let Some(frame) = self.open(next) {
                    stack.push(frame);
                }
                continue;
            }
            let current = *current;
            stack.pop();
            let h = self.compute(current, at)?;
            if let Some(state) = self.state.get_mut(current) {
                *state = HashState::Done(h);
            }
        }
        Ok(self.done(index))
    }

    /// Type `index`'s hash, the hashes it needs being done.
    fn compute(&self, index: usize, at: usize) -> Result<u32> {
        let Some(t) = self.types.get(index) else {
            return Ok(0);
        };
        let word = |sig: u32, v: u64| crc32_continue(sig, &low32(v).to_le_bytes());
        let mut sig = self.done(t.parent);
        if t.flags & 64 != 0 {
            for &(interface, stored) in &t.interfaces {
                let name = self.types.get(interface).map_or("", |i| i.name.as_str());
                sig = crc32_continue(sig, name.as_bytes());
                sig = word(sig, stored);
            }
        }
        if let Some(format) = t.format {
            sig = word(sig, format);
            if format & 15 == 8 && (format >> 5) & 3 == 1 {
                sig = word(sig, u64::from(self.done(t.subtype.unwrap_or(0))));
            }
        }
        sig = crc32_continue(sig, t.name.as_bytes());
        sig = self.template(sig, t, at)?;
        if let Some(version) = t.version {
            sig = word(sig, version);
        }
        if let (Some(size), Some(alignment), Some(requested)) = (t.size, t.alignment, t.requested_alignment) {
            let alignment_word = (alignment as u64) | (u64::from(requested) << 12);
            sig = word(sig, (size as u64) | (alignment_word << 16));
        }
        if let Some(flags) = t.type_flags {
            sig = word(sig, u64::from(flags.0));
        }
        if t.flags & 32 != 0 {
            // the type's members after those of its parents, the root's first
            let mut chain = Vec::new();
            let mut owner = index;
            while owner != 0 && chain.len() <= self.types.len() {
                chain.push(owner);
                owner = self.types.get(owner).map_or(0, |o| o.parent);
            }
            for owner in chain.iter().rev().filter_map(|&o| self.types.get(o)) {
                for f in &owner.fields {
                    sig = crc32_continue(sig, f.name.as_bytes());
                    sig = word(sig, f.offset as u64);
                    sig = word(sig, u64::from(self.done(f.type_index)));
                }
            }
        }
        Ok(sig)
    }

    /// `sig` continued over the template parameters of `t`.
    fn template(&self, mut sig: u32, t: &Type, at: usize) -> Result<u32> {
        // the parameter lists being walked, each with the next parameter to take
        let mut stack = vec![(t, 0usize)];
        let mut steps = 0usize;
        while let Some((current, next)) = stack.last_mut() {
            let Some((name, value)) = current.parameters.get(*next) else {
                stack.pop();
                continue;
            };
            *next = next.saturating_add(1);
            steps = steps.saturating_add(1);
            if steps > TEMPLATE_STEPS {
                return Err(invalid(
                    at,
                    format!(
                        "template parameters of type {} nest or repeat past {TEMPLATE_STEPS} steps",
                        t.name
                    ),
                ));
            }
            if name.starts_with('t') {
                let Some(parameter) = usize::try_from(*value).ok().and_then(|i| self.types.get(i)) else {
                    return Err(invalid(
                        at,
                        format!("template parameter {name} of {} names no type", t.name),
                    ));
                };
                sig = crc32_continue(sig, parameter.name.as_bytes());
                stack.push((parameter, 0));
            } else {
                sig = crc32_continue(sig, &low32(*value).to_le_bytes());
            }
        }
        Ok(sig)
    }
}

/// Every stored type hash against the hash the type's description gives (see [`Hashes`]).
fn check_hashes(types: &[Type], at: usize) -> Result<()> {
    let mut hashes = Hashes::new(types);
    for (i, t) in types.iter().enumerate() {
        let Some(stored) = t.hash else { continue };
        let computed = hashes.hash(i, at)?;
        if computed != stored {
            return Err(invalid(
                at,
                format!(
                    "type {i} ({}): stored hash {stored:#x}, the table gives {computed:#x}",
                    t.name
                ),
            ));
        }
    }
    Ok(())
}

/// The bits a member record's flags word has: `0x20` is set on every member of both installs.
const FIELD_FLAGS_PRESENT: u64 = 0x20;
/// The bits a member record's flags word may have: `0x20` and the low three, whose values the tables use in every
/// combination but `0x02` with `0x04`.
const FIELD_FLAGS_KNOWN: u64 = 0x27;

/// The first type that stores a subtype though its kind (the low four bits of its format word, or its nearest parent's;
/// 16 when none has a format) is not a pointer (6) or an array (8), with that kind. Every type that stores one is
/// one of the two in the tables of both installs (Sekiro 25,275 pointers and 20,192 arrays; Elden Ring 111,486 and
/// 91,351). The parent chains must end: [`parent_cycle`] has checked.
fn subtype_on_wrong_kind(types: &[Type]) -> Option<(usize, u64)> {
    types.iter().enumerate().skip(1).find_map(|(i, t)| {
        t.subtype?;
        let mut at = Some(t);
        let mut format = None;
        for _ in 0..types.len() {
            let Some(current) = at else { break };
            if let Some(f) = current.format {
                format = Some(f);
                break;
            }
            at = types.get(current.parent).filter(|_| current.parent != 0);
        }
        let kind = format.map_or(16, |f| f & 15);
        (kind != 6 && kind != 8).then_some((i, kind))
    })
}

/// A type on a cycle of parents, if there is one: every type's parent chain must end at the null type 0. Linear in the
/// types: a type is visited once, however many chains pass through it. Every `parent` must be below `types.len()`.
fn parent_cycle(types: &[Type]) -> Option<usize> {
    // 0: not visited, 1: on the chain being followed, 2: known to end at the null type
    let mut state = vec![0u8; types.len()];
    if let Some(null) = state.first_mut() {
        *null = 2;
    }
    for start in 1..types.len() {
        let mut chain = Vec::new();
        let mut at = start;
        // the state the chain ends on (a parent past the types, which the caller excludes, ends it as the null type)
        let end = loop {
            let (Some(s), Some(t)) = (state.get_mut(at), types.get(at)) else {
                break 2;
            };
            if *s != 0 {
                break *s;
            }
            *s = 1;
            chain.push(at);
            at = t.parent;
        };
        if end == 1 {
            return Some(at);
        }
        for t in chain {
            if let Some(s) = state.get_mut(t) {
                *s = 2;
            }
        }
    }
    None
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
    #[test]
    fn packed_integer_widths_preserve_high_bits() {
        for (bytes, expected) in [
            (vec![0x7f], 127u64),
            (vec![0xbf, 0xff], 0x3fff),
            (vec![0xdf, 0xff, 0xff], 0x1fffff),
            (vec![0xe7, 0xff, 0xff, 0xff], 0x7ffffff),
            (vec![0xef, 0xff, 0xff, 0xff, 0xff], 0x7ffffffff),
            (
                vec![0xf7, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
                0x7ffffffffffffff,
            ),
            (vec![0xf8, 0xff, 0xff, 0xff, 0xff, 0xff], 0xffffffffff),
            (
                vec![0xf9, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
                u64::MAX,
            ),
        ] {
            let mut c = Cursor {
                bytes: &bytes,
                position: 0,
                base: 0,
            };
            assert_eq!(c.packed().expect("integer"), expected);
            assert_eq!(c.position, bytes.len());
            for n in 0..bytes.len() {
                let mut c = Cursor {
                    bytes: &bytes[..n],
                    position: 0,
                    base: 0,
                };
                assert!(c.packed().is_err());
            }
        }
    }
    /// Parent chains that end at the null type pass, however long and however many share a tail (a type visited once,
    /// not once per chain through it: a chain of this many was ~5e9 steps); a cycle is found, a self-parent too.
    #[test]
    fn parent_chains_are_checked_in_one_pass() {
        let typed = |parents: &[usize]| -> Vec<Type> {
            parents
                .iter()
                .map(|&parent| Type {
                    parent,
                    ..Type::default()
                })
                .collect()
        };
        let n = 100_000;
        let chain: Vec<usize> = (0..n).map(|i: usize| i.saturating_sub(1)).collect();
        assert_eq!(parent_cycle(&typed(&chain)), None);
        // 0 null, 1 and 2 hang from 0, 3 from 2, 4 from 3
        assert_eq!(parent_cycle(&typed(&[0, 0, 0, 2, 3])), None);
        assert!(
            parent_cycle(&typed(&[0, 2, 1])).is_some(),
            "1 and 2 are each other's parent"
        );
        assert!(
            parent_cycle(&typed(&[0, 1])).is_some(),
            "a type that is its own parent"
        );
        assert!(
            parent_cycle(&typed(&[0, 0, 3, 4, 2])).is_some(),
            "a cycle beyond the first type"
        );
    }

    /// One type of a hand-built type table: what the file stores of it.
    #[derive(Default, Clone)]
    struct Spec {
        name: &'static str,
        parent: usize,
        format: Option<u64>,
        subtype: Option<usize>,
        version: Option<u64>,
        /// The size and the alignment word.
        size: Option<(u64, u64)>,
        type_flags: Option<u64>,
        /// Name, member flags, offset, type.
        fields: Vec<(&'static str, u64, u64, usize)>,
        /// Type and stored word.
        interfaces: Vec<(usize, u64)>,
        /// Name and value.
        parameters: Vec<(&'static str, u64)>,
    }

    fn packed(v: u64) -> Vec<u8> {
        if v < 128 {
            vec![v as u8]
        } else if v < 16384 {
            ((v as u16) | 0x8000).to_be_bytes().to_vec()
        } else if v < 1 << 21 {
            vec![0xC0 | (v >> 16) as u8, (v >> 8) as u8, v as u8]
        } else {
            let mut out = vec![0xF8];
            out.extend(&v.to_be_bytes()[3..]);
            out
        }
    }

    fn section_bytes(name: &[u8; 4], leaf: bool, payload: &[u8]) -> Vec<u8> {
        let mut out = ((payload.len() as u32 + 8) | if leaf { 0x4000_0000 } else { 0 })
            .to_be_bytes()
            .to_vec();
        out.extend(name);
        out.extend(payload);
        out
    }

    /// The index of `name` in a string table, adding it first if it is new.
    fn index<'a>(table: &mut Vec<&'a str>, name: &'a str) -> u64 {
        let at = table.iter().position(|&n| n == name).unwrap_or_else(|| {
            table.push(name);
            table.len() - 1
        });
        at as u64
    }

    /// The TYPE section of `specs` (type 1 first), with a THSH section of `hashes` (type index, hash) if any.
    fn type_section(specs: &[Spec], hashes: &[(usize, u32)]) -> Vec<u8> {
        let (mut names, mut field_names) = (Vec::<&str>::new(), Vec::<&str>::new());
        let mut type_names = packed(specs.len() as u64 + 1);
        for s in specs {
            type_names.extend(packed(index(&mut names, s.name)));
            type_names.extend(packed(s.parameters.len() as u64));
            for &(name, value) in &s.parameters {
                type_names.extend(packed(index(&mut names, name)));
                type_names.extend(packed(value));
            }
        }
        let mut bodies = Vec::new();
        for (i, s) in specs.iter().enumerate() {
            let mut record = Vec::new();
            let mut presence = 0u64;
            if let Some(format) = s.format {
                presence |= 1;
                record.extend(packed(format));
            }
            if let Some(subtype) = s.subtype {
                presence |= 2;
                record.extend(packed(subtype as u64));
            }
            if let Some(version) = s.version {
                presence |= 4;
                record.extend(packed(version));
            }
            if let Some((size, alignment)) = s.size {
                presence |= 8;
                record.extend(packed(size));
                record.extend(packed(alignment));
            }
            if let Some(flags) = s.type_flags {
                presence |= 16;
                record.extend(packed(flags));
            }
            if !s.fields.is_empty() {
                presence |= 32;
                record.extend(packed(s.fields.len() as u64));
                for &(name, flags, offset, type_index) in &s.fields {
                    record.extend(packed(index(&mut field_names, name)));
                    record.extend(packed(flags));
                    record.extend(packed(offset));
                    record.extend(packed(type_index as u64));
                }
            }
            if !s.interfaces.is_empty() {
                presence |= 64;
                record.extend(packed(s.interfaces.len() as u64));
                for &(type_index, stored) in &s.interfaces {
                    record.extend(packed(type_index as u64));
                    record.extend(packed(stored));
                }
            }
            bodies.extend(packed(i as u64 + 1));
            bodies.extend(packed(s.parent as u64));
            bodies.extend(packed(presence));
            bodies.extend(record);
        }
        // a table ends in a NUL, so an empty one is that alone
        let strings = |table: &[&str]| -> Vec<u8> {
            let list: Vec<u8> = table.iter().flat_map(|n| n.bytes().chain([0])).collect();
            if list.is_empty() { vec![0] } else { list }
        };
        let mut payload = section_bytes(b"TSTR", true, &strings(&names));
        payload.extend(section_bytes(b"TNA1", true, &type_names));
        payload.extend(section_bytes(b"FSTR", true, &strings(&field_names)));
        payload.extend(section_bytes(b"TBDY", true, &bodies));
        if !hashes.is_empty() {
            let mut table = packed(hashes.len() as u64);
            for &(type_index, hash) in hashes {
                table.extend(packed(type_index as u64));
                table.extend(hash.to_le_bytes());
            }
            payload.extend(section_bytes(b"THSH", true, &table));
        }
        section_bytes(b"TYPE", false, &payload)
    }

    fn read(bytes: &[u8]) -> Result<Vec<Type>> {
        let roots = sections(bytes)?;
        read_types(*roots.first().expect("one section"))
    }

    /// An integer type; a base class with a member, a derived class that adds one and an interface; an array type and
    /// a type taking it as a template parameter.
    fn mini() -> Vec<Spec> {
        vec![
            Spec {
                name: "u32",
                format: Some(0x8002),
                size: Some((4, 4)),
                ..Spec::default()
            },
            Spec {
                name: "Base",
                format: Some(7),
                version: Some(1),
                size: Some((8, 0x3008)),
                type_flags: Some(147),
                fields: vec![("a", 0x20, 0, 1)],
                ..Spec::default()
            },
            Spec {
                name: "Derived",
                parent: 2,
                format: Some(7),
                version: Some(2),
                size: Some((16, 8)),
                fields: vec![("b", 0x20, 8, 1)],
                interfaces: vec![(5, 8)],
                ..Spec::default()
            },
            Spec {
                name: "hkArray",
                format: Some(0x28),
                subtype: Some(1),
                size: Some((16, 8)),
                parameters: vec![("t0", 1), ("v1", 7)],
                ..Spec::default()
            },
            Spec {
                name: "Iface",
                format: Some(7),
                type_flags: Some(16),
                ..Spec::default()
            },
            Spec {
                name: "Holder",
                format: Some(7),
                size: Some((8, 8)),
                parameters: vec![("t0", 4)],
                ..Spec::default()
            },
        ]
    }

    /// The hashes of the six types of [`mini`], computed by a separate implementation of the census's recipe (a
    /// script that encodes the same table itself, parses it and hashes it). Derived's depends on its parent's member,
    /// an interface and the parent's hash; hkArray's on its subtype; Holder's on the template parameters it nests.
    const MINI_HASHES: [(usize, u32); 6] = [
        (1, 0xd9bf3216),
        (2, 0xc35323a7),
        (3, 0x1fddaf95),
        (4, 0x867f4123),
        (5, 0xb4d8c9bd),
        (6, 0xadf001aa),
    ];

    /// `mini()` with the flags of its member `a` (Base's) changed.
    fn with_member_flags(flags: u64) -> Vec<Spec> {
        let mut specs = mini();
        specs[1].fields[0].1 = flags;
        specs
    }

    #[test]
    fn member_flags_have_bit_0x20_and_none_outside_0x27() {
        for flags in [0x20, 0x21, 0x22, 0x23, 0x24, 0x25] {
            let types = read(&type_section(&with_member_flags(flags), &[])).expect("flags in the data");
            assert_eq!(types[2].fields[0].flags, flags);
        }
        for flags in [0, 1, 0x05, 0x28, 0x30, 0x40, 0xA0, 0x100] {
            let error = read(&type_section(&with_member_flags(flags), &[])).unwrap_err();
            assert!(
                error.detail.contains(&format!("has flags {flags:#x}")),
                "{flags:#x}: {error}"
            );
        }
    }

    /// The six stored values of the member flags name who may reach the member and whether its value is written.
    #[test]
    fn member_flags_name_their_access_and_storage() {
        let want = [
            (0x20, DeclAccess::Public, true),
            (0x21, DeclAccess::Public, false),
            (0x22, DeclAccess::Protected, true),
            (0x23, DeclAccess::Protected, false),
            (0x24, DeclAccess::Private, true),
            (0x25, DeclAccess::Private, false),
        ];
        for (flags, access, serialized) in want {
            let types = read(&type_section(&with_member_flags(flags), &[])).expect("flags in the data");
            let decl = types[2].fields[0].decl_flags();
            assert_eq!(decl.access(), Some(access), "{flags:#x}");
            assert_eq!(decl.serialized(), serialized, "{flags:#x}");
            assert!(decl.contains(DeclFlags::DATA_FIELD), "{flags:#x}");
            assert_eq!(decl.unseen(), 0, "{flags:#x}");
        }
        // both access bits: within 0x27, so the reader accepts the record, and neither access names it
        let types = read(&type_section(&with_member_flags(0x26), &[])).expect("0x26 is within 0x27");
        assert_eq!(types[2].fields[0].decl_flags().access(), None);
        assert_eq!(DeclFlags(0x28 | 0x20).unseen(), 0x08);
    }

    /// A type that stores a subtype is a pointer (6) or an array (8), by its own format or its nearest parent's.
    #[test]
    fn a_subtype_belongs_to_a_pointer_or_an_array() {
        let subtyped = |format: Option<u64>, parent: usize| Spec {
            name: "Subtyped",
            parent,
            format,
            subtype: Some(1),
            ..Spec::default()
        };
        let table = |extra: Spec| {
            let mut specs = mini();
            specs.push(extra);
            read(&type_section(&specs, &[]))
        };
        // its own format: a pointer (6) or an array (8, also a fixed one with bits above)
        assert!(table(subtyped(Some(6), 0)).is_ok());
        assert!(table(subtyped(Some(0x28 | 8 << 8), 0)).is_ok());
        // inherited: type 4 is the array `hkArray`; type 2 is a class
        assert!(table(subtyped(None, 4)).is_ok());
        // another kind, own or inherited, or none at all
        for (format, parent, kind) in [(Some(7), 0, 7), (Some(0x8002), 0, 2), (None, 2, 7), (None, 0, 16)] {
            let error = table(subtyped(format, parent)).unwrap_err();
            assert!(
                error.detail.contains("type 7 stores a subtype")
                    && error.detail.contains(&format!("of kind {kind}")),
                "{error}"
            );
        }
    }

    #[test]
    fn crc_chains_continue_the_crc_32() {
        // the standard check value of the IEEE CRC-32
        assert_eq!(crc32_continue(0, b"123456789"), 0xcbf4_3926);
        assert_eq!(crc32_continue(0, b""), 0);
        let whole = crc32_continue(0, b"hknpShape");
        assert_eq!(crc32_continue(crc32_continue(0, b"hkn"), b"pShape"), whole);
    }

    #[test]
    fn a_type_table_whose_hashes_follow_from_its_types_is_read() {
        let types = read(&type_section(&mini(), &MINI_HASHES)).expect("the table");
        for (i, hash) in MINI_HASHES {
            assert_eq!(types[i].hash, Some(hash), "type {i}");
        }
        // the alignment word is split, the type flags typed
        let base = &types[2];
        assert_eq!(
            (base.size, base.alignment, base.requested_alignment),
            (Some(8), Some(8), Some(3))
        );
        assert_eq!(base.type_flags, Some(TypeFlags(147)));
        assert!(TypeFlags(147).contains(TypeFlags::OWN_VTABLE | TypeFlags::INTERFACE));
        assert_eq!(TypeFlags(147).unseen(), 0);
        assert_eq!(TypeFlags(0x1000).unseen(), 0x1000);
        assert_eq!(types[3].requested_alignment, Some(0));
        assert_eq!(types[1].type_flags, None);
        // a table without hashes, or with some, is read all the same
        read(&type_section(&mini(), &[])).expect("no THSH");
        read(&type_section(&mini(), &MINI_HASHES[2..3])).expect("one hash");
    }

    #[test]
    fn a_stored_hash_that_the_description_does_not_give_is_refused() {
        for i in 0..MINI_HASHES.len() {
            let mut hashes = MINI_HASHES;
            hashes[i].1 ^= 1;
            let e = read(&type_section(&mini(), &hashes)).unwrap_err().to_string();
            assert!(
                e.contains(&format!("type {} (", hashes[i].0)) && e.contains("stored hash"),
                "{e}"
            );
        }
    }

    /// Every part of a type's description is in its hash: a table that changes any one of them and keeps the stored
    /// hashes is refused, naming the first type whose hash it breaks.
    #[test]
    fn every_component_of_the_description_is_hashed() {
        type Change = fn(&mut Vec<Spec>);
        let changes: [(&str, usize, Change); 14] = [
            ("parent", 3, |t| t[2].parent = 0),
            ("interface type", 3, |t| t[2].interfaces[0].0 = 1),
            ("interface word", 3, |t| t[2].interfaces[0].1 = 9),
            ("format", 2, |t| t[1].format = Some(5 | 0x20)),
            ("subtype", 4, |t| t[3].subtype = Some(2)),
            ("name", 1, |t| t[0].name = "u64"),
            ("template value", 4, |t| t[3].parameters[1].1 = 8),
            ("template type", 4, |t| t[3].parameters[0].1 = 2),
            ("version", 2, |t| t[1].version = Some(2)),
            ("size", 1, |t| t[0].size = Some((8, 4))),
            ("alignment", 1, |t| t[0].size = Some((4, 8))),
            ("requested alignment", 2, |t| t[1].size = Some((8, 8))),
            ("type flags", 2, |t| t[1].type_flags = Some(3)),
            ("member offset", 2, |t| t[1].fields[0].2 = 4),
        ];
        for (what, first_broken, change) in changes {
            let mut specs = mini();
            change(&mut specs);
            let e = read(&type_section(&specs, &MINI_HASHES)).unwrap_err().to_string();
            assert!(e.contains(&format!("type {first_broken} (")), "{what}: {e}");
        }
        // a parent's member is part of the derived class's hash, and a member's type
        let mut specs = mini();
        specs[1].fields[0].0 = "c";
        let e = read(&type_section(&specs, &MINI_HASHES[2..]))
            .unwrap_err()
            .to_string();
        assert!(e.contains("type 3 ("), "{e}");
        let mut specs = mini();
        specs[2].fields[0].3 = 2;
        let e = read(&type_section(&specs, &MINI_HASHES[2..3]))
            .unwrap_err()
            .to_string();
        assert!(e.contains("type 3 ("), "{e}");
    }

    #[test]
    fn a_type_table_that_refers_to_itself_is_hashed_without_recursion_and_without_end() {
        // a class holding a member of its own type; its stored hash is whatever the table gives
        let own = vec![Spec {
            name: "Node",
            format: Some(7),
            size: Some((8, 8)),
            fields: vec![("next", 0x20, 0, 1)],
            ..Spec::default()
        }];
        let e = read(&type_section(&own, &[(1, 0x1234_5678)]))
            .unwrap_err()
            .to_string();
        assert!(e.contains("stored hash"), "{e}");
        // a chain of members 50,000 types deep: no stack grows with it
        let n = 50_000;
        let chain: Vec<Spec> = (1..=n)
            .map(|i| Spec {
                name: "Link",
                format: Some(7),
                size: Some((8, 8)),
                fields: vec![("next", 0x20, 0, (i % n) + 1)],
                ..Spec::default()
            })
            .collect();
        let e = read(&type_section(&chain, &[(1, 0)])).unwrap_err().to_string();
        assert!(e.contains("stored hash"), "{e}");
        // template parameters that name each other are stopped
        let looped = vec![Spec {
            name: "Loop",
            format: Some(7),
            parameters: vec![("t0", 1), ("t1", 1)],
            ..Spec::default()
        }];
        let e = read(&type_section(&looped, &[(1, 0)])).unwrap_err().to_string();
        assert!(e.contains("template parameters"), "{e}");
    }

    #[test]
    fn the_alignment_word_has_nothing_above_bit_15() {
        let mut specs = mini();
        specs[0].size = Some((4, 0x1_0004));
        let e = read(&type_section(&specs, &[])).unwrap_err().to_string();
        assert!(e.contains("alignment word"), "{e}");
        specs[0].size = Some((4, 0xF010));
        let types = read(&type_section(&specs, &[])).expect("bits 12 to 15 are the requested alignment");
        assert_eq!(
            (types[1].alignment, types[1].requested_alignment),
            (Some(16), Some(15))
        );
        // type flags wider than 32 bits are not flags
        let mut specs = mini();
        specs[1].type_flags = Some(1 << 32);
        assert!(read(&type_section(&specs, &[])).is_err());
    }

    #[test]
    fn reserved_integer_markers_are_errors() {
        for first in 0xfa..=0xff {
            let bytes = [first; 9];
            let mut c = Cursor {
                bytes: &bytes,
                position: 0,
                base: 0,
            };
            assert!(c.packed().is_err());
        }
    }
}
