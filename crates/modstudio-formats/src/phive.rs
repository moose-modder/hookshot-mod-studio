//! Phive files (the cloth files `Phive/Cloth/*.bphcl`): a 48-byte header, a Havok tagfile (`TAG0`, read with the
//! `havok-formats` crate) and after it a parameter archive (AAMP) that names the simulated meshes and colliders.
//!
//! Header, little-endian: `Phive\0`, `01 00`, the byte-order mark `FF FE`, `03 03`, then at 0x0C five words: the
//! tagfile's offset, the archive's offset, the file's size, the tagfile's size and the archive's size. Of the game's
//! 171 cloth files, 121 give the tagfile's size rounded up to 16 (zeros follow the tagfile) and 50 the exact size;
//! the tagfile's own section header always has the exact one, which is what [`Phive::tagfile`] spans.

use crate::bytes::{ReadLe, check_magic};
use crate::error::{Result, invalid, unsupported};

/// The header's size, and where the tagfile starts in every game file.
pub const HEADER_SIZE: usize = 0x30;

/// A Phive file's two parts, borrowed from its bytes.
#[derive(Debug, Clone, Copy)]
pub struct Phive<'a> {
    /// The Havok tagfile: the `TAG0` section, as long as its own section header says.
    pub tagfile: &'a [u8],
    /// The parameter archive after it (AAMP), as long as the header says.
    pub params: &'a [u8],
}

impl<'a> Phive<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        check_magic(data, 0, "Phive\0")?;
        if data.u16_at(0x08)? != 0xFEFF {
            return Err(unsupported("big-endian Phive file"));
        }
        let tag_offset = data.u32_at(0x0C)? as usize;
        let params_offset = data.u32_at(0x10)? as usize;
        let file_size = data.u32_at(0x14)? as usize;
        let tag_size = data.u32_at(0x18)? as usize;
        let params_size = data.u32_at(0x1C)? as usize;
        if file_size != data.len() {
            return Err(invalid(format!(
                "Phive header gives the file's size as {file_size}, the file has {}",
                data.len()
            )));
        }
        if tag_offset < HEADER_SIZE || tag_offset + tag_size > params_offset {
            return Err(invalid(
                "Phive tagfile overlaps the header or the parameters",
            ));
        }
        check_magic(data, tag_offset + 4, "TAG0")?;
        // The section header: a big-endian word, two flag bits and the section's size (header included).
        let word = data.slice_at(tag_offset, 4)?;
        let section =
            (u32::from_be_bytes([word[0], word[1], word[2], word[3]]) & 0x3FFF_FFFF) as usize;
        if section > tag_size {
            return Err(invalid(format!(
                "Phive tagfile section of {section} bytes is longer than the header's {tag_size}"
            )));
        }
        Ok(Phive {
            tagfile: data.slice_at(tag_offset, section)?,
            params: data.slice_at(params_offset, params_size)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Phive file around a tagfile of `tag` bytes (a TAG0 section header and zeros) and `params`, the tagfile's
    /// size in the header rounded up to 16 when `rounded`.
    fn file(tag: usize, params: &[u8], rounded: bool) -> Vec<u8> {
        let mut out = b"Phive\0\x01\x00\xff\xfe\x03\x03".to_vec();
        let tag_size = if rounded { tag.div_ceil(16) * 16 } else { tag };
        let params_offset = (HEADER_SIZE + tag).div_ceil(16) * 16;
        let size = params_offset + params.len();
        for w in [HEADER_SIZE, params_offset, size, tag_size, params.len()] {
            out.extend((w as u32).to_le_bytes());
        }
        out.resize(HEADER_SIZE, 0);
        out.extend(((tag as u32) | 0x4000_0000).to_be_bytes());
        out.extend(b"TAG0");
        out.resize(params_offset, 0);
        out.extend(params);
        out
    }

    #[test]
    fn splits_tagfile_and_parameters() {
        for rounded in [false, true] {
            let data = file(52, b"AAMP....", rounded);
            let p = Phive::parse(&data).unwrap();
            assert_eq!(p.tagfile.len(), 52);
            assert_eq!(&p.tagfile[4..8], b"TAG0");
            assert_eq!(p.params, b"AAMP....");
        }
    }

    #[test]
    fn refuses_inconsistent_headers() {
        let mut data = file(52, b"AAMP....", false);
        data.push(0);
        assert!(Phive::parse(&data).is_err(), "file size");
        let mut data = file(52, b"AAMP....", false);
        data[0x18] = 40;
        assert!(
            Phive::parse(&data).is_err(),
            "tagfile section longer than the header says"
        );
        assert!(Phive::parse(b"Phive").is_err());
    }
}
