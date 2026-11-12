//! A minimal glTF 2.0 binary (`.glb`) writer: one buffer, accessors over it, and JSON the caller builds.

use serde_json::{Map, Value, json};

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
        let mut text = serde_json::to_vec(&Value::Object(doc)).expect("JSON serialises");
        text.resize(text.len().next_multiple_of(4), b' ');

        let total = 12 + 8 + text.len() + 8 + self.bin.len();
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&(total as u32).to_le_bytes());
        out.extend_from_slice(&(text.len() as u32).to_le_bytes());
        out.extend_from_slice(b"JSON");
        out.extend_from_slice(&text);
        out.extend_from_slice(&(self.bin.len() as u32).to_le_bytes());
        out.extend_from_slice(b"BIN\0");
        out.extend_from_slice(&self.bin);
        out
    }
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
