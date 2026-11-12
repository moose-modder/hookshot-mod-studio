//! Writing a BNTX file: the original file with a new image for its texture.
//!
//! The header, the texture's BRTI block, names and the rest stay where they are; fields are patched (format, size,
//! channel sources, layout, image size, mip pointers). The image data (BRTD) is written again: every mip level of
//! the new image, encoded and laid out in the file's GPU's block-linear layout with a block height of one GOB (as
//! two thirds of the game's textures are; the others use the usual rule and the game reads both). The relocation
//! table follows on the next 4 KiB, as in the game's files, and is moved there.

use crate::bc_encode;
use crate::bntx::{Bntx, Encoding, Format};
use crate::bytes::ReadLe;
use crate::error::{Result, invalid, unsupported};
use crate::image::Image;
use crate::math_srgb::{linear_to_srgb8, srgb8_to_linear};
use crate::tegra::Surface;

const RLT_ALIGN: usize = 0x1000;

/// Formats this writer can encode.
pub fn can_encode(format: Format) -> bool {
    matches!(
        format.encoding(),
        Some(
            Encoding::Bc1
                | Encoding::Bc3
                | Encoding::Bc4
                | Encoding::Bc5
                | Encoding::Bc7
                | Encoding::Rgba8
        )
    ) && !format.is_signed()
}

/// Rows of texels (blocks) of one image in `format`.
fn encode(format: Format, img: &Image) -> Result<Vec<u8>> {
    let (w, h, px) = (img.width as usize, img.height as usize, &img.rgba);
    Ok(match format.encoding() {
        Some(Encoding::Bc1) => bc_encode::encode_bc1(px, w, h),
        Some(Encoding::Bc3) => bc_encode::encode_bc3(px, w, h),
        Some(Encoding::Bc4) => bc_encode::encode_bc4(px, w, h),
        Some(Encoding::Bc5) => bc_encode::encode_bc5(px, w, h),
        Some(Encoding::Bc7) => bc_encode::encode_bc7(px, w, h),
        Some(Encoding::Rgba8) => px.clone(),
        _ => return Err(unsupported(format!("encoding {}", format.name()))),
    })
}

/// The next mip level: 2x2 boxes averaged (in linear light for sRGB colour).
pub fn half_size(img: &Image, srgb: bool) -> Image {
    let (w, h) = (img.width as usize, img.height as usize);
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    let mut out = vec![0u8; nw * nh * 4];
    for y in 0..nh {
        for x in 0..nw {
            for c in 0..4 {
                let mut sum = 0f32;
                let mut n = 0f32;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (sx, sy) = ((x * 2 + dx).min(w - 1), (y * 2 + dy).min(h - 1));
                    let v = img.rgba[(sy * w + sx) * 4 + c];
                    sum += if srgb && c < 3 {
                        srgb8_to_linear(v)
                    } else {
                        f32::from(v) / 255.0
                    };
                    n += 1.0;
                }
                let avg = sum / n;
                out[(y * nw + x) * 4 + c] = if srgb && c < 3 {
                    linear_to_srgb8(avg)
                } else {
                    (avg * 255.0).round() as u8
                };
            }
        }
    }
    Image {
        width: nw as u32,
        height: nh as u32,
        rgba: out,
    }
}

/// `file` (one 2D texture) with `image` in `format`, read through `channel_sources`. The mip count stays the
/// texture's (fewer if the new size has fewer levels).
pub fn write(
    file: &[u8],
    image: &Image,
    format: Format,
    channel_sources: [u8; 4],
) -> Result<Vec<u8>> {
    if !can_encode(format) {
        return Err(unsupported(format!("writing {}", format.name())));
    }
    let bntx = Bntx::parse(file)?;
    let [texture] = bntx.textures.as_slice() else {
        return Err(unsupported(format!(
            "{} textures in one file",
            bntx.textures.len()
        )));
    };
    if texture.array_length != 1 || texture.depth != 1 || texture.dim != 2 {
        return Err(unsupported(format!(
            "texture {}: arrays, cubes and 3D textures",
            texture.name
        )));
    }
    let brti = d_u64(file, d_u64(file, 0x28)? as usize)? as usize;
    let brtd = d_u64(file, 0x30)? as usize;
    let data_start = brtd + 0x10;
    let mips_at = d_u64(file, brti + 0x70)? as usize;
    if d_u64(file, mips_at)? as usize != data_start {
        return Err(unsupported("image data not right after the BRTD header"));
    }
    let rlt = file.u32_at(0x18)? as usize;
    crate::bytes::check_magic(file, rlt, "_RLT")?;

    let full_chain = 32 - image.width.max(image.height).leading_zeros();
    let levels = texture.mip_count.min(full_chain);
    let block = format.block_size().expect("encodable");
    let bytes = format.bytes_per_block().expect("encodable");
    let mut data = Vec::new();
    let mut offsets = Vec::new();
    let mut level_image = image.clone();
    for level in 0..levels {
        if level > 0 {
            level_image = half_size(&level_image, format.is_srgb());
        }
        let surface = Surface::mip(
            texture.gob,
            image.width,
            image.height,
            level,
            block,
            bytes,
            0,
        );
        let texels = encode(format, &level_image)?;
        let swizzled = surface
            .swizzle(&texels)
            .ok_or_else(|| invalid("encoded level too short"))?;
        offsets.push(data_start + data.len());
        data.extend_from_slice(&swizzled);
    }
    let new_rlt = (data_start + data.len()).next_multiple_of(RLT_ALIGN);

    let mut out = file[..data_start].to_vec();
    out.extend_from_slice(&data);
    out.resize(new_rlt, 0);
    out.extend_from_slice(&file[rlt..]);

    let put32 =
        |out: &mut Vec<u8>, at: usize, v: u32| out[at..at + 4].copy_from_slice(&v.to_le_bytes());
    let put64 =
        |out: &mut Vec<u8>, at: usize, v: u64| out[at..at + 8].copy_from_slice(&v.to_le_bytes());
    // The texture.
    put32(&mut out, brti + 0x1C, format.0);
    put32(&mut out, brti + 0x24, image.width);
    put32(&mut out, brti + 0x28, image.height);
    out[brti + 0x16..brti + 0x18].copy_from_slice(&(levels as u16).to_le_bytes());
    let layout = file.u32_at(brti + 0x34)?;
    put32(&mut out, brti + 0x34, layout & !7);
    put32(&mut out, brti + 0x50, data.len() as u32);
    out[brti + 0x58..brti + 0x5C].copy_from_slice(&channel_sources);
    for (i, &o) in offsets.iter().enumerate() {
        put64(&mut out, mips_at + i * 8, o as u64);
    }
    // Pointer slots of levels no longer there point at the last level (the relocation table still lists them).
    for i in offsets.len()..texture.mip_count as usize {
        put64(
            &mut out,
            mips_at + i * 8,
            *offsets.last().expect("one level") as u64,
        );
    }
    // The BRTD block runs to the relocation table.
    put64(&mut out, brtd + 8, (new_rlt - brtd) as u64);
    // The relocation table: where it is, and the data section's size.
    put32(&mut out, 0x18, new_rlt as u32);
    put32(&mut out, new_rlt + 4, new_rlt as u32);
    let sections = out.as_slice().u32_at(new_rlt + 8)? as usize;
    for i in 0..sections {
        let s = new_rlt + 0x10 + i * 0x18;
        if out.as_slice().u32_at(s + 8)? as usize == brtd {
            put32(&mut out, s + 12, (new_rlt - brtd) as u32);
        }
    }
    let len = out.len() as u32;
    put32(&mut out, 0x1C, len);
    Ok(out)
}

fn d_u64(d: &[u8], at: usize) -> Result<u64> {
    d.u64_at(at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tegra::Gob;
    use crate::testing::tiny_bntx;

    fn picture(w: u32, h: u32) -> Image {
        let rgba = (0..h)
            .flat_map(|y| {
                (0..w).flat_map(move |x| [(x * 7) as u8, (y * 5) as u8, (x ^ y) as u8, 255])
            })
            .collect();
        Image::new(w, h, rgba).unwrap()
    }

    #[test]
    fn rgba8_writes_back_losslessly_for_both_gpus() {
        for gob in [Gob::TegraX1, Gob::Orin] {
            let file = tiny_bntx("Tex_Test_Alb", 37, 21, gob);
            let img = picture(50, 30);
            let out = write(&file, &img, Format(0x0b01), [2, 3, 4, 5]).unwrap();
            let bntx = Bntx::parse(&out).unwrap();
            let t = &bntx.textures[0];
            assert_eq!((t.width, t.height, t.format), (50, 30, Format(0x0b01)));
            assert_eq!(t.decode(0, 0).unwrap(), img);
            let rlt = out.as_slice().u32_at(0x18).unwrap() as usize;
            assert_eq!(rlt % RLT_ALIGN, 0);
            assert_eq!(&out[rlt..rlt + 4], b"_RLT");
            assert_eq!(out.as_slice().u32_at(0x1C).unwrap() as usize, out.len());
        }
    }

    #[test]
    fn compressed_formats_write_and_read_back() {
        let file = tiny_bntx("T", 64, 64, Gob::Orin);
        let img = picture(64, 48);
        for (format, channels) in [
            (0x1a06, 3),
            (0x1d01, 1),
            (0x1e01, 2),
            (0x2006, 4),
            (0x1c01, 4),
        ] {
            let out = write(&file, &img, Format(format), [2, 3, 4, 5]).unwrap();
            let t = Bntx::parse(&out).unwrap().textures[0].clone();
            let back = t.decode(0, 0).unwrap();
            let err = img
                .rgba
                .chunks(4)
                .zip(back.rgba.chunks(4))
                .map(|(a, b)| {
                    (0..channels)
                        .map(|c| a[c].abs_diff(b[c]) as u32)
                        .max()
                        .unwrap()
                })
                .max()
                .unwrap();
            assert!(err < 40, "format {format:04x}: {err}");
        }
    }

    #[test]
    fn mip_levels_halve() {
        let img = Image::new(4, 2, vec![255u8; 32]).unwrap();
        let next = half_size(&img, true);
        assert_eq!((next.width, next.height), (2, 1));
        assert!(next.rgba.iter().all(|&v| v == 255));
        let checker = Image::new(2, 2, [0u8, 0, 0, 255, 255, 255, 255, 255].repeat(2)).unwrap();
        // Black and white averaged in linear light is mid grey in sRGB (188), not 128.
        assert_eq!(half_size(&checker, true).rgba[0], 188);
        assert_eq!(half_size(&checker, false).rgba[0], 128);
    }
}
