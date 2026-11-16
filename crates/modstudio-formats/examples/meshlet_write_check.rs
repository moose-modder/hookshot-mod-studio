//! Writes every meshlet model in the game back with [`modstudio_formats::bfres_write::write_meshlets`]: unchanged (it
//! must give the file's bytes), then with every shape given its own geometry, each LOD as the reader decodes it with
//! the vertices meshlets share made one. The written file is read again and compared: every LOD's triangles (their
//! corners' positions and tangent signs the same; UVs and colours the same, or within what a meshlet of other
//! vertices holds them to; normals and tangents within the precision written), the header's minimum, the shape's box
//! and sphere against the game's; and the buffer's size against the game's. Counts only.
//!
//!     cargo run --release -p modstudio-formats --example meshlet_write_check -- <romfs> [name filter]

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use modstudio_formats::bfres::{Bfres, ExternalStrings};
use modstudio_formats::bfres_write::write_meshlets;
use modstudio_formats::meshlet::{MeshletGeometry, MeshletShape};
use modstudio_formats::romfs::Romfs;

#[derive(Default)]
struct Counts(BTreeMap<String, usize>);
impl Counts {
    fn add(&mut self, k: impl Into<String>) {
        *self.0.entry(k.into()).or_default() += 1;
    }
}

fn f32_at(d: &[u8], at: usize) -> f32 {
    f32::from_le_bytes(d[at..at + 4].try_into().unwrap())
}

/// A corner: position steps from the minimum, UV bits, colour bits, the tangent's sign.
type Corner = (Vec<i64>, Vec<u32>, Vec<u32>, i8);

/// A triangle's corners, and their normals and tangents.
type Tri = (Vec<Corner>, Vec<[f32; 3]>, Vec<[f32; 3]>);

/// Each triangle's corners, its smallest corner first (the winding kept), sorted; and per triangle its normals and
/// tangents in the same order.
fn triangles(g: &MeshletGeometry, min: [f32; 3], step: f32) -> Vec<Tri> {
    let corner = |v: usize| -> Corner {
        (
            (0..3)
                .map(|k| ((g.positions[v][k] - min[k]) / step).round() as i64)
                .collect(),
            g.uvs.iter().flat_map(|u| u[v]).map(f32::to_bits).collect(),
            g.colours
                .iter()
                .flat_map(|c| c.1[v])
                .map(f32::to_bits)
                .collect(),
            g.tangents.as_ref().map_or(0, |t| t[v][3] as i8),
        )
    };
    let mut out: Vec<Tri> = g
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .filter(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2])
        .map(|t| {
            let vs = [t[0] as usize, t[1] as usize, t[2] as usize];
            let cs: Vec<Corner> = vs.iter().map(|&v| corner(v)).collect();
            let first = (0..3).min_by(|&a, &b| cs[a].cmp(&cs[b])).unwrap();
            let order: Vec<usize> = (0..3).map(|k| (first + k) % 3).collect();
            (
                order.iter().map(|&k| cs[k].clone()).collect(),
                order.iter().map(|&k| g.normals[vs[k]]).collect(),
                order
                    .iter()
                    .map(|&k| {
                        g.tangents
                            .as_ref()
                            .map_or([0.0; 3], |t| [t[vs[k]][0], t[vs[k]][1], t[vs[k]][2]])
                    })
                    .collect(),
            )
        })
        .collect();
    let key = |n: &[[f32; 3]]| -> Vec<i32> {
        n.iter()
            .flatten()
            .map(|x| (x * 8.0).round() as i32)
            .collect()
    };
    out.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| key(&a.1).cmp(&key(&b.1)))
            .then_with(|| key(&a.2).cmp(&key(&b.2)))
    });
    out
}

fn angle(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2]).clamp(-1.0, 1.0);
    d.acos().to_degrees()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(root) = args.first() else {
        eprintln!("usage: meshlet_write_check <romfs> [name filter]");
        std::process::exit(2);
    };
    let filter = args.get(1).cloned().unwrap_or_default();
    let romfs = Romfs::open(root).expect("romfs");
    let mut d = romfs.decoder().unwrap();
    let strings = ExternalStrings::parse(
        &romfs
            .read(&mut d, "Shader/ExternalBinaryString.bfres.zs")
            .unwrap(),
    )
    .unwrap();
    let mut files: Vec<String> = std::fs::read_dir(Path::new(root).join("Model"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| {
            n.ends_with(".bfres.zs") && !n.contains(".anim.") && n.contains(filter.as_str())
        })
        .collect();
    files.sort();
    let mut c = Counts::default();
    let mut failures = Vec::new();
    let (mut game_bytes, mut our_bytes, mut game_meshlets, mut our_meshlets) =
        (0usize, 0usize, 0usize, 0usize);
    let (mut worst_normal, mut worst_tangent) = (0f32, 0f32);
    let (mut worst_uv, mut worst_colour) = (0f32, 0f32);
    let mut slowest = (0f64, String::new());
    for f in &files {
        let Ok(data) = romfs.read(&mut d, format!("Model/{f}")) else {
            continue;
        };
        let Ok(b) = Bfres::parse(&data, Some(&strings)) else {
            continue;
        };
        let Some(model) = b.models.first() else {
            continue;
        };
        if !model.shapes.iter().any(|s| s.is_meshlet()) {
            continue;
        }
        // unchanged
        match write_meshlets(&b, &BTreeMap::new()) {
            Ok(out) => c.add(format!("written unchanged: the same bytes {}", out == data)),
            Err(e) => {
                failures.push(format!("{f} unchanged: {e}"));
                continue;
            }
        }
        // every shape its own geometry
        let mut replace = BTreeMap::new();
        let mut own = Vec::new();
        for (si, s) in model.shapes.iter().enumerate() {
            let ms = MeshletShape::read(s).unwrap();
            let lods: Vec<MeshletGeometry> = (0..ms.lods.len())
                .map(|k| ms.geometry(k).unwrap().welded())
                .collect();
            game_bytes += ms.data.len();
            game_meshlets += ms.meshlets.len();
            own.push((ms.bounds_min, ms.step, lods.clone()));
            replace.insert(si, lods);
        }
        let t = Instant::now();
        let out = match write_meshlets(&b, &replace) {
            Ok(x) => x,
            Err(e) => {
                failures.push(format!("{f}: {e}"));
                continue;
            }
        };
        let secs = t.elapsed().as_secs_f64();
        if secs > slowest.0 {
            slowest = (secs, f.clone());
        }
        let b2 = match Bfres::parse(&out, Some(&strings)) {
            Ok(x) => x,
            Err(e) => {
                failures.push(format!("{f}: written, not read again: {e}"));
                continue;
            }
        };
        c.add(format!("file size: the same {}", out.len() == data.len()));
        for (si, s) in b2.models[0].shapes.iter().enumerate() {
            let ms = match MeshletShape::read(s) {
                Ok(x) => x,
                Err(e) => {
                    failures.push(format!("{f} shape {si}: {e}"));
                    continue;
                }
            };
            our_bytes += ms.data.len();
            our_meshlets += ms.meshlets.len();
            let (min, step, lods) = &own[si];
            c.add(format!(
                "header minimum the game's: {}",
                ms.bounds_min == *min
            ));
            for (k, game) in lods.iter().enumerate() {
                let ours = ms.geometry(k).unwrap();
                let (a, b) = (triangles(game, *min, *step), triangles(&ours, *min, *step));
                let exact = a.len() == b.len() && a.iter().zip(&b).all(|(x, y)| x.0 == y.0);
                // each written triangle against the game's of the same positions closest to it
                let mut by_pos: BTreeMap<Vec<&Vec<i64>>, Vec<usize>> = BTreeMap::new();
                for (i, x) in a.iter().enumerate() {
                    by_pos
                        .entry(x.0.iter().map(|c| &c.0).collect())
                        .or_default()
                        .push(i);
                }
                let (mut uv, mut colour, mut unmatched) = (0f32, 0f32, a.len() != b.len());
                for y in &b {
                    let Some(xs) = by_pos.get(&y.0.iter().map(|c| &c.0).collect::<Vec<_>>()) else {
                        unmatched = true;
                        continue;
                    };
                    let closest = |f: &dyn Fn(&Tri) -> f32| {
                        xs.iter().map(|&i| f(&a[i])).fold(f32::MAX, f32::min)
                    };
                    let bits_apart = |p: &[u32], q: &[u32]| {
                        p.iter()
                            .zip(q)
                            .map(|(u, v)| (f32::from_bits(*u) - f32::from_bits(*v)).abs())
                            .fold(0.0, f32::max)
                    };
                    let corners = |x: &Tri, part: usize| {
                        (0..3)
                            .map(|k| match part {
                                0 => bits_apart(&x.0[k].1, &y.0[k].1),
                                1 => bits_apart(&x.0[k].2, &y.0[k].2),
                                2 => angle(x.1[k], y.1[k]),
                                3 => angle(x.2[k], y.2[k]),
                                _ => (x.0[k].3 != y.0[k].3) as u8 as f32,
                            })
                            .fold(0.0, f32::max)
                    };
                    uv = uv.max(closest(&|x| corners(x, 0)));
                    colour = colour.max(closest(&|x| corners(x, 1)));
                    worst_normal = worst_normal.max(closest(&|x| corners(x, 2)));
                    if ours.tangents.is_some() {
                        worst_tangent = worst_tangent.max(closest(&|x| corners(x, 3)));
                    }
                    unmatched |= closest(&|x| corners(x, 4)) > 0.0;
                }
                worst_uv = worst_uv.max(uv);
                worst_colour = worst_colour.max(colour);
                // a meshlet of other vertices can hold a UV at a coarser scale (half a step of 2^-9 at most) and
                // colours in bytes where the game's meshlet had f32
                let outcome = if exact {
                    "exact"
                } else if !unmatched && uv <= 2f32.powi(-10) + 1e-6 && colour <= 0.5 / 255.0 + 1e-6
                {
                    "UVs or colours within the precision written"
                } else {
                    "different"
                };
                c.add(format!("LOD triangles: {outcome}"));
                if outcome == "different" && failures.len() < 30 {
                    failures.push(format!(
                        "{f} shape {si} LOD {k}: {} triangles, {} written; UVs up to {uv} apart, colours {colour}",
                        a.len(),
                        b.len()
                    ));
                }
            }
            // bounds against the game's
            let s0 = &model.shapes[si];
            let gb: Vec<f32> = (0..6)
                .map(|k| f32_at(&data, s0.bounding_at + 4 * k))
                .collect();
            let ob: Vec<f32> = (0..6)
                .map(|k| f32_at(&out, s.bounding_at + 4 * k))
                .collect();
            c.add(format!(
                "box within 2 mm of the game's: {}",
                gb.iter().zip(&ob).all(|(a, b)| (a - b).abs() <= 0.002)
            ));
            let gs: Vec<f32> = (0..4)
                .map(|k| f32_at(&data, s0.radius_at + 4 * k))
                .collect();
            let os: Vec<f32> = (0..4).map(|k| f32_at(&out, s.radius_at + 4 * k)).collect();
            let dr = os[3] - gs[3];
            c.add(format!(
                "sphere radius against the game's: {}",
                if dr < -0.001 {
                    "smaller"
                } else if dr <= 0.001 {
                    "within 1 mm"
                } else {
                    "larger"
                }
            ));
        }
    }
    for e in failures.iter().take(30) {
        println!("FAILED {e}");
    }
    for (k, n) in &c.0 {
        println!("  {n:7}  {k}");
    }
    println!(
        "buffers: {our_bytes} bytes written for the game's {game_bytes} ({:.2}x); {our_meshlets} meshlets for {game_meshlets}",
        our_bytes as f64 / game_bytes.max(1) as f64
    );
    println!("largest normal change {worst_normal:.3} degrees, tangent {worst_tangent:.3}");
    println!("largest UV change {worst_uv:.6}, colour {worst_colour:.6}");
    println!("slowest write {:.2} s", slowest.0);
}
