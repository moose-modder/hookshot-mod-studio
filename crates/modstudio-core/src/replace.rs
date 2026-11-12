//! A model rebuilt from Blender's glTF export of its scene: each mesh replaces the game's shape of the same
//! name, in that shape's vertex formats, skinning and LOD count.
//!
//! - Positions, normals and tangents are taken to the rest pose through the export's own joints and inverse bind
//!   matrices (so the armature's orientation in Blender doesn't matter), then stored as the shape stores them:
//!   in model space for smooth skinning, in the bone's space for rigid (1 influence) and unskinned (0) shapes.
//! - Influences are matched to the game's bones by name. Smooth shapes keep their skin count (the strongest
//!   influences, weights in whole steps that add up exactly); a bone without a smooth matrix gives its weight to
//!   its nearest ancestor that has one. Rigid shapes take each vertex's strongest bone (its nearest ancestor with
//!   a rigid matrix); unskinned shapes follow their bone.
//! - Every attribute the game's vertex buffer has is written: UVs from TEXCOORD_n, colours from COLOR_n (white
//!   where the scene has none), tangents from TANGENT (computed from UV 0 if missing).
//! - Every LOD of the shape gets the new mesh (simplified LODs are for later); sub-mesh counts stay the game's.
//! - A shape the scene has no mesh for is removed (drawn as nothing), as Switch Toolbox's replace did.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use modstudio_formats::bfres::{Bfres, Model, Shape};
use modstudio_formats::bfres_write::{self, LodGeometry, ShapeGeometry};
use modstudio_formats::romfs::{self, Romfs};
use serde::Serialize;

use crate::gltf::Gltf;
use crate::math::{self, Mat4};
use crate::model::{Loaded, bone_matrices};
use crate::{Context, Error, Result};

#[derive(Debug, Clone, Serialize)]
pub struct ShapeReport {
    pub name: String,
    /// "replaced", "removed"
    pub status: String,
    pub vertices: u32,
    pub triangles: u32,
}

#[derive(Debug, Default, Serialize)]
pub struct GeometryReport {
    pub shapes: Vec<ShapeReport>,
    pub warnings: Vec<String>,
}

impl GeometryReport {
    fn warn(&mut self, w: String) {
        if !self.warnings.contains(&w) {
            self.warnings.push(w);
        }
    }
}

fn plain_name(name: &str) -> &str {
    // Blender's duplicate suffix: "Mesh.001" -> "Mesh".
    match name.rsplit_once('.') {
        Some((base, n)) if n.len() == 3 && n.bytes().all(|b| b.is_ascii_digit()) => base,
        _ => name,
    }
}

/// One mesh of the scene, every vertex in the game's model space.
struct SceneMesh {
    positions: Vec<[f32; 3]>,
    normals: Option<Vec<[f32; 3]>>,
    /// xyz, handedness
    tangents: Option<Vec<[f32; 4]>>,
    uvs: Vec<Vec<[f32; 2]>>,
    /// Linear, as glTF stores them.
    colors: Vec<Vec<[f32; 4]>>,
    /// Per vertex: (game bone, weight), strongest first, weights adding up to 1.
    influences: Vec<Vec<(usize, f32)>>,
    indices: Vec<u32>,
}

fn read_mesh(
    gltf: &Gltf,
    node: &crate::gltf::MeshNode,
    world: &[Mat4],
    bones: &BTreeMap<&str, usize>,
    report: &mut GeometryReport,
) -> Result<SceneMesh> {
    let prims = gltf.json["meshes"][node.mesh]["primitives"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if prims.len() > 1 {
        report.warn(format!(
            "{}: {} materials in the scene; the game draws the mesh with its own material",
            node.name,
            prims.len()
        ));
    }
    let skin = node.skin.map(|s| gltf.skin(s)).transpose()?;
    let joint_names: Vec<String> = skin
        .as_ref()
        .map(|(joints, _)| joints.iter().map(|&j| gltf.node_name(j)).collect())
        .unwrap_or_default();
    // Joint world * inverse bind: what takes a vertex to the rest pose.
    let joint_mats: Vec<Mat4> = skin
        .as_ref()
        .map(|(joints, ibm)| {
            joints
                .iter()
                .zip(ibm)
                .map(|(&j, m)| math::mul(&world[j], m))
                .collect()
        })
        .unwrap_or_default();

    let mut mesh = SceneMesh {
        positions: Vec::new(),
        normals: Some(Vec::new()),
        tangents: Some(Vec::new()),
        uvs: Vec::new(),
        colors: Vec::new(),
        influences: Vec::new(),
        indices: Vec::new(),
    };
    for (pi, p) in prims.iter().enumerate() {
        if p["mode"].as_u64().unwrap_or(4) != 4 {
            return Err(Error(format!(
                "{}: primitive {pi} isn't triangles",
                node.name
            )));
        }
        let attrs = &p["attributes"];
        let get = |name: &str| -> Result<Option<(Vec<f32>, usize)>> {
            attrs[name]
                .as_u64()
                .map(|a| gltf.floats(a as usize))
                .transpose()
        };
        let (pos, _) =
            get("POSITION")?.ok_or_else(|| Error(format!("{}: no positions", node.name)))?;
        let n = pos.len() / 3;
        let base = mesh.positions.len() as u32;
        match p["indices"].as_u64() {
            Some(a) => mesh
                .indices
                .extend(gltf.integers(a as usize)?.into_iter().map(|i| i + base)),
            None => mesh.indices.extend(base..base + n as u32),
        }

        // Influences, by game bone.
        let mut infl: Vec<Vec<(usize, f32)>> = vec![Vec::new(); n];
        let mut mats: Vec<Mat4> = vec![
            if skin.is_some() {
                [0.0; 16]
            } else {
                world[node.node]
            };
            n
        ];
        if skin.is_some() {
            for set in 0.. {
                let (Some(j), Some(w)) = (
                    attrs[format!("JOINTS_{set}")].as_u64(),
                    attrs[format!("WEIGHTS_{set}")].as_u64(),
                ) else {
                    break;
                };
                let joints = gltf.integers(j as usize)?;
                let (weights, _) = gltf.floats(w as usize)?;
                for v in 0..n {
                    for c in 0..4 {
                        let (joint, weight) = (joints[v * 4 + c] as usize, weights[v * 4 + c]);
                        if weight <= 0.0 {
                            continue;
                        }
                        let m = joint_mats
                            .get(joint)
                            .ok_or_else(|| Error(format!("{}: joint {joint}", node.name)))?;
                        for k in 0..16 {
                            mats[v][k] += weight * m[k];
                        }
                        let name = &joint_names[joint];
                        match bones.get(name.as_str()) {
                            Some(&b) => infl[v].push((b, weight)),
                            None => report.warn(format!(
                                "{}: weights on {name}, which the game's skeleton doesn't have, were dropped",
                                node.name
                            )),
                        }
                    }
                }
            }
            for m in mats.iter_mut() {
                let total: f32 = (0..4).map(|r| m[r * 5]).sum::<f32>() / 4.0;
                if total.abs() < 1e-6 {
                    // No weights: the rest pose is the mesh as stored.
                    *m = math::IDENTITY;
                }
            }
        }
        for list in infl.iter_mut() {
            // Duplicates merged, strongest first, adding up to 1.
            let mut merged: BTreeMap<usize, f32> = BTreeMap::new();
            for &(b, w) in list.iter() {
                *merged.entry(b).or_default() += w;
            }
            let mut l: Vec<(usize, f32)> = merged.into_iter().collect();
            l.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
            let total: f32 = l.iter().map(|x| x.1).sum();
            if total > 0.0 {
                l.iter_mut().for_each(|x| x.1 /= total);
            }
            *list = l;
        }

        let normals = get("NORMAL")?;
        let tangents = get("TANGENT")?;
        for v in 0..n {
            let m = &mats[v];
            mesh.positions.push(math::transform_point(
                m,
                [pos[v * 3], pos[v * 3 + 1], pos[v * 3 + 2]],
            ));
        }
        match (&mut mesh.normals, normals) {
            (Some(out), Some((nv, _))) => {
                for v in 0..n {
                    out.push(math::transform_normal(
                        &mats[v],
                        [nv[v * 3], nv[v * 3 + 1], nv[v * 3 + 2]],
                    ));
                }
            }
            (out, _) => *out = None,
        }
        match (&mut mesh.tangents, tangents) {
            (Some(out), Some((tv, 4))) => {
                for v in 0..n {
                    let t =
                        transform_direction(&mats[v], [tv[v * 4], tv[v * 4 + 1], tv[v * 4 + 2]]);
                    out.push([
                        t[0],
                        t[1],
                        t[2],
                        if tv[v * 4 + 3] < 0.0 { -1.0 } else { 1.0 },
                    ]);
                }
            }
            (out, _) => *out = None,
        }
        for set in 0.. {
            let Some((uv, _)) = get(&format!("TEXCOORD_{set}"))? else {
                break;
            };
            if pi == 0 {
                mesh.uvs.push(Vec::new());
            }
            if let Some(out) = mesh.uvs.get_mut(set) {
                out.extend((0..n).map(|v| [uv[v * 2], uv[v * 2 + 1]]));
            }
        }
        for set in 0.. {
            let Some((c, w)) = get(&format!("COLOR_{set}"))? else {
                break;
            };
            if pi == 0 {
                mesh.colors.push(Vec::new());
            }
            if let Some(out) = mesh.colors.get_mut(set) {
                out.extend((0..n).map(|v| {
                    let a = if w == 4 { c[v * 4 + 3] } else { 1.0 };
                    [c[v * w], c[v * w + 1], c[v * w + 2], a]
                }));
            }
        }
        mesh.influences.extend(infl);
    }
    let n = mesh.positions.len();
    mesh.uvs.retain(|u| u.len() == n);
    mesh.colors.retain(|c| c.len() == n);
    if mesh.indices.iter().any(|&i| i as usize >= n) || !mesh.indices.len().is_multiple_of(3) {
        return Err(Error(format!(
            "{}: indices don't make triangles of its vertices",
            node.name
        )));
    }
    Ok(mesh)
}

fn transform_direction(m: &Mat4, d: [f32; 3]) -> [f32; 3] {
    math::normalize3([0, 1, 2].map(|r| m[r] * d[0] + m[4 + r] * d[1] + m[8 + r] * d[2]))
}

/// Tangents from UV 0, for scenes exported without them: per triangle, then averaged and made orthogonal.
fn compute_tangents(mesh: &SceneMesh, normals: &[[f32; 3]]) -> Vec<[f32; 4]> {
    let n = mesh.positions.len();
    let mut tan = vec![[0f32; 3]; n];
    let mut bit = vec![[0f32; 3]; n];
    if let Some(uv) = mesh.uvs.first() {
        for t in mesh.indices.chunks(3) {
            let [a, b, c] = [t[0] as usize, t[1] as usize, t[2] as usize];
            let (p0, p1, p2) = (mesh.positions[a], mesh.positions[b], mesh.positions[c]);
            let e1 = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
            let e2 = [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]];
            let (du1, dv1) = (uv[b][0] - uv[a][0], uv[b][1] - uv[a][1]);
            let (du2, dv2) = (uv[c][0] - uv[a][0], uv[c][1] - uv[a][1]);
            let det = du1 * dv2 - du2 * dv1;
            if det.abs() < 1e-12 {
                continue;
            }
            let r = 1.0 / det;
            let s = [0, 1, 2].map(|i| (e1[i] * dv2 - e2[i] * dv1) * r);
            let u = [0, 1, 2].map(|i| (e2[i] * du1 - e1[i] * du2) * r);
            for &v in &[a, b, c] {
                for i in 0..3 {
                    tan[v][i] += s[i];
                    bit[v][i] += u[i];
                }
            }
        }
    }
    (0..n)
        .map(|v| {
            let nv = normals[v];
            let t = tan[v];
            let d = t[0] * nv[0] + t[1] * nv[1] + t[2] * nv[2];
            let mut o = math::normalize3([t[0] - nv[0] * d, t[1] - nv[1] * d, t[2] - nv[2] * d]);
            if o.iter().all(|c| *c == 0.0) || o.iter().any(|c| !c.is_finite()) {
                // Any direction across the normal.
                let a = if nv[0].abs() < 0.9 {
                    [1.0, 0.0, 0.0]
                } else {
                    [0.0, 1.0, 0.0]
                };
                o = math::normalize3(cross(a, nv));
            }
            let w = if dot(cross(nv, o), bit[v]) < 0.0 {
                -1.0
            } else {
                1.0
            };
            [o[0], o[1], o[2], w]
        })
        .collect()
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Weights as whole steps of `1 / max` that add up to exactly 1 (largest remainders get the leftover steps).
fn quantize_weights(weights: &[f32], max: u32) -> Vec<f32> {
    let scaled: Vec<f32> = weights.iter().map(|w| w * max as f32).collect();
    let mut steps: Vec<u32> = scaled.iter().map(|s| s.floor() as u32).collect();
    let mut left = max.saturating_sub(steps.iter().sum());
    let mut order: Vec<usize> = (0..weights.len()).collect();
    order.sort_by(|&a, &b| {
        (scaled[b] - scaled[b].floor()).total_cmp(&(scaled[a] - scaled[a].floor()))
    });
    for &i in order.iter().cycle().take(weights.len() * 2) {
        if left == 0 {
            break;
        }
        steps[i] += 1;
        left -= 1;
    }
    steps.iter().map(|&s| s as f32 / max as f32).collect()
}

/// The nearest bone up the hierarchy (itself first) that `has` accepts.
fn nearest_with(model: &Model, bone: usize, has: impl Fn(usize) -> bool) -> Option<usize> {
    let mut b = Some(bone);
    while let Some(i) = b {
        if has(i) {
            return Some(i);
        }
        b = model.skeleton.bones[i].parent;
    }
    None
}

fn shape_geometry(
    model: &Model,
    shape: &Shape,
    mesh: &SceneMesh,
    world: &[Mat4],
    report: &mut GeometryReport,
) -> Result<ShapeGeometry> {
    let vb = &model.vertex_buffers[shape.vertex_buffer];
    let bones = &model.skeleton.bones;
    let n = mesh.positions.len();
    let skin = shape.skin_count as usize;

    // Skinning in the shape's terms: per vertex the matrix indices and weights of its slots, and the bone whose
    // space it is stored in (rigid and unskinned).
    let mut slots: Vec<Vec<(f32, f32)>> = vec![Vec::new(); n];
    let mut space: Vec<Option<usize>> = vec![None; n];
    let weight_max = vb
        .attribute("_w0")
        .and_then(|a| a.format.size().zip(a.format.components()))
        .map_or(
            255,
            |(size, comps)| if size / comps >= 2 { 65535 } else { 255 },
        );
    match skin {
        0 => space.iter_mut().for_each(|s| *s = Some(shape.bone)),
        1 => {
            for v in 0..n {
                let strongest = mesh.influences[v].first().map_or(shape.bone, |x| x.0);
                let bone = nearest_with(model, strongest, |b| bones[b].rigid_matrix.is_some())
                    .ok_or_else(|| {
                        Error(format!(
                            "{}: bone {} has no rigid matrix, nor do its parents",
                            shape.name, bones[strongest].name
                        ))
                    })?;
                if bone != strongest {
                    report.warn(format!(
                        "{}: {} can't hold a rigid mesh; its vertices follow {}",
                        shape.name, bones[strongest].name, bones[bone].name
                    ));
                }
                slots[v] = vec![(bones[bone].rigid_matrix.expect("checked") as f32, 1.0)];
                space[v] = Some(bone);
            }
        }
        _ => {
            for (v, slot) in slots.iter_mut().enumerate() {
                let mut merged: Vec<(usize, f32)> = Vec::new();
                for &(b, w) in &mesh.influences[v] {
                    let to = nearest_with(model, b, |x| bones[x].smooth_matrix.is_some())
                        .ok_or_else(|| {
                            Error(format!(
                                "{}: {} has no smooth matrix, nor do its parents",
                                shape.name, bones[b].name
                            ))
                        })?;
                    if to != b {
                        report.warn(format!(
                            "{}: {} isn't skinned in the game; its weights went to {}",
                            shape.name, bones[b].name, bones[to].name
                        ));
                    }
                    match merged.iter_mut().find(|x| x.0 == to) {
                        Some(x) => x.1 += w,
                        None => merged.push((to, w)),
                    }
                }
                merged.sort_by(|a, b| b.1.total_cmp(&a.1));
                if merged.len() > skin {
                    merged.truncate(skin);
                }
                if merged.is_empty() {
                    report.warn(format!(
                        "{}: vertices without weights follow {}",
                        shape.name, bones[shape.bone].name
                    ));
                    let b = nearest_with(model, shape.bone, |x| bones[x].smooth_matrix.is_some())
                        .unwrap_or(0);
                    merged.push((b, 1.0));
                }
                let total: f32 = merged.iter().map(|x| x.1).sum();
                let weights: Vec<f32> = merged.iter().map(|x| x.1 / total).collect();
                let q = quantize_weights(&weights, weight_max);
                *slot = merged
                    .iter()
                    .zip(q)
                    .filter(|(_, w)| *w > 0.0)
                    .map(|(&(b, _), w)| (bones[b].smooth_matrix.expect("checked") as f32, w))
                    .collect();
            }
        }
    }

    // Positions and directions where the shape keeps them.
    let inverse: BTreeMap<usize, Mat4> = space
        .iter()
        .flatten()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|&b| {
            Ok((
                b,
                math::inverse_affine(&world[b])
                    .ok_or_else(|| Error("bone matrix can't be inverted".into()))?,
            ))
        })
        .collect::<Result<_>>()?;
    let local =
        |v: usize, p: [f32; 3]| space[v].map_or(p, |b| math::transform_point(&inverse[&b], p));
    let local_dir =
        |v: usize, d: [f32; 3]| space[v].map_or(d, |b| transform_direction(&inverse[&b], d));
    // Normals and tangents only where the game's buffer has them.
    let needs_normals = ["_n0", "_t0", "_b0"]
        .iter()
        .any(|n| vb.attribute(n).is_some());
    let normals: Vec<[f32; 3]> = match &mesh.normals {
        Some(nv) => nv.clone(),
        None if needs_normals => {
            report.warn(format!(
                "{}: the scene has no normals; they are made flat",
                shape.name
            ));
            flat_normals(mesh)
        }
        None => vec![[0.0, 0.0, 1.0]; n],
    };
    let tangents = match &mesh.tangents {
        Some(t) => t.clone(),
        None if vb.attribute("_t0").is_some() || vb.attribute("_b0").is_some() => {
            compute_tangents(mesh, &normals)
        }
        None => vec![[1.0, 0.0, 0.0, 1.0]; n],
    };

    let mut attributes = BTreeMap::new();
    for a in &vb.attributes {
        let name = a.name.as_str();
        let values: Vec<[f32; 4]> = match name {
            "_p0" => (0..n)
                .map(|v| {
                    let p = local(v, mesh.positions[v]);
                    [p[0], p[1], p[2], 1.0]
                })
                .collect(),
            "_n0" => (0..n)
                .map(|v| {
                    let d = local_dir(v, normals[v]);
                    [d[0], d[1], d[2], 1.0]
                })
                .collect(),
            "_t0" => (0..n)
                .map(|v| {
                    let d = local_dir(v, [tangents[v][0], tangents[v][1], tangents[v][2]]);
                    [d[0], d[1], d[2], tangents[v][3]]
                })
                .collect(),
            "_b0" => (0..n)
                .map(|v| {
                    let nn = local_dir(v, normals[v]);
                    let t = local_dir(v, [tangents[v][0], tangents[v][1], tangents[v][2]]);
                    let b = cross(nn, t).map(|c| c * tangents[v][3]);
                    [b[0], b[1], b[2], 1.0]
                })
                .collect(),
            _ if name.starts_with("_u") => {
                let set: usize = name[2..].parse().unwrap_or(usize::MAX);
                match mesh.uvs.get(set) {
                    Some(uv) => {
                        if a.format.kind() == 1
                            && uv
                                .iter()
                                .any(|t| t.iter().any(|c| !(0.0..=1.0).contains(c)))
                        {
                            report.warn(format!(
                                "{}: UV map {set} goes outside 0..1, which the game's format for it can't hold: clamped",
                                shape.name
                            ));
                        }
                        uv.iter().map(|t| [t[0], t[1], 0.0, 0.0]).collect()
                    }
                    None => {
                        report.warn(format!(
                            "{}: the scene has no UV map {set}; it is all 0",
                            shape.name
                        ));
                        vec![[0.0; 4]; n]
                    }
                }
            }
            _ if name.starts_with("_c") => {
                let set: usize = name[2..].parse().unwrap_or(usize::MAX);
                match mesh.colors.get(set) {
                    Some(c) => c
                        .iter()
                        .map(|c| {
                            [
                                math::linear_to_srgb(c[0]),
                                math::linear_to_srgb(c[1]),
                                math::linear_to_srgb(c[2]),
                                c[3],
                            ]
                        })
                        .collect(),
                    None => vec![[1.0; 4]; n],
                }
            }
            "_i0" | "_i1" | "_w0" | "_w1" => {
                let first = if name.ends_with('1') { 4 } else { 0 };
                let weight = name.starts_with("_w");
                (0..n)
                    .map(|v| {
                        let mut out = [0f32; 4];
                        for (c, o) in out.iter_mut().enumerate() {
                            if let Some(&(m, w)) = slots[v].get(first + c) {
                                *o = if weight { w } else { m };
                            }
                        }
                        out
                    })
                    .collect()
            }
            _ => {
                report.warn(format!(
                    "{}: attribute {name} isn't made from the scene; it is all 0",
                    shape.name
                ));
                vec![[0.0; 4]; n]
            }
        };
        attributes.insert(a.name.clone(), values);
    }

    // Every LOD gets the mesh; sub-meshes split its triangles in order.
    let triangles = mesh.indices.len() / 3;
    let lods = shape
        .meshes
        .iter()
        .map(|m| {
            let k = m.sub_meshes.len().max(1);
            let counts = (0..k)
                .map(|i| ((triangles * (i + 1) / k - triangles * i / k) * 3) as u32)
                .collect();
            LodGeometry {
                first_vertex: 0,
                indices: mesh.indices.clone(),
                sub_mesh_counts: counts,
            }
        })
        .collect();
    Ok(ShapeGeometry {
        vertex_count: n as u32,
        attributes,
        lods,
    })
}

fn flat_normals(mesh: &SceneMesh) -> Vec<[f32; 3]> {
    let mut out = vec![[0f32; 3]; mesh.positions.len()];
    for t in mesh.indices.chunks(3) {
        let [a, b, c] = [t[0] as usize, t[1] as usize, t[2] as usize];
        let (p0, p1, p2) = (mesh.positions[a], mesh.positions[b], mesh.positions[c]);
        let nn = cross(
            [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]],
            [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]],
        );
        for &v in &[a, b, c] {
            for i in 0..3 {
                out[v][i] += nn[i];
            }
        }
    }
    out.into_iter().map(math::normalize3).collect()
}

/// A shape removed: one vertex, every LOD one empty triangle.
fn removed_shape(model: &Model, shape: &Shape) -> ShapeGeometry {
    let vb = &model.vertex_buffers[shape.vertex_buffer];
    let attributes = vb
        .attributes
        .iter()
        .map(|a| (a.name.clone(), vec![[0.0; 4]]))
        .collect();
    let lods = shape
        .meshes
        .iter()
        .map(|m| {
            let mut counts = vec![0u32; m.sub_meshes.len().max(1)];
            counts[0] = 3;
            LodGeometry {
                first_vertex: 0,
                indices: vec![0, 0, 0],
                sub_mesh_counts: counts,
            }
        })
        .collect();
    ShapeGeometry {
        vertex_count: 1,
        attributes,
        lods,
    }
}

/// The geometry of every shape of `model` from the scene: replaced where the scene has a mesh of its name,
/// removed where it doesn't.
pub fn geometry_from_gltf(
    model: &Model,
    gltf: &Gltf,
) -> Result<(BTreeMap<usize, ShapeGeometry>, GeometryReport)> {
    let mut report = GeometryReport::default();
    let (_, game_world) = bone_matrices(model)?;
    let scene_world = gltf.world_matrices()?;
    let bones: BTreeMap<&str, usize> = model
        .skeleton
        .bones
        .iter()
        .enumerate()
        .map(|(i, b)| (b.name.as_str(), i))
        .collect();
    let mut by_shape: BTreeMap<usize, SceneMesh> = BTreeMap::new();
    for node in gltf.mesh_nodes() {
        let name = plain_name(&node.name);
        let Some(si) = model
            .shapes
            .iter()
            .position(|s| s.name == node.name || s.name == name)
        else {
            report.warn(format!(
                "{}: no game mesh of that name; left out (new meshes can't be added)",
                node.name
            ));
            continue;
        };
        if by_shape.contains_key(&si) {
            report.warn(format!(
                "{}: more than one scene mesh for it; the first is used",
                model.shapes[si].name
            ));
            continue;
        }
        by_shape.insert(
            si,
            read_mesh(gltf, &node, &scene_world, &bones, &mut report)?,
        );
    }
    let mut out = BTreeMap::new();
    for (si, shape) in model.shapes.iter().enumerate() {
        if shape.is_meshlet() {
            continue;
        }
        let (g, status) = match by_shape.get(&si) {
            Some(mesh) => (
                shape_geometry(model, shape, mesh, &game_world, &mut report)?,
                "replaced",
            ),
            None => {
                report.warn(format!("{}: not in the scene; removed", shape.name));
                (removed_shape(model, shape), "removed")
            }
        };
        report.shapes.push(ShapeReport {
            name: shape.name.clone(),
            status: status.to_owned(),
            vertices: g.vertex_count,
            triangles: g.lods.first().map_or(0, |l| l.indices.len() as u32 / 3),
        });
        out.insert(si, g);
    }
    Ok((out, report))
}

#[derive(Debug, Serialize)]
pub struct ReplaceSummary {
    pub model: PathBuf,
    #[serde(flatten)]
    pub geometry: GeometryReport,
}

/// Rebuilds `model_path` (relative to the romfs) from `glb` and writes it, compressed, to the same path under
/// `out_romfs`.
pub fn replace_model(
    romfs: &Romfs,
    model_path: &Path,
    glb: &Path,
    out_romfs: &Path,
) -> Result<ReplaceSummary> {
    let mut decoder = romfs.decoder()?;
    let loaded = Loaded::read(romfs, &mut decoder, model_path)?;
    let bfres = loaded.bfres().context(model_path.display())?;
    let model = bfres
        .models
        .first()
        .ok_or_else(|| Error("no model in the file".into()))?;
    let gltf = Gltf::parse(&romfs::read_file(glb)?).context(glb.display())?;
    let (shapes, report) = geometry_from_gltf(model, &gltf)?;
    let data = bfres_write::write(&bfres, &shapes).context("writing the model")?;
    // Read back what was written: it must parse.
    Bfres::parse(&data, Some(&loaded.strings)).context("reading the written model back")?;
    let out = out_romfs.join(model_path);
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).context(dir.display())?;
    }
    let compressed = modstudio_formats::zs::compress(&data);
    std::fs::write(&out, compressed).context(out.display())?;
    Ok(ReplaceSummary {
        model: out,
        geometry: report,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_add_up_in_whole_steps() {
        for ws in [
            vec![0.5, 0.3, 0.2],
            vec![1.0 / 3.0; 3],
            vec![0.999, 0.001],
            vec![0.25; 4],
            vec![1.0],
        ] {
            let q = quantize_weights(&ws, 255);
            let steps: Vec<u32> = q.iter().map(|w| (w * 255.0).round() as u32).collect();
            assert_eq!(steps.iter().sum::<u32>(), 255, "{ws:?} -> {steps:?}");
            for (a, b) in q.iter().zip(&ws) {
                assert!((a - b).abs() <= 1.0 / 255.0);
            }
        }
    }

    #[test]
    fn blender_suffixes() {
        assert_eq!(plain_name("Body__Mt_Skin.001"), "Body__Mt_Skin");
        assert_eq!(plain_name("Body__Mt_Skin"), "Body__Mt_Skin");
        assert_eq!(plain_name("v1.2"), "v1.2");
    }
}
