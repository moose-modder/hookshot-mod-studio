//! The class metadata a packfile carries when it was written with reflection, and the one schema every class table of
//! ours has: [`ClassTable`], a list of [`Class`]es, each with its parent, version, size, enums and [`Member`]s (type,
//! subtype, array size, flags, offset). A table is read from the `__types__` section of a packfile
//! ([`ClassTable::from_packfile`]) or built by hand, and gives
//!
//! - the class signature a packfile stores beside each class name ([`ClassTable::signature`]), which is how a table is
//!   checked against a file;
//! - the offsets and sizes the layout rules give the members ([`ClassTable::layout`]), which is how a table for another
//!   platform is derived from one for a platform whose offsets are known.
//!
//! Eight of the 490 Tony Hawk's SHRED (PS3) files carry their classes: `RAGDOLL.HKX`, `RAGDOLLB.HKX`, `RAGDOLLF.HKX`,
//! `RAGDOLLFB.HKX` and the four `AVATAR_` ones, each 80 objects (68 `hkClass`, 12 `hkClassEnum`) with 544 declared
//! members between them. No other packfile of SHRED, Sekiro or Elden Ring has a `__types__` section.
//!
//! The layout of the metadata objects, as every one of those objects holds it (4-byte pointers, big endian):
//!
//! | object | offset | member |
//! |---|---|---|
//! | `hkClass` (48 bytes) | `+0x00` | name (a string) |
//! | | `+0x04` | parent (an `hkClass`) |
//! | | `+0x08` | object size |
//! | | `+0x0C` | number of implemented interfaces |
//! | | `+0x10`, `+0x14` | declared enums (an array of `hkClassEnum` structs, 20 bytes each, and its length) |
//! | | `+0x18`, `+0x1C` | declared members (an array of `hkClassMember` structs, 24 bytes each, and its length) |
//! | | `+0x20`, `+0x24` | default struct, attributes (pointers, null in all) |
//! | | `+0x28`, `+0x2C` | flags, described version |
//! | `hkClassMember` (24 bytes) | `+0x00` | name |
//! | | `+0x04`, `+0x08` | class, enum (pointers) |
//! | | `+0x0C`, `+0x0D` | type, subtype (bytes) |
//! | | `+0x0E` | array size (`u16`) |
//! | | `+0x10`, `+0x12` | flags, offset (`u16`) |
//! | | `+0x14` | attributes (a pointer, null in all) |
//! | `hkClassEnum` (20 bytes) | `+0x00` | name |
//! | | `+0x04`, `+0x08` | items (an array of 8-byte `{value: i32, name: string}`) and its length |
//! | | `+0x0C`, `+0x10` | attributes (a pointer), flags |
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use primase_foundations::hash::crc32;

use super::{Packfile, Rules};
use crate::{Error, Result, invalid};

/// A member's type (`hkClassMember::Type`), a number. The constants are the types the 544 members of the eight files
/// use; the others are kept as stored and have no layout here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MemberType(pub u8);

impl MemberType {
    /// `bool`, one byte.
    pub const BOOL: Self = Self(1);
    /// A signed 8-bit integer.
    pub const INT8: Self = Self(3);
    /// An unsigned 8-bit integer.
    pub const UINT8: Self = Self(4);
    /// A signed 16-bit integer.
    pub const INT16: Self = Self(5);
    /// An unsigned 16-bit integer.
    pub const UINT16: Self = Self(6);
    /// A signed 32-bit integer.
    pub const INT32: Self = Self(7);
    /// An unsigned 32-bit integer.
    pub const UINT32: Self = Self(8);
    /// An unsigned 64-bit integer.
    pub const UINT64: Self = Self(10);
    /// A 32-bit float.
    pub const REAL: Self = Self(11);
    /// Four floats, 16-byte aligned.
    pub const VECTOR4: Self = Self(12);
    /// A quaternion: four floats, 16-byte aligned.
    pub const QUATERNION: Self = Self(13);
    /// Three vectors, 16-byte aligned.
    pub const MATRIX3: Self = Self(14);
    /// A rotation and a translation: 64 bytes, 16-byte aligned.
    pub const TRANSFORM: Self = Self(18);
    /// The "zero" type: what a serialize-ignored member is hashed as.
    pub const ZERO: Self = Self(19);
    /// A pointer; the subtype is what it points to.
    pub const POINTER: Self = Self(20);
    /// An `hkArray`: pointer, size, capacity; the subtype is the element.
    pub const ARRAY: Self = Self(22);
    /// An enum, stored in the integer type its subtype names.
    pub const ENUM: Self = Self(24);
    /// A struct held in place; the member's class is the struct.
    pub const STRUCT: Self = Self(25);
    /// A pointer-sized unsigned integer.
    pub const ULONG: Self = Self(30);
    /// Flags, stored in the integer type the subtype names.
    pub const FLAGS: Self = Self(31);
    /// A half float (the top 16 bits of an `f32`).
    pub const HALF: Self = Self(32);
    /// A string pointer.
    pub const STRING_PTR: Self = Self(33);
}

/// `hkClassMember::Flags`: the member is 8-byte aligned.
pub const ALIGN_8: u16 = 128;
/// The member is 16-byte aligned.
pub const ALIGN_16: u16 = 256;
/// The member is not owned (the pointer or array does not free it).
pub const NOT_OWNED: u16 = 512;
/// The member is not written to the file: its bytes in the data are zero.
pub const SERIALIZE_IGNORED: u16 = 1024;
/// The member is 32-byte aligned.
pub const ALIGN_32: u16 = 2048;

/// One enum item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumItem {
    /// The item's name.
    pub name: String,
    /// The item's value.
    pub value: i32,
}

/// An enum: a name and its items.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Enum {
    /// The enum's name.
    pub name: String,
    /// The items, in order.
    pub items: Vec<EnumItem>,
    /// The stored flags word.
    pub flags: u32,
}

/// A member of a class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// The member's name.
    pub name: String,
    /// The class of a struct, a pointer to one or an array of them.
    pub class: Option<String>,
    /// The enum of an enum or flags member.
    pub enumeration: Option<Enum>,
    /// The member's type.
    pub kind: MemberType,
    /// The element, pointee or storage type.
    pub subtype: MemberType,
    /// The length of a C array of the type (0 for none).
    pub array_size: u16,
    /// The flags ([`ALIGN_16`], [`SERIALIZE_IGNORED`], ...).
    pub flags: u16,
    /// The offset in the object as stored; `None` in a table built to be laid out.
    pub offset: Option<usize>,
}

/// A class: the `hkClass` data model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Class {
    /// The class name.
    pub name: String,
    /// The parent class's name.
    pub parent: Option<String>,
    /// The object size as stored; `None` in a table built to be laid out.
    pub size: Option<usize>,
    /// The number of implemented interfaces.
    pub interfaces: u32,
    /// The enums the class declares.
    pub enums: Vec<Enum>,
    /// The members the class declares, in order.
    pub members: Vec<Member>,
    /// The stored flags word.
    pub flags: u32,
    /// The described version.
    pub version: u32,
    /// The signature the packfile lists the class with, if it lists it.
    pub signature: Option<u32>,
}

/// A table of classes: the one description of a class set that [`signature`](Self::signature),
/// [`layout`](Self::layout) and the readers share.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClassTable {
    /// The classes, in the order they are stored.
    pub classes: Vec<Class>,
}

/// The offsets and size the layout rules give a class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    /// The class's size in bytes, rounded up to its alignment.
    pub size: usize,
    /// The class's alignment.
    pub alignment: usize,
    /// Where the class's own members end (the next member of a class derived from it may start here when the rules
    /// reuse padding).
    pub data_size: usize,
    /// The offset of each declared member, in order.
    pub offsets: Vec<usize>,
}

/// How deep a signature may descend into the classes a class holds; a table that nests deeper is refused.
const MAX_DEPTH: usize = 32;
/// The most bytes a signature may hash.
const MAX_SIGNATURE_BYTES: usize = 1 << 24;

fn too_big() -> Error {
    invalid(0, "the class table nests or repeats classes without bound")
}

fn i32_bytes(v: i32) -> [u8; 4] {
    v.to_le_bytes()
}

fn i16_bytes(v: u16) -> [u8; 2] {
    v.to_le_bytes()
}

impl ClassTable {
    /// The class called `name`.
    pub fn class(&self, name: &str) -> Option<&Class> {
        self.classes.iter().find(|c| c.name == name)
    }

    /// Read the classes a packfile carries in its `__types__` section; `None` when it has none (517 of the 525 files
    /// of the games have none). Every pointer, string and count is checked; an object that is not an `hkClass` or an
    /// `hkClassEnum`, a name two classes share, a parent or struct class the file lacks and a bad array are refused.
    pub fn from_packfile(pack: &Packfile<'_>) -> Result<Option<Self>> {
        let Some(view) = pack.types()? else {
            return Ok(None);
        };
        if view.rules.pointer_size != 4 || view.rules.little_endian {
            return Err(view.err(
                0,
                "reflection is read from files of 4-byte pointers and big endian words",
            ));
        }
        let mut names: Vec<(usize, String)> = Vec::new();
        for object in view.objects() {
            let class = view.class_name(object);
            match class {
                "hkClass" => {
                    let name = view.c_string(view.target(object.offset, "a class name")?)?;
                    names.push((object.offset, name));
                }
                "hkClassEnum" => {}
                other => return Err(view.err(object.offset, format!("a {other} object in __types__"))),
            }
        }
        let name_of = |offset: usize| -> Result<String> {
            names
                .binary_search_by_key(&offset, |(o, _)| *o)
                .ok()
                .and_then(|i| names.get(i))
                .map(|(_, n)| n.clone())
                .ok_or_else(|| view.err(offset, "a class pointer that is not an hkClass object"))
        };
        let read_enum = |at: usize| -> Result<Enum> {
            let name = view.c_string(view.target(at, "an enum name")?)?;
            let items = view.simple_array(at.saturating_add(4), 8)?;
            let mut out = Vec::new();
            for i in 0..items.len {
                let item = view.at(items.data.unwrap_or(0), i.saturating_mul(8))?;
                out.push(EnumItem {
                    value: i32::from_ne_bytes(view.u32(item)?.to_ne_bytes()),
                    name: view.c_string(view.target(view.at(item, 4)?, "an enum item name")?)?,
                });
            }
            Ok(Enum {
                name,
                items: out,
                flags: view.u32(view.at(at, 0x10)?)?,
            })
        };
        let mut classes = Vec::new();
        for object in view.objects().iter().filter(|o| view.class_name(o) == "hkClass") {
            let at = object.offset;
            let name = name_of(at)?;
            if classes.iter().any(|c: &Class| c.name == name) {
                return Err(view.err(at, format!("two classes are called {name}")));
            }
            let parent = view.pointer(view.at(at, 4)?)?.map(name_of).transpose()?;
            let mut enums = Vec::new();
            let declared = view.simple_array(view.at(at, 0x10)?, 20)?;
            for i in 0..declared.len {
                enums.push(read_enum(
                    view.at(declared.data.unwrap_or(0), i.saturating_mul(20))?,
                )?);
            }
            let mut members = Vec::new();
            let list = view.simple_array(view.at(at, 0x18)?, 24)?;
            for i in 0..list.len {
                let m = view.at(list.data.unwrap_or(0), i.saturating_mul(24))?;
                let class = view.pointer(view.at(m, 4)?)?.map(name_of).transpose()?;
                let enumeration = view.pointer(view.at(m, 8)?)?.map(read_enum).transpose()?;
                if view.pointer(view.at(m, 0x14)?)?.is_some() {
                    return Err(view.err(m, "a member with attributes"));
                }
                members.push(Member {
                    name: view.c_string(view.target(m, "a member name")?)?,
                    class,
                    enumeration,
                    kind: MemberType(view.u8(view.at(m, 0x0C)?)?),
                    subtype: MemberType(view.u8(view.at(m, 0x0D)?)?),
                    array_size: view.u16(view.at(m, 0x0E)?)?,
                    flags: view.u16(view.at(m, 0x10)?)?,
                    offset: Some(usize::from(view.u16(view.at(m, 0x12)?)?)),
                });
            }
            for (what, off) in [("default struct", 0x20), ("attributes", 0x24)] {
                if view.pointer(view.at(at, off)?)?.is_some() {
                    return Err(view.err(at, format!("a class with {what}")));
                }
            }
            classes.push(Class {
                signature: pack
                    .class_names()
                    .iter()
                    .find(|c| c.name == name)
                    .map(|c| c.signature),
                name,
                parent,
                size: Some(view.count(view.at(at, 8)?)?),
                interfaces: view.u32(view.at(at, 0x0C)?)?,
                enums,
                members,
                flags: view.u32(view.at(at, 0x28)?)?,
                version: view.u32(view.at(at, 0x2C)?)?,
            });
        }
        Ok(Some(ClassTable { classes }))
    }

    /// The offset in an object of class `class` of the member `path` names, under `rules`: the first name is a member of
    /// the class or one of its ancestors, and each further name a member of the struct the one before holds
    /// (`["collidable", "allowedPenetrationDepth"]` on `hkpRigidBody` is `0x5C`).
    pub fn member_offset(&self, class: &str, path: &[&str], rules: Rules) -> Result<usize> {
        let (first, rest) = path
            .split_first()
            .ok_or_else(|| invalid(0, "an empty member path"))?;
        for c in self.chain(class)? {
            let Some(index) = c.members.iter().position(|m| m.name == *first) else {
                continue;
            };
            let layout = self.layout(&c.name, rules)?;
            let at = layout
                .offsets
                .get(index)
                .copied()
                .ok_or_else(|| invalid(0, format!("{}.{first} has no offset", c.name)))?;
            if rest.is_empty() {
                return Ok(at);
            }
            let inner = c
                .members
                .get(index)
                .and_then(|m| m.class.as_deref())
                .ok_or_else(|| invalid(0, format!("{}.{first} holds no struct", c.name)))?;
            return at
                .checked_add(self.member_offset(inner, rest, rules)?)
                .ok_or_else(too_big);
        }
        Err(invalid(0, format!("{class} has no member {first}")))
    }

    fn require(&self, name: &str) -> Result<&Class> {
        self.class(name)
            .ok_or_else(|| invalid(0, format!("the table has no class {name}")))
    }

    /// The class and then each ancestor.
    fn chain<'t>(&'t self, name: &str) -> Result<Vec<&'t Class>> {
        let mut out = Vec::new();
        let mut next = Some(name.to_owned());
        while let Some(n) = next {
            let class = self.require(&n)?;
            if out.len() >= MAX_DEPTH {
                return Err(too_big());
            }
            out.push(class);
            next = class.parent.clone();
        }
        Ok(out)
    }

    /// The class signature: the CRC-32 of a description of the class and its ancestors, derived first. The bytes (every
    /// integer little endian): for the class, then each ancestor, the number of implemented interfaces; each declared
    /// enum's name, item names with their values and the item count, then the enum count; each declared member, with
    /// first the description of its struct class (the class and its ancestors) unless it or its subtype is a pointer,
    /// and then its enum, followed by its name and the type, subtype, array size and flags as 16-bit words, then the
    /// member count. An enum or flags member hashes its subtype as 0 and the storage size in bits XORed into its flags;
    /// a serialize-ignored member hashes as the zero type with its type as the subtype, the flag cleared and, when it
    /// had a subtype, that as one more word before the array size.
    ///
    /// The recipe is the one that reproduces the signatures stored beside the class names: in the eight SHRED files
    /// with reflection, all 96 classes that a file lists with a signature (12 in each) recompute to it.
    pub fn signature(&self, name: &str) -> Result<u32> {
        let mut bytes = Vec::new();
        self.describe(name, &mut bytes, 0)?;
        Ok(crc32(&bytes))
    }

    fn describe(&self, name: &str, out: &mut Vec<u8>, depth: usize) -> Result<()> {
        if depth > MAX_DEPTH || out.len() > MAX_SIGNATURE_BYTES {
            return Err(too_big());
        }
        for class in self.chain(name)? {
            out.extend(i32_bytes(i32::try_from(class.interfaces).map_err(|_| too_big())?));
            for e in &class.enums {
                describe_enum(e, out);
            }
            out.extend(i32_bytes(
                i32::try_from(class.enums.len()).map_err(|_| too_big())?,
            ));
            for m in &class.members {
                if let Some(c) = &m.class
                    && m.kind != MemberType::POINTER
                    && m.subtype != MemberType::POINTER
                {
                    self.describe(c, out, depth.saturating_add(1))?;
                }
                if let Some(e) = &m.enumeration {
                    describe_enum(e, out);
                }
                let (mut kind, mut subtype, mut flags, mut extra) = (m.kind, m.subtype, m.flags, None);
                if kind == MemberType::ENUM || kind == MemberType::FLAGS {
                    flags ^= match storage_size(subtype) {
                        Some(size) => u16::try_from(size.saturating_mul(8)).map_err(|_| too_big())?,
                        None => 0,
                    };
                    subtype = MemberType(0);
                }
                if flags & SERIALIZE_IGNORED != 0 {
                    flags ^= SERIALIZE_IGNORED;
                    if subtype.0 != 0 {
                        extra = Some(u16::from(subtype.0));
                    }
                    subtype = kind;
                    kind = MemberType::ZERO;
                }
                out.extend(m.name.as_bytes());
                out.extend(i16_bytes(u16::from(kind.0)));
                out.extend(i16_bytes(u16::from(subtype.0)));
                if let Some(e) = extra {
                    out.extend(i16_bytes(e));
                }
                out.extend(i16_bytes(m.array_size));
                out.extend(i16_bytes(flags));
            }
            out.extend(i32_bytes(
                i32::try_from(class.members.len()).map_err(|_| too_big())?,
            ));
        }
        Ok(())
    }

    /// The layout the rules give class `name`.
    ///
    /// A class starts where its parent's last member ends when the rules reuse padding, and where the parent's size ends
    /// when they do not (0 for a class without a parent). Each interface the class implements then takes one pointer, its
    /// table pointer, which no member lists: `hkBaseObject` implements one and starts its members at 4, `hkpShapeContainer`
    /// likewise, and `hkpCollisionFilter`, which implements four on top of its 8-byte parent, starts at 24. Each member sits
    /// at the next offset its alignment allows after the one before. The sizes and alignments are those of the member
    /// types the eight SHRED files use (see [`MemberType`]); any other type is refused.
    pub fn layout(&self, name: &str, rules: Rules) -> Result<Layout> {
        self.layout_at(name, rules, 0)
    }

    fn layout_at(&self, name: &str, rules: Rules, depth: usize) -> Result<Layout> {
        if depth > MAX_DEPTH {
            return Err(too_big());
        }
        let class = self.require(name)?;
        let pointer = rules.pointer_size;
        let (mut alignment, mut cursor) = match &class.parent {
            Some(parent) => {
                let p = self.layout_at(parent, rules, depth.saturating_add(1))?;
                (
                    p.alignment,
                    if rules.reuse_padding { p.data_size } else { p.size },
                )
            }
            None => (1, 0),
        };
        if class.interfaces > 0 {
            let tables = usize::try_from(class.interfaces)
                .ok()
                .and_then(|n| n.checked_mul(pointer))
                .ok_or_else(too_big)?;
            cursor = align_to(cursor, pointer)?
                .checked_add(tables)
                .ok_or_else(too_big)?;
            alignment = alignment.max(pointer);
        }
        let mut offsets = Vec::with_capacity(class.members.len());
        for m in &class.members {
            let (mut size, mut align) = self.member_size(m, rules, depth)?;
            if m.flags & ALIGN_16 != 0 {
                align = align.max(16);
            }
            if m.flags & ALIGN_8 != 0 {
                align = align.max(8);
            }
            if m.flags & ALIGN_32 != 0 {
                align = align.max(32);
            }
            if m.array_size > 0 {
                size = size.checked_mul(usize::from(m.array_size)).ok_or_else(too_big)?;
            }
            let at = align_to(cursor, align)?;
            offsets.push(at);
            cursor = at.checked_add(size).ok_or_else(too_big)?;
            alignment = alignment.max(align);
        }
        Ok(Layout {
            size: align_to(cursor, alignment)?,
            alignment,
            data_size: cursor,
            offsets,
        })
    }

    /// Size and alignment of one member, before its array size and flags.
    fn member_size(&self, m: &Member, rules: Rules, depth: usize) -> Result<(usize, usize)> {
        let p = rules.pointer_size;
        let t = m.kind;
        Ok(match t {
            MemberType::BOOL | MemberType::INT8 | MemberType::UINT8 => (1, 1),
            MemberType::INT16 | MemberType::UINT16 | MemberType::HALF => (2, 2),
            MemberType::INT32 | MemberType::UINT32 | MemberType::REAL => (4, 4),
            MemberType::UINT64 => (8, 8),
            MemberType::VECTOR4 | MemberType::QUATERNION => (16, 16),
            MemberType::MATRIX3 => (48, 16),
            MemberType::TRANSFORM => (64, 16),
            MemberType::POINTER | MemberType::STRING_PTR | MemberType::ULONG => (p, p),
            MemberType::ARRAY => (p.saturating_add(8), p),
            MemberType::ENUM | MemberType::FLAGS => {
                let size = storage_size(m.subtype).ok_or_else(|| {
                    invalid(
                        0,
                        format!("member {}: enum stored as type {}", m.name, m.subtype.0),
                    )
                })?;
                (size, size)
            }
            MemberType::STRUCT => {
                let class = m
                    .class
                    .as_deref()
                    .ok_or_else(|| invalid(0, format!("member {}: a struct without a class", m.name)))?;
                let l = self.layout_at(class, rules, depth.saturating_add(1))?;
                (l.size, l.alignment)
            }
            MemberType(other) => {
                return Err(invalid(
                    0,
                    format!(
                        "member {}: type {other} has no layout in the data this was read from",
                        m.name
                    ),
                ));
            }
        })
    }
}

fn describe_enum(e: &Enum, out: &mut Vec<u8>) {
    out.extend(e.name.as_bytes());
    for item in &e.items {
        out.extend(item.name.as_bytes());
        out.extend(i32_bytes(item.value));
    }
    out.extend(i32_bytes(i32::try_from(e.items.len()).unwrap_or(i32::MAX)));
}

/// The bytes an enum or flags member stores its value in, by the integer type of its subtype.
fn storage_size(subtype: MemberType) -> Option<usize> {
    match subtype {
        MemberType::INT8 | MemberType::UINT8 => Some(1),
        MemberType::INT16 | MemberType::UINT16 => Some(2),
        MemberType::INT32 | MemberType::UINT32 => Some(4),
        _ => None,
    }
}

fn align_to(value: usize, to: usize) -> Result<usize> {
    let to = to.max(1);
    value
        .checked_add(to.saturating_sub(1))
        .and_then(|v| v.checked_div(to))
        .and_then(|q| q.checked_mul(to))
        .ok_or_else(too_big)
}
