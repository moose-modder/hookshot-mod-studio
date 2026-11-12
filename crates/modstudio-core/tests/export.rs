//! The glTF export of a synthetic model (no game files).

use modstudio_core::gltf;
use modstudio_core::model::model_glb;
use modstudio_formats::bfres::Bfres;
use modstudio_formats::testing::tiny_bfres;

fn floats(json: &serde_json::Value, bin: &[u8], accessor: u64) -> Vec<f32> {
    let a = &json["accessors"][accessor as usize];
    let view = &json["bufferViews"][a["bufferView"].as_u64().unwrap() as usize];
    let start = view["byteOffset"].as_u64().unwrap() as usize;
    let len = view["byteLength"].as_u64().unwrap() as usize;
    bin[start..start + len]
        .chunks(4)
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
        .collect()
}

#[test]
fn exports_skeleton_mesh_skin_and_material() {
    let file = tiny_bfres();
    let bfres = Bfres::parse(&file, None).unwrap();
    let model = &bfres.models[0];
    let (glb, meshes, skipped) = model_glb(model, &["Tex_Test_Alb".to_owned()]).unwrap();
    assert_eq!((meshes, skipped.len()), (1, 0));
    let (json, bin) = gltf::split(&glb).unwrap();

    // Two bones, the armature, the mesh.
    let nodes = json["nodes"].as_array().unwrap();
    let names: Vec<&str> = nodes.iter().map(|n| n["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Root", "Arm", "Tiny", "Tri__Mt_Test"]);
    assert_eq!(nodes[0]["children"], serde_json::json!([1]));
    assert_eq!(nodes[1]["translation"], serde_json::json!([1.0, 0.0, 0.0]));
    assert_eq!(json["skins"][0]["joints"], serde_json::json!([0, 1]));

    // Arm's inverse bind matrix moves x back by 1.
    let ibm = floats(
        &json,
        bin,
        json["skins"][0]["inverseBindMatrices"].as_u64().unwrap(),
    );
    assert_eq!(ibm[16 + 12], -1.0);

    let prim = &json["meshes"][0]["primitives"][0];
    let pos = floats(&json, bin, prim["attributes"]["POSITION"].as_u64().unwrap());
    assert_eq!(pos, [0., 0., 0., 1., 0., 0., 0., 1., 0.]);
    let pos_accessor =
        &json["accessors"][prim["attributes"]["POSITION"].as_u64().unwrap() as usize];
    assert_eq!(pos_accessor["max"], serde_json::json!([1.0, 1.0, 0.0]));
    let uv = floats(
        &json,
        bin,
        prim["attributes"]["TEXCOORD_0"].as_u64().unwrap(),
    );
    assert_eq!(uv, [0., 0., 1., 0., 0., 1.]);
    let weights = floats(
        &json,
        bin,
        prim["attributes"]["WEIGHTS_0"].as_u64().unwrap(),
    );
    assert_eq!(&weights[4..8], &[1., 0., 0., 0.]);

    // Joint of vertex 1 is bone 1 (Arm).
    let joints_view = &json["bufferViews"][json["accessors"]
        [prim["attributes"]["JOINTS_0"].as_u64().unwrap() as usize]["bufferView"]
        .as_u64()
        .unwrap() as usize];
    let at = joints_view["byteOffset"].as_u64().unwrap() as usize;
    assert_eq!(u16::from_le_bytes([bin[at + 8], bin[at + 9]]), 1);

    let mat = &json["materials"][0];
    assert_eq!(mat["name"], "Mt_Test");
    assert_eq!(mat["pbrMetallicRoughness"]["baseColorTexture"]["index"], 0);
    assert_eq!(json["images"][0]["uri"], "Tex_Test_Alb.png");
}

#[test]
fn weights_are_kept_exactly_and_repeated_bones_merged() {
    let mut file = tiny_bfres();
    {
        // Patch the vertex buffer: vertex 0 names bone 0 twice (128 + 127), vertex 2 splits 163 / 92.
        let bfres = Bfres::parse(&file, None).unwrap();
        let data = bfres.models[0].vertex_buffers[0].buffers[0].data;
        let at = data.as_ptr() as usize - file.as_ptr() as usize;
        drop(bfres);
        file[at + 16..at + 20].copy_from_slice(&[0, 0, 128, 127]);
        file[at + 40 + 16..at + 40 + 20].copy_from_slice(&[0, 1, 163, 92]);
    }
    let bfres = Bfres::parse(&file, None).unwrap();
    let (glb, _, _) = model_glb(&bfres.models[0], &[]).unwrap();
    let (json, bin) = gltf::split(&glb).unwrap();
    let prim = &json["meshes"][0]["primitives"][0];
    let weights = floats(
        &json,
        bin,
        prim["attributes"]["WEIGHTS_0"].as_u64().unwrap(),
    );
    assert!(
        (weights[0] - 1.0).abs() < 1e-6 && weights[1] == 0.0,
        "{:?}",
        &weights[..4]
    );
    assert_eq!(weights[8], 163.0 / 255.0);
    assert_eq!(weights[9], 92.0 / 255.0);
}

#[test]
fn an_unchanged_mesh_keeps_the_game_shape() {
    use modstudio_core::replace::geometry_from_gltf;
    use modstudio_formats::bfres_write::write;

    let file = tiny_bfres();
    let bfres = Bfres::parse(&file, None).unwrap();
    let model = &bfres.models[0];
    let (glb, _, _) = model_glb(model, &[]).unwrap();
    let gltf = gltf::Gltf::parse(&glb).unwrap();
    let (shapes, report) = geometry_from_gltf(model, &gltf).unwrap();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(report.shapes[0].status, "kept");
    assert!(shapes.is_empty());
    // Nothing to replace: the file is written back as it was.
    assert_eq!(write(&bfres, &shapes).unwrap(), file);
}

#[test]
fn export_then_replace_gives_the_same_geometry() {
    use modstudio_core::replace::geometry_from_gltf;
    use modstudio_formats::bfres_write::{ShapeGeometry, write};

    let file = tiny_bfres();
    let bfres = Bfres::parse(&file, None).unwrap();
    let model = &bfres.models[0];
    let (glb, _, _) = model_glb(model, &[]).unwrap();
    // One vertex moved a little in the scene: the shape is rebuilt, everything else as it was.
    let (json, bin) = gltf::split(&glb).unwrap();
    let mut bin = bin.to_vec();
    let mesh = json["meshes"][0]["primitives"][0]["attributes"]["POSITION"]
        .as_u64()
        .unwrap() as usize;
    let accessor = &json["accessors"][mesh];
    let view = &json["bufferViews"][accessor["bufferView"].as_u64().unwrap() as usize];
    let at = view["byteOffset"].as_u64().unwrap_or(0) as usize
        + accessor["byteOffset"].as_u64().unwrap_or(0) as usize;
    let x = f32::from_le_bytes(bin[at..at + 4].try_into().unwrap()) + 0.25;
    bin[at..at + 4].copy_from_slice(&x.to_le_bytes());
    let moved = glb_with(&json, &bin);
    let gltf = gltf::Gltf::parse(&moved).unwrap();
    let (shapes, report) = geometry_from_gltf(model, &gltf).unwrap();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(report.shapes[0].status, "replaced");

    let original = ShapeGeometry::from_shape(model, &model.shapes[0]).unwrap();
    let new = &shapes[&0];
    assert_eq!(new.lods, original.lods);
    let mut moved_values = 0;
    for name in ["_p0", "_u0", "_i0", "_w0"] {
        for (a, b) in new.attributes[name].iter().zip(&original.attributes[name]) {
            for c in 0..2 {
                if (a[c] - b[c]).abs() >= 1e-6 {
                    assert_eq!(name, "_p0", "{name}: {a:?} vs {b:?}");
                    moved_values += 1;
                }
            }
        }
    }
    assert_eq!(moved_values, 1);
    // Written and read back, the shape has the new vertices and the same skinning.
    let out = write(&bfres, &shapes).unwrap();
    let back = Bfres::parse(&out, None).unwrap();
    let read = ShapeGeometry::from_shape(&back.models[0], &back.models[0].shapes[0]).unwrap();
    let xyz = |v: &Vec<[f32; 4]>| v.iter().map(|p| [p[0], p[1], p[2]]).collect::<Vec<_>>();
    assert_eq!(xyz(&read.attributes["_p0"]), xyz(&new.attributes["_p0"]));
    assert_eq!(read.attributes["_w0"], original.attributes["_w0"]);
}

#[test]
fn a_shape_missing_from_the_scene_is_removed() {
    use modstudio_core::replace::geometry_from_gltf;

    let file = tiny_bfres();
    let bfres = Bfres::parse(&file, None).unwrap();
    let model = &bfres.models[0];
    let (glb, _, _) = model_glb(model, &[]).unwrap();
    // Rename the mesh node: the scene no longer has the shape.
    let (mut json, bin) = gltf::split(&glb).unwrap();
    json["nodes"][3]["name"] = serde_json::json!("Something_Else");
    let _ = bin;
    let renamed = glb_with_json(&glb, &json);
    let gltf = gltf::Gltf::parse(&renamed).unwrap();
    let (shapes, report) = geometry_from_gltf(model, &gltf).unwrap();
    assert_eq!(report.shapes[0].status, "removed");
    assert_eq!(shapes[&0].vertex_count, 1);
    assert!(report.warnings.iter().any(|w| w.contains("Something_Else")));
}

/// The same .glb with other JSON.
fn glb_with_json(glb: &[u8], json: &serde_json::Value) -> Vec<u8> {
    let (_, bin) = gltf::split(glb).unwrap();
    glb_with(json, bin)
}

/// A .glb of `json` and `bin`.
fn glb_with(json: &serde_json::Value, bin: &[u8]) -> Vec<u8> {
    let mut text = serde_json::to_vec(json).unwrap();
    text.resize(text.len().next_multiple_of(4), b' ');
    let total = 12 + 8 + text.len() + 8 + bin.len();
    let mut out = b"glTF".to_vec();
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
