//! Changed textures written into a mod: each PNG replaces the game's texture of its name.
//!
//! A PNG holds what the shader reads (the texture after its channel sources, as `export` writes them). A texture
//! keeps its format where it can be encoded (BC1, BC3, BC4, BC5, BC7, RGBA8; BC2 becomes BC3, other uncompressed
//! formats RGBA8). ASTC, which isn't encoded here, becomes BC5 for two-channel textures (normal maps), BC4 for
//! one channel and BC7 otherwise; Switch Toolbox did the same and the game shows those right. sRGB stays sRGB.
//! Channel sources are kept where they are constants; the others read the PNG's channels in place.

use std::path::{Path, PathBuf};

use modstudio_formats::bntx::{Bntx, Encoding, Format, Texture};
use modstudio_formats::bntx_write;
use modstudio_formats::image::Image;
use modstudio_formats::romfs::{self, Romfs};
use serde::Serialize;

use crate::model::TEXTURE_DIR;
use crate::{Context, Error, Result};

/// The format and channel sources a changed texture is written with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Choice {
    pub format: Format,
    pub sources: [u8; 4],
}

const ZERO: u8 = 0;
const ONE: u8 = 1;

/// How `texture` is written when it changes.
pub fn choose(texture: &Texture) -> Result<Choice> {
    let f = texture.format;
    let srgb = f.is_srgb();
    let kind = |layout: u32| Format((layout << 8) | if srgb { 0x06 } else { 0x01 });
    let unorm = |layout: u32| Format((layout << 8) | 0x01);
    let orig = texture.channel_sources;
    let constant = |s: u8| s == ZERO || s == ONE;
    let variable: Vec<usize> = (0..4).filter(|&c| !constant(orig[c])).collect();
    let distinct: std::collections::BTreeSet<u8> = variable.iter().map(|&c| orig[c]).collect();

    let format = match f.encoding() {
        Some(Encoding::Bc1) => kind(0x1a),
        Some(Encoding::Bc2 | Encoding::Bc3) => kind(0x1c),
        Some(Encoding::Bc4) if !f.is_signed() => unorm(0x1d),
        Some(Encoding::Bc5) if !f.is_signed() => unorm(0x1e),
        Some(Encoding::Bc7) => kind(0x20),
        Some(Encoding::R8 | Encoding::Rg8 | Encoding::Rgba8 | Encoding::Bgra8) => kind(0x0b),
        Some(Encoding::Astc) => {
            if distinct.len() <= 1 && !srgb {
                unorm(0x1d)
            } else if variable == [0, 1] && !srgb {
                unorm(0x1e)
            } else {
                kind(0x20)
            }
        }
        _ => {
            return Err(Error(format!(
                "texture {}: {} can't be written (yet)",
                texture.name,
                f.name()
            )));
        }
    };
    let sources = match format.encoding() {
        Some(Encoding::Bc4) => orig.map(|s| if constant(s) { s } else { 2 }),
        Some(Encoding::Bc5) => {
            let mut s = [0u8; 4];
            for c in 0..4 {
                s[c] = if constant(orig[c]) {
                    orig[c]
                } else if c < 2 {
                    2 + c as u8
                } else {
                    ZERO
                };
            }
            s
        }
        _ => {
            let mut s = [0u8; 4];
            for c in 0..4 {
                s[c] = if constant(orig[c]) {
                    orig[c]
                } else {
                    2 + c as u8
                };
            }
            s
        }
    };
    Ok(Choice { format, sources })
}

#[derive(Debug, Clone, Serialize)]
pub struct TextureReport {
    pub name: String,
    pub from: String,
    pub to: String,
    pub width: u32,
    pub height: u32,
    pub path: PathBuf,
}

/// The game's texture `name` with `image`, as a compressed `.bntx.zs` file's bytes.
pub fn rebuild(romfs: &Romfs, name: &str, image: &Image) -> Result<(Vec<u8>, Choice, Format)> {
    let path = romfs
        .root()
        .join(TEXTURE_DIR)
        .join(format!("{name}.bntx.zs"));
    let data = romfs::read_maybe_zs(&mut romfs.decoder()?, &path).context(path.display())?;
    let bntx = Bntx::parse(&data).context(name)?;
    let texture = bntx
        .textures
        .first()
        .ok_or_else(|| Error(format!("{name}: no texture in the file")))?;
    let choice = choose(texture)?;
    let out = bntx_write::write(&data, image, choice.format, choice.sources).context(name)?;
    Ok((
        modstudio_formats::zs::compress(&out),
        choice,
        texture.format,
    ))
}

/// Every `<name>.png` in `dir` written as the game's texture `<name>` under `out_romfs/Tex`. PNGs that match no
/// game texture are reported and left out.
pub fn replace_textures(
    romfs: &Romfs,
    dir: &Path,
    out_romfs: &Path,
) -> Result<(Vec<TextureReport>, Vec<String>)> {
    let mut pngs: Vec<PathBuf> = std::fs::read_dir(dir)
        .context(dir.display())?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("png")))
        .collect();
    pngs.sort();
    let out_dir = out_romfs.join(TEXTURE_DIR);
    std::fs::create_dir_all(&out_dir).context(out_dir.display())?;

    let mut warnings = Vec::new();
    let mut jobs = Vec::new();
    for png in pngs {
        let name = png
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        if romfs
            .root()
            .join(TEXTURE_DIR)
            .join(format!("{name}.bntx.zs"))
            .exists()
        {
            jobs.push((name, png));
        } else {
            warnings.push(format!(
                "{name}.png: the game has no texture of that name; left out"
            ));
        }
    }
    // One texture per thread.
    let results: Vec<Result<TextureReport>> = std::thread::scope(|s| {
        let handles: Vec<_> = jobs
            .iter()
            .map(|(name, png)| {
                let out_dir = &out_dir;
                s.spawn(move || -> Result<TextureReport> {
                    let image = Image::from_png(&romfs::read_file(png)?).context(png.display())?;
                    let (bytes, choice, from) = rebuild(romfs, name, &image)?;
                    let path = out_dir.join(format!("{name}.bntx.zs"));
                    std::fs::write(&path, bytes).context(path.display())?;
                    Ok(TextureReport {
                        name: name.clone(),
                        from: from.name(),
                        to: choice.format.name(),
                        width: image.width,
                        height: image.height,
                        path,
                    })
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or_else(|_| Err(Error("a texture thread panicked".into())))
            })
            .collect()
    });
    let reports = results.into_iter().collect::<Result<Vec<_>>>()?;
    Ok((reports, warnings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use modstudio_formats::bntx::Bntx;
    use modstudio_formats::tegra::Gob;
    use modstudio_formats::testing::tiny_bntx;

    fn texture_with(format: u32, sources: [u8; 4]) -> Texture<'static> {
        let file: &'static [u8] = Box::leak(tiny_bntx("T", 8, 8, Gob::Orin).into_boxed_slice());
        let mut t = Bntx::parse(file).unwrap().textures[0].clone();
        t.format = Format(format);
        t.channel_sources = sources;
        t
    }

    #[test]
    fn astc_becomes_bc5_bc4_or_bc7() {
        // A normal map read as (R, A, 0, 1).
        let c = choose(&texture_with(0x2f01, [2, 5, 0, 1])).unwrap();
        assert_eq!(
            (c.format.name().as_str(), c.sources),
            ("BC5_UNORM", [2, 3, 0, 1])
        );
        // One channel everywhere.
        let c = choose(&texture_with(0x3301, [2, 2, 2, 1])).unwrap();
        assert_eq!(
            (c.format.name().as_str(), c.sources),
            ("BC4_UNORM", [2, 2, 2, 1])
        );
        // Colour, sRGB.
        let c = choose(&texture_with(0x3106, [2, 3, 4, 5])).unwrap();
        assert_eq!(
            (c.format.name().as_str(), c.sources),
            ("BC7_SRGB", [2, 3, 4, 5])
        );
    }

    #[test]
    fn encodable_formats_stay() {
        for (f, name) in [
            (0x1a06, "BC1_SRGB"),
            (0x1d01, "BC4_UNORM"),
            (0x1e01, "BC5_UNORM"),
            (0x2001, "BC7_UNORM"),
        ] {
            assert_eq!(
                choose(&texture_with(f, [2, 3, 4, 5]))
                    .unwrap()
                    .format
                    .name(),
                name
            );
        }
        assert_eq!(
            choose(&texture_with(0x1b06, [2, 3, 4, 5]))
                .unwrap()
                .format
                .name(),
            "BC3_SRGB"
        );
        assert!(choose(&texture_with(0x1f0a, [2, 3, 4, 5])).is_err());
    }
}
