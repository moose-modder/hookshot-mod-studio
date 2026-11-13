//! A minimal glTF 2.0 binary (`.glb`) writer: one buffer, accessors over it, and JSON the caller builds.

use serde_json::{Map, Value, json};

use crate::math::{self, Mat4};

const FLOAT: u32 = 5126;
const UNSIGNED_SHORT: u32 = 5123;
const UNSIGNED_INT: u32 = 5125;
const ARRAY_BUFFER: u32 = 34962;
const ELEMENT_ARRAY_BUFFER: u32 = 34963;

#[derive(Default)]
pub struct Glb {
    bin: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
}

impl Glb {
    pub fn new() -> Self {
        Self::default()
    }

    fn view(&mut self, bytes: &[u8], target: u32) -> usize {
        self.bin.resize(self.bin.len().next_multiple_of(4), 0);
        self.views.push(json!({
            "buffer": 0,
            "byteOffset": self.bin.len(),
            "byteLength": bytes.len(),
            "target": target,
        }));
        self.bin.extend_from_slice(bytes);
        self.views.len() - 1
    }

    fn accessor(
        &mut self,
        view: usize,
        component: u32,
        count: usize,
        kind: &str,
        extra: Map<String, Value>,
    ) -> usize {
        let mut a = json!({
            "bufferView": view,
            "componentType": component,
            "count": count,
            "type": kind,
        });
        a.as_object_mut().expect("object").extend(extra);
        self.accessors.push(a);
        self.accessors.len() - 1
    }

    /// Floats with `width` components per element (`VEC2`, `VEC3`, `VEC4`, `MAT4`); `bounds` adds min/max
    /// (required for positions).
    pub fn floats(&mut self, data: &[f32], width: usize, bounds: bool) -> usize {
        let kind = match width {
            1 => "SCALAR",
            2 => "VEC2",
            3 => "VEC3",
            4 => "VEC4",
            16 => "MAT4",
            _ => panic!("no glTF type with {width} components"),
        };
        let count = data.len() / width;
        let bytes: Vec<u8> = data.iter().flat_map(|v| v.to_le_bytes()).collect();
        let view = self.view(&bytes, ARRAY_BUFFER);
        let mut extra = Map::new();
        if bounds && count > 0 {
            let mut min = vec![f32::INFINITY; width];
            let mut max = vec![f32::NEG_INFINITY; width];
            for e in data.chunks(width) {
                for c in 0..width {
                    min[c] = min[c].min(e[c]);
                    max[c] = max[c].max(e[c]);
                }
            }
            extra.insert("min".into(), json!(min));
            extra.insert("max".into(), json!(max));
        }
        self.accessor(view, FLOAT, count, kind, extra)
    }

    /// Joint indices, four per vertex.
    pub fn joints(&mut self, data: &[u16]) -> usize {
        let bytes: Vec<u8> = data.iter().flat_map(|v| v.to_le_bytes()).collect();
        let view = self.view(&bytes, ARRAY_BUFFER);
        self.accessor(view, UNSIGNED_SHORT, data.len() / 4, "VEC4", Map::new())
    }

    pub fn indices(&mut self, data: &[u32]) -> usize {
        let bytes: Vec<u8> = data.iter().flat_map(|v| v.to_le_bytes()).collect();
        let view = self.view(&bytes, ELEMENT_ARRAY_BUFFER);
        self.accessor(view, UNSIGNED_INT, data.len(), "SCALAR", Map::new())
    }

    /// Floats not tied to vertices (inverse bind matrices): a view without a target.
    pub fn matrices(&mut self, data: &[f32]) -> usize {
        self.bin.resize(self.bin.len().next_multiple_of(4), 0);
        self.views
            .push(json!({"buffer": 0, "byteOffset": self.bin.len(), "byteLength": data.len() * 4}));
        self.bin.extend(data.iter().flat_map(|v| v.to_le_bytes()));
        let view = self.views.len() - 1;
        self.accessor(view, FLOAT, data.len() / 16, "MAT4", Map::new())
    }

    /// The `.glb` file: `doc` holds everything but `asset`, `buffers`, `bufferViews` and `accessors`.
    pub fn finish(mut self, mut doc: Map<String, Value>) -> Vec<u8> {
        self.bin.resize(self.bin.len().next_multiple_of(4), 0);
        doc.insert(
            "asset".into(),
            json!({"version": "2.0", "generator": concat!("modstudio ", env!("CARGO_PKG_VERSION"))}),
        );
        doc.insert("buffers".into(), json!([{"byteLength": self.bin.len()}]));
        doc.insert("bufferViews".into(), Value::Array(self.views));
        doc.insert("accessors".into(), Value::Array(self.accessors));
        assemble(&Value::Object(doc), &self.bin)
    }
}

/// A `.glb` file from its JSON and binary chunk (`bin`'s length a multiple of 4).
fn assemble(doc: &Value, bin: &[u8]) -> Vec<u8> {
    let mut text = serde_json::to_vec(doc).expect("JSON serialises");
    text.resize(text.len().next_multiple_of(4), b' ');
    let total = 12 + 8 + text.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(text.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&text);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(bin);
    out
}

/// `glb` with its images (`"uri": "<name>.png"`) stored in the file, from `pngs` by image name; images not in
/// `pngs` are dropped, with the textures and material slots that use them. For a preview that is one file.
pub fn embed_images(
    glb: &[u8],
    pngs: &std::collections::BTreeMap<String, Vec<u8>>,
) -> crate::Result<Vec<u8>> {
    let (mut doc, bin) = split(glb).ok_or_else(|| crate::Error("not a .glb file".into()))?;
    let mut bin = bin.to_vec();
    let images = doc
        .get("images")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    // Old image index -> new texture index (one texture per image, as model_glb makes them).
    let mut kept_images = Vec::new();
    let mut texture_of = vec![None; images.len()];
    let mut views = doc["bufferViews"].as_array().cloned().unwrap_or_default();
    for (i, image) in images.iter().enumerate() {
        let Some(png) = image["name"].as_str().and_then(|n| pngs.get(n)) else {
            continue;
        };
        bin.resize(bin.len().next_multiple_of(4), 0);
        views.push(json!({"buffer": 0, "byteOffset": bin.len(), "byteLength": png.len()}));
        bin.extend_from_slice(png);
        texture_of[i] = Some(kept_images.len());
        kept_images.push(
            json!({"name": image["name"], "bufferView": views.len() - 1, "mimeType": "image/png"}),
        );
    }
    bin.resize(bin.len().next_multiple_of(4), 0);
    let old_textures = doc
        .get("textures")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let new_texture = |t: &Value| -> Option<usize> {
        let old = t["index"].as_u64()? as usize;
        let source = old_textures.get(old)?["source"].as_u64()? as usize;
        *texture_of.get(source)?
    };
    if let Some(materials) = doc.get_mut("materials").and_then(Value::as_array_mut) {
        for m in materials {
            let Some(m) = m.as_object_mut() else { continue };
            for slot in ["normalTexture", "occlusionTexture", "emissiveTexture"] {
                if let Some(t) = m.get(slot).cloned() {
                    match new_texture(&t) {
                        Some(n) => m[slot]["index"] = json!(n),
                        None => {
                            m.remove(slot);
                        }
                    }
                }
            }
            if let Some(pbr) = m
                .get_mut("pbrMetallicRoughness")
                .and_then(Value::as_object_mut)
            {
                for slot in ["baseColorTexture", "metallicRoughnessTexture"] {
                    if let Some(t) = pbr.get(slot).cloned() {
                        match new_texture(&t) {
                            Some(n) => pbr[slot]["index"] = json!(n),
                            None => {
                                pbr.remove(slot);
                            }
                        }
                    }
                }
            }
        }
    }
    let doc_obj = doc.as_object_mut().expect("glTF JSON is an object");
    doc_obj.remove("images");
    doc_obj.remove("textures");
    if !kept_images.is_empty() {
        let textures: Vec<Value> = (0..kept_images.len())
            .map(|i| json!({"source": i}))
            .collect();
        doc_obj.insert("images".into(), Value::Array(kept_images));
        doc_obj.insert("textures".into(), Value::Array(textures));
    }
    doc_obj.insert("bufferViews".into(), Value::Array(views));
    doc_obj.insert("buffers".into(), json!([{"byteLength": bin.len()}]));
    Ok(assemble(&doc, &bin))
}

/// `glb` with its JSON changed by `edit` (the binary chunk as it is).
pub fn edit_json(glb: &[u8], edit: impl FnOnce(&mut Value)) -> crate::Result<Vec<u8>> {
    let (mut doc, bin) = split(glb).ok_or_else(|| crate::Error("not a .glb file".into()))?;
    edit(&mut doc);
    Ok(assemble(&doc, bin))
}

/// Splits a `.glb` into its JSON and binary chunk (for tests and checks).
pub fn split(glb: &[u8]) -> Option<(Value, &[u8])> {
    if glb.get(..4)? != b"glTF" {
        return None;
    }
    let json_len = u32::from_le_bytes(glb.get(12..16)?.try_into().ok()?) as usize;
    let json: Value = serde_json::from_slice(glb.get(20..20 + json_len)?).ok()?;
    let bin_at = 20 + json_len;
    let bin_len = u32::from_le_bytes(glb.get(bin_at..bin_at + 4)?.try_into().ok()?) as usize;
    Some((json, glb.get(bin_at + 8..bin_at + 8 + bin_len)?))
}

// ---------------------------------------------------------------- reading (Blender's export)

/// A parsed `.glb`: what reading meshes back needs (accessors, nodes, skins).
pub struct Gltf {
    pub json: Value,
    bin: Vec<u8>,
}

/// One mesh in the scene: the node's name, its primitives, its skin.
#[derive(Debug, Clone)]
pub struct MeshNode {
    pub node: usize,
    pub name: String,
    pub mesh: usize,
    pub skin: Option<usize>,
}

fn err(what: impl Into<String>) -> crate::Error {
    crate::Error(format!("glTF: {}", what.into()))
}

impl Gltf {
    pub fn parse(glb: &[u8]) -> crate::Result<Self> {
        let (json, bin) = split(glb).ok_or_else(|| err("not a .glb file (binary glTF 2.0)"))?;
        if json["buffers"].as_array().map_or(0, Vec::len) > 1 {
            return Err(err("more than one buffer"));
        }
        Ok(Gltf {
            json,
            bin: bin.to_vec(),
        })
    }

    fn index(v: &Value, what: &str) -> crate::Result<usize> {
        v.as_u64()
            .map(|i| i as usize)
            .ok_or_else(|| err(format!("{what} missing")))
    }

    /// An accessor's values as floats, `width` per element (normalised integers scaled to 0..1 / -1..1).
    pub fn floats(&self, accessor: usize) -> crate::Result<(Vec<f32>, usize)> {
        let a = &self.json["accessors"][accessor];
        if a.get("sparse").is_some() {
            return Err(err("sparse accessors"));
        }
        let count = Self::index(&a["count"], "accessor count")?;
        let width = match a["type"].as_str() {
            Some("SCALAR") => 1,
            Some("VEC2") => 2,
            Some("VEC3") => 3,
            Some("VEC4") => 4,
            Some("MAT4") => 16,
            t => return Err(err(format!("accessor type {t:?}"))),
        };
        let component = Self::index(&a["componentType"], "componentType")? as u32;
        let normalized = a["normalized"].as_bool().unwrap_or(false);
        let size = match component {
            5120 | 5121 => 1,
            5122 | 5123 => 2,
            5125 | 5126 => 4,
            c => return Err(err(format!("component type {c}"))),
        };
        let view = &self.json["bufferViews"][Self::index(&a["bufferView"], "bufferView")?];
        let start = view["byteOffset"].as_u64().unwrap_or(0) as usize
            + a["byteOffset"].as_u64().unwrap_or(0) as usize;
        let stride = view["byteStride"]
            .as_u64()
            .map_or(width * size, |s| s as usize);
        let mut out = Vec::with_capacity(count * width);
        for e in 0..count {
            for c in 0..width {
                let at = start + e * stride + c * size;
                let b = self
                    .bin
                    .get(at..at + size)
                    .ok_or_else(|| err("accessor past the buffer"))?;
                let v = match component {
                    5126 => f32::from_le_bytes(b.try_into().expect("4")),
                    5125 => u32::from_le_bytes(b.try_into().expect("4")) as f32,
                    5123 => {
                        let u = u16::from_le_bytes([b[0], b[1]]);
                        if normalized {
                            f32::from(u) / 65535.0
                        } else {
                            f32::from(u)
                        }
                    }
                    5122 => {
                        let s = i16::from_le_bytes([b[0], b[1]]);
                        if normalized {
                            (f32::from(s) / 32767.0).max(-1.0)
                        } else {
                            f32::from(s)
                        }
                    }
                    5121 => {
                        if normalized {
                            f32::from(b[0]) / 255.0
                        } else {
                            f32::from(b[0])
                        }
                    }
                    _ => {
                        let s = b[0] as i8;
                        if normalized {
                            (f32::from(s) / 127.0).max(-1.0)
                        } else {
                            f32::from(s)
                        }
                    }
                };
                out.push(v);
            }
        }
        Ok((out, width))
    }

    /// An accessor of integers (indices, joints).
    pub fn integers(&self, accessor: usize) -> crate::Result<Vec<u32>> {
        let a = &self.json["accessors"][accessor];
        if a["componentType"].as_u64() == Some(5126) {
            return Err(err("float values where integers belong"));
        }
        // Integers up to 2^24 are exact as f32; indices beyond that aren't game-sized anyway.
        Ok(self
            .floats(accessor)?
            .0
            .into_iter()
            .map(|v| v as u32)
            .collect())
    }

    /// Every node's transform in scene space.
    pub fn world_matrices(&self) -> crate::Result<Vec<Mat4>> {
        let nodes = self.json["nodes"].as_array().cloned().unwrap_or_default();
        let local: Vec<Mat4> = nodes
            .iter()
            .map(|n| {
                if let Some(m) = n["matrix"].as_array() {
                    let mut out = math::IDENTITY;
                    for (k, v) in m.iter().take(16).enumerate() {
                        out[k] = v.as_f64().unwrap_or(0.0) as f32;
                    }
                    return out;
                }
                let f = |key: &str, default: &[f32]| -> Vec<f32> {
                    n[key].as_array().map_or(default.to_vec(), |a| {
                        a.iter().map(|v| v.as_f64().unwrap_or(0.0) as f32).collect()
                    })
                };
                let (t, r, s) = (
                    f("translation", &[0.0; 3]),
                    f("rotation", &[0.0, 0.0, 0.0, 1.0]),
                    f("scale", &[1.0; 3]),
                );
                math::from_trs(
                    [t[0], t[1], t[2]],
                    [r[0], r[1], r[2], r[3]],
                    [s[0], s[1], s[2]],
                )
            })
            .collect();
        let parent = self.parents();
        (0..nodes.len())
            .map(|i| {
                let mut m = local[i];
                let mut p = parent[i];
                let mut steps = 0;
                while let Some(j) = p {
                    m = math::mul(&local[j], &m);
                    p = parent[j];
                    steps += 1;
                    if steps > nodes.len() {
                        return Err(err("node hierarchy has a loop"));
                    }
                }
                Ok(m)
            })
            .collect()
    }

    /// Each node's parent.
    pub fn parents(&self) -> Vec<Option<usize>> {
        let nodes = self.json["nodes"].as_array().cloned().unwrap_or_default();
        let mut parent = vec![None; nodes.len()];
        for (i, n) in nodes.iter().enumerate() {
            for c in n["children"].as_array().into_iter().flatten() {
                if let Some(c) = c.as_u64().map(|c| c as usize).filter(|&c| c < nodes.len()) {
                    parent[c] = Some(i);
                }
            }
        }
        parent
    }

    pub fn node_name(&self, node: usize) -> String {
        self.json["nodes"][node]["name"]
            .as_str()
            .unwrap_or("")
            .to_owned()
    }

    /// The nodes that carry a mesh.
    pub fn mesh_nodes(&self) -> Vec<MeshNode> {
        let nodes = self.json["nodes"].as_array().cloned().unwrap_or_default();
        nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                Some(MeshNode {
                    node: i,
                    name: n["name"].as_str().unwrap_or("").to_owned(),
                    mesh: n["mesh"].as_u64()? as usize,
                    skin: n["skin"].as_u64().map(|s| s as usize),
                })
            })
            .collect()
    }

    /// A skin's joints (node indices) and inverse bind matrices.
    pub fn skin(&self, skin: usize) -> crate::Result<(Vec<usize>, Vec<Mat4>)> {
        let s = &self.json["skins"][skin];
        let joints: Vec<usize> = s["joints"]
            .as_array()
            .ok_or_else(|| err("skin without joints"))?
            .iter()
            .map(|j| Self::index(j, "joint"))
            .collect::<crate::Result<_>>()?;
        let ibm = match s["inverseBindMatrices"].as_u64() {
            Some(a) => {
                let (v, w) = self.floats(a as usize)?;
                if w != 16 || v.len() != joints.len() * 16 {
                    return Err(err("inverse bind matrices don't match the joints"));
                }
                v.chunks(16).map(|c| c.try_into().expect("16")).collect()
            }
            None => vec![math::IDENTITY; joints.len()],
        };
        Ok((joints, ibm))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_and_bounds() {
        let mut g = Glb::new();
        let pos = g.floats(&[0., 1., 2., -1., 5., 0.5], 3, true);
        let idx = g.indices(&[0, 1, 0]);
        let mut doc = Map::new();
        doc.insert(
            "meshes".into(),
            json!([{"primitives": [{"attributes": {"POSITION": pos}, "indices": idx}]}]),
        );
        let glb = g.finish(doc);
        assert_eq!(glb.len() % 4, 0);
        assert_eq!(
            u32::from_le_bytes(glb[8..12].try_into().unwrap()) as usize,
            glb.len()
        );
        let (json, bin) = split(&glb).unwrap();
        assert_eq!(json["accessors"][pos]["min"], json!([-1.0, 1.0, 0.5]));
        assert_eq!(json["accessors"][pos]["max"], json!([0.0, 5.0, 2.0]));
        assert_eq!(json["buffers"][0]["byteLength"], json!(bin.len()));
        let view =
            &json["bufferViews"][json["accessors"][idx]["bufferView"].as_u64().unwrap() as usize];
        assert_eq!(view["byteOffset"], json!(24));
        assert_eq!(&bin[24..28], &0u32.to_le_bytes());
    }
}
