//! BNTX texture containers (`Tex/*.bntx.zs`, one texture each in this game).
//!
//! Ported from Syroot.NintenTools.Bntx (`BntxFile.cs`, `Texture/TextureInfo.cs`, `GFX/Enums.cs`).
//!
//! Layout: a 0x20-byte file header ("BNTX"), the platform header ("NX  ", or "Ounc" on the Switch 2: texture
//! count, an array of offsets to BRTI blocks, the BRTD data block, a dictionary of names), BRTI blocks (one per
//! texture: size, format, layout, offsets of each mip level), the string pool (`_STR`, strings prefixed by a u16
//! length), the image data (BRTD) and the relocation table (`_RLT`).

use crate::bytes::{ReadLe, check_magic};
use crate::error::{Error, Result, invalid, unsupported};
use crate::image::Image;
use crate::tegra::{Gob, Surface};

/// A parsed container; texture data borrows from the file's bytes.
#[derive(Debug)]
pub struct Bntx<'a> {
    pub name: String,
    /// The GPU the textures are laid out for ("NX  ": Switch, "Ounc": Switch 2).
    pub gob: Gob,
    pub textures: Vec<Texture<'a>>,
}

/// One texture (BRTI) and its image data.
#[derive(Debug, Clone)]
pub struct Texture<'a> {
    pub name: String,
    pub gob: Gob,
    pub format: Format,
    /// 0 for block-linear (swizzled), 1 for linear.
    pub tile_mode: u16,
    /// Texture dimension (`Dim`: 1 = 1D, 2 = 2D, 3 = 3D).
    pub dim: u8,
    /// View dimension (`SurfaceDim`: 1 = 2D, 3 = cube, 5 = 2D array, ...).
    pub surface_dim: u32,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub array_length: u32,
    pub mip_count: u32,
    /// log2 of the block height in GOBs of mip 0 (block-linear layout).
    pub block_height_log2: u32,
    /// Per channel (r, g, b, a): 0 zero, 1 one, 2 red, 3 green, 4 blue, 5 alpha.
    pub channel_sources: [u8; 4],
    /// Start of each mip level in `data` (for array layer 0).
    pub mip_offsets: Vec<usize>,
    /// All image data: every array layer with its mips.
    pub data: &'a [u8],
}

impl<'a> Bntx<'a> {
    pub fn parse(file: &'a [u8]) -> Result<Self> {
        check_magic(file, 0, "BNTX")?;
        if file.u16_at(0x0C)? != 0xFEFF {
            return Err(unsupported("big-endian BNTX"));
        }
        let name_offset = file.u32_at(0x10)? as usize;
        let name = if name_offset >= 2 {
            length_prefixed(file, name_offset - 2)?.to_owned()
        } else {
            String::new()
        };

        // "NX  " on the Switch, "Ounc" on the Switch 2; the layout that follows is the same.
        let gob = match file.slice_at(0x20, 4)? {
            b"NX  " => Gob::TegraX1,
            b"Ounc" => Gob::Orin,
            other => return Err(unsupported(format!("BNTX platform {other:02x?}"))),
        };
        let count = file.u32_at(0x24)? as usize;
        let array = file.offset_at(0x28)?;
        let mut textures = Vec::with_capacity(count);
        for i in 0..count {
            textures.push(Texture::parse(file, file.offset_at(array + i * 8)?, gob)?);
        }
        Ok(Bntx {
            name,
            gob,
            textures,
        })
    }
}

impl<'a> Texture<'a> {
    fn parse(file: &'a [u8], at: usize, gob: Gob) -> Result<Self> {
        check_magic(file, at, "BRTI")?;
        let tile_mode = file.u16_at(at + 0x12)?;
        let mip_count = u32::from(file.u16_at(at + 0x16)?);
        let format = Format(file.u32_at(at + 0x1C)?);
        let width = file.u32_at(at + 0x24)?;
        let height = file.u32_at(at + 0x28)?;
        let depth = file.u32_at(at + 0x2C)?;
        let array_length = file.u32_at(at + 0x30)?;
        let layout = file.u32_at(at + 0x34)?;
        let image_size = file.u32_at(at + 0x50)? as usize;
        let channels = file.u32_at(at + 0x58)?;
        let surface_dim = file.u32_at(at + 0x5C)?;
        let name = length_prefixed(file, file.offset_at(at + 0x60)?)?.to_owned();
        let mips_at = file.offset_at(at + 0x70)?;

        if mip_count == 0 || array_length == 0 || width == 0 || height == 0 {
            return Err(invalid(format!(
                "texture {name}: empty size, mip or layer count"
            )));
        }
        let mut absolute = Vec::with_capacity(mip_count as usize);
        for i in 0..mip_count as usize {
            absolute.push(file.offset_at(mips_at + i * 8)?);
        }
        let start = absolute[0];
        let data = file.slice_at(start, image_size)?;
        let mip_offsets = absolute
            .iter()
            .map(|&o| {
                o.checked_sub(start)
                    .filter(|&rel| rel < image_size)
                    .ok_or_else(|| invalid(format!("texture {name}: mip offset 0x{o:x}")))
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(Texture {
            name,
            gob,
            format,
            tile_mode,
            dim: file.u8_at(at + 0x11)?,
            surface_dim,
            width,
            height,
            depth,
            array_length,
            mip_count,
            block_height_log2: layout & 7,
            channel_sources: channels.to_le_bytes(),
            mip_offsets,
            data,
        })
    }

    /// Bytes per array layer (all mips of one layer).
    pub fn layer_size(&self) -> usize {
        self.data.len() / self.array_length as usize
    }

    /// Width and height of mip `level` in pixels.
    pub fn level_size(&self, level: u32) -> (u32, u32) {
        ((self.width >> level).max(1), (self.height >> level).max(1))
    }

    /// The block-linear surface of mip `level`.
    pub fn surface(&self, level: u32) -> Result<Surface> {
        let (Some(block), Some(bytes)) = (self.format.block_size(), self.format.bytes_per_block())
        else {
            return Err(unsupported(format!(
                "texture {}: format {}",
                self.name,
                self.format.name()
            )));
        };
        Ok(Surface::mip(
            self.gob,
            self.width,
            self.height,
            level,
            block,
            bytes,
            self.block_height_log2,
        ))
    }

    /// The stored (swizzled) bytes of mip `level` of array layer `layer`.
    pub fn level_bytes(&self, level: u32, layer: u32) -> Result<&'a [u8]> {
        if level >= self.mip_count || layer >= self.array_length {
            return Err(invalid(format!(
                "texture {}: no mip {level} of layer {layer}",
                self.name
            )));
        }
        let start = layer as usize * self.layer_size() + self.mip_offsets[level as usize];
        let len = self.surface(level)?.swizzled_size();
        self.data.get(start..start + len).ok_or_else(|| {
            invalid(format!(
                "texture {}: mip {level} of layer {layer} runs past the data",
                self.name
            ))
        })
    }

    /// Mip `level` of layer `layer` as RGBA, channels as the GPU reads them before the texture's channel
    /// sources apply (missing colour channels 0, missing alpha 255). sRGB data is left as stored.
    pub fn decode(&self, level: u32, layer: u32) -> Result<Image> {
        if self.tile_mode != 0 {
            return Err(unsupported(format!(
                "texture {}: linear tile mode",
                self.name
            )));
        }
        let surface = self.surface(level)?;
        let blocks = surface
            .deswizzle(self.level_bytes(level, layer)?)
            .ok_or_else(|| invalid(format!("texture {}: mip {level} too short", self.name)))?;
        let (w, h) = self.level_size(level);
        let rgba = decode_texels(self.format, &blocks, w as usize, h as usize)
            .map_err(|e| Error::Invalid(format!("texture {}: {e}", self.name)))?;
        Image::new(w, h, rgba)
    }

    /// Applies the texture's channel sources (what the shader reads for r, g, b, a) to a decoded image.
    pub fn apply_channel_sources(&self, image: &mut Image) {
        let sources = self.channel_sources;
        if sources == [2, 3, 4, 5] {
            return;
        }
        for px in image.rgba.as_chunks_mut::<4>().0 {
            let src = [px[0], px[1], px[2], px[3]];
            for (c, &s) in sources.iter().enumerate() {
                px[c] = match s {
                    0 => 0,
                    2..=5 => src[s as usize - 2],
                    _ => 255,
                };
            }
        }
    }
}

/// Decodes texels stored row by row (blocks for compressed formats) to RGBA.
fn decode_texels(
    format: Format,
    data: &[u8],
    w: usize,
    h: usize,
) -> std::result::Result<Vec<u8>, String> {
    use texture2ddecoder as t2d;
    type Decode<'d> = &'d dyn Fn(&mut [u32]) -> std::result::Result<(), &'static str>;

    let encoding = format
        .encoding()
        .ok_or_else(|| format!("format {}", format.name()))?;
    let pixels = w * h;
    let need = |n: usize| {
        data.get(..n)
            .ok_or_else(|| format!("{} bytes of texels, {n} needed", data.len()))
    };
    let signed = format.is_signed();
    if signed && !matches!(encoding, Encoding::Bc4 | Encoding::Bc5) {
        return Err(format!("format {}", format.name()));
    }
    let bgra_words = |decode: Decode| {
        let mut words = vec![0u32; pixels];
        // The decoder panics on block modes it doesn't know (HDR ASTC); report those as errors.
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| decode(&mut words))) {
            Ok(Ok(())) => Ok(words
                .iter()
                .flat_map(|v| {
                    let [b, g, r, a] = v.to_le_bytes();
                    [r, g, b, a]
                })
                .collect()),
            Ok(Err(e)) => Err(e.to_owned()),
            Err(_) => Err(format!(
                "{} block the decoder doesn't handle",
                format.name()
            )),
        }
    };
    match encoding {
        Encoding::R8 => Ok(need(pixels)?.iter().flat_map(|&r| [r, 0, 0, 255]).collect()),
        Encoding::Rg8 => Ok(need(pixels * 2)?
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], 0, 255])
            .collect()),
        Encoding::Rgba8 => Ok(need(pixels * 4)?.to_vec()),
        Encoding::Bgra8 => Ok(need(pixels * 4)?
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[2], p[1], p[0], p[3]])
            .collect()),
        Encoding::Bc4 | Encoding::Bc5 => {
            let channels = if encoding == Encoding::Bc4 { 1 } else { 2 };
            crate::bcn::decode(data, w, h, channels, signed)
                .ok_or_else(|| "BC data too short".to_owned())
        }
        Encoding::Bc1 => bgra_words(&|out| t2d::decode_bc1a(data, w, h, out)),
        Encoding::Bc2 => bgra_words(&|out| t2d::decode_bc2(data, w, h, out)),
        Encoding::Bc3 => bgra_words(&|out| t2d::decode_bc3(data, w, h, out)),
        Encoding::Bc6 => bgra_words(&|out| t2d::decode_bc6(data, w, h, out, false)),
        Encoding::Bc7 => bgra_words(&|out| t2d::decode_bc7(data, w, h, out)),
        Encoding::Astc => {
            let (bw, bh) = format.block_size().expect("ASTC has a block size");
            bgra_words(&|out| t2d::decode_astc(data, w, h, bw as usize, bh as usize, out))
        }
    }
}

/// A string stored as a u16 length followed by the bytes (and a zero).
fn length_prefixed(file: &[u8], at: usize) -> Result<&str> {
    let len = file.u16_at(at)? as usize;
    std::str::from_utf8(file.slice_at(at + 2, len)?)
        .map_err(|_| invalid(format!("string at 0x{at:x} is not UTF-8")))
}

/// A surface format: the high byte is the layout, the low byte the type (1 unorm, 2 snorm, 5 uint/float,
/// 6 sRGB, 0x0a ufloat).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Format(pub u32);

/// How a format's texels are stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    R8,
    Rg8,
    Rgba8,
    Bgra8,
    Bc1,
    Bc2,
    Bc3,
    Bc4,
    Bc5,
    Bc6,
    Bc7,
    Astc,
}

impl Format {
    pub fn layout(self) -> u32 {
        self.0 >> 8
    }

    pub fn is_srgb(self) -> bool {
        self.0 & 0xFF == 0x06
    }

    pub fn is_signed(self) -> bool {
        self.0 & 0xFF == 0x02
    }

    /// The storage of this format, if it is one this code knows.
    pub fn encoding(self) -> Option<Encoding> {
        Some(match self.layout() {
            0x02 => Encoding::R8,
            0x09 => Encoding::Rg8,
            0x0b => Encoding::Rgba8,
            0x0c => Encoding::Bgra8,
            0x1a => Encoding::Bc1,
            0x1b => Encoding::Bc2,
            0x1c => Encoding::Bc3,
            0x1d => Encoding::Bc4,
            0x1e => Encoding::Bc5,
            0x1f => Encoding::Bc6,
            0x20 => Encoding::Bc7,
            0x2d..=0x3a => Encoding::Astc,
            _ => return None,
        })
    }

    /// Width and height of a block in texels (1x1 for uncompressed formats).
    pub fn block_size(self) -> Option<(u32, u32)> {
        Some(match self.encoding()? {
            Encoding::R8 | Encoding::Rg8 | Encoding::Rgba8 | Encoding::Bgra8 => (1, 1),
            Encoding::Astc => ASTC_BLOCKS[(self.layout() - 0x2d) as usize],
            _ => (4, 4),
        })
    }

    /// Bytes per block (per texel for uncompressed formats).
    pub fn bytes_per_block(self) -> Option<u32> {
        Some(match self.encoding()? {
            Encoding::R8 => 1,
            Encoding::Rg8 => 2,
            Encoding::Rgba8 | Encoding::Bgra8 => 4,
            Encoding::Bc1 | Encoding::Bc4 => 8,
            _ => 16,
        })
    }

    /// A readable name, e.g. `BC7_SRGB`.
    pub fn name(self) -> String {
        let base = match self.encoding() {
            Some(Encoding::R8) => "R8".to_owned(),
            Some(Encoding::Rg8) => "R8_G8".to_owned(),
            Some(Encoding::Rgba8) => "R8_G8_B8_A8".to_owned(),
            Some(Encoding::Bgra8) => "B8_G8_R8_A8".to_owned(),
            Some(Encoding::Astc) => {
                let (w, h) = self.block_size().expect("ASTC has a block size");
                format!("ASTC_{w}x{h}")
            }
            Some(e) => format!("{e:?}").to_uppercase(),
            None => format!("0x{:04x}", self.0),
        };
        let kind = match self.0 & 0xFF {
            0x01 => "UNORM",
            0x02 => "SNORM",
            0x05 => "UINT",
            0x06 => "SRGB",
            0x0a => "UFLOAT",
            _ => "?",
        };
        format!("{base}_{kind}")
    }
}

const ASTC_BLOCKS: [(u32, u32); 14] = [
    (4, 4),
    (5, 4),
    (5, 5),
    (6, 5),
    (6, 6),
    (8, 5),
    (8, 6),
    (8, 8),
    (10, 5),
    (10, 6),
    (10, 8),
    (10, 10),
    (12, 10),
    (12, 12),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::tiny_bntx;

    #[test]
    fn decodes_synthetic_textures_for_both_gpus() {
        for gob in [Gob::TegraX1, Gob::Orin] {
            let file = tiny_bntx("Tex_Test_Alb", 37, 21, gob);
            let bntx = Bntx::parse(&file).unwrap();
            assert_eq!(bntx.gob, gob);
            let t = &bntx.textures[0];
            assert_eq!(
                (t.name.as_str(), t.width, t.height),
                ("Tex_Test_Alb", 37, 21)
            );
            assert_eq!(t.format.name(), "R8_G8_B8_A8_UNORM");
            let image = t.decode(0, 0).unwrap();
            for (y, x) in [(0, 0), (5, 36), (20, 17), (13, 9)] {
                let at = (y * 37 + x) * 4;
                assert_eq!(
                    &image.rgba[at..at + 4],
                    &[x as u8, y as u8, (x ^ y) as u8, 255]
                );
            }
        }
    }

    #[test]
    fn layouts_differ_between_gpus() {
        // Read with the other GPU's layout, the same bytes give a different picture.
        let mut file = tiny_bntx("T", 64, 16, Gob::Orin);
        let orin = Bntx::parse(&file).unwrap().textures[0]
            .decode(0, 0)
            .unwrap();
        file[0x20..0x24].copy_from_slice(b"NX  ");
        let x1 = Bntx::parse(&file).unwrap().textures[0]
            .decode(0, 0)
            .unwrap();
        assert_ne!(orin, x1);
    }

    #[test]
    fn format_table() {
        assert_eq!(Format(0x2006).name(), "BC7_SRGB");
        assert_eq!(Format(0x2f01).block_size(), Some((5, 5)));
        assert_eq!(Format(0x3a06).name(), "ASTC_12x12_SRGB");
        assert_eq!(Format(0x1d01).bytes_per_block(), Some(8));
        assert!(Format(0x1505).encoding().is_none());
    }
}
