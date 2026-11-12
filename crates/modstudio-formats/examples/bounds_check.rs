//! Prints a shape's stored bounding boxes and spheres next to values computed from its vertices, to work out how
//! the game computes them (local analysis).
//!
//! `cargo run --release --example bounds_check -- <romfs> <model.bfres.zs> <shape index>`

use modstudio_formats::bfres::{Bfres, ExternalStrings};
use modstudio_formats::romfs::Romfs;

fn aabb(points: impl Iterator<Item = [f32; 3]>) -> ([f32; 3], [f32; 3]) {
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for p in points {
        for c in 0..3 {
            lo[c] = lo[c].min(p[c]);
            hi[c] = hi[c].max(p[c]);
        }
    }
    let center = [0, 1, 2].map(|c| (lo[c] + hi[c]) / 2.0);
    let extent = [0, 1, 2].map(|c| (hi[c] - lo[c]) / 2.0);
    (center, extent)
}

type V = [f64; 3];

fn sub(a: V, b: V) -> V {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: V, b: V) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V, b: V) -> V {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// The sphere through up to 4 boundary points (centre, radius squared).
fn sphere(b: &[V]) -> (V, f64) {
    match b.len() {
        0 => ([0.0; 3], -1.0),
        1 => (b[0], 0.0),
        2 => {
            let c = [0, 1, 2].map(|i| (b[0][i] + b[1][i]) / 2.0);
            (c, dot(sub(b[0], c), sub(b[0], c)))
        }
        3 => {
            let (a, bb) = (sub(b[1], b[0]), sub(b[2], b[0]));
            let n = cross(a, bb);
            let d = 2.0 * dot(n, n);
            let t =
                [0, 1, 2].map(|i| (dot(a, a) * cross(bb, n)[i] + dot(bb, bb) * cross(n, a)[i]) / d);
            let c = [0, 1, 2].map(|i| b[0][i] + t[i]);
            (c, dot(t, t))
        }
        _ => {
            let (a, bb, cc) = (sub(b[1], b[0]), sub(b[2], b[0]), sub(b[3], b[0]));
            let d = 2.0 * dot(a, cross(bb, cc));
            let t = [0, 1, 2].map(|i| {
                (dot(a, a) * cross(bb, cc)[i]
                    + dot(bb, bb) * cross(cc, a)[i]
                    + dot(cc, cc) * cross(a, bb)[i])
                    / d
            });
            let c = [0, 1, 2].map(|i| b[0][i] + t[i]);
            (c, dot(t, t))
        }
    }
}

/// Smallest enclosing sphere (Welzl, iterative with move-to-front).
fn welzl(points: &[V]) -> (V, f64) {
    let inside = |s: &(V, f64), p: V| dot(sub(p, s.0), sub(p, s.0)) <= s.1 * (1.0 + 1e-12) + 1e-12;
    let mut s = sphere(&[]);
    for i in 0..points.len() {
        if inside(&s, points[i]) {
            continue;
        }
        s = sphere(&[points[i]]);
        for j in 0..i {
            if inside(&s, points[j]) {
                continue;
            }
            s = sphere(&[points[i], points[j]]);
            for k in 0..j {
                if inside(&s, points[k]) {
                    continue;
                }
                s = sphere(&[points[i], points[j], points[k]]);
                for l in 0..k {
                    if inside(&s, points[l]) {
                        continue;
                    }
                    s = sphere(&[points[i], points[j], points[k], points[l]]);
                }
            }
        }
    }
    (s.0, s.1.max(0.0).sqrt())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [romfs, model, shape] = args.as_slice() else {
        eprintln!("usage: bounds_check <romfs> <model> <shape>");
        std::process::exit(2);
    };
    let romfs = Romfs::open(romfs).expect("romfs");
    let mut d = romfs.decoder().unwrap();
    let strings = ExternalStrings::parse(
        &romfs
            .read(&mut d, "Shader/ExternalBinaryString.bfres.zs")
            .unwrap(),
    )
    .unwrap();
    let data = romfs.read(&mut d, model).expect("read");
    let bfres = Bfres::parse(&data, Some(&strings)).expect("bfres");
    let m = &bfres.models[0];
    let s = &m.shapes[shape.parse::<usize>().unwrap()];
    let vb = &m.vertex_buffers[s.vertex_buffer];
    let pos = vb.read(vb.attribute("_p0").unwrap()).unwrap();
    let p3 = |i: u32| {
        let v = pos[i as usize];
        [v[0], v[1], v[2]]
    };
    println!(
        "{} skin {} bone {} skin bones {:?}",
        s.name, s.skin_count, s.bone, s.skin_bone_indices
    );
    println!("stored boundings ({}):", s.boundings.len());
    for b in &s.boundings {
        println!("   c {:?} e {:?}", b.center, b.extent);
    }
    for (l, mesh) in s.meshes.iter().enumerate().take(3) {
        let idx = mesh.indices().unwrap();
        let (c, e) = aabb(idx.iter().map(|&i| p3(i)));
        println!(
            "LOD {l}: {} indices, first vertex {}, sub meshes {:?}",
            idx.len(),
            mesh.first_vertex,
            mesh.sub_meshes
        );
        println!("   computed AABB c {c:?} e {e:?}");
        for &(off, count) in &mesh.sub_meshes {
            let size = if mesh.index_format == 2 { 4 } else { 2 };
            let start = off as usize / size;
            let (c, e) = aabb(idx[start..start + count as usize].iter().map(|&i| p3(i)));
            println!("   sub mesh @{off} n{count}: c {c:?} e {e:?}");
        }
    }
    println!("stored spheres ({}):", s.spheres.len());
    for sp in &s.spheres {
        println!("   {sp:?}");
    }
    // Candidates, in each bone's local space (the stored inverse model matrices, 3 rows of 4).
    let (Some(i0), Some(w0)) = (vb.attribute("_i0"), vb.attribute("_w0")) else {
        return;
    };
    let (i0, w0) = (vb.read(i0).unwrap(), vb.read(w0).unwrap());
    let local = |b: usize, p: [f32; 3]| -> [f32; 3] {
        let mi = m.skeleton.bones[b].smooth_matrix.expect("smooth bone");
        let r = &m.skeleton.inverse_model_matrices[mi];
        [0, 1, 2].map(|row| {
            r[row * 4] * p[0] + r[row * 4 + 1] * p[1] + r[row * 4 + 2] * p[2] + r[row * 4 + 3]
        })
    };
    let influences = |v: u32| -> Vec<(usize, f32)> {
        (0..(s.skin_count as usize).min(4))
            .map(|k| {
                (
                    m.skeleton.matrix_to_bone[i0[v as usize][k] as usize] as usize,
                    w0[v as usize][k],
                )
            })
            .collect()
    };
    for (label, verts) in [
        (
            "LOD 0",
            s.meshes[0]
                .indices()
                .unwrap()
                .into_iter()
                .collect::<std::collections::BTreeSet<u32>>(),
        ),
        ("all vertices", (0..vb.vertex_count).collect()),
    ] {
        println!("-- {label}");
        // Box: union over every (vertex, bone) of the vertex in that bone's space; with and without zero weights.
        for zero in [false, true] {
            let pts: Vec<[f32; 3]> = verts
                .iter()
                .flat_map(|&v| {
                    influences(v)
                        .into_iter()
                        .filter(move |&(_, w)| zero || w > 0.0)
                        .map(move |(b, _)| (v, b))
                })
                .map(|(v, b)| local(b, p3(v)))
                .collect();
            let (c, e) = aabb(pts.iter().copied());
            println!("   union box (zero weights {zero}): c {c:?} e {e:?}");
        }
        for (k, &b) in s.skin_bone_indices.iter().take(4).enumerate() {
            let pts: Vec<[f32; 3]> = verts
                .iter()
                .filter(|&&v| {
                    influences(v)
                        .iter()
                        .any(|&(bb, w)| bb == b as usize && w > 0.0)
                })
                .map(|&v| local(b as usize, p3(v)))
                .collect();
            let (c, e) = aabb(pts.iter().copied());
            let r = pts
                .iter()
                .map(|p| {
                    ((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2) + (p[2] - c[2]).powi(2)).sqrt()
                })
                .fold(0.0f32, f32::max);
            let diag = (e[0] * e[0] + e[1] * e[1] + e[2] * e[2]).sqrt();
            let pts64: Vec<V> = pts.iter().map(|p| p.map(f64::from)).collect();
            let (wc, wr) = welzl(&pts64);
            println!("      smallest sphere {wc:.6?} r {wr:.6}");
            println!(
                "   bone {b} {}: {} verts, centre {c:?} max dist {r} half diagonal {diag}   stored {:?}",
                m.skeleton.bones[b as usize].name,
                pts.len(),
                s.spheres[k]
            );
        }
    }
}
