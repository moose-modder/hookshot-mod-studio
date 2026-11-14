//! Bounds-checked view of Havok binary *packfiles* (the pre-tagfile `0x57E0E057 0x10C0C010` container): version 7 with
//! the 7.1.0-r1 contents version, 4-byte pointers, big endian (the layout Tony Hawk's SHRED (PS3) ships, and the one
//! [`physics`] decodes), and versions 9 and 11 with 8-byte pointers, little endian, the `hk_2013.1.0-r1` and
//! `hk_2014.1.0-r1` contents versions (the face animations and the behaviour file Sekiro and Elden Ring ship). Any
//! other version or layout rules are refused by name.
//!
//! Layout (offsets from the file start; every field held on the 490 Tony Hawk's SHRED (PS3) files, and, where it says
//! "v9/v11", on the 35 distinct packfiles of Sekiro (29) and Elden Ring (6)):
//!
//! | offset | field | check |
//! |---|---|---|
//! | `0x00` | magic `57 E0 E0 57 10 C0 C0 10` | exact |
//! | `0x08` | user tag, kept in [`Packfile::user_tag`] | read, not interpreted (0 in every file) |
//! | `0x0C` | file version | 7, 9 or 11 |
//! | `0x10` | layout rules: pointer size, little-endian flag, reuse-padding, empty-base-optimisation | `4, 0, 1, 1` (7); `8, 1, 0, 1` (9, 11) |
//! | `0x14` | section count | headers fit the file |
//! | `0x18`, `0x1C` | contents section index and offset (the root object) | the data section; a virtual-fixup target |
//! | `0x20`, `0x24` | class-name section index and the root's class-name offset | the class-name section; a listed name |
//! | `0x28` | contents version, 16 bytes, NUL-terminated | `Havok-7.1.0-r1`; `hk_2013.1.0-r1` (9); `hk_2014.1.0-r1` (11) |
//! | `0x38`, `0x3C` | two header words, kept in [`Packfile::filler`] | read, not interpreted (7: `0xFFFFFFFF` twice; 9: `0`, `0xFFFFFFFF`; 11: `0` and the two 16-bit fields below, `0x15`, `0x10`; `0x15`, `0` in the file without a region) |
//! | `0x40` | version 11 only: the predicate region, as many bytes as the 16-bit field at `0x3E` says (16 bytes in 28 of the 29 version 11 files, none in the other), kept in [`Packfile::predicate_region`] | read, not interpreted |
//! | `0x40 + 48 i` (7, 9) or `+ region + 64 i` (11) | section header: tag (20 bytes: the name, NULs, and `0xFF` as byte 19), then `start, local, global, virtual, exports, imports, end`; in 11 sixteen `0xFF` bytes follow | in order, inside the file |
//!
//! In version 7 the two words at `0x38` hold no value (`0xFFFFFFFF` in every file). In version 11 they are a flags word
//! and two 16-bit fields (`0x3C`: 21, `0x3E`: the predicate region's size, 16; the one file without a region stores 0 there).
//!
//! `__classnames__` holds `u32 signature, 0x09, name\0` entries (then `0xFF` padding). A signature is a CRC-32 of the
//! class's member list (and its ancestors'), so it names the class definition the file was written against; each of
//! the 24 names in the 7.1 corpus has one signature, listed in [`CLASS_SIGNATURES`], and a version 7 file that stores
//! another signature for one of those names is refused. `__types__` is empty in 517 of the 525 files, so their
//! class layouts live in [`physics`], stated per SDK version and confirmed on the data; the other eight (SHRED's ragdolls)
//! carry the classes themselves, read by [`reflection`]. `__data__` holds the
//! objects; its fixups are local `{src,dst}`, global `{src,section,dst}` and virtual `{src,section,class-name offset}`,
//! each list padded with `0xFFFFFFFF` words to a 16-byte boundary (a list that ends on one has no terminator). An *object* is a
//! virtual-fixup target on a 16-byte boundary; it extends to the next one (arrays and strings that follow belong to it). A
//! pointer slot is non-null iff it has a local or global fixup, and holds zero either way. A fixup's source is a
//! pointer-aligned word; a local fixup's target is 4-byte aligned and a global fixup's 16-byte aligned. An array header
//! is `ptr, size, size | 0x80000000`, with a fixup on the pointer iff the array is not empty.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use std::collections::BTreeMap;

use primase_foundations::bytes::{
    BoundsError,
    Endian::{Big, Little},
    View,
};

use crate::{Error, Result, invalid};

pub mod image;
pub mod physics;
pub mod reflection;

/// The contents version of the version 7 files, the one [`physics`] decodes.
pub const CONTENTS_VERSION: &str = "Havok-7.1.0-r1";

/// How a packfile lays its words out: the four bytes at `0x10`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rules {
    /// Bytes in a pointer: 4 or 8.
    pub pointer_size: usize,
    /// Whether the words of the file are little endian.
    pub little_endian: bool,
    /// Whether a derived class's first member may start in its parent's tail padding.
    pub reuse_padding: bool,
    /// Whether an empty base class takes no byte.
    pub empty_base: bool,
}

impl Rules {
    /// The rules of the version 7 files (PowerPC: 4-byte pointers, big endian, padding reused): `4, 0, 1, 1`.
    pub const POWERPC_32: Rules = Rules {
        pointer_size: 4,
        little_endian: false,
        reuse_padding: true,
        empty_base: true,
    };
    /// The rules of the version 9 and 11 files (x86-64 Windows: 8-byte pointers, little endian, no padding reuse):
    /// `8, 1, 0, 1`.
    pub const WINDOWS_64: Rules = Rules {
        pointer_size: 8,
        little_endian: true,
        reuse_padding: false,
        empty_base: true,
    };
    fn from_bytes([pointer, little, reuse, empty_base]: [u8; 4]) -> Option<Self> {
        let flag = |b: u8| match b {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        };
        Some(Rules {
            pointer_size: usize::from(pointer),
            little_endian: flag(little)?,
            reuse_padding: flag(reuse)?,
            empty_base: flag(empty_base)?,
        })
    }
    /// The four bytes at `0x10`.
    pub fn bytes(self) -> [u8; 4] {
        [
            u8::try_from(self.pointer_size).unwrap_or(0),
            u8::from(self.little_endian),
            u8::from(self.reuse_padding),
            u8::from(self.empty_base),
        ]
    }
    fn order(self) -> primase_foundations::bytes::Endian {
        if self.little_endian { Little } else { Big }
    }
}

/// A packfile layout the readers know: the file version, the rules, the contents version and the size of a section
/// header.
#[derive(Debug, Clone, Copy)]
struct Known {
    version: u32,
    rules: Rules,
    contents: &'static str,
    /// Bytes from one section header to the next.
    stride: usize,
}

/// The layouts read: Tony Hawk's SHRED's (490 files), and Sekiro's and Elden Ring's (26 + 3 files of Sekiro and 3 + 3
/// of Elden Ring, distinct).
const KNOWN: [Known; 3] = [
    Known {
        version: 7,
        rules: Rules::POWERPC_32,
        contents: CONTENTS_VERSION,
        stride: 48,
    },
    Known {
        version: 9,
        rules: Rules::WINDOWS_64,
        contents: "hk_2013.1.0-r1",
        stride: 48,
    },
    Known {
        version: 11,
        rules: Rules::WINDOWS_64,
        contents: "hk_2014.1.0-r1",
        stride: 64,
    },
];
/// Every class name of the 490 Tony Hawk's SHRED packfiles with the one signature each stores for it in every file that
/// lists it. A file storing another signature for one of these names was written against another definition of the
/// class, so [`Packfile::read`] refuses it; a name not listed here is kept with its signature, unchecked.
pub const CLASS_SIGNATURES: [(&str, u32); 24] = [
    ("hkClass", 0x1442_5E51),
    ("hkClassMember", 0x4A98_6551),
    ("hkClassEnum", 0x8A36_09CF),
    ("hkClassEnumItem", 0xCE6F_8A6C),
    ("hkRootLevelContainer", 0x2772_C11E),
    ("hkpPhysicsData", 0xC2A4_61E4),
    ("hkpPhysicsSystem", 0xFF72_4C17),
    ("hkpRigidBody", 0xAA8C_3B19),
    ("hkpMoppBvTreeShape", 0x90B2_9D39),
    ("hkpMoppCode", 0x924C_2661),
    ("hkpStorageExtendedMeshShape", 0x3D52_E7D2),
    ("hkpStorageExtendedMeshShapeMeshSubpartStorage", 0x5AAD_4DE6),
    ("hkpConvexVerticesShape", 0x4E8A_B2B7),
    ("hkpConvexVerticesConnectivity", 0x63D3_8E9C),
    ("hkpConvexTranslateShape", 0x5BA0_A5F7),
    ("hkpBoxShape", 0x3444_D2D5),
    ("hkpCylinderShape", 0x3E46_3C3A),
    ("hkpSphereShape", 0x0795_D9FA),
    ("hkpCapsuleShape", 0xDD0B_1FD3),
    ("hkpListShape", 0xA193_7CBD),
    ("hkpConstraintInstance", 0x4FB4_E6BB),
    ("hkpRagdollConstraintData", 0x17A4_D42F),
    ("hkpLimitedHingeConstraintData", 0x1C56_6B11),
    ("hkpBreakableConstraintData", 0x7034_BDB6),
];
const MAGIC: [u8; 8] = [0x57, 0xE0, 0xE0, 0x57, 0x10, 0xC0, 0xC0, 0x10];
const ROOT_HEADER: usize = 0x40;
/// The bytes of a section header that hold values: the 20-byte tag and seven 32-bit offsets.
const SECTION_FIELDS: usize = 48;
const END_OF_FIXUPS: u32 = 0xFFFF_FFFF;
/// The flag the capacity word of every array in the files carries beside the size (`size | 0x80000000`, also when the
/// array is empty).
const ARRAY_CAPACITY_FLAG: u32 = 0x8000_0000;

/// A section header; every offset but `start` is relative to `start`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// The section's name (`__classnames__`, `__types__`, `__data__`).
    pub tag: String,
    /// Offset of the section's bytes in the file.
    pub start: usize,
    /// Offset of the local fixups.
    pub local: usize,
    /// Offset of the global fixups.
    pub global: usize,
    /// Offset of the virtual fixups.
    pub virtual_fixups: usize,
    /// Offset of the exports.
    pub exports: usize,
    /// Offset of the imports.
    pub imports: usize,
    /// Offset of the end of the section.
    pub end: usize,
}

/// An object: a virtual-fixup target of the data section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Object {
    /// Offset in the data section.
    pub offset: usize,
    /// Bytes up to the next object (or the end of the data), including its trailing arrays and strings.
    pub size: usize,
    class: usize,
}

/// One `__classnames__` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassName {
    /// The class name.
    pub name: String,
    /// The class signature stored before it.
    pub signature: u32,
    /// Offset of the name in the class-name section (what virtual fixups name).
    offset: usize,
}

/// A hkArray header: element pointer (None when null) and element count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Array {
    /// Data-section offset of the elements; `None` when null.
    pub data: Option<usize>,
    /// The element count.
    pub len: usize,
}

/// A parsed packfile: the section table, the class names, the fixups and the objects, over the borrowed file bytes.
///
/// Offsets taken and returned by its accessors are offsets in the `__data__` section; errors report file offsets.
#[derive(Debug)]
pub struct Packfile<'a> {
    bytes: &'a [u8],
    /// The file version at `0x0C`: 7, 9 or 11.
    pub version: u32,
    /// The layout rules at `0x10`.
    pub rules: Rules,
    /// The contents version text at `0x28`.
    pub contents_version: String,
    /// The section headers, in file order.
    pub sections: Vec<Section>,
    /// The user tag at `0x08`, as stored.
    pub user_tag: u32,
    /// The two header words at `0x38` and `0x3C`, as stored (`0xFFFFFFFF` twice in every version 7 file).
    pub filler: [u32; 2],
    predicate_region: &'a [u8],
    classes: Vec<ClassName>,
    data_start: usize,
    data_len: usize,
    local: Vec<(usize, usize)>,
    global: Vec<(usize, usize)>,
    objects: Vec<Object>,
    root: usize,
}

fn bounds(offset: usize, what: &str) -> impl FnOnce(BoundsError) -> Error + '_ {
    move |_| invalid(offset, format!("{what} runs past the file"))
}

fn sum(offset: usize, a: usize, b: usize) -> Result<usize> {
    a.checked_add(b)
        .ok_or_else(|| invalid(offset, "offset overflows the address space"))
}

/// The first two class names of a packfile (`hkClass`, then `hkClassMember`) carry the signatures of the SDK's class
/// definitions, so the SDK of a file this reader refuses shows without reading its layout. The first row is the
/// `Havok-7.1.0-r1` this reader reads (all 490 Tony Hawk's SHRED files). The second is on every packfile of the games'
/// other Havok versions: the 29 distinct ones of Sekiro (3 of contents version `hk_2013.1.0-r1`, 26 of `hk_2014.1.0-r1`)
/// and the 6 of Elden Ring (3 and 3), and it is the pair a 2014.2 reflection dump gives (sourced), so these signatures
/// do not tell 2013.1, 2014.1 and 2014.2 apart; the header's contents version does.
const SDK_OF_FIRST_CLASSES: [(&str, u32, u32); 2] = [
    ("Havok 7.1.0-r1", 0x1442_5E51, 0x4A98_6551),
    ("Havok 2013.1 to 2014.2", 0x33D4_2383, 0xB0EF_A719),
];

/// What a packfile this reader refuses is, as text to append to the refusal: the contents version its header names
/// (unless `contents` is false) and the SDK its first two class names' signatures belong to, or, when they are
/// none the reader knows, the signatures themselves. Empty when the file has no readable class-name section. The section
/// is found by its tag in the first 256 bytes, so the header layout of the file's version does not matter; its words
/// are read in the byte order the layout rules name.
fn foreign_sdk(bytes: &[u8], contents: bool) -> String {
    let text = bytes
        .get(0x28..0x38)
        .map(|t| t.split(|&b| b == 0).next().unwrap_or_default())
        .filter(|t| contents && !t.is_empty() && t.iter().all(u8::is_ascii_graphic))
        .map(|t| format!("contents version {:?}", String::from_utf8_lossy(t)));
    let classes = first_classes(bytes).map(|(class, member)| {
        match SDK_OF_FIRST_CLASSES
            .iter()
            .find(|&&(_, a, b)| (a, b) == (class, member))
        {
            Some((sdk, ..)) => {
                format!("class names of {sdk} (hkClass {class:#010x}, hkClassMember {member:#010x})")
            }
            None => format!(
                "hkClass {class:#010x} and hkClassMember {member:#010x} are those of no SDK this reader knows"
            ),
        }
    });
    let parts: Vec<String> = text.into_iter().chain(classes).collect();
    if parts.is_empty() {
        String::new()
    } else {
        format!(": {}", parts.join(", "))
    }
}

/// The signatures of the first two class names if they are `hkClass` and `hkClassMember`.
fn first_classes(bytes: &[u8]) -> Option<(u32, u32)> {
    let order = if *bytes.get(0x11)? != 0 { Little } else { Big };
    let head = bytes.get(..0x100.min(bytes.len()))?;
    let tag = head.windows(14).position(|w| w == b"__classnames__")?;
    let v = View::new(bytes);
    let mut at = usize::try_from(v.u32(tag.checked_add(20)?, order).ok()?).ok()?;
    let mut found = [(&[][..], 0u32); 2];
    for slot in &mut found {
        let signature = v.u32(at, order).ok()?;
        if *bytes.get(at.checked_add(4)?)? != 9 {
            return None;
        }
        let name_at = at.checked_add(5)?;
        let name = bytes.get(name_at..)?.split(|&b| b == 0).next()?;
        *slot = (name, signature);
        at = name_at.checked_add(name.len())?.checked_add(1)?;
    }
    let [(first, class), (second, member)] = found;
    (first == b"hkClass" && second == b"hkClassMember").then_some((class, member))
}

fn to_usize(w: u32) -> usize {
    usize::try_from(w).unwrap_or(usize::MAX)
}

impl<'a> Packfile<'a> {
    /// Parse and check the container (header, sections, class names, every fixup). The accessors read the `__data__`
    /// section; [`types`](Self::types) is the same view of `__types__`.
    pub fn read(bytes: &'a [u8]) -> Result<Self> {
        Self::read_section(bytes, "__data__")
    }

    /// The view of the `__types__` section, which holds the class metadata (`hkClass`, `hkClassEnum` objects) of a file
    /// written with reflection: the same parse over that section's bytes and fixups, with its own objects. `None` when
    /// the section is empty (it is in 517 of the 525 packfiles of the games). It has no root: [`root`](Self::root) is 0.
    pub fn types(&self) -> Result<Option<Packfile<'a>>> {
        let empty = self.sections.iter().any(|s| s.tag == "__types__" && s.end == 0);
        if empty {
            return Ok(None);
        }
        Self::read_section(self.bytes, "__types__").map(Some)
    }

    fn read_section(bytes: &'a [u8], subject: &str) -> Result<Self> {
        let v = View::new(bytes);
        let magic = v.array::<8>(0).map_err(bounds(0, "the magic"))?;
        if magic != MAGIC {
            return Err(invalid(0, "not a Havok packfile (magic)"));
        }
        // the header words of a little-endian file are little endian: the version is read in the order the layout
        // rules name (a file too short to name one is read big endian)
        let order = match v.array::<4>(0x10) {
            Ok([_, little, ..]) if little != 0 => Little,
            _ => Big,
        };
        let version = v.u32(0x0C, order).map_err(bounds(0x0C, "the file version"))?;
        let Some(known) = KNOWN.iter().find(|k| k.version == version) else {
            return Err(invalid(
                0x0C,
                format!(
                    "unsupported packfile version {version} (7, 9 and 11 only){}",
                    foreign_sdk(bytes, true)
                ),
            ));
        };
        let layout = v.array::<4>(0x10).map_err(bounds(0x10, "the layout rules"))?;
        let [ptr, little, reuse, empty_base] = layout;
        let want = known.rules;
        if usize::from(ptr) != want.pointer_size {
            return Err(invalid(
                0x10,
                format!(
                    "unsupported pointer size {ptr} ({} only in version {version}){}",
                    want.pointer_size,
                    foreign_sdk(bytes, true)
                ),
            ));
        }
        if (little != 0) != want.little_endian {
            return Err(invalid(
                0x11,
                format!(
                    "unsupported byte order: {} ({} only in version {version}){}",
                    if little != 0 {
                        "little endian"
                    } else {
                        "big endian"
                    },
                    if want.little_endian {
                        "little endian"
                    } else {
                        "big endian"
                    },
                    foreign_sdk(bytes, true)
                ),
            ));
        }
        let rules = Rules::from_bytes(layout).filter(|r| *r == want);
        let Some(rules) = rules else {
            return Err(invalid(
                0x12,
                format!(
                    "unsupported layout rules: reuse padding {reuse}, empty base optimisation {empty_base} ({}, {} only in version {version}){}",
                    u8::from(want.reuse_padding),
                    u8::from(want.empty_base),
                    foreign_sdk(bytes, true)
                ),
            ));
        };
        let order = rules.order();
        let u32_at = |o: usize, what: &str| v.u32(o, order).map_err(bounds(o, what));
        let usize_at = |o: usize, what: &str| -> Result<usize> {
            usize::try_from(u32_at(o, what)?).map_err(|_| invalid(o, "value exceeds the address space"))
        };
        let version_text = v.slice(0x28, 16).map_err(bounds(0x28, "the contents version"))?;
        let version_text = version_text.split(|&b| b == 0).next().unwrap_or_default();
        if version_text != known.contents.as_bytes() {
            return Err(invalid(
                0x28,
                format!(
                    "unsupported contents version {:?} ({} only in version {version}){}",
                    String::from_utf8_lossy(version_text),
                    known.contents,
                    foreign_sdk(bytes, false)
                ),
            ));
        }
        let user_tag = u32_at(0x08, "the user tag")?;
        let filler = [
            u32_at(0x38, "the header filler")?,
            u32_at(0x3C, "the header filler")?,
        ];
        // version 11 puts a predicate region between the root header and the first section header; its size is the
        // 16-bit field at 0x3E (16 in every file)
        let region = if version == 11 {
            let size = usize::from(
                v.u16(0x3E, order)
                    .map_err(bounds(0x3E, "the predicate region size"))?,
            );
            v.slice(ROOT_HEADER, size)
                .map_err(bounds(ROOT_HEADER, "the predicate region"))?
        } else {
            &[]
        };
        let first_section = sum(ROOT_HEADER, ROOT_HEADER, region.len())?;
        let stride = known.stride;
        let count = usize_at(0x14, "the section count")?;
        let table_end = count
            .checked_mul(stride)
            .and_then(|n| n.checked_add(first_section))
            .filter(|&end| end <= bytes.len())
            .ok_or_else(|| invalid(0x14, "the section headers run past the file"))?;
        let mut sections = Vec::new();
        for i in 0..count {
            // in range: the table fits the file (checked above)
            let o = first_section.saturating_add(i.saturating_mul(stride));
            // a version 11 header ends in sixteen 0xFF bytes
            if let Some(pad) = stride
                .checked_sub(SECTION_FIELDS)
                .and_then(|n| v.slice(o.saturating_add(SECTION_FIELDS), n).ok())
                && pad.iter().any(|&b| b != 0xFF)
            {
                return Err(invalid(
                    o.saturating_add(SECTION_FIELDS),
                    "the end of a section header is not 0xFF padding",
                ));
            }
            let field = v.array::<20>(o).map_err(bounds(o, "a section tag"))?;
            let [tag_area @ .., marker] = field;
            if marker != 0xFF {
                return Err(invalid(
                    o.saturating_add(19),
                    format!("section header byte 19 is {marker:#04x}, not 0xFF"),
                ));
            }
            let name_len = tag_area.iter().position(|&b| b == 0).unwrap_or(tag_area.len());
            let (tag_bytes, after_name) = tag_area.split_at(name_len);
            if after_name.iter().any(|&b| b != 0) {
                return Err(invalid(
                    o.saturating_add(name_len),
                    "section tag is followed by bytes that are not zero",
                ));
            }
            let tag = std::str::from_utf8(tag_bytes)
                .map_err(|_| invalid(o, "section tag is not text"))?
                .to_string();
            let f = |k: usize| {
                usize_at(
                    o.saturating_add(20).saturating_add(k.saturating_mul(4)),
                    "a section field",
                )
            };
            let s = Section {
                tag,
                start: f(0)?,
                local: f(1)?,
                global: f(2)?,
                virtual_fixups: f(3)?,
                exports: f(4)?,
                imports: f(5)?,
                end: f(6)?,
            };
            let ordered = s.local <= s.global
                && s.global <= s.virtual_fixups
                && s.virtual_fixups <= s.exports
                && s.exports <= s.imports
                && s.imports <= s.end;
            if !ordered {
                return Err(invalid(
                    o,
                    format!("section {:?}: fixup offsets are not in order", s.tag),
                ));
            }
            if s.start < table_end || sum(o, s.start, s.end)? > bytes.len() {
                return Err(invalid(o, format!("section {:?} lies outside the file", s.tag)));
            }
            sections.push(s);
        }
        let section_named = |tag: &str| -> Result<usize> {
            let mut found = sections
                .iter()
                .enumerate()
                .filter(|(_, s)| s.tag == tag)
                .map(|(i, _)| i);
            match (found.next(), found.next()) {
                (Some(i), None) => Ok(i),
                _ => Err(invalid(
                    first_section,
                    format!("expected exactly one {tag} section"),
                )),
            }
        };
        let class_section = section_named("__classnames__")?;
        let data_section = section_named("__data__")?;
        let subject_section = section_named(subject)?;
        if usize_at(0x18, "the contents section")? != data_section {
            return Err(invalid(0x18, "the root object is not in the data section"));
        }
        if usize_at(0x20, "the class-name section")? != class_section {
            return Err(invalid(
                0x20,
                "the root class name is not in the class-name section",
            ));
        }
        let (Some(class_header), Some(data_header)) =
            (sections.get(class_section), sections.get(subject_section))
        else {
            return Err(invalid(first_section, "section index out of range"));
        };
        // class names
        let class_bytes = v
            .slice(class_header.start, class_header.end)
            .map_err(bounds(class_header.start, "the class names"))?;
        let mut classes = Vec::new();
        let mut at = 0usize;
        // an entry is `u32 signature, 0x09, name, NUL`; the signature may start with any byte, so the separator decides
        while class_bytes.get(at..).unwrap_or_default().get(4) == Some(&9) {
            let here = sum(class_header.start, class_header.start, at)?;
            let signature = u32_at(here, "a class signature")?;
            let name_at = sum(here, at, 5)?;
            let tail = class_bytes.get(name_at..).unwrap_or_default();
            let len = tail
                .iter()
                .position(|&b| b == 0)
                .ok_or_else(|| invalid(here, "class name is not terminated"))?;
            let name = std::str::from_utf8(tail.get(..len).unwrap_or_default())
                .map_err(|_| invalid(here, "class name is not text"))?;
            if let Some((_, want)) = CLASS_SIGNATURES
                .iter()
                .find(|(known, want)| version == 7 && *known == name && *want != signature)
            {
                return Err(invalid(
                    here,
                    format!(
                        "class {name} has signature {signature:#010x}, not the {want:#010x} of the {CONTENTS_VERSION} class this reader knows"
                    ),
                ));
            }
            classes.push(ClassName {
                name: name.to_string(),
                signature,
                offset: name_at,
            });
            at = name_at.saturating_add(len).saturating_add(1);
        }
        if class_bytes
            .get(at..)
            .is_some_and(|rest| rest.iter().any(|&b| b != 0xFF))
        {
            return Err(invalid(
                class_header.start,
                "class names are followed by something but 0xFF padding",
            ));
        }
        let data_start = data_header.start;
        let data_len = data_header.local;
        let fixups = |from: usize, to: usize, words: usize| -> Result<Vec<Vec<usize>>> {
            let width = words.saturating_mul(4);
            let span = to
                .checked_sub(from)
                .ok_or_else(|| invalid(data_start, "fixup range is reversed"))?;
            let n = span.checked_div(width).unwrap_or(0);
            let mut out = Vec::new();
            let mut ended = false;
            for i in 0..n {
                let o = data_start
                    .saturating_add(from)
                    .saturating_add(i.saturating_mul(width));
                let mut e = Vec::new();
                for w in 0..words {
                    e.push(u32_at(o.saturating_add(w.saturating_mul(4)), "a fixup")?);
                }
                let first = e.first().copied().unwrap_or(END_OF_FIXUPS);
                if first == END_OF_FIXUPS {
                    ended = true;
                    if e.iter().any(|&w| w != END_OF_FIXUPS) {
                        return Err(invalid(o, "fixup list terminator is not 0xFFFFFFFF padding"));
                    }
                } else if ended {
                    return Err(invalid(o, "fixup after the list's terminator"));
                } else {
                    out.push(e.into_iter().map(to_usize).collect());
                }
            }
            // sections are 16-aligned, so a list may end in a partial record of 0xFF padding
            let tail = data_start
                .saturating_add(from)
                .saturating_add(n.saturating_mul(width));
            let rest = v
                .slice(tail, span.saturating_sub(n.saturating_mul(width)))
                .map_err(bounds(tail, "fixup padding"))?;
            if rest.iter().any(|&b| b != 0xFF) {
                return Err(invalid(tail, "partial fixup record that is not 0xFF padding"));
            }
            Ok(out)
        };
        // in the 490 files a fixup's source is a 4-byte word at a 4-byte offset (all 130,610), the target of a local one
        // is 4-byte aligned (82,847) and the target of a global one 16-byte aligned (47,763)
        let pointer_size = rules.pointer_size;
        // the strings of an array of strings in the version 9 and 11 files follow one another from 2-byte boundaries: of
        // the 2,526 local targets of the 35 files 43 are not 4-byte aligned and none is odd
        let local_alignment = if version == 7 { 4 } else { 2 };
        let slot = |at: usize, dst: usize, dst_alignment: usize, what: &str| -> Result<()> {
            let in_range = sum(data_start, at, pointer_size)? <= data_len;
            if !in_range || dst > data_len {
                return Err(invalid(
                    data_start.saturating_add(at),
                    format!("{what} fixup {at:#x} -> {dst:#x} is outside the data"),
                ));
            }
            if !at.is_multiple_of(pointer_size) {
                return Err(invalid(
                    data_start.saturating_add(at),
                    format!("{what} fixup source {at:#x} is not {pointer_size}-byte aligned"),
                ));
            }
            if !dst.is_multiple_of(dst_alignment) {
                return Err(invalid(
                    data_start.saturating_add(at),
                    format!("{what} fixup target {dst:#x} is not {dst_alignment}-byte aligned"),
                ));
            }
            Ok(())
        };
        let mut local = Vec::new();
        for e in fixups(data_header.local, data_header.global, 2)? {
            let (src, dst) = (e.first().copied().unwrap_or(0), e.get(1).copied().unwrap_or(0));
            slot(src, dst, local_alignment, "local")?;
            local.push((src, dst));
        }
        let mut global = Vec::new();
        for e in fixups(data_header.global, data_header.virtual_fixups, 3)? {
            let (src, sec, dst) = (
                e.first().copied().unwrap_or(0),
                e.get(1).copied().unwrap_or(0),
                e.get(2).copied().unwrap_or(0),
            );
            if sec != subject_section {
                return Err(invalid(
                    data_start.saturating_add(src),
                    format!("global fixup into section {sec} (its own section only)"),
                ));
            }
            slot(src, dst, 16, "global")?;
            global.push((src, dst));
        }
        let mut virtuals = Vec::new();
        for e in fixups(data_header.virtual_fixups, data_header.exports, 3)? {
            let (src, sec, name) = (
                e.first().copied().unwrap_or(0),
                e.get(1).copied().unwrap_or(0),
                e.get(2).copied().unwrap_or(0),
            );
            let here = data_start.saturating_add(src);
            if sec != class_section {
                return Err(invalid(
                    here,
                    format!("virtual fixup names section {sec}, not the class names"),
                ));
            }
            let class = classes.binary_search_by_key(&name, |c| c.offset).map_err(|_| {
                invalid(
                    here,
                    format!("virtual fixup class-name offset {name:#x} is not a name"),
                )
            })?;
            if src >= data_len {
                return Err(invalid(here, "object starts outside the data"));
            }
            if !src.is_multiple_of(16) {
                return Err(invalid(here, "object is not 16-byte aligned"));
            }
            virtuals.push((src, class));
        }
        if data_header.exports != data_header.end {
            return Err(invalid(data_start, "export/import tables are not supported"));
        }
        local.sort_unstable();
        global.sort_unstable();
        virtuals.sort_unstable();
        let repeated = |l: &[(usize, usize)]| {
            l.windows(2)
                .any(|w| w.first().map(|a| a.0) == w.get(1).map(|b| b.0))
        };
        if repeated(&local) || repeated(&global) {
            return Err(invalid(data_start, "two fixups for one pointer slot"));
        }
        if virtuals
            .windows(2)
            .any(|w| w.first().map(|a| a.0) == w.get(1).map(|b| b.0))
        {
            return Err(invalid(data_start, "two virtual fixups for one object"));
        }
        if local
            .iter()
            .any(|(s, _)| global.binary_search_by_key(s, |(g, _)| *g).is_ok())
        {
            return Err(invalid(
                data_start,
                "a pointer slot has a local and a global fixup",
            ));
        }
        let mut objects = Vec::new();
        for (i, &(offset, class)) in virtuals.iter().enumerate() {
            let next = virtuals.get(i.saturating_add(1)).map_or(data_len, |n| n.0);
            objects.push(Object {
                offset,
                size: next.saturating_sub(offset),
                class,
            });
        }
        let root = if subject_section == data_section {
            let root = usize_at(0x1C, "the root offset")?;
            let root_name = usize_at(0x24, "the root class name")?;
            let root_object = objects
                .binary_search_by_key(&root, |o| o.offset)
                .ok()
                .and_then(|i| objects.get(i));
            match root_object.and_then(|o| classes.get(o.class)) {
                Some(c) if c.offset == root_name => {}
                _ => {
                    return Err(invalid(
                        0x1C,
                        "the root offset/class is not a virtual-fixup object",
                    ));
                }
            }
            root
        } else {
            0
        };
        Ok(Self {
            bytes,
            version,
            rules,
            contents_version: known.contents.to_owned(),
            sections,
            user_tag,
            filler,
            predicate_region: region,
            classes,
            data_start,
            data_len,
            local,
            global,
            objects,
            root,
        })
    }

    /// The predicate region of a version 11 file (between the root header and the first section header), as stored;
    /// empty in the other versions.
    pub fn predicate_region(&self) -> &'a [u8] {
        self.predicate_region
    }

    /// An error unless the file has the layout [`physics`] decodes: version 7, 4-byte pointers, big endian.
    pub(crate) fn require_710(&self) -> Result<()> {
        if self.version == 7 {
            Ok(())
        } else {
            Err(invalid(
                0x0C,
                format!(
                    "the physics views decode version 7 files (Havok-7.1.0-r1), not version {} ({})",
                    self.version, self.contents_version
                ),
            ))
        }
    }

    /// Every object, in data order.
    pub fn objects(&self) -> &[Object] {
        &self.objects
    }

    /// Offset of the root object (`hkRootLevelContainer`).
    pub fn root(&self) -> usize {
        self.root
    }

    /// The class name of `object`.
    pub fn class_name(&self, object: &Object) -> &str {
        self.classes.get(object.class).map_or("", |c| c.name.as_str())
    }

    /// The `__classnames__` entries, in file order.
    pub fn class_names(&self) -> &[ClassName] {
        &self.classes
    }

    /// Offsets of the objects of class `name`, in data order.
    pub fn objects_of<'s>(&'s self, name: &'s str) -> impl Iterator<Item = usize> + 's {
        self.objects
            .iter()
            .filter(move |o| self.class_name(o) == name)
            .map(|o| o.offset)
    }

    /// Object count per class name.
    pub fn census(&self) -> BTreeMap<&str, usize> {
        let mut out = BTreeMap::new();
        for o in &self.objects {
            let n: &mut usize = out.entry(self.class_name(o)).or_default();
            *n = n.saturating_add(1);
        }
        out
    }

    /// The class name of the object that starts at `offset`.
    pub fn class_at(&self, offset: usize) -> Result<&str> {
        self.objects
            .binary_search_by_key(&offset, |o| o.offset)
            .ok()
            .and_then(|i| self.objects.get(i))
            .map(|o| self.class_name(o))
            .ok_or_else(|| self.err(offset, "no object starts here"))
    }

    /// An error at a data-section offset, reported at its file offset.
    pub(crate) fn err(&self, offset: usize, detail: impl Into<String>) -> Error {
        invalid(self.data_start.saturating_add(offset), detail)
    }

    /// `offset + add`, an error instead of wrapping.
    pub(crate) fn at(&self, offset: usize, add: usize) -> Result<usize> {
        offset
            .checked_add(add)
            .ok_or_else(|| self.err(offset, "offset overflows the address space"))
    }

    /// `len` bytes of the data section at `offset`.
    pub fn bytes(&self, offset: usize, len: usize) -> Result<&'a [u8]> {
        let end = self.at(offset, len)?;
        if end > self.data_len {
            return Err(self.err(offset, format!("{len} bytes run past the data section")));
        }
        let start = self.at(self.data_start, offset)?;
        self.bytes
            .get(start..self.at(start, len)?)
            .ok_or_else(|| self.err(offset, "the data section runs past the file"))
    }

    /// The byte at `offset` in the data section.
    pub fn u8(&self, offset: usize) -> Result<u8> {
        self.bytes(offset, 1)?
            .first()
            .copied()
            .ok_or_else(|| self.err(offset, "empty read"))
    }
    /// The `u16` at `offset` in the data section, in the file's byte order.
    pub fn u16(&self, offset: usize) -> Result<u16> {
        let b = self.array(offset)?;
        Ok(if self.rules.little_endian {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    }
    /// The `u32` at `offset` in the data section, in the file's byte order.
    pub fn u32(&self, offset: usize) -> Result<u32> {
        let b = self.array(offset)?;
        Ok(if self.rules.little_endian {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    }
    /// The pointer-sized word at `offset` (4 or 8 bytes by the file's rules), as stored.
    pub fn word(&self, offset: usize) -> Result<u64> {
        if self.rules.pointer_size == 8 {
            let b = self.array(offset)?;
            Ok(if self.rules.little_endian {
                u64::from_le_bytes(b)
            } else {
                u64::from_be_bytes(b)
            })
        } else {
            self.u32(offset).map(u64::from)
        }
    }
    /// A `u32` as a size or count.
    pub fn count(&self, offset: usize) -> Result<usize> {
        usize::try_from(self.u32(offset)?).map_err(|_| self.err(offset, "count exceeds the address space"))
    }
    /// A finite `f32` (NaN and infinity are errors).
    pub fn f32(&self, offset: usize) -> Result<f32> {
        let bits = self.u32(offset)?;
        let x = f32::from_bits(bits);
        if x.is_finite() {
            Ok(x)
        } else {
            Err(self.err(offset, "non-finite float"))
        }
    }
    /// `N` consecutive finite floats.
    pub fn floats<const N: usize>(&self, offset: usize) -> Result<[f32; N]> {
        let mut out = [0.0; N];
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = self.f32(self.at(offset, i.saturating_mul(4))?)?;
        }
        Ok(out)
    }
    pub(crate) fn array<const N: usize>(&self, offset: usize) -> Result<[u8; N]> {
        self.bytes(offset, N)?
            .try_into()
            .map_err(|_| self.err(offset, "short read"))
    }

    /// The pointer stored at `offset`: None when null. A slot with a fixup resolves to its target in the data section;
    /// the slot holds zero either way.
    pub fn pointer(&self, offset: usize) -> Result<Option<usize>> {
        let word = self.word(offset)?;
        let local = self
            .local
            .binary_search_by_key(&offset, |(s, _)| *s)
            .ok()
            .and_then(|i| self.local.get(i));
        let global = self
            .global
            .binary_search_by_key(&offset, |(s, _)| *s)
            .ok()
            .and_then(|i| self.global.get(i));
        match (local.or(global), word) {
            (Some(&(_, dst)), 0) => Ok(Some(dst)),
            (Some(_), _) => Err(self.err(
                offset,
                format!("pointer slot with a fixup holds {word:#x}, not zero"),
            )),
            (None, 0) => Ok(None),
            (None, _) => Err(self.err(offset, format!("pointer slot holds {word:#x} but has no fixup"))),
        }
    }

    /// A pointer that must not be null.
    pub fn target(&self, offset: usize, what: &str) -> Result<usize> {
        self.pointer(offset)?
            .ok_or_else(|| self.err(offset, format!("{what} is null")))
    }

    /// A hkArray header at `offset` (`ptr, size, capacityAndFlags`). The capacity word is the size with `0x80000000`
    /// set, also for an empty array; a non-empty array has data (a fixup) and an empty one has none.
    pub fn array_header(&self, offset: usize) -> Result<Array> {
        let data = self.pointer(offset)?;
        let size_at = self.at(offset, self.rules.pointer_size)?;
        let len = self.count(size_at)?;
        let cap = self.u32(self.at(size_at, 4)?)?;
        let want = u32::try_from(len).map(|n| n | ARRAY_CAPACITY_FLAG).ok();
        if want != Some(cap) {
            return Err(self.err(
                offset,
                format!("array of {len} has capacity word {cap:#010x}, not its size or {ARRAY_CAPACITY_FLAG:#010x}"),
            ));
        }
        if len != 0 && data.is_none() {
            return Err(self.err(offset, "non-empty array has no data"));
        }
        if len == 0 && data.is_some() {
            return Err(self.err(offset, "empty array has a fixup"));
        }
        Ok(Array { data, len })
    }

    /// A pointer and a 32-bit count at `offset` and `offset + pointer size` (an `hkSimpleArray`), its `stride`-byte
    /// elements inside the section. A non-empty array has data (a fixup) and an empty one has none.
    pub fn simple_array(&self, offset: usize, stride: usize) -> Result<Array> {
        let data = self.pointer(offset)?;
        let len = self.count(self.at(offset, self.rules.pointer_size)?)?;
        match data {
            None if len != 0 => return Err(self.err(offset, "non-empty array has no data")),
            Some(_) if len == 0 => return Err(self.err(offset, "empty array has a fixup")),
            Some(at) => {
                let size = len
                    .checked_mul(stride)
                    .ok_or_else(|| self.err(offset, "array size overflows the address space"))?;
                self.bytes(at, size)?;
            }
            None => {}
        }
        Ok(Array { data, len })
    }

    /// The NUL-terminated text at `offset`, bytes mapped to chars one to one (Latin-1).
    pub fn c_string(&self, offset: usize) -> Result<String> {
        let tail = self.bytes(offset, self.data_len.saturating_sub(offset))?;
        let len = tail
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| self.err(offset, "string is not terminated"))?;
        Ok(tail
            .get(..len)
            .unwrap_or_default()
            .iter()
            .map(|&b| char::from(b))
            .collect())
    }
}
