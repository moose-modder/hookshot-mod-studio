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
