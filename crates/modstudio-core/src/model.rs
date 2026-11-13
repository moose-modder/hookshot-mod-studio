//! Inspecting and exporting a model from a romfs.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use modstudio_formats::bfres::{Bfres, ExternalStrings, Model, PRIMITIVE_TRIANGLES, Shape};
use modstudio_formats::bntx::Bntx;
use modstudio_formats::romfs::{self, Romfs};
use modstudio_formats::zs::Decoder;
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::gltf::Glb;
use crate::look::{AlphaMode, Appearance, Look, TextureKind, appearance, appearances};
use crate::math::{self, Mat4};
use crate::{Context, Error, Result};

/// Where the game keeps the names models refer to by ID.
pub const EXTERNAL_STRINGS: &str = "Shader/ExternalBinaryString.bfres.zs";
/// Where textures are: one `<name>.bntx.zs` per texture.
pub const TEXTURE_DIR: &str = "Tex";

pub fn load_external_strings(romfs: &Romfs, decoder: &mut Decoder) -> Result<ExternalStrings> {
    let data = romfs
        .read(decoder, EXTERNAL_STRINGS)
        .context(EXTERNAL_STRINGS)?;
    ExternalStrings::parse(&data).context(EXTERNAL_STRINGS)
}

// ---------------------------------------------------------------- inspect (model.json)

#[derive(Debug, Serialize)]
pub struct Inspection {
    pub models: Vec<ModelInfo>,
    pub textures: Vec<TextureInfo>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub name: String,
    pub bones: Vec<String>,
    pub meshes: Vec<MeshInfo>,
    pub materials: Vec<MaterialInfo>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeshInfo {
    pub name: String,
    pub material: String,
    /// Vertices in the vertex buffer (all LODs).
    pub vertices: u32,
    pub skin_count: u8,
    pub lods: usize,
    /// Triangles of LOD 0.
    pub triangles: u32,
    /// Triangles of each LOD (the build makes lower LODs of an edited mesh in these proportions).
    pub lod_triangles: Vec<u32>,
    /// Stored as meshlets, which can't be read or replaced yet.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub meshlet: bool,
    pub attributes: Vec<AttributeInfo>,
}

#[derive(Debug, Serialize)]
pub struct AttributeInfo {
    pub name: String,
    pub format: String,
}

#[derive(Debug, Serialize)]
pub struct MaterialInfo {
    pub name: String,
    pub textures: Vec<String>,
    pub samplers: Vec<String>,
    /// How it's drawn with one base colour, its tint and alpha (the app's 3D view, the Blender scene;
    /// `look.rs`): keys `look`, `tint`, `alpha`.
    #[serde(flatten)]
    pub appearance: Appearance,
}

#[derive(Debug, Serialize)]
pub struct TextureInfo {
    pub name: String,
    pub format: String,
    pub width: u32,
    pub height: u32,
    pub mips: u32,
}

/// A model file read from the romfs, with what it needs.
pub struct Loaded {
    pub data: Vec<u8>,
    pub strings: ExternalStrings,
}

impl Loaded {
    pub fn read(romfs: &Romfs, decoder: &mut Decoder, model: &Path) -> Result<Self> {
        let strings = load_external_strings(romfs, decoder)?;
        let path = romfs.resolve(model);
        let data = romfs::read_maybe_zs(decoder, &path).context(path.display())?;
        Ok(Loaded { data, strings })
    }

    pub fn bfres(&self) -> Result<Bfres<'_>> {
        Ok(Bfres::parse(&self.data, Some(&self.strings))?)
    }
}

/// The textures a model's materials name, in first-use order.
pub fn texture_names(bfres: &Bfres) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for m in &bfres.models {
        for mat in &m.materials {
            for t in &mat.textures {
                if seen.insert(t.clone()) {
                    out.push(t.clone());
                }
            }
        }
    }
    out
}

/// Whether texture `name` has `role` ("_Alb", "_Nrm"): `<base>_Alb`, or one of its colour versions
/// `<base>_Alb.0`, `<base>_Alb.1`, ... (clothes the game tints per character).
pub fn has_role(name: &str, role: &str) -> bool {
    let base = match name.rsplit_once('.') {
        Some((base, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => base,
        _ => name,
    };
    base.ends_with(role)
}

fn texture_path(romfs: &Romfs, name: &str) -> PathBuf {
    romfs.resolve(Path::new(TEXTURE_DIR).join(format!("{name}.bntx.zs")))
}

pub fn inspect(romfs: &Romfs, decoder: &mut Decoder, bfres: &Bfres) -> Result<Inspection> {
    let models = bfres
        .models
        .iter()
        .map(|m| ModelInfo {
            name: m.name.clone(),
            bones: m.skeleton.bones.iter().map(|b| b.name.clone()).collect(),
            meshes: m.shapes.iter().map(|s| mesh_info(m, s)).collect(),
            materials: m
                .materials
                .iter()
                .zip(appearances(romfs, decoder, m))
                .map(|(mat, appearance)| MaterialInfo {
                    name: mat.name.clone(),
                    textures: mat.textures.clone(),
                    samplers: mat.samplers.clone(),
                    appearance,
                })
                .collect(),
        })
        .collect();
    let mut textures = Vec::new();
    for name in texture_names(bfres) {
        let path = texture_path(romfs, &name);
        if !path.exists() {
            continue;
        }
        let data = romfs::read_maybe_zs(decoder, &path).context(path.display())?;
        let bntx = Bntx::parse(&data).context(&name)?;
        if let Some(t) = bntx.textures.first() {
            textures.push(TextureInfo {
                name: name.clone(),
                format: t.format.name(),
                width: t.width,
                height: t.height,
                mips: t.mip_count,
            });
        }
    }
    Ok(Inspection { models, textures })
}

fn mesh_info(model: &Model, shape: &Shape) -> MeshInfo {
    let vb = &model.vertex_buffers[shape.vertex_buffer];
    MeshInfo {
        name: shape.name.clone(),
        material: model.materials[shape.material].name.clone(),
        vertices: vb.vertex_count,
        skin_count: shape.skin_count,
        lods: shape.meshes.len(),
        triangles: shape.meshes.first().map_or(0, |m| m.index_count / 3),
        lod_triangles: shape.meshes.iter().map(|m| m.index_count / 3).collect(),
        meshlet: shape.is_meshlet(),
        attributes: vb
            .attributes
            .iter()
            .map(|a| AttributeInfo {
                name: a.name.clone(),
                format: a.format.name(),
            })
            .collect(),
    }
}

// ---------------------------------------------------------------- export (.glb + PNG)

#[derive(Debug, Default, Serialize)]
pub struct ExportSummary {
    pub glb: PathBuf,
    pub meshes: usize,
    pub textures: Vec<String>,
    /// Textures the materials name that aren't in the romfs's `Tex` folder.
    pub missing_textures: Vec<String>,
    /// Shapes left out, with the reason.
    pub skipped: Vec<String>,
}

/// Writes `<model>.glb`, the textures as `<texture>.png` and `model.json` into `out_dir`.
pub fn export(romfs: &Romfs, model_path: &Path, out_dir: &Path) -> Result<ExportSummary> {
    export_lod(romfs, model_path, out_dir, 0)
}

/// [`export`] with each shape's LOD `lod` (a shape with fewer LODs: its last) in place of LOD 0.
pub fn export_lod(
    romfs: &Romfs,
    model_path: &Path,
    out_dir: &Path,
    lod: usize,
) -> Result<ExportSummary> {
    let mut decoder = romfs.decoder()?;
    let loaded = Loaded::read(romfs, &mut decoder, model_path)?;
    let bfres = loaded.bfres().context(model_path.display())?;
    std::fs::create_dir_all(out_dir).context(out_dir.display())?;

    let inspection = inspect(romfs, &mut decoder, &bfres)?;
    let json = serde_json::to_vec_pretty(&inspection).context("model.json")?;
    write(&out_dir.join("model.json"), &json)?;

    let mut summary = ExportSummary::default();
    for name in texture_names(&bfres) {
        let path = texture_path(romfs, &name);
        if !path.exists() {
            summary.missing_textures.push(name);
            continue;
        }
        let data = romfs::read_maybe_zs(&mut decoder, &path).context(path.display())?;
        let bntx = Bntx::parse(&data).context(&name)?;
        let Some(texture) = bntx.textures.first() else {
            summary.missing_textures.push(name);
            continue;
        };
        let mut image = texture.decode(0, 0).context(&name)?;
        texture.apply_channel_sources(&mut image);
        write(&out_dir.join(format!("{name}.png")), &image.to_png()?)?;
        summary.textures.push(name);
    }

    let model = bfres
        .models
        .first()
        .ok_or_else(|| Error(format!("{}: no models", model_path.display())))?;
    let looks = appearances(romfs, &mut decoder, model);
    let (glb, meshes, skipped) = model_glb_lod(model, &summary.textures, lod, &looks)?;
    summary.glb = out_dir.join(format!("{}.glb", model.name));
    write(&summary.glb, &glb)?;
    summary.meshes = meshes;
    summary.skipped = skipped;
    Ok(summary)
}

fn write(path: &Path, data: &[u8]) -> Result<()> {
    std::fs::write(path, data).context(path.display())
}

/// Each bone's local and model-space matrix.
pub fn bone_matrices(model: &Model) -> Result<(Vec<Mat4>, Vec<Mat4>)> {
    let skeleton = &model.skeleton;
    let local: Vec<Mat4> = skeleton
        .bones
        .iter()
        .map(|b| math::from_trs(b.translation, bone_rotation(model, b), b.scale))
        .collect();
    let mut world: Vec<Option<Mat4>> = vec![None; local.len()];
    for i in 0..local.len() {
        // Walk up to the first bone with a known matrix, then back down.
        let mut chain = vec![i];
        while let Some(p) = skeleton.bones[*chain.last().expect("non-empty")].parent {
            if world[p].is_some() {
                break;
            }
            if chain.contains(&p) {
                return Err(Error(format!(
                    "bone {}: parent loop",
                    skeleton.bones[i].name
                )));
            }
            chain.push(p);
        }
        for &b in chain.iter().rev() {
            let m = match skeleton.bones[b].parent {
                Some(p) => math::mul(&world[p].expect("parent done first"), &local[b]),
                None => local[b],
            };
            world[b] = Some(m);
        }
    }
    Ok((
        local,
        world.into_iter().map(|m| m.expect("all done")).collect(),
    ))
}

fn bone_rotation(model: &Model, b: &modstudio_formats::bfres::Bone) -> math::Quat {
    if model.skeleton.euler_rotations() {
        math::quat_from_euler_xyz([b.rotation[0], b.rotation[1], b.rotation[2]])
    } else {
        math::normalize_quat(b.rotation)
    }
}

/// The geometry of one shape's LOD 0, ready for glTF.
struct ShapeGeometry {
    positions: Vec<f32>,
    normals: Option<Vec<f32>>,
    uvs: Vec<Vec<f32>>,
    colors: Vec<Vec<f32>>,
    /// Bone indices and weights, `sets` x 4 per vertex.
    joints: Vec<u16>,
    weights: Vec<f32>,
    sets: usize,
    indices: Vec<u32>,
}

fn shape_geometry(
    model: &Model,
    shape: &Shape,
    world: &[Mat4],
    lod: usize,
) -> Result<ShapeGeometry> {
    let vb = &model.vertex_buffers[shape.vertex_buffer];
    let lod = &shape.meshes[lod.min(shape.meshes.len().saturating_sub(1))];
    if lod.primitive != PRIMITIVE_TRIANGLES {
        return Err(Error(format!(
            "primitive type {} (not triangles)",
            lod.primitive
        )));
    }
    let lod_indices = lod.indices()?;
    // Keep only the vertices LOD 0 uses (the buffer also holds the other LODs'), in their original order.
    let used: BTreeSet<u32> = lod_indices.iter().copied().collect();
    if used.last().is_some_and(|&v| v >= vb.vertex_count) {
        return Err(Error("index past the vertex buffer".into()));
    }
    let remap: BTreeMap<u32, u32> = used
        .iter()
        .enumerate()
        .map(|(new, &old)| (old, new as u32))
        .collect();
    let indices = lod_indices.iter().map(|i| remap[i]).collect();

    let read = |name: &str| -> Result<Option<Vec<[f32; 4]>>> {
        match vb.attribute(name) {
            Some(a) => {
                let all = vb.read(a)?;
                Ok(Some(used.iter().map(|&i| all[i as usize]).collect()))
            }
            None => Ok(None),
        }
    };
    let positions = read("_p0")?.ok_or_else(|| Error("no positions (_p0)".into()))?;
    let normals = read("_n0")?;
    let mut uv_names: Vec<&str> = vb
        .attributes
        .iter()
        .map(|a| a.name.as_str())
        .filter(|n| n.starts_with("_u"))
        .collect();
    uv_names.sort();
    let mut color_names: Vec<&str> = vb
        .attributes
        .iter()
        .map(|a| a.name.as_str())
        .filter(|n| n.starts_with("_c"))
        .collect();
    color_names.sort();

    // Skinning: which bones each vertex follows, and how much.
    let n = positions.len();
    let skin = shape.skin_count as usize;
    let sets = skin.div_ceil(4).max(1);
    let mut joints = vec![0u16; n * sets * 4];
    let mut weights = vec![0f32; n * sets * 4];
    let mut transform: Vec<Option<usize>> = vec![None; n];
    let bones = &model.skeleton.bones;
    let matrix_bone = |m: f32| -> Result<usize> {
        model
            .skeleton
            .matrix_to_bone
            .get(m as usize)
            .map(|&b| b as usize)
            .ok_or_else(|| Error(format!("bone matrix {m} out of range")))
    };
    match skin {
        0 => {
            let bone = shape.bone;
            if bone >= bones.len() {
                return Err(Error(format!("bone {bone} out of range")));
            }
            for v in 0..n {
                joints[v * sets * 4] = bone as u16;
                weights[v * sets * 4] = 1.0;
                transform[v] = Some(bone);
            }
        }
        1 => {
            let i0 = read("_i0")?;
            for v in 0..n {
                let bone = match &i0 {
                    Some(i0) => matrix_bone(i0[v][0])?,
                    None => shape.bone,
                };
                joints[v * sets * 4] = bone as u16;
                weights[v * sets * 4] = 1.0;
                // Rigid vertices are stored relative to their bone (as Switch Toolbox reads them).
                if bones[bone].rigid_matrix.is_some() {
                    transform[v] = Some(bone);
                }
            }
        }
        _ => {
            let i = [read("_i0")?, read("_i1")?];
            let w = [read("_w0")?, read("_w1")?];
            for v in 0..n {
                let base = v * sets * 4;
                let mut total = 0.0;
                for k in 0..skin {
                    let (set, c) = (k / 4, k % 4);
                    let (Some(iv), Some(wv)) = (&i[set], &w[set]) else {
                        continue;
                    };
                    let (bone, weight) = (matrix_bone(iv[v][c])? as u16, wv[v][c]);
                    total += weight;
                    // A bone named twice (files Switch Toolbox wrote have these) is one influence.
                    match (0..k).find(|&e| weights[base + e] > 0.0 && joints[base + e] == bone) {
                        Some(e) if weight > 0.0 => weights[base + e] += weight,
                        _ => {
                            joints[base + k] = bone;
                            weights[base + k] = weight;
                        }
                    }
                }
                // The game's weights are bytes that sum to 255: they are kept exactly. (Rescaling moves them
                // off the 1/255 steps, and a writer that truncates, like Switch Toolbox's, then loses a step.)
                let slot = &mut weights[base..base + sets * 4];
                if total <= 0.0 {
                    slot[0] = 1.0;
                } else if (total - 1.0).abs() > 0.5 / 255.0 {
                    slot.iter_mut().for_each(|x| *x /= total);
                }
            }
        }
    }

    let mut pos = Vec::with_capacity(n * 3);
    let mut nrm = normals.as_ref().map(|_| Vec::with_capacity(n * 3));
    for v in 0..n {
        let p = [positions[v][0], positions[v][1], positions[v][2]];
        let m = transform[v].map(|b| &world[b]);
        pos.extend(m.map_or(p, |m| math::transform_point(m, p)));
        if let (Some(out), Some(src)) = (&mut nrm, &normals) {
            let d = math::normalize3([src[v][0], src[v][1], src[v][2]]);
            out.extend(m.map_or(d, |m| math::transform_normal(m, d)));
        }
    }
    let uvs = uv_names
        .iter()
        .map(|name| {
            Ok(read(name)?
                .expect("listed")
                .iter()
                .flat_map(|v| [v[0], v[1]])
                .collect())
        })
        .collect::<Result<Vec<Vec<f32>>>>()?;
    // glTF colours are linear and Blender keeps them as sRGB bytes: writing the game's values as sRGB makes
    // the bytes in Blender the game's bytes (as with Switch Toolbox's FBX), and Blender's glTF export gives
    // them back the same way.
    let colors = color_names
        .iter()
        .map(|name| {
            Ok(read(name)?
                .expect("listed")
                .iter()
                .flat_map(|v| {
                    [
                        math::srgb_to_linear(v[0]),
                        math::srgb_to_linear(v[1]),
                        math::srgb_to_linear(v[2]),
                        v[3],
                    ]
                })
                .collect())
        })
        .collect::<Result<Vec<Vec<f32>>>>()?;
    Ok(ShapeGeometry {
        positions: pos,
        normals: nrm,
        uvs,
        colors,
        joints,
        weights,
        sets,
        indices,
    })
}

/// The model as `.glb`: the skeleton as nodes, one skinned mesh per shape (LOD 0), materials with their
/// textures (`<texture>.png` beside the file). Returns the file, the number of meshes and the shapes left out.
/// (Materials as their samplers and render settings say, without tints: [`model_glb_lod`] takes the
/// appearances read with the textures.)
pub fn model_glb(model: &Model, textures: &[String]) -> Result<(Vec<u8>, usize, Vec<String>)> {
    let looks: Vec<Appearance> = model
        .materials
        .iter()
        .map(|m| appearance(m, |_| TextureKind::Coloured))
        .collect();
    model_glb_lod(model, textures, 0, &looks)
}

/// [`model_glb`] with each shape's LOD `lod` (a shape with fewer LODs: its last) and each material's
/// appearance (`looks`, in material order: [`crate::look::appearances`]).
pub fn model_glb_lod(
    model: &Model,
    textures: &[String],
    lod: usize,
    looks: &[Appearance],
) -> Result<(Vec<u8>, usize, Vec<String>)> {
    let (_, world) = bone_matrices(model)?;
    let bones = &model.skeleton.bones;
    let mut glb = Glb::new();
    let mut nodes = Vec::new();
    for (i, b) in bones.iter().enumerate() {
        let children: Vec<usize> = (0..bones.len())
            .filter(|&c| bones[c].parent == Some(i))
            .collect();
        let mut node = json!({
            "name": b.name,
            "translation": b.translation,
            "rotation": bone_rotation(model, b),
            "scale": b.scale,
        });
        if !children.is_empty() {
            node["children"] = json!(children);
        }
        nodes.push(node);
    }
    let roots: Vec<usize> = (0..bones.len())
        .filter(|&i| bones[i].parent.is_none())
        .collect();
    let armature = nodes.len();
    nodes.push(json!({"name": model.name, "children": roots}));

    let ibm: Vec<f32> = world
        .iter()
        .map(|m| {
            math::inverse_affine(m).ok_or_else(|| Error("a bone's matrix can't be inverted".into()))
        })
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect();
    let ibm = glb.matrices(&ibm);
    let skins = json!([{
        "name": model.name,
        "joints": (0..bones.len()).collect::<Vec<_>>(),
        "inverseBindMatrices": ibm,
        "skeleton": armature,
    }]);

    // Materials, with the textures that were written.
    let written: BTreeSet<&str> = textures.iter().map(String::as_str).collect();
    let mut images = Vec::new();
    let mut image_index = BTreeMap::new();
    let mut texture_of = |name: &str| -> Option<usize> {
        if !written.contains(name) {
            return None;
        }
        Some(*image_index.entry(name.to_owned()).or_insert_with(|| {
            images.push(json!({"name": name, "uri": format!("{name}.png")}));
            images.len() - 1
        }))
    };
    let materials: Vec<Value> = model
        .materials
        .iter()
        .enumerate()
        .map(|(i, mat)| {
            let mut m = json!({
                "name": mat.name,
                "pbrMetallicRoughness": {"metallicFactor": 0.0, "roughnessFactor": 1.0},
                "extras": {"textures": mat.textures, "samplers": mat.samplers},
            });
            let fallback;
            let app = match looks.get(i) {
                Some(a) => a,
                None => {
                    fallback = appearance(mat, |_| TextureKind::Coloured);
                    &fallback
                }
            };
            if app.look == Look::Overlay {
                m["alphaMode"] = json!("BLEND");
                m["pbrMetallicRoughness"]["baseColorFactor"] = json!([1.0, 1.0, 1.0, 0.0]);
            } else {
                if let Some([r, g, b]) = app.tint {
                    m["pbrMetallicRoughness"]["baseColorFactor"] = json!([r, g, b, 1.0]);
                }
                if let Some(alpha) = app.alpha {
                    match alpha.mode {
                        AlphaMode::Opaque => {}
                        AlphaMode::Mask => {
                            m["alphaMode"] = json!("MASK");
                            m["alphaCutoff"] = json!(alpha.cutoff);
                        }
                        AlphaMode::Blend => m["alphaMode"] = json!("BLEND"),
                    }
                    if alpha.double_sided {
                        m["doubleSided"] = json!(true);
                    }
                }
            }
            if let Some(t) = app.look.color_texture().and_then(&mut texture_of) {
                m["pbrMetallicRoughness"]["baseColorTexture"] = json!({"index": t});
            }
            if let Some(t) = mat
                .textures
                .iter()
                .find(|t| has_role(t, "_Nrm"))
                .and_then(|t| texture_of(t))
            {
                m["normalTexture"] = json!({"index": t});
            }
            m
        })
        .collect();
    let gltf_textures: Vec<Value> = (0..images.len()).map(|i| json!({"source": i})).collect();

    let mut meshes = Vec::new();
    let mut skipped = Vec::new();
    let mut scene_nodes = vec![armature];
    for shape in &model.shapes {
        if shape.is_meshlet() || shape.meshes.is_empty() {
            skipped.push(format!("{}: stored as meshlets", shape.name));
            continue;
        }
        let g = match shape_geometry(model, shape, &world, lod) {
            Ok(g) => g,
            Err(e) => {
                skipped.push(format!("{}: {e}", shape.name));
                continue;
            }
        };
        let mut attributes = Map::new();
        attributes.insert("POSITION".into(), json!(glb.floats(&g.positions, 3, true)));
        if let Some(n) = &g.normals {
            attributes.insert("NORMAL".into(), json!(glb.floats(n, 3, false)));
        }
        for (k, uv) in g.uvs.iter().enumerate() {
            attributes.insert(format!("TEXCOORD_{k}"), json!(glb.floats(uv, 2, false)));
        }
        for (k, c) in g.colors.iter().enumerate() {
            attributes.insert(format!("COLOR_{k}"), json!(glb.floats(c, 4, false)));
        }
        let n = g.positions.len() / 3;
        for set in 0..g.sets {
            let pick = |v: usize| set * 4 + v * g.sets * 4;
            let joints: Vec<u16> = (0..n)
                .flat_map(|v| g.joints[pick(v)..pick(v) + 4].to_vec())
                .collect();
            let weights: Vec<f32> = (0..n)
                .flat_map(|v| g.weights[pick(v)..pick(v) + 4].to_vec())
                .collect();
            attributes.insert(format!("JOINTS_{set}"), json!(glb.joints(&joints)));
            attributes.insert(
                format!("WEIGHTS_{set}"),
                json!(glb.floats(&weights, 4, false)),
            );
        }
        let indices = glb.indices(&g.indices);
        meshes.push(json!({
            "name": shape.name,
            "primitives": [{"attributes": attributes, "indices": indices, "material": shape.material, "mode": 4}],
        }));
        scene_nodes.push(nodes.len());
        nodes.push(json!({"name": shape.name, "mesh": meshes.len() - 1, "skin": 0}));
    }

    let mesh_count = meshes.len();
    let mut doc = Map::new();
    doc.insert("scene".into(), json!(0));
    doc.insert(
        "scenes".into(),
        json!([{"name": model.name, "nodes": scene_nodes}]),
    );
    doc.insert("nodes".into(), Value::Array(nodes));
    doc.insert("skins".into(), skins);
    doc.insert("meshes".into(), Value::Array(meshes));
    doc.insert("materials".into(), Value::Array(materials));
    if !images.is_empty() {
        doc.insert("images".into(), Value::Array(images));
        doc.insert("textures".into(), Value::Array(gltf_textures));
    }
    Ok((glb.finish(doc), mesh_count, skipped))
}
