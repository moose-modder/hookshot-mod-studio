//! The resource size table (`System/Resource/ResourceSizeTable.Product.<version>.<platform>.rsizetable.zs`):
//! how much memory the game sets aside for each file it loads. Layout (as the game's own table has it):
//!
//! - `RESTBL`, version (u32, 1), the length of a name entry's path (u32, 160)
//! - the number of hash entries and of name entries (u32 each)
//! - hash entries: CRC-32 of the file's path (without `.zs`) and its size, sorted by hash
//! - name entries: the path (zero-padded to 160 bytes) and its size, sorted by path; for paths whose hash another
//!   path has too
//!
//! A file's path is as the romfs spells it, `Model/Npc_X.Npc_X.bfres` for `Model/Npc_X.Npc_X.bfres.zs`.

use std::collections::BTreeMap;

use crate::error::{Result, invalid};

pub const MAGIC: &[u8; 6] = b"RESTBL";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Restbl {
    pub version: u32,
    /// The length of a name entry's path field.
    pub name_len: usize,
    /// CRC-32 of the path -> size.
    pub hashes: BTreeMap<u32, u32>,
    /// Path -> size, for paths whose hash isn't theirs alone.
    pub names: BTreeMap<String, u32>,
}

/// CRC-32 (IEEE, as zlib's) of a path's bytes: the hash entries' key.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

impl Restbl {
    /// Reads a decompressed table.
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < 22 || &data[..6] != MAGIC {
            return Err(invalid("not a resource size table (no RESTBL magic)"));
        }
        let u32_at = |at: usize| -> Result<u32> {
            data.get(at..at + 4)
                .map(|b| u32::from_le_bytes(b.try_into().expect("4 bytes")))
                .ok_or_else(|| invalid(format!("resource size table cut short at 0x{at:x}")))
        };
        let version = u32_at(6)?;
        let name_len = u32_at(10)? as usize;
        let (hash_count, name_count) = (u32_at(14)? as usize, u32_at(18)? as usize);
        let mut hashes = BTreeMap::new();
        for i in 0..hash_count {
            hashes.insert(u32_at(22 + i * 8)?, u32_at(26 + i * 8)?);
        }
        let mut names = BTreeMap::new();
        let names_at = 22 + hash_count * 8;
        for i in 0..name_count {
            let at = names_at + i * (name_len + 4);
            let raw = data
                .get(at..at + name_len)
                .ok_or_else(|| invalid("resource size table cut short in its names"))?;
            let end = raw.iter().position(|&b| b == 0).unwrap_or(name_len);
            let name = String::from_utf8(raw[..end].to_vec())
                .map_err(|_| invalid("a resource size table name isn't UTF-8"))?;
            names.insert(name, u32_at(at + name_len)?);
        }
        if names_at + name_count * (name_len + 4) != data.len() {
            return Err(invalid("resource size table: data after the entries"));
        }
        Ok(Restbl {
            version,
            name_len,
            hashes,
            names,
        })
    }

    /// The table's bytes (uncompressed).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out =
            Vec::with_capacity(22 + self.hashes.len() * 8 + self.names.len() * (self.name_len + 4));
        out.extend_from_slice(MAGIC);
        for v in [
            self.version,
            self.name_len as u32,
            self.hashes.len() as u32,
            self.names.len() as u32,
        ] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for (hash, size) in &self.hashes {
            out.extend_from_slice(&hash.to_le_bytes());
            out.extend_from_slice(&size.to_le_bytes());
        }
        for (name, size) in &self.names {
            let mut field = name.as_bytes().to_vec();
            field.resize(self.name_len, 0);
            out.extend_from_slice(&field);
            out.extend_from_slice(&size.to_le_bytes());
        }
        out
    }

    /// The size for a path (by name if it has a name entry, else by its hash).
    pub fn get(&self, path: &str) -> Option<u32> {
        self.names
            .get(path)
            .or_else(|| self.hashes.get(&crc32(path.as_bytes())))
            .copied()
    }

    /// Sets the size for a path that has an entry (its name entry, else its hash entry), or adds one: a hash
    /// entry, or a name entry when another path already has that hash (`known` says whether `path` is a file
    /// the table already counts).
    pub fn set(&mut self, path: &str, size: u32, known: bool) {
        if let Some(s) = self.names.get_mut(path) {
            *s = size;
            return;
        }
        let hash = crc32(path.as_bytes());
        if !known && self.hashes.contains_key(&hash) {
            self.names.insert(path.to_owned(), size);
        } else {
            self.hashes.insert(hash, size);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_is_zlibs() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn round_trip_and_lookups() {
        let mut t = Restbl {
            version: 1,
            name_len: 160,
            hashes: BTreeMap::new(),
            names: BTreeMap::new(),
        };
        t.set("Model/A.A.bfres", 100, false);
        t.set("Tex/B.bntx", 200, false);
        t.names.insert("Tex/Clash.bntx".into(), 300);
        let bytes = t.to_bytes();
        assert_eq!(&bytes[..6], b"RESTBL");
        assert_eq!(bytes.len(), 22 + 2 * 8 + 164);
        let back = Restbl::parse(&bytes).unwrap();
        assert_eq!(back, t);
        assert_eq!(back.get("Model/A.A.bfres"), Some(100));
        assert_eq!(back.get("Tex/Clash.bntx"), Some(300));
        assert_eq!(back.get("Tex/Missing.bntx"), None);

        // a new path whose hash another path has goes in by name; a known one updates its entry
        let mut t = back;
        let taken = *t.hashes.keys().next().unwrap();
        let other = t.hashes[&taken];
        t.hashes.insert(crc32(b"Tex/New.bntx"), 1);
        t.set("Tex/New.bntx", 5, false);
        assert_eq!(t.names.get("Tex/New.bntx"), Some(&5));
        t.set("Tex/Clash.bntx", 301, true);
        assert_eq!(t.get("Tex/Clash.bntx"), Some(301));
        assert_eq!(t.hashes[&taken], other);
        assert!(Restbl::parse(&bytes[..30]).is_err());
    }
}
