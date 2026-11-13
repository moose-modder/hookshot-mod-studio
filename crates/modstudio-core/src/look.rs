//! How a material looks with one base colour, for the app's 3D view and the Blender scene (`model.json`'s
//! `look`; `make_scene.py` follows it).
//!
//! The base colour comes from the texture on the material's colour sampler (`_a0`), not from texture names:
//! eye shadows bind a texture named `..._Alb` to a mask sampler (`_gn3`), and drawn as a colour it covers the
//! eye in red. Eyes drawn by the game's eye shader get the iris over the sclera ([`crate::eyes`]); the eye's
//! overlays without a colour of their own (the cornea's highlights, eye shadows) are left out.
//!
//! [`appearance`] adds what else decides the colour, from the material's parameters and render settings: a tint
//! for grey colour textures (or the colour itself where the game has no colour texture), and how alpha is used
//! (opaque materials keep other data in it).

use std::path::Path;

use modstudio_formats::bfres::{Material, Model};
use modstudio_formats::bntx::Bntx;
use modstudio_formats::romfs::{self, Romfs};
use modstudio_formats::zs::Decoder;
use serde::{Deserialize, Serialize};

use crate::eyes::EyeShader;
use crate::model::TEXTURE_DIR;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Look {
    /// The texture on the colour sampler (or, for a material without one, its emission colour).
    #[serde(rename_all = "camelCase")]
    Color { texture: String },
    /// The game's eye shader: the sclera, with the iris and its mask scaled about the centre.
    #[serde(rename_all = "camelCase")]
    Eye {
        iris: String,
        sclera: String,
        mask: String,
        iris_scale: f32,
    },
    /// Drawn over the eye in the game (cornea highlights, eye shadows): left out (transparent).
    Overlay,
    /// No texture for its colour (a colour from shader parameters): a plain surface.
    Plain,
}

impl Look {
    /// The texture the look's base colour starts from (the iris for an eye).
    pub fn color_texture(&self) -> Option<&str> {
        match self {
            Look::Color { texture } => Some(texture),
            Look::Eye { iris, .. } => Some(iris),
            Look::Overlay | Look::Plain => None,
        }
    }
}

/// A colour sampler: `_a0`, `_a1`, ... (a few materials name them `a0`).
fn is_color_sampler(s: &str) -> bool {
    let s = s.strip_prefix('_').unwrap_or(s);
    s.strip_prefix('a')
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// An emission colour sampler: `_ec0`, ...
fn is_emission_color_sampler(s: &str) -> bool {
    s.strip_prefix("_ec")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

pub fn look(material: &Material) -> Look {
    if let Some(eye) = EyeShader::of(material) {
        return Look::Eye {
            iris: eye.iris,
            sclera: eye.sclera,
            mask: eye.mask,
            iris_scale: eye.iris_scale,
        };
    }
    let on = |test: fn(&str) -> bool| {
        material
            .samplers
            .iter()
            .zip(&material.textures)
            .find(|(s, _)| test(s))
            .map(|(_, t)| t.clone())
    };
    if let Some(texture) = on(is_color_sampler) {
        return Look::Color { texture };
    }
    if material.name.to_lowercase().contains("eye") {
        return Look::Overlay;
    }
    match on(is_emission_color_sampler) {
        Some(texture) => Look::Color { texture },
        None => Look::Plain,
    }
}

// ---------------------------------------------------------------- tint and alpha

/// A look with what else decides the colour: a tint, and how the game treats alpha.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Appearance {
    pub look: Look,
    /// Linear RGB the game multiplies the colour by (`ns_param_color0`), for a colour texture that is grey (shared
    /// eyebrow and eyelash textures: each character's colour is the tint) or that the game doesn't have (child
    /// Link's hair: the tint is the colour), and for a plain material. Coloured textures aren't tinted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tint: Option<[f32; 3]>,
    /// From the material's render settings; `None` for a material without them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alpha: Option<Alpha>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Alpha {
    pub mode: AlphaMode,
    /// The alpha test's threshold (mode "mask").
    pub cutoff: f32,
    pub double_sided: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AlphaMode {
    /// Alpha isn't opacity (the Zoras' skin keeps other data in it): drawn solid.
    Opaque,
    /// Alpha-tested at the cutoff (hair, lashes).
    Mask,
    /// Blended (`gsys_render_state_mode` "translucent").
    Blend,
}

/// What a colour texture holds, for the tint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureKind {
    /// The game doesn't have it.
    Missing,
    Grey,
    Coloured,
}

/// The parameter that tints a material's colour.
pub const TINT: &str = "ns_param_color0";

pub fn appearance(material: &Material, kind: impl Fn(&str) -> TextureKind) -> Appearance {
    let look = look(material);
    let color0 = material
        .shader
        .as_ref()
        .and_then(|s| s.param(TINT))
        .map(|p| p.floats())
        .filter(|c| c.len() >= 3)
        .map(|c| [c[0], c[1], c[2]])
        .filter(|c| c.iter().any(|v| (v - 1.0).abs() > 0.02));
    let tint = match &look {
        Look::Color { texture } if kind(texture) != TextureKind::Coloured => color0,
        Look::Plain => color0,
        _ => None,
    };
    Appearance {
        look,
        tint,
        alpha: alpha(material),
    }
}

/// The material's alpha handling, from its render settings (`gsys_render_state_mode`, `gsys_alpha_test_*`,
/// `gsys_render_state_display_face`).
pub fn alpha(material: &Material) -> Option<Alpha> {
    let s = material.shader.as_ref()?;
    let mode = s.render_string("gsys_render_state_mode")?;
    let tested = s.render_string("gsys_alpha_test_enable") == Some("true");
    Some(Alpha {
        mode: if tested {
            AlphaMode::Mask
        } else if mode == "translucent" {
            AlphaMode::Blend
        } else {
            AlphaMode::Opaque
        },
        cutoff: s.render_float("gsys_alpha_test_value").unwrap_or(0.5),
        double_sided: s.render_string("gsys_render_state_display_face") == Some("both"),
    })
}

/// Whether a texture is grey (mean saturation below 4 of 255, on a small mip level), coloured, or missing.
pub fn texture_kind(romfs: &Romfs, decoder: &mut Decoder, name: &str) -> TextureKind {
    let path = romfs.resolve(Path::new(TEXTURE_DIR).join(format!("{name}.bntx.zs")));
    let Ok(data) = romfs::read_maybe_zs(decoder, &path) else {
        return TextureKind::Missing;
    };
    let Some(image) = Bntx::parse(&data).ok().and_then(|b| {
        let t = b.textures.first()?;
        let mut image = t.decode(t.mip_count.saturating_sub(5), 0).ok()?;
        t.apply_channel_sources(&mut image);
        Some(image)
    }) else {
        return TextureKind::Missing;
    };
    let px = image.rgba.as_chunks::<4>().0;
    let saturation: u32 = px
        .iter()
        .map(|p| u32::from(p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2])))
        .sum();
    if (saturation as f32) < 4.0 * px.len().max(1) as f32 {
        TextureKind::Grey
    } else {
        TextureKind::Coloured
    }
}

/// Every material's appearance, the textures read from `romfs`.
pub fn appearances(romfs: &Romfs, decoder: &mut Decoder, model: &Model) -> Vec<Appearance> {
    let mut kinds = std::collections::HashMap::new();
    model
        .materials
        .iter()
        .map(|m| {
            if let Some(t) = look(m).color_texture()
                && !kinds.contains_key(t)
            {
                kinds.insert(t.to_owned(), texture_kind(romfs, decoder, t));
            }
            appearance(m, |t| {
                kinds.get(t).copied().unwrap_or(TextureKind::Coloured)
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use modstudio_formats::bfres::{ShaderAssign, ShaderParam};

    fn material(name: &str, slots: &[(&str, &str)], iris_scale: Option<f32>) -> Material {
        Material {
            at: 0,
            name: name.into(),
            samplers: slots.iter().map(|(s, _)| s.to_string()).collect(),
            textures: slots.iter().map(|(_, t)| t.to_string()).collect(),
            shader: iris_scale.map(|v| ShaderAssign {
                params: vec![ShaderParam {
                    name: crate::eyes::IRIS_SCALE.into(),
                    kind: 12,
                    data: v.to_le_bytes().to_vec(),
                }],
                ..ShaderAssign::default()
            }),
        }
    }

    #[test]
    fn looks_by_sampler() {
        let skin = material("Mt_Skin", &[("_n0", "Skin_Nrm"), ("_a0", "Skin_Alb")], None);
        assert_eq!(
            look(&skin),
            Look::Color {
                texture: "Skin_Alb".into()
            }
        );
        let fur = material("Mt_Fur", &[("a0", "Fur_Alb")], None);
        assert_eq!(look(&fur).color_texture(), Some("Fur_Alb"));
        // a texture named _Alb on a mask sampler is not a colour
        let shadow = material("Mt_EyeShadow", &[("_gn3", "Mt_EyeShadow_X_Alb")], None);
        assert_eq!(look(&shadow), Look::Overlay);
        let cornea = material(
            "Mt_Eye_Outer",
            &[("_e0", "Spec_Emc"), ("_n0", "Eye_Nrm")],
            None,
        );
        assert_eq!(look(&cornea), Look::Overlay);
        let gem = material("Mt_Gem", &[("_ec0", "Gem_Emc"), ("_n0", "Gem_Nrm")], None);
        assert_eq!(look(&gem).color_texture(), Some("Gem_Emc"));
        assert_eq!(look(&material("Mt_Black", &[], None)), Look::Plain);
        let eye = material(
            "Mt_Eye_Inner",
            &[
                ("_a0", "Iris_Alb"),
                ("_a1", "Sclera_Alb"),
                ("_gn3", "Mask_Gn3"),
            ],
            Some(1.8),
        );
        assert_eq!(
            look(&eye),
            Look::Eye {
                iris: "Iris_Alb".into(),
                sclera: "Sclera_Alb".into(),
                mask: "Mask_Gn3".into(),
                iris_scale: 1.8
            }
        );
        // the same textures without the eye shader's parameter: the colour sampler
        let plain_eye = material(
            "Mt_Eye_Inner",
            &[("_a0", "Iris_Alb"), ("_a1", "Sclera_Alb"), ("_gn3", "M")],
            None,
        );
        assert_eq!(look(&plain_eye).color_texture(), Some("Iris_Alb"));
    }

    fn with_settings(
        mut m: Material,
        color0: [f32; 3],
        mode: &str,
        test: bool,
        faces: &str,
    ) -> Material {
        use modstudio_formats::bfres::{RenderInfo, RenderValue};
        let s = m.shader.get_or_insert_with(ShaderAssign::default);
        s.params.push(ShaderParam {
            name: TINT.into(),
            kind: 15,
            data: [color0[0], color0[1], color0[2], 1.0]
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect(),
        });
        let text = |n: &str, v: &str| RenderInfo {
            name: n.into(),
            value: RenderValue::Strings(vec![v.into()]),
        };
        s.render_info = vec![
            text("gsys_render_state_mode", mode),
            text(
                "gsys_alpha_test_enable",
                if test { "true" } else { "false" },
            ),
            text("gsys_render_state_display_face", faces),
            RenderInfo {
                name: "gsys_alpha_test_value".into(),
                value: RenderValue::Floats(vec![0.4]),
            },
        ];
        m
    }

    #[test]
    fn tints_and_alpha() {
        let blond = [0.5, 0.17, 0.02];
        let hair = with_settings(
            material("Mt_Hairs", &[("_a0", "Hair_Alb")], None),
            blond,
            "mask",
            true,
            "both",
        );
        // grey or missing: tinted; coloured: not
        for (kind, tint) in [
            (TextureKind::Grey, Some(blond)),
            (TextureKind::Missing, Some(blond)),
            (TextureKind::Coloured, None),
        ] {
            assert_eq!(appearance(&hair, |_| kind).tint, tint, "{kind:?}");
        }
        assert_eq!(
            alpha(&hair),
            Some(Alpha {
                mode: AlphaMode::Mask,
                cutoff: 0.4,
                double_sided: true
            })
        );
        // the Zoras' skin: alpha holds data, the material is opaque
        let skin = with_settings(
            material("Mt_Skin", &[("_a0", "Skin_Alb")], None),
            [1.0; 3],
            "opaque",
            false,
            "front",
        );
        let a = appearance(&skin, |_| TextureKind::Grey);
        assert_eq!(a.tint, None, "a white tint changes nothing");
        assert_eq!(a.alpha.unwrap().mode, AlphaMode::Opaque);
        let glass = with_settings(
            material("Mt_Glass", &[("_a0", "G")], None),
            [1.0; 3],
            "translucent",
            false,
            "front",
        );
        assert_eq!(alpha(&glass).unwrap().mode, AlphaMode::Blend);
        assert_eq!(alpha(&material("Mt_Old", &[("_a0", "A")], None)), None);
    }

    #[test]
    fn json_for_make_scene() {
        let eye = Look::Eye {
            iris: "I".into(),
            sclera: "S".into(),
            mask: "M".into(),
            iris_scale: 1.5,
        };
        assert_eq!(
            serde_json::to_value(&eye).unwrap(),
            serde_json::json!({"kind": "eye", "iris": "I", "sclera": "S", "mask": "M", "irisScale": 1.5})
        );
        assert_eq!(
            serde_json::to_value(Look::Overlay).unwrap(),
            serde_json::json!({"kind": "overlay"})
        );
    }
}
