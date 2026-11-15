//! Reads every meshlet shape in the game with [`modstudio_formats::meshlet`] and checks what the reader assumes:
//! each meshlet's vertex bits are accounted for, its indices are in range, the data blocks follow one another, the
//! positions sit in each meshlet's box, normals agree with the faces, the tangent sign with the UVs. Prints the
//! values of the fields that aren't understood, so a change in them shows.
//!
//!     cargo run --release -p modstudio-formats --example meshlet_decode -- <romfs> [name filter]

use std::collections::BTreeMap;
use std::path::Path;

use modstudio_formats::bfres::{Bfres, ExternalStrings};
use modstudio_formats::meshlet::{self, MeshletShape};
use modstudio_formats::romfs::Romfs;

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
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
fn unit(a: [f32; 3]) -> [f32; 3] {
    let l = dot(a, a).sqrt();
    if l == 0.0 {
        a
    } else {
        [a[0] / l, a[1] / l, a[2] / l]
    }
}

#[derive(Default)]
struct Counts(BTreeMap<String, usize>);
impl Counts {
    fn add(&mut self, k: impl Into<String>) {
        *self.0.entry(k.into()).or_default() += 1;
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(root) = args.first() else {
        eprintln!("usage: meshlet_decode <romfs> [name filter]");
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
    let (mut shapes, mut read, mut meshlets) = (0, 0, 0usize);
    let mut failures = Vec::new();
    let mut unknown = Counts::default();
    let mut checks = Counts::default();
    let (mut normal_dot, mut normal_n) = (0f64, 0usize);
    let mut uv_base_hi = Counts::default();
    let mut colours = (f32::MAX, f32::MIN, 0usize);
    let mut by_flags = Counts::default();
    for f in &files {
        let Ok(data) = romfs.read(&mut d, format!("Model/{f}")) else {
            continue;
        };
        let Ok(b) = Bfres::parse(&data, Some(&strings)) else {
            continue;
        };
        for m in &b.models {
            for s in m.shapes.iter().filter(|s| s.is_meshlet()) {
                shapes += 1;
                let ms = match MeshletShape::read(s) {
                    Ok(ms) => ms,
                    Err(e) => {
                        failures.push(format!("{f} {}: {e}", s.name));
                        continue;
                    }
                };
                read += 1;
                meshlets += ms.meshlets.len();
                by_flags.add(format!("{:#07x}", ms.flags));
                unknown.add(format!(
                    "header flags outside the known ones: {:#x}",
                    ms.flags & !0x13c61
                ));
                unknown.add(format!("step {}", ms.step));
                // the data blocks: right after the records, one after another
                let records_end =
                    meshlet::HEADER_SIZE + ms.meshlets.len() * meshlet::record_size(ms.flags);
                checks.add(format!(
                    "first data right after the records: {}",
                    ms.meshlets.first().is_none_or(|m| m.data_at == records_end)
                ));
                for (i, me) in ms.meshlets.iter().enumerate() {
                    let w = &me.record;
                    unknown.add(format!(
                        "w7 {}, tangents {}",
                        w[7],
                        ms.flags & meshlet::flags::TANGENTS != 0
                    ));
                    unknown.add(format!("w6 bit 31: {}", w[6] >> 31));
                    checks.add(format!("normal bits {:2}", me.normal_bits));
                    unknown.add(format!("w3 top 2 bits: {}", w[3] >> 30));
                    for u in 0..me.uv_sets.len() {
                        unknown.add(format!(
                            "uv form low 24 bits {:#08x}",
                            w[9 + 2 * u] & 0xff_ffff
                        ));
                        for base in me.uv_sets[u].base {
                            uv_base_hi.add(format!("{:x}", (base as u16) >> 12));
                        }
                    }
                    let colour_at = 8 + 2 * me.uv_sets.len();
                    for c in 0..me.colour_sets.len() {
                        unknown.add(format!(
                            "colour mask word's top 24 bits {:#x}",
                            w[colour_at + 2 * c] >> 8
                        ));
                    }
                    if w[4] & 0xffff == 0 {
                        unknown.add(format!("no cone axis: cutoff {}", (w[6] >> 9) & 0x7f));
                    }
                    let (_, end) = me.index_span();
                    if let Some(next) = ms.meshlets.get(i + 1) {
                        checks.add(format!(
                            "next data at the byte after: {}",
                            next.data_at == end.div_ceil(8)
                        ));
                    } else {
                        let slack = ms.data.len() as i64 - end.div_ceil(8) as i64;
                        unknown.add(format!(
                            "bytes after the last meshlet: {}",
                            if slack < 0x40 {
                                slack.to_string()
                            } else {
                                "0x40 or more".into()
                            }
                        ));
                    }
                    checks.add(format!("vertices <= 64: {}", me.vertex_count <= 64));
                    checks.add(format!("triangles <= 124: {}", me.triangle_count <= 124));
                }
                for lod in 0..ms.lods.len() {
                    let g = match ms.geometry(lod) {
                        Ok(g) => g,
                        Err(e) => {
                            failures.push(format!("{f} {} LOD {lod}: {e}", s.name));
                            continue;
                        }
                    };
                    if lod > 0 {
                        continue;
                    }
                    // offsets: from 0 up to the extent the record gives (half its doubled extent)
                    let mut v0 = 0;
                    for me in &ms.meshlets[ms.lods[0].clone()] {
                        let w = &me.record;
                        let big = w[0] as u128
                            | (w[1] as u128) << 32
                            | (w[2] as u128) << 64
                            | (w[3] as u128) << 96;
                        for k in 0..3 {
                            let ext2 = ((big >> (63 + 21 * k)) & 0x1f_ffff) as i64;
                            let offs: Vec<i64> = g.positions[v0..v0 + me.vertex_count]
                                .iter()
                                .map(|p| {
                                    ((p[k] - ms.bounds_min[k]) / ms.step).round() as i64
                                        - me.position_base[k] as i64
                                })
                                .collect();
                            let lo = offs.iter().min().copied().unwrap_or(0);
                            let hi = offs.iter().max().copied().unwrap_or(0);
                            checks.add(format!("smallest offset 0: {}", lo == 0));
                            checks.add(format!(
                                "doubled extent = 2 x largest offset: {}",
                                ext2 == 2 * hi
                            ));
                        }
                        v0 += me.vertex_count;
                    }
                    // UV scales: vertices two meshlets share (same grid position) have the same UVs (seams aside),
                    // whether or not the meshlets' UV forms (scales) differ
                    {
                        let mut owner = Vec::new();
                        for me in &ms.meshlets[ms.lods[0].clone()] {
                            owner.extend(std::iter::repeat_n(me, me.vertex_count));
                        }
                        let mut at: BTreeMap<[i64; 3], Vec<usize>> = BTreeMap::new();
                        for (v, p) in g.positions.iter().enumerate() {
                            let key = std::array::from_fn(|k| {
                                ((p[k] - ms.bounds_min[k]) / ms.step).round() as i64
                            });
                            at.entry(key).or_default().push(v);
                        }
                        for vs in at.values() {
                            for (x, &a) in vs.iter().enumerate() {
                                for &b2 in &vs[x + 1..] {
                                    let (ma, mb) = (owner[a], owner[b2]);
                                    for set in
                                        0..g.uvs.len().min(ma.uv_sets.len()).min(mb.uv_sets.len())
                                    {
                                        let fa = ma.record[9 + 2 * set] & 0xff_ffff;
                                        let fb = mb.record[9 + 2 * set] & 0xff_ffff;
                                        let same = (0..2).all(|c| {
                                            (g.uvs[set][a][c] - g.uvs[set][b2][c]).abs() < 1e-3
                                        });
                                        let forms = if fa == fb {
                                            "the same UV form"
                                        } else {
                                            "UV forms that differ"
                                        };
                                        checks.add(format!("vertex two meshlets share, with {forms}: same UV (seams aside) {same}"));
                                    }
                                }
                            }
                        }
                    }
                    let uv = g.uvs.first();
                    for t in g.indices.chunks(3) {
                        let [a, b2, c] = [t[0] as usize, t[1] as usize, t[2] as usize];
                        let (e1, e2) = (
                            sub(g.positions[b2], g.positions[a]),
                            sub(g.positions[c], g.positions[a]),
                        );
                        let face = cross(e1, e2);
                        if dot(face, face) == 0.0 {
                            continue;
                        }
                        let face = unit(face);
                        for &v in &[a, b2, c] {
                            normal_dot += dot(face, g.normals[v]) as f64;
                            normal_n += 1;
                        }
                        // the tangent sign: the bitangent the UVs give, against cross(normal, tangent) * w
                        if let (Some(tan), Some(uv)) = (&g.tangents, uv) {
                            let (du1, dv1) = (uv[b2][0] - uv[a][0], uv[b2][1] - uv[a][1]);
                            let (du2, dv2) = (uv[c][0] - uv[a][0], uv[c][1] - uv[a][1]);
                            let det = du1 * dv2 - du2 * dv1;
                            if det.abs() < 1e-12 {
                                continue;
                            }
                            let bit: [f32; 3] =
                                std::array::from_fn(|k| (e2[k] * du1 - e1[k] * du2) / det);
                            let t = tan[a];
                            let bc = cross(g.normals[a], [t[0], t[1], t[2]]);
                            let agree = dot(bc, bit) * t[3] > 0.0;
                            checks.add(format!("bitangent = cross(n, t) * w: {agree}"));
                        }
                    }
                    for (_, values) in &g.colours {
                        for v in values {
                            for &x in v {
                                colours.0 = colours.0.min(x);
                                colours.1 = colours.1.max(x);
                                colours.2 += !x.is_finite() as usize;
                            }
                        }
                    }
                }
            }
        }
    }
    println!(
        "{shapes} meshlet shapes, {read} read ({meshlets} meshlets); flags: {:?}",
        by_flags.0
    );
    for e in failures.iter().take(20) {
        println!("  FAILED {e}");
    }
    println!(
        "normals: mean cosine with the faces {:.3} over {normal_n} corners",
        normal_dot / normal_n.max(1) as f64
    );
    println!(
        "colours: from {} to {}, {} not finite",
        colours.0, colours.1, colours.2
    );
    println!("UV bases' top hex digit: {:?}", uv_base_hi.0);
    println!("checks:");
    for (k, n) in &checks.0 {
        println!("  {n:9}  {k}");
    }
    println!("fields not understood:");
    for (k, n) in &unknown.0 {
        println!("  {n:9}  {k}");
    }
}
