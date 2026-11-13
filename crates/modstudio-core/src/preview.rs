//! A model as one self-contained `.glb` for the app's 3D view: each mesh's LOD 0, the skeleton, and each
//! material's base colour texture stored in the file (a mip level at most `max_size` pixels a side). Materials
//! whose base colour has cut-out pixels (hair, lashes) are alpha-masked, as the game draws them. Vertex colours
//! are left out: the game's hold shading data, not colours, and a glTF viewer would multiply them in (a face
//! drawn black).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use modstudio_formats::bntx::Bntx;
use modstudio_formats::romfs::{self, Romfs};
use serde::Serialize;
use serde_json::{Value, json};

use modstudio_formats::image::Image;
use modstudio_formats::zs::Decoder;

use crate::eyes::EyeShader;
use crate::look::appearances;
use crate::model::{self, Loaded, TEXTURE_DIR};
use crate::{Context, Error, Result, gltf};

#[derive(Debug, Serialize)]
pub struct Preview {
    #[serde(skip)]
    pub glb: Vec<u8>,
    pub meshes: usize,
    /// Shapes left out (meshlets), with the reason.
    pub skipped: Vec<String>,
    pub missing_textures: Vec<String>,
}

/// The preview of `model_path` as `romfs` has it (with a mod's overlay, the mod's version).
pub fn preview(romfs: &Romfs, model_path: &Path, max_size: u32) -> Result<Preview> {
    let mut decoder = romfs.decoder()?;
    let loaded = Loaded::read(romfs, &mut decoder, model_path)?;
    let bfres = loaded.bfres().context(model_path.display())?;
    let model = bfres
        .models
        .first()
        .ok_or_else(|| Error(format!("{}: no models", model_path.display())))?;

    let looks = appearances(romfs, &mut decoder, model);
    let names: BTreeSet<String> = looks
        .iter()
        .filter_map(|a| a.look.color_texture().map(str::to_owned))
        .collect();
    let mut pngs = BTreeMap::new();
    let mut cut_out = BTreeSet::new();
    let mut missing = Vec::new();
    for name in &names {
        let Some(image) = load(romfs, &mut decoder, name, max_size)? else {
            missing.push(name.clone());
            continue;
        };
        if image.rgba.as_chunks::<4>().0.iter().any(|p| p[3] < 128) {
            cut_out.insert(name.clone());
        }
        pngs.insert(name.clone(), image.to_png()?);
    }
    // Eyes: the material's base colour (its iris texture) becomes the iris drawn over the sclera, as the game's
    // eye shader draws them (eyes.rs).
    for material in &model.materials {
        let Some(eye) = EyeShader::of(material) else {
            continue;
        };
        let (Some(sclera), Some(iris), Some(mask)) = (
            load(romfs, &mut decoder, &eye.sclera, max_size)?,
            load(romfs, &mut decoder, &eye.iris, max_size)?,
            load(romfs, &mut decoder, &eye.mask, max_size)?,
        ) else {
            continue;
        };
        cut_out.remove(&eye.iris);
        pngs.insert(
            eye.iris.clone(),
            eye.composite(&sclera, &iris, &mask).to_png()?,
        );
    }

    let textures: Vec<String> = pngs.keys().cloned().collect();
    // Cut-outs from the pixels only for materials without render settings: the others say how they use alpha.
    for a in &looks {
        if a.alpha.is_some()
            && let Some(t) = a.look.color_texture()
        {
            cut_out.remove(t);
        }
    }
    let (glb, meshes, skipped) = model::model_glb_lod(model, &textures, 0, &looks)?;
    let glb = gltf::embed_images(&glb, &pngs)?;
    let glb = gltf::edit_json(&glb, |doc| {
        mask_cut_outs(doc, &cut_out);
        drop_vertex_colours(doc);
    })?;
    Ok(Preview {
        glb,
        meshes,
        skipped,
        missing_textures: missing,
    })
}

/// A texture's image at the first mip level at most `max_size` pixels a side, or `None` if the game doesn't have
/// the texture.
fn load(romfs: &Romfs, decoder: &mut Decoder, name: &str, max_size: u32) -> Result<Option<Image>> {
    let path = romfs.resolve(Path::new(TEXTURE_DIR).join(format!("{name}.bntx.zs")));
    if !path.exists() {
        return Ok(None);
    }
    let data = romfs::read_maybe_zs(decoder, &path).context(path.display())?;
    let bntx = Bntx::parse(&data).context(name)?;
    let Some(texture) = bntx.textures.first() else {
        return Ok(None);
    };
    let mut level = 0;
    while level + 1 < texture.mip_count
        && (texture.width >> level).max(texture.height >> level) > max_size
    {
        level += 1;
    }
    let mut image = texture.decode(level, 0).context(name)?;
    texture.apply_channel_sources(&mut image);
    Ok(Some(image))
}

/// Removes the vertex colours (`COLOR_n`, `_C<n>`) from every primitive (the accessors stay, unused).
fn drop_vertex_colours(doc: &mut Value) {
    for mesh in doc["meshes"].as_array_mut().into_iter().flatten() {
        for prim in mesh["primitives"].as_array_mut().into_iter().flatten() {
            if let Some(attributes) = prim["attributes"].as_object_mut() {
                attributes.retain(|name, _| {
                    !name.starts_with("COLOR_") && !name.starts_with(crate::model::COLOR_DATA)
                });
            }
        }
    }
}

/// Alpha mask (and both sides drawn) for materials whose base colour image is in `cut_out`.
fn mask_cut_outs(doc: &mut Value, cut_out: &BTreeSet<String>) {
    let image_of_texture: Vec<Option<String>> = doc["textures"]
        .as_array()
        .map(|ts| {
            ts.iter()
                .map(|t| {
                    let source = t["source"].as_u64()? as usize;
                    doc["images"][source]["name"].as_str().map(str::to_owned)
                })
                .collect()
        })
        .unwrap_or_default();
    let Some(materials) = doc["materials"].as_array_mut() else {
        return;
    };
    for m in materials {
        let image = m["pbrMetallicRoughness"]["baseColorTexture"]["index"]
            .as_u64()
            .and_then(|i| image_of_texture.get(i as usize).cloned().flatten());
        if image.is_some_and(|name| cut_out.contains(&name)) {
            m["alphaMode"] = json!("MASK");
            m["alphaCutoff"] = json!(0.5);
            m["doubleSided"] = json!(true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previews_leave_out_vertex_colours_and_mask_cut_outs() {
        let mut doc = json!({
            "meshes": [{"primitives": [{"attributes": {"POSITION": 0, "COLOR_0": 1, "COLOR_1": 2, "TEXCOORD_0": 3}}]}],
            "images": [{"name": "Mt_Hair_Alb"}, {"name": "Mt_Skin_Alb"}],
            "textures": [{"source": 0}, {"source": 1}],
            "materials": [
                {"name": "Mt_Hair", "pbrMetallicRoughness": {"baseColorTexture": {"index": 0}}},
                {"name": "Mt_Skin", "pbrMetallicRoughness": {"baseColorTexture": {"index": 1}}}
            ]
        });
        drop_vertex_colours(&mut doc);
        mask_cut_outs(&mut doc, &BTreeSet::from(["Mt_Hair_Alb".to_owned()]));
        let attributes = doc["meshes"][0]["primitives"][0]["attributes"]
            .as_object()
            .unwrap();
        assert_eq!(
            attributes.keys().collect::<Vec<_>>(),
            ["POSITION", "TEXCOORD_0"]
        );
        assert_eq!(doc["materials"][0]["alphaMode"], "MASK");
        assert!(doc["materials"][1].get("alphaMode").is_none());
    }
}
