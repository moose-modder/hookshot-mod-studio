//! SARC archives (`.pack`, and the dictionary pack `Pack/ZsDic.pack.zs` once decompressed).
//!
//! Layout: a 0x14-byte header ("SARC"), the SFAT block (one 16-byte node per file: name hash, name offset, data
//! range), the SFNT block (zero-terminated names, each padded to 4 bytes), then the file data.

use crate::bytes::{ReadLe, check_magic};
use crate::error::{Result, invalid, unsupported};

const HEADER_SIZE: usize = 0x14;
const SFAT_HEADER_SIZE: usize = 0x0C;
const SFAT_NODE_SIZE: usize = 0x10;
const SFNT_HEADER_SIZE: usize = 0x08;
const HASH_KEY: u32 = 0x65;

/// One file in an archive.
#[derive(Debug, Clone, Copy)]
pub struct SarcFile<'a> {
    /// The file's path in the archive; archives may leave names out and keep only the hash.
    pub name: Option<&'a str>,
    pub hash: u32,
    pub data: &'a [u8],
}

/// A parsed archive; files borrow from the archive's bytes.
#[derive(Debug)]
pub struct Sarc<'a> {
    files: Vec<SarcFile<'a>>,
}

impl<'a> Sarc<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        check_magic(data, 0, "SARC")?;
        if data.u16_at(0x04)? as usize != HEADER_SIZE {
            return Err(invalid("SARC header size"));
        }
        if data.u16_at(0x06)? != 0xFEFF {
            return Err(unsupported("big-endian SARC"));
        }
        let data_start = data.u32_at(0x0C)? as usize;

        let sfat = HEADER_SIZE;
        check_magic(data, sfat, "SFAT")?;
        let count = data.u16_at(sfat + 0x06)? as usize;
        let key = data.u32_at(sfat + 0x08)?;
        if key != HASH_KEY {
            return Err(unsupported(format!("SARC hash key 0x{key:x}")));
        }
        let nodes = sfat + SFAT_HEADER_SIZE;
        let sfnt = nodes + count * SFAT_NODE_SIZE;
        check_magic(data, sfnt, "SFNT")?;
        let names = sfnt + SFNT_HEADER_SIZE;

        let mut files = Vec::with_capacity(count);
        for i in 0..count {
            let node = nodes + i * SFAT_NODE_SIZE;
            let hash = data.u32_at(node)?;
            let attributes = data.u32_at(node + 4)?;
            let start = data.u32_at(node + 8)? as usize;
            let end = data.u32_at(node + 12)? as usize;
            if end < start {
                return Err(invalid(format!(
                    "SARC file {i}: data ends before it starts"
                )));
            }
            let name = if attributes >> 24 != 0 {
                Some(data.cstr_at(names + (attributes & 0xFFFF) as usize * 4)?)
            } else {
                None
            };
            let file_data = data.slice_at(data_start + start, end - start)?;
            files.push(SarcFile {
                name,
                hash,
                data: file_data,
            });
        }
        Ok(Sarc { files })
    }

    pub fn files(&self) -> &[SarcFile<'a>] {
        &self.files
    }

    /// The data of the file called `name`.
    pub fn get(&self, name: &str) -> Option<&'a [u8]> {
        let hash = name_hash(name);
        self.files
            .iter()
            .find(|f| f.hash == hash && f.name.is_none_or(|n| n == name))
            .map(|f| f.data)
    }
}

/// The hash SFAT nodes are sorted by.
pub fn name_hash(name: &str) -> u32 {
    // The game hashes the bytes as signed chars; for ASCII names that's the same as unsigned.
    name.bytes().fold(0u32, |h, b| {
        h.wrapping_mul(HASH_KEY).wrapping_add(b as i8 as i32 as u32)
    })
}

/// Builds an archive with each file's data aligned to `alignment` bytes (a power of two).
pub fn write(files: &[(&str, &[u8])], alignment: usize) -> Vec<u8> {
    assert!(alignment.is_power_of_two());
    let mut sorted: Vec<(u32, &str, &[u8])> =
        files.iter().map(|&(n, d)| (name_hash(n), n, d)).collect();
    sorted.sort_by_key(|&(h, n, _)| (h, n));

    let mut names = Vec::new();
    let mut name_offsets = Vec::new();
    for &(_, name, _) in &sorted {
        name_offsets.push(names.len() / 4);
        names.extend_from_slice(name.as_bytes());
        names.push(0);
        names.resize(names.len().next_multiple_of(4), 0);
    }

    let nodes_end = HEADER_SIZE + SFAT_HEADER_SIZE + sorted.len() * SFAT_NODE_SIZE;
    let data_start = (nodes_end + SFNT_HEADER_SIZE + names.len()).next_multiple_of(alignment);

    let mut body = Vec::new();
    let mut ranges = Vec::new();
    for &(_, _, data) in &sorted {
        body.resize(body.len().next_multiple_of(alignment), 0);
        ranges.push((body.len(), body.len() + data.len()));
        body.extend_from_slice(data);
    }

    let mut out = Vec::with_capacity(data_start + body.len());
    out.extend_from_slice(b"SARC");
    out.extend_from_slice(&(HEADER_SIZE as u16).to_le_bytes());
    out.extend_from_slice(&0xFEFFu16.to_le_bytes());
    out.extend_from_slice(&((data_start + body.len()) as u32).to_le_bytes());
    out.extend_from_slice(&(data_start as u32).to_le_bytes());
    out.extend_from_slice(&0x0100u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());

    out.extend_from_slice(b"SFAT");
    out.extend_from_slice(&(SFAT_HEADER_SIZE as u16).to_le_bytes());
    out.extend_from_slice(&(sorted.len() as u16).to_le_bytes());
    out.extend_from_slice(&HASH_KEY.to_le_bytes());
    for (i, &(hash, _, _)) in sorted.iter().enumerate() {
        out.extend_from_slice(&hash.to_le_bytes());
        out.extend_from_slice(&(0x0100_0000 | name_offsets[i] as u32).to_le_bytes());
        out.extend_from_slice(&(ranges[i].0 as u32).to_le_bytes());
        out.extend_from_slice(&(ranges[i].1 as u32).to_le_bytes());
    }

    out.extend_from_slice(b"SFNT");
    out.extend_from_slice(&(SFNT_HEADER_SIZE as u16).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&names);
    out.resize(data_start, 0);
    out.extend_from_slice(&body);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_then_parse() {
        let a = b"first file".as_slice();
        let b = vec![7u8; 300];
        let packed = write(&[("dir/b.bin", &b), ("a.txt", a), ("empty", &[])], 0x80);
        let sarc = Sarc::parse(&packed).unwrap();
        assert_eq!(sarc.files().len(), 3);
        assert_eq!(sarc.get("a.txt"), Some(a));
        assert_eq!(sarc.get("dir/b.bin"), Some(b.as_slice()));
        assert_eq!(sarc.get("empty"), Some(&[][..]));
        assert_eq!(sarc.get("missing"), None);
        // Nodes are sorted by hash, and every file's data is aligned.
        let hashes: Vec<u32> = sarc.files().iter().map(|f| f.hash).collect();
        assert!(hashes.is_sorted());
        for f in sarc.files() {
            assert_eq!(
                (f.data.as_ptr() as usize - packed.as_ptr() as usize) % 0x80,
                0
            );
        }
    }

    #[test]
    fn known_hash() {
        // h = h * 0x65 + c over "ab": 'a' * 0x65 + 'b'
        assert_eq!(name_hash("ab"), 0x61 * 0x65 + 0x62);
    }

    #[test]
    fn rejects_damaged_archives() {
        let packed = write(&[("a", b"xyz")], 4);
        assert!(Sarc::parse(&packed[..0x20]).is_err());
        let mut bad = packed.clone();
        bad[0] = b'X';
        assert!(Sarc::parse(&bad).is_err());
        let mut bad = packed;
        let node = HEADER_SIZE + SFAT_HEADER_SIZE;
        bad[node + 12..node + 16].copy_from_slice(&0x1000u32.to_le_bytes());
        assert!(Sarc::parse(&bad).is_err());
    }
}
