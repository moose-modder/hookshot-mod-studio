//! The game's eye shader (materials with an iris, a sclera and an iris mask: `Mt_Eye_Inner`), for previews and
//! Blender scenes, which have one base colour per material.
//!
//! The eyeball's UVs cover the eye's whole surface around its front at (0.5, 0.5). The sclera texture (`_a1`)
//! is drawn at those UVs; the iris texture (`_a0`) and the iris mask (`_gn3`) at the UVs scaled about the
//! centre by the material's `ns_param_scalar3` (larger: a smaller iris; per character, 0.55 for Gorons to 3.3
//! for Kotake); the iris shows where the mask is white. The cornea's material (`Mt_Eye_Outer`) has the same value
//! in its `ns_param_scalar4`. Worked out from the textures and the values across the game's models (local
//! analysis, not the shader's code), and checked by rendering: with the mask in the eye's own UV space instead,
//! the iris covers the whole visible eye; scaled with the iris, the white shows around it as in the game.

use modstudio_formats::bfres::Material;
use modstudio_formats::image::Image;

/// Where a material's eye textures are, and the iris scale.
#[derive(Debug, Clone, PartialEq)]
pub struct EyeShader {
    pub iris: String,
    pub sclera: String,
    pub mask: String,
    pub iris_scale: f32,
}

/// The parameter that scales the iris and its mask.
pub const IRIS_SCALE: &str = "ns_param_scalar3";

/// Samplers of the eye shader's textures.
const IRIS: &str = "_a0";
const SCLERA: &str = "_a1";
const MASK: &str = "_gn3";

impl EyeShader {
    /// The material's eye shader setup, if it draws an eye this way.
    pub fn of(material: &Material) -> Option<Self> {
        let texture = |sampler: &str| {
            material
                .samplers
                .iter()
                .position(|s| s == sampler)
                .and_then(|i| material.textures.get(i).cloned())
        };
        if !material.name.contains("Eye") {
            return None;
        }
        let scale = material.shader.as_ref()?.param(IRIS_SCALE)?.floats();
        Some(EyeShader {
            iris: texture(IRIS)?,
            sclera: texture(SCLERA)?,
            mask: texture(MASK)?,
            iris_scale: *scale.first()?,
        })
    }

    /// The eye's colour as one image in the eyeball's UV space, the size of the sclera texture.
    pub fn composite(&self, sclera: &Image, iris: &Image, mask: &Image) -> Image {
        let (w, h) = (sclera.width, sclera.height);
        let mut rgba = Vec::with_capacity(w as usize * h as usize * 4);
        for y in 0..h {
            for x in 0..w {
                let u = (x as f32 + 0.5) / w as f32;
                let v = (y as f32 + 0.5) / h as f32;
                let iu = (u - 0.5) * self.iris_scale + 0.5;
                let iv = (v - 0.5) * self.iris_scale + 0.5;
                let s = sample(sclera, u, v);
                let i = sample(iris, iu, iv);
                let m = sample(mask, iu, iv)[0] / 255.0;
                for c in 0..3 {
                    rgba.push((s[c] * (1.0 - m) + i[c] * m).round() as u8);
                }
                rgba.push(255);
            }
        }
        Image {
            width: w,
            height: h,
            rgba,
        }
    }
}

/// Bilinear sample at (u, v), edges clamped.
fn sample(image: &Image, u: f32, v: f32) -> [f32; 4] {
    let (w, h) = (image.width as usize, image.height as usize);
    let x = (u * w as f32 - 0.5).clamp(0.0, (w - 1) as f32);
    let y = (v * h as f32 - 0.5).clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let px = |x: usize, y: usize, c: usize| image.rgba[(y * w + x) * 4 + c] as f32;
    let mut out = [0.0; 4];
    for (c, o) in out.iter_mut().enumerate() {
        let top = px(x0, y0, c) * (1.0 - fx) + px(x1, y0, c) * fx;
        let bottom = px(x0, y1, c) * (1.0 - fx) + px(x1, y1, c) * fx;
        *o = top * (1.0 - fy) + bottom * fy;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(w: u32, h: u32, f: impl Fn(f32, f32) -> [u8; 4]) -> Image {
        let mut rgba = Vec::new();
        for y in 0..h {
            for x in 0..w {
                rgba.extend(f((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32));
            }
        }
        Image::new(w, h, rgba).unwrap()
    }

    #[test]
    fn the_iris_shows_inside_its_scaled_mask() {
        let sclera = flat(64, 64, |_, _| [200, 200, 200, 255]);
        let iris = flat(32, 32, |_, _| [0, 0, 255, 255]);
        // white inside radius 0.25 of the mask's own UV space
        let mask = flat(64, 64, |u, v| {
            let inside = (u - 0.5).hypot(v - 0.5) < 0.25;
            [if inside { 255 } else { 0 }; 4]
        });
        let eye = EyeShader {
            iris: String::new(),
            sclera: String::new(),
            mask: String::new(),
            iris_scale: 2.0,
        };
        let out = eye.composite(&sclera, &iris, &mask);
        let at = |u: f32, v: f32| {
            let (x, y) = ((u * 64.0) as usize, (v * 64.0) as usize);
            out.rgba[(y * 64 + x) * 4..(y * 64 + x) * 4 + 3].to_vec()
        };
        // scale 2: the mask's circle (0.25) lands at 0.125 around the centre
        assert_eq!(at(0.5, 0.5), [0, 0, 255]);
        assert_eq!(at(0.5 + 0.1, 0.5), [0, 0, 255]);
        assert_eq!(at(0.5 + 0.2, 0.5), [200, 200, 200]);
        assert_eq!(at(0.02, 0.02), [200, 200, 200]);
    }
}
