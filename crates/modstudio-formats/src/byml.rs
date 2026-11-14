//! BYML (versions 2 to 7, little-endian), read: the game's parameter files (`*.bgyml`: an actor pack's model info,
//! its physics controllers and their cloth lists).
//!
//! Layout: `YB`, the version (u16), then the offsets of the key table, the string table and the root node (u32
//! each; 0 for none). A string table is `0xC2`, its count (u24), the offsets of its strings from the table (one more
//! than the count) and the zero-terminated strings. A container is its type byte, its count (u24) and then:
//!
//! - an array (`0xC0`): the type of each element (one byte each, padded to 4), then each element's value (u32);
//! - a dictionary (`0xC1`): per entry the key's index in the key table (u24), the value's type (u8) and the value
//!   (u32), sorted by key;
//! - a hash map (`0x20`, `0x21`): per entry the hash (u32 or u64) and the value (u32), then the types (a byte each).
//!
//! A value is held in its u32 for a bool (`0xD0`), int (`0xD1`), float (`0xD2`), uint (`0xD3`) or a string (`0xA0`,
//! its index in the string table); otherwise it is the offset of the 64-bit value (`0xD4` int, `0xD5` uint, `0xD6`
//! double), of binary data (`0xA1`: u32 size, then the bytes; `0xA2`: size, alignment, the bytes) or of a container.
//! `0xFF` is null.

use crate::bytes::{ReadLe, check_magic};
use crate::error::{Result, invalid, unsupported};

/// How deep containers may nest (a file pointing a container at itself would loop).
const MAX_DEPTH: usize = 64;

/// A value of a BYML document.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i32),
    UInt(u32),
    Float(f32),
    Int64(i64),
    UInt64(u64),
    Double(f64),
    String(String),
    Binary(Vec<u8>),
    Array(Vec<Value>),
    /// A dictionary, in the file's order (sorted by key).
    Map(Vec<(String, Value)>),
    /// A map keyed by hashes.
    HashMap(Vec<(u64, Value)>),
}

impl Value {
    /// A dictionary's value for `key`.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Map(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(items) => Some(items),
            _ => None,
        }
    }

    /// A string at `path` (dictionary keys, from this value), if every step is there.
    pub fn str_at(&self, path: &[&str]) -> Option<&str> {
        let mut v = self;
        for key in path {
            v = v.get(key)?;
        }
        v.as_str()
    }
}

/// Reads a BYML document: its root (`Value::Null` for a document without one).
pub fn parse(data: &[u8]) -> Result<Value> {
    if data.get(..2) == Some(b"BY") {
        return Err(unsupported("big-endian BYML"));
    }
    check_magic(data, 0, "YB")?;
    let version = data.u16_at(2)?;
    if !(2..=7).contains(&version) {
        return Err(unsupported(format!("BYML version {version}")));
    }
    let keys = string_table(data, data.u32_at(4)? as usize)?;
    let strings = string_table(data, data.u32_at(8)? as usize)?;
    let root = data.u32_at(12)? as usize;
    if root == 0 {
        return Ok(Value::Null);
    }
    let doc = Doc {
        data,
        keys,
        strings,
    };
    doc.container(root, 0)
}

struct Doc<'a> {
    data: &'a [u8],
    keys: Vec<&'a str>,
    strings: Vec<&'a str>,
}

fn u24_at(data: &[u8], offset: usize) -> Result<usize> {
    let b = data.array_at::<3>(offset)?;
    Ok(b[0] as usize | (b[1] as usize) << 8 | (b[2] as usize) << 16)
}

fn string_table(data: &[u8], at: usize) -> Result<Vec<&str>> {
    if at == 0 {
        return Ok(Vec::new());
    }
    if data.u8_at(at)? != 0xC2 {
        return Err(invalid(format!("no string table at 0x{at:x}")));
    }
    let n = u24_at(data, at + 1)?;
    (0..n)
        .map(|i| {
            let off = data.u32_at(at + 4 + 4 * i)? as usize;
            data.cstr_at(at + off)
        })
        .collect()
}

impl Doc<'_> {
    fn string(&self, index: u32) -> Result<String> {
        self.strings
            .get(index as usize)
            .map(|s| (*s).to_owned())
            .ok_or_else(|| invalid(format!("string {index} isn't in the string table")))
    }

    fn node(&self, kind: u8, value: u32, depth: usize) -> Result<Value> {
        let at = value as usize;
        Ok(match kind {
            0xA0 => Value::String(self.string(value)?),
            0xA1 => {
                let n = self.data.u32_at(at)? as usize;
                Value::Binary(self.data.slice_at(at + 4, n)?.to_vec())
            }
            0xA2 => {
                let n = self.data.u32_at(at)? as usize;
                Value::Binary(self.data.slice_at(at + 8, n)?.to_vec())
            }
            0xD0 => Value::Bool(value != 0),
            0xD1 => Value::Int(value as i32),
            0xD2 => Value::Float(f32::from_bits(value)),
            0xD3 => Value::UInt(value),
            0xD4 => Value::Int64(self.data.u64_at(at)? as i64),
            0xD5 => Value::UInt64(self.data.u64_at(at)?),
            0xD6 => Value::Double(f64::from_bits(self.data.u64_at(at)?)),
            0xFF => Value::Null,
            0xC0 | 0xC1 | 0x20 | 0x21 => self.container(at, depth + 1)?,
            other => return Err(unsupported(format!("BYML node type 0x{other:02x}"))),
        })
    }

    fn container(&self, at: usize, depth: usize) -> Result<Value> {
        if depth > MAX_DEPTH {
            return Err(invalid(format!("containers nested over {MAX_DEPTH} deep")));
        }
        let d = self.data;
        let kind = d.u8_at(at)?;
        let n = u24_at(d, at + 1)?;
        Ok(match kind {
            0xC0 => {
                let types = d.slice_at(at + 4, n)?;
                let values = at + 4 + n.next_multiple_of(4);
                let mut items = Vec::with_capacity(n);
                for (i, &t) in types.iter().enumerate() {
                    items.push(self.node(t, d.u32_at(values + 4 * i)?, depth)?);
                }
                Value::Array(items)
            }
            0xC1 => {
                let mut entries = Vec::with_capacity(n);
                for i in 0..n {
                    let e = at + 4 + 8 * i;
                    let k = u24_at(d, e)?;
                    let key = self
                        .keys
                        .get(k)
                        .ok_or_else(|| invalid(format!("key {k} isn't in the key table")))?;
                    let value = self.node(d.u8_at(e + 3)?, d.u32_at(e + 4)?, depth)?;
                    entries.push(((*key).to_owned(), value));
                }
                Value::Map(entries)
            }
            0x20 | 0x21 => {
                let wide = kind == 0x21;
                let entry = if wide { 12 } else { 8 };
                let types = at + 4 + entry * n;
                let mut entries = Vec::with_capacity(n);
                for i in 0..n {
                    let e = at + 4 + entry * i;
                    let (hash, value) = if wide {
                        (d.u64_at(e)?, d.u32_at(e + 8)?)
                    } else {
                        (d.u32_at(e)? as u64, d.u32_at(e + 4)?)
                    };
                    entries.push((hash, self.node(d.u8_at(types + i)?, value, depth)?));
                }
                Value::HashMap(entries)
            }
            other => {
                return Err(invalid(format!(
                    "no BYML container at 0x{at:x} (type 0x{other:02x})"
                )));
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small BYML v7 document, built by hand: `{Cloth: [{Name: "Default", Path: "Work/X.phcl"}], Count: -3,
    /// Flag: true, Size: 1.5}`.
    fn sample() -> Vec<u8> {
        fn table(strings: &[&str]) -> Vec<u8> {
            let mut t = vec![0xC2];
            t.extend_from_slice(&(strings.len() as u32).to_le_bytes()[..3]);
            let head = 4 + 4 * (strings.len() + 1);
            let mut body = Vec::new();
            for s in strings {
                t.extend_from_slice(&((head + body.len()) as u32).to_le_bytes());
                body.extend_from_slice(s.as_bytes());
                body.push(0);
            }
            t.extend_from_slice(&((head + body.len()) as u32).to_le_bytes());
            t.extend_from_slice(&body);
            while t.len() % 4 != 0 {
                t.push(0);
            }
            t
        }
        let keys = ["Cloth", "Count", "Flag", "Name", "Path", "Size"];
        let strings = ["Default", "Work/X.phcl"];
        let mut data = vec![0u8; 16];
        data[..2].copy_from_slice(b"YB");
        data[2..4].copy_from_slice(&7u16.to_le_bytes());
        let key_at = data.len();
        data.extend(table(&keys));
        let string_at = data.len();
        data.extend(table(&strings));
        // the inner dictionary: Name, Path
        let inner = data.len();
        data.extend([0xC1, 2, 0, 0]);
        data.extend([3, 0, 0, 0xA0, 0, 0, 0, 0]);
        data.extend([4, 0, 0, 0xA0, 1, 0, 0, 0]);
        // the array of one dictionary
        let array = data.len();
        data.extend([0xC0, 1, 0, 0, 0xC1, 0, 0, 0]);
        data.extend((inner as u32).to_le_bytes());
        // the root
        let root = data.len();
        data.extend([0xC1, 4, 0, 0]);
        data.extend([0, 0, 0, 0xC0]);
        data.extend((array as u32).to_le_bytes());
        data.extend([1, 0, 0, 0xD1]);
        data.extend((-3i32).to_le_bytes());
        data.extend([2, 0, 0, 0xD0, 1, 0, 0, 0]);
        data.extend([5, 0, 0, 0xD2]);
        data.extend(1.5f32.to_le_bytes());
        data[4..8].copy_from_slice(&(key_at as u32).to_le_bytes());
        data[8..12].copy_from_slice(&(string_at as u32).to_le_bytes());
        data[12..16].copy_from_slice(&(root as u32).to_le_bytes());
        data
    }

    #[test]
    fn reads_dictionaries_arrays_and_scalars() {
        let root = parse(&sample()).unwrap();
        let cloth = root.get("Cloth").and_then(Value::as_array).unwrap();
        assert_eq!(cloth.len(), 1);
        assert_eq!(cloth[0].str_at(&["Name"]), Some("Default"));
        assert_eq!(cloth[0].str_at(&["Path"]), Some("Work/X.phcl"));
        assert_eq!(root.get("Count"), Some(&Value::Int(-3)));
        assert_eq!(root.get("Flag"), Some(&Value::Bool(true)));
        assert_eq!(root.get("Size"), Some(&Value::Float(1.5)));
        assert_eq!(root.get("Missing"), None);
    }

    #[test]
    fn a_document_without_a_root_is_null() {
        let mut data = sample();
        data[12..16].fill(0);
        assert_eq!(parse(&data).unwrap(), Value::Null);
    }

    #[test]
    fn bad_and_cut_files_are_errors_not_panics() {
        assert!(parse(b"BY\0\x02").is_err());
        assert!(parse(b"XX").is_err());
        let data = sample();
        for len in 0..data.len() {
            let _ = parse(&data[..len]);
        }
        // a container pointing at itself
        let mut looped = sample();
        let root = u32::from_le_bytes(looped[12..16].try_into().unwrap()) as usize;
        looped[root + 8..root + 12].copy_from_slice(&(root as u32).to_le_bytes());
        assert!(parse(&looped).is_err());
    }
}
