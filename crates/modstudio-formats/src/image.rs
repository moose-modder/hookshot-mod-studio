//! Plain RGBA images, and PNG files.

use crate::error::{Error, Result, invalid};

/// 8-bit RGBA pixels, rows top to bottom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self> {
        if rgba.len() != width as usize * height as usize * 4 {
            return Err(invalid(format!(
                "{} bytes for a {width}x{height} RGBA image",
                rgba.len()
            )));
        }
        Ok(Image {
            width,
            height,
            rgba,
        })
    }

    pub fn to_png(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, self.width, self.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header().map_err(png_error)?;
        writer.write_image_data(&self.rgba).map_err(png_error)?;
        writer.finish().map_err(png_error)?;
        Ok(out)
    }

    /// Any 8- or 16-bit PNG (grey, grey + alpha, RGB, RGBA, palette), as RGBA.
    pub fn from_png(data: &[u8]) -> Result<Self> {
        let mut decoder = png::Decoder::new(std::io::Cursor::new(data));
        decoder.set_transformations(png::Transformations::normalize_to_color8());
        let mut reader = decoder.read_info().map_err(png_error)?;
        let size = reader
            .output_buffer_size()
            .ok_or_else(|| invalid("PNG too large"))?;
        let mut buf = vec![0u8; size];
        let info = reader.next_frame(&mut buf).map_err(png_error)?;
        buf.truncate(info.buffer_size());
        let rgba = match info.color_type {
            png::ColorType::Rgba => buf,
            png::ColorType::Rgb => buf
                .as_chunks::<3>()
                .0
                .iter()
                .flat_map(|p| [p[0], p[1], p[2], 255])
                .collect(),
            png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
            png::ColorType::GrayscaleAlpha => buf
                .as_chunks::<2>()
                .0
                .iter()
                .flat_map(|p| [p[0], p[0], p[0], p[1]])
                .collect(),
            png::ColorType::Indexed => return Err(invalid("PNG palette not expanded")),
        };
        Image::new(info.width, info.height, rgba)
    }
}

fn png_error(e: impl std::fmt::Display) -> Error {
    Error::Invalid(format!("PNG: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_round_trip() {
        let rgba: Vec<u8> = (0..5 * 3 * 4).map(|i| (i * 13) as u8).collect();
        let image = Image::new(5, 3, rgba).unwrap();
        let png = image.to_png().unwrap();
        assert_eq!(&png[1..4], b"PNG");
        assert_eq!(Image::from_png(&png).unwrap(), image);
        assert!(Image::new(2, 2, vec![0; 15]).is_err());
    }
}
