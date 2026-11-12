//! `.zs` files: single zstd frames, most compressed with one of the dictionaries the game keeps in
//! `romfs/Pack/ZsDic.pack.zs` (itself a zstd frame without a dictionary, holding a SARC of `*.zsdic` files).

use std::io::Read;

use ruzstd::decoding::errors::FrameDecoderError;
use ruzstd::decoding::{Dictionary, FrameDecoder, StreamingDecoder};

use crate::bytes::ReadLe;
use crate::error::{Error, Result, invalid};
use crate::sarc::Sarc;

const FRAME_MAGIC: u32 = 0xFD2F_B528;

/// What a frame header says about the frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameInfo {
    pub dictionary_id: Option<u32>,
    pub content_size: Option<u64>,
}

/// Reads the header of the zstd frame at the start of `data`.
pub fn frame_info(data: &[u8]) -> Result<FrameInfo> {
    let magic = data.u32_at(0)?;
    if magic != FRAME_MAGIC {
        return Err(Error::BadMagic {
            expected: "zstd",
            found: data.slice_at(0, 4)?.to_vec(),
        });
    }
    let descriptor = data.u8_at(4)?;
    let fcs_flag = descriptor >> 6;
    let single_segment = descriptor & 0x20 != 0;
    let dict_flag = descriptor & 0x03;

    let mut pos = 5 + usize::from(!single_segment);
    let dict_len = [0, 1, 2, 4][dict_flag as usize];
    let dictionary_id = match dict_len {
        0 => None,
        n => {
            let mut b = [0u8; 4];
            b[..n].copy_from_slice(data.slice_at(pos, n)?);
            Some(u32::from_le_bytes(b)).filter(|&id| id != 0)
        }
    };
    pos += dict_len;
    let content_size = match (fcs_flag, single_segment) {
        (0, false) => None,
        (0, true) => Some(u64::from(data.u8_at(pos)?)),
        (1, _) => Some(u64::from(data.u16_at(pos)?) + 256),
        (2, _) => Some(u64::from(data.u32_at(pos)?)),
        _ => Some(data.u64_at(pos)?),
    };
    Ok(FrameInfo {
        dictionary_id,
        content_size,
    })
}

/// The game's zstd dictionaries, as raw `.zsdic` files.
#[derive(Debug, Clone, Default)]
pub struct Dictionaries {
    raw: Vec<(String, Vec<u8>)>,
}

impl Dictionaries {
    /// From the contents of `romfs/Pack/ZsDic.pack.zs`.
    pub fn from_pack(zs_dic_pack: &[u8]) -> Result<Self> {
        let sarc_bytes = Decoder::plain().decompress(zs_dic_pack)?;
        let sarc = Sarc::parse(&sarc_bytes)?;
        let mut raw = Vec::new();
        for file in sarc.files() {
            if let Some(name) = file.name.filter(|n| n.ends_with(".zsdic")) {
                raw.push((name.to_owned(), file.data.to_vec()));
            }
        }
        if raw.is_empty() {
            return Err(invalid("ZsDic.pack holds no .zsdic files"));
        }
        Ok(Dictionaries { raw })
    }

    /// The dictionaries' names (`zs.zsdic`, `pack.zsdic`, ...).
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.raw.iter().map(|(n, _)| n.as_str())
    }

    /// A decoder that knows all of them. Each thread needs its own.
    pub fn decoder(&self) -> Result<Decoder> {
        let mut decoder = Decoder::plain();
        for (name, raw) in &self.raw {
            let dict = Dictionary::decode_dict(raw)
                .map_err(|e| Error::Zstd(format!("dictionary {name}: {e}")))?;
            decoder.inner.add_dict(dict).map_err(zstd_error)?;
        }
        Ok(decoder)
    }
}

/// Decompresses `.zs` files; reusable across files.
pub struct Decoder {
    inner: FrameDecoder,
}

impl Decoder {
    /// A decoder without dictionaries (for frames that don't need one).
    pub fn plain() -> Self {
        Decoder {
            inner: FrameDecoder::new(),
        }
    }

    pub fn decompress(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        let info = frame_info(data)?;
        let mut out = Vec::with_capacity(info.content_size.unwrap_or(0).min(1 << 30) as usize);
        let mut stream =
            StreamingDecoder::new_with_decoder(data, &mut self.inner).map_err(zstd_error)?;
        stream
            .read_to_end(&mut out)
            .map_err(|e| Error::Zstd(e.to_string()))?;
        if let Some(size) = info.content_size
            && out.len() as u64 != size
        {
            return Err(Error::Zstd(format!(
                "frame says {size} bytes, decoded {}",
                out.len()
            )));
        }
        Ok(out)
    }
}

/// A zstd frame of `data` that the game loads: without a dictionary (the game reads those too; Switch Toolbox's
/// mods are written so), stating its decompressed size and without a content checksum, as every file the game
/// ships does. The size is required: the game never loads a file whose frame leaves it out (seen in game: the
/// model was opened, its textures never asked for).
pub fn compress(data: &[u8]) -> Vec<u8> {
    let frame =
        ruzstd::encoding::compress_to_vec(data, ruzstd::encoding::CompressionLevel::Fastest);
    with_content_size(&frame, data.len() as u64)
}

/// The frame's header rewritten to state `size` and its content checksum dropped. ruzstd's encoder writes neither
/// a content size nor a single-segment frame, only a window descriptor; the blocks stay as they are.
fn with_content_size(frame: &[u8], size: u64) -> Vec<u8> {
    let descriptor = frame[4];
    assert_eq!(
        descriptor & 0b1110_0011,
        0,
        "ruzstd frame header changed: descriptor {descriptor:#04x}"
    );
    let checksum = if descriptor & 0b100 != 0 { 4 } else { 0 };
    let window = frame[5];
    let blocks = &frame[6..frame.len() - checksum];
    let mut out = Vec::with_capacity(frame.len() + 8);
    out.extend_from_slice(&FRAME_MAGIC.to_le_bytes());
    match u32::try_from(size) {
        Ok(s) => {
            out.extend_from_slice(&[2 << 6, window]);
            out.extend_from_slice(&s.to_le_bytes());
        }
        Err(_) => {
            out.extend_from_slice(&[3 << 6, window]);
            out.extend_from_slice(&size.to_le_bytes());
        }
    }
    out.extend_from_slice(blocks);
    out
}

fn zstd_error(e: FrameDecoderError) -> Error {
    match e {
        FrameDecoderError::DictNotProvided { dict_id } => Error::DictionaryMissing(dict_id),
        e => Error::Zstd(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ruzstd::encoding::{CompressionLevel, compress_to_vec};

    #[test]
    fn round_trip_without_dictionary() {
        let data: Vec<u8> = (0..100_000u32)
            .flat_map(|i| (i % 251).to_le_bytes())
            .collect();
        let packed = compress_to_vec(data.as_slice(), CompressionLevel::Fastest);
        let info = frame_info(&packed).unwrap();
        assert_eq!(info.dictionary_id, None);
        let mut d = Decoder::plain();
        assert_eq!(d.decompress(&packed).unwrap(), data);
        // The decoder can be reused.
        assert_eq!(d.decompress(&packed).unwrap(), data);
    }

    #[test]
    fn compressed_frames_state_their_size() {
        for len in [0usize, 1, 255, 256, 70_000, 1_000_003] {
            let data: Vec<u8> = (0..len).map(|i| (i * 7 % 253) as u8).collect();
            let packed = compress(&data);
            // As the game's own files: content size given, no dictionary, no checksum (descriptor bit 2).
            assert_eq!(
                frame_info(&packed).unwrap(),
                FrameInfo {
                    dictionary_id: None,
                    content_size: Some(len as u64)
                },
                "{len} bytes"
            );
            assert_eq!(packed[4] & 0b100, 0, "{len} bytes: checksum flag");
            assert_eq!(
                Decoder::plain().decompress(&packed).unwrap(),
                data,
                "{len} bytes"
            );
        }
    }

    /// A frame by hand: single segment, a 4-byte dictionary ID, a 1-byte content size, one raw block.
    fn frame_with_dict(dict_id: u32, content: &[u8]) -> Vec<u8> {
        let mut f = FRAME_MAGIC.to_le_bytes().to_vec();
        f.push(0x20 | 0x03);
        f.extend_from_slice(&dict_id.to_le_bytes());
        f.push(content.len() as u8);
        let block_header = 1 | ((content.len() as u32) << 3); // last block, raw
        f.extend_from_slice(&block_header.to_le_bytes()[..3]);
        f.extend_from_slice(content);
        f
    }

    #[test]
    fn frame_header_fields() {
        let f = frame_with_dict(0x1234_5678, b"hello");
        assert_eq!(
            frame_info(&f).unwrap(),
            FrameInfo {
                dictionary_id: Some(0x1234_5678),
                content_size: Some(5)
            }
        );
        assert!(frame_info(b"SARC....").is_err());
        assert!(frame_info(&f[..6]).is_err());
    }

    #[test]
    fn missing_dictionary_is_reported() {
        let f = frame_with_dict(0xCAFE, b"hello");
        assert!(matches!(
            Decoder::plain().decompress(&f),
            Err(Error::DictionaryMissing(0xCAFE))
        ));
    }

    #[test]
    fn dictionary_pack_needs_zsdic_files() {
        let sarc = crate::sarc::write(&[("readme.txt", b"no dictionaries")], 4);
        let packed = compress_to_vec(sarc.as_slice(), CompressionLevel::Fastest);
        assert!(matches!(
            Dictionaries::from_pack(&packed),
            Err(Error::Invalid(_))
        ));
    }
}
