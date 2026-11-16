//! What the game's meshlet buffers choose, to write them alike: how bits and scales follow from the values, how the
//! buffers and the file's GPU section are laid out, how the header's box and the shape's bounds follow from the
//! vertices. Counts only.
//!
//!     cargo run --release -p modstudio-formats --example meshlet_census -- <romfs> [name filter]

use std::collections::BTreeMap;
use std::path::Path;

use modstudio_formats::bfres::{Bfres, ExternalStrings};
use modstudio_formats::meshlet::{self, Channel, MeshletShape, bits};
use modstudio_formats::romfs::Romfs;

#[derive(Default)]
struct Counts(BTreeMap<String, usize>);
impl Counts {
    fn add(&mut self, k: impl Into<String>) {
        *self.0.entry(k.into()).or_default() += 1;
    }
}

fn u32_at(d: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(d[at..at + 4].try_into().unwrap())
}
fn u64_at(d: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(d[at..at + 8].try_into().unwrap())
}
fn f32_at(d: &[u8], at: usize) -> f32 {
    f32::from_le_bytes(d[at..at + 4].try_into().unwrap())
}
fn bitlen(x: u32) -> u32 {
    32 - x.leading_zeros()
}

/// A vertex's fields as stored.
struct Raw {
    pos: [u32; 3],
    normal: [u32; 2],
    tangent: Option<(u32, u32, u32)>,
    colours: Vec<[Option<u32>; 4]>,
    uvs: Vec<[u32; 2]>,
}

fn raw_vertices(s: &MeshletShape, m: &meshlet::Meshlet) -> Vec<Raw> {
    (0..m.vertex_count)
        .map(|v| {
            let mut at = m.data_at * 8 + v * m.vertex_bits as usize;
            let mut take = |w: u32| {
                let x = bits(s.data, at, w);
                at += w as usize;
                x
            };
            let pos = [
                take(m.position_bits[0]),
                take(m.position_bits[1]),
                take(m.position_bits[2]),
            ];
            let normal = [take(m.normal_bits), take(m.normal_bits)];
            let tangent =
                (s.flags & meshlet::flags::TANGENTS != 0).then(|| (take(1), take(8), take(8)));
            let colours = m
                .colour_sets
                .iter()
                .map(|c| {
                    c.channels.map(|ch| match ch {
                        Channel::Constant(_) => None,
                        Channel::Unorm8 => Some(take(8)),
                        Channel::Float => Some(take(32)),
                    })
                })
                .collect();
            let uvs = m
                .uv_sets
                .iter()
                .map(|u| [take(u.bits[0]), take(u.bits[1])])
                .collect();
            Raw {
                pos,
                normal,
                tangent,
                colours,
                uvs,
            }
        })
        .collect()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(root) = args.first() else {
        eprintln!("usage: meshlet_census <romfs> [name filter]");
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
    let mut file_c = Counts::default();
    let mut shape_c = Counts::default();
    let mut ml = Counts::default();
    let mut normal_bits_by_size = Counts::default();
    let mut uv_e = Counts::default();
    let mut fill = Counts::default();
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
        let shapes: Vec<_> = model.shapes.iter().filter(|s| s.is_meshlet()).collect();
        if shapes.is_empty() {
            continue;
        }
        file_c.add(format!("models in the file: {}", b.models.len()));
        file_c.add(format!(
            "meshlet shapes {} of {} shapes",
            if shapes.len() == model.shapes.len() {
                "all"
            } else {
                "some"
            },
            if shapes.len() > 1 { "several" } else { "one" }
        ));
        file_c.add(format!("vertex buffers: {}", model.vertex_buffers.len()));
        // the relocation table's sections
        let rlt = u32_at(&data, 0x18) as usize;
        let count = u32_at(&data, rlt + 8) as usize;
        let secs: Vec<(usize, usize)> = (0..count)
            .map(|i| {
                let s = rlt + 16 + i * 24;
                (
                    u32_at(&data, s + 8) as usize,
                    u32_at(&data, s + 12) as usize,
                )
            })
            .collect();
        file_c.add(format!("sections {count}"));
        file_c.add(format!(
            "sections 1, 2 empty at the data's end: {}",
            secs[1].1 == 0 && secs[2].1 == 0 && secs[1].0 == secs[0].1 && secs[2].0 == secs[0].1
        ));
        file_c.add(format!(
            "section 3 at the GPU data: {}",
            secs[3].0 == b.buffer_offset
        ));
        file_c.add(format!(
            "GPU data on the 4 KiB after the data: {}",
            b.buffer_offset == secs[0].1.next_multiple_of(0x1000)
        ));
        let info = u64_at(&data, 0xB0) as usize;
        file_c.add(format!(
            "buffer info: word 0 {:#x}, size = section 3's: {}",
            u32_at(&data, info),
            u32_at(&data, info + 4) as usize == secs[3].1
        ));
        file_c.add(format!(
            "section 4 right after section 3: {}, its size {:#x}",
            secs[4].0 == secs[3].0 + secs[3].1,
            secs[4].1
        ));
        file_c.add(format!(
            "the table right after section 4: {}",
            rlt == secs[4].0 + secs[4].1
        ));
        // the buffers, one after another
        let mut expect = 0usize;
        let mut last_end = 0usize;
        for (k, s) in shapes.iter().enumerate() {
            let rec = s.meshlet.unwrap().record_at;
            let infos = u64_at(&data, rec + 0x10) as usize;
            let size = u32_at(&data, infos) as usize;
            let align = u32_at(&data, infos + 4) as usize;
            let off = u64_at(&data, u64_at(&data, rec + 0x18) as usize) as usize;
            shape_c.add(format!("buffer alignment {align:#x}"));
            shape_c.add(format!(
                "buffer size multiple of 0x100: {}",
                size.is_multiple_of(0x100)
            ));
            shape_c.add(format!(
                "buffer at the previous one's end, aligned: {}",
                off == expect.next_multiple_of(align)
            ));
            if k == 0 {
                shape_c.add(format!("first buffer at 0: {}", off == 0));
            }
            expect = off + size;
            last_end = off + size;
            shape_c.add(format!(
                "record +0 points at the memory pool: {}",
                u64_at(&data, rec) == u64_at(&data, 0xA8)
            ));
            let p8 = u64_at(&data, rec + 0x08) as usize;
            shape_c.add(format!(
                "record +8: 0x48 zero bytes right before the infos: {}",
                p8 + 0x48 == infos && data[p8..p8 + 0x48].iter().all(|&x| x == 0)
            ));
            shape_c.add(format!(
                "record 0x30..0x3c zero: {}",
                (0x30..0x3c).step_by(4).all(|o| u32_at(&data, rec + o) == 0)
            ));
            shape_c.add(format!(
                "record +0x46: {:#x}",
                u16::from_le_bytes([data[rec + 0x46], data[rec + 0x47]])
            ));
            let lods = data[rec + 0x45] as usize;
            shape_c.add(format!("LODs {lods}"));
            let ms = match MeshletShape::read(s) {
                Ok(x) => x,
                Err(_) => continue,
            };
            // the slack after the last meshlet
            let end = ms
                .meshlets
                .last()
                .map_or(0, |m| m.index_span().1.div_ceil(8));
            shape_c.add(format!(
                "size = data end on 0x100: {}",
                size == end.next_multiple_of(0x100)
            ));
            shape_c.add(format!(
                "bytes after the data zero: {}",
                ms.data[end..].iter().all(|&x| x == 0)
            ));
            // the header's LOD totals
            let totals: Vec<u32> = (0..16).map(|k| u32_at(ms.data, 0x14 + 4 * k)).collect();
            shape_c.add(format!(
                "header totals past the LODs repeat the last: {}",
                totals[lods..].iter().all(|&t| t == totals[lods - 1])
            ));
            let tris = u64_at(&data, rec + 0x20) as usize;
            let mls = u64_at(&data, rec + 0x28) as usize;
            let ok_tris = ms.lods.iter().enumerate().all(|(k, r)| {
                u32_at(&data, tris + 4 * k) as usize
                    == ms.meshlets[r.clone()]
                        .iter()
                        .map(|m| m.triangle_count)
                        .sum::<usize>()
            });
            let ok_mls = ms
                .lods
                .iter()
                .enumerate()
                .all(|(k, r)| u32_at(&data, mls + 4 * k) as usize == r.len());
            shape_c.add(format!(
                "record triangles per LOD right: {ok_tris}, meshlets per LOD right: {ok_mls}"
            ));
            shape_c.add(format!(
                "the per-LOD arrays: triangles' then meshlets' then infos at {:#x} {:#x}",
                mls as i64 - tris as i64,
                infos as i64 - p8 as i64
            ));
            // positions: the header's minimum against the vertices'
            let mut lo = [f32::MAX; 3];
            let mut hi = [f32::MIN; 3];
            let mut lo0 = [f32::MAX; 3];
            for lod in 0..ms.lods.len() {
                let g = ms.geometry(lod).unwrap();
                for p in &g.positions {
                    for k in 0..3 {
                        lo[k] = lo[k].min(p[k]);
                        hi[k] = hi[k].max(p[k]);
                        if lod == 0 {
                            lo0[k] = lo0[k].min(p[k]);
                        }
                    }
                }
            }
            let min_grid: Vec<i64> = (0..3)
                .map(|k| ((lo[k] - ms.bounds_min[k]) / ms.step).round() as i64)
                .collect();
            shape_c.add(format!(
                "the lowest vertex, steps above the header's minimum: {min_grid:?}"
            ));
            let bx = modstudio_formats::bfres::Bounding {
                center: std::array::from_fn(|k| f32_at(&data, s.bounding_at + 4 * k)),
                extent: std::array::from_fn(|k| f32_at(&data, s.bounding_at + 12 + 4 * k)),
            };
            let bx = &bx;
            let box_lo: [f32; 3] = std::array::from_fn(|k| bx.center[k] - bx.extent[k]);
            let box_hi: [f32; 3] = std::array::from_fn(|k| bx.center[k] + bx.extent[k]);
            let close = |a: f32, b: f32| (a - b).abs() <= 0.002;
            shape_c.add(format!(
                "shape box = the vertices' (all LODs, 2 mm): {}",
                (0..3).all(|k| close(box_lo[k], lo[k]) && close(box_hi[k], hi[k]))
            ));
            shape_c.add(format!(
                "header minimum = the shape box's: {}",
                (0..3).all(|k| ms.bounds_min[k] == box_lo[k])
            ));
            shape_c.add(format!(
                "header minimum = the vertices' (f32): {}",
                (0..3).all(|k| ms.bounds_min[k] == lo[k])
            ));
            shape_c.add(format!(
                "bounding at {}, radius at {}: radius right after the box: {}",
                s.bounding_at != 0,
                s.radius_at != 0,
                s.radius_at == s.bounding_at + 24
            ));
            let sp: [f32; 4] = std::array::from_fn(|k| f32_at(&data, s.radius_at + 4 * k));
            {
                let r_box =
                    (bx.extent[0].powi(2) + bx.extent[1].powi(2) + bx.extent[2].powi(2)).sqrt();
                shape_c.add(format!(
                    "sphere centre = box centre: {}, radius vs the box's half diagonal: {}",
                    (0..3).all(|k| close(sp[k], bx.center[k])),
                    if sp[3] < r_box * 0.999 {
                        "smaller"
                    } else if sp[3] <= r_box * 1.001 {
                        "same"
                    } else {
                        "larger"
                    }
                ));
            }
            let flags_seen = ms.flags;
            shape_c.add(format!("flags {flags_seen:#x}"));
            for me in &ms.meshlets {
                let raw = raw_vertices(&ms, me);
                fill.add(format!(
                    "vertices {:>2}, triangles {}",
                    me.vertex_count / 8 * 8,
                    if me.triangle_count >= 120 {
                        ">=120"
                    } else {
                        "<120"
                    }
                ));
                // vertex order: in order of first use by the indices
                let ix = me.indices(ms.data);
                let mut next = 0u32;
                let mut first_use = true;
                for &i in &ix {
                    if i == next {
                        next += 1;
                    } else if i > next {
                        first_use = false;
                    }
                }
                ml.add(format!("vertices in order of first use: {first_use}"));
                // positions: bits = bit length of the largest offset
                for k in 0..3 {
                    let max = raw.iter().map(|r| r.pos[k]).max().unwrap_or(0);
                    let min = raw.iter().map(|r| r.pos[k]).min().unwrap_or(0);
                    ml.add(format!(
                        "position bits = bit length of the largest offset: {}{}",
                        me.position_bits[k] == bitlen(max),
                        if min == 0 { "" } else { " (smallest not 0)" }
                    ));
                }
                // normals: bits against the meshlet's size
                let ext = (0..3)
                    .map(|k| raw.iter().map(|r| r.pos[k]).max().unwrap_or(0))
                    .max()
                    .unwrap_or(0);
                normal_bits_by_size.add(format!(
                    "extent 2^{:>2} steps: normal bits {:>2}",
                    bitlen(ext),
                    me.normal_bits
                ));
                let nmax = raw
                    .iter()
                    .map(|r| r.normal[0].max(r.normal[1]))
                    .max()
                    .unwrap_or(0);
                ml.add(format!(
                    "normal: the largest value uses the top bit: {}",
                    bitlen(nmax) == me.normal_bits
                ));
                if let Some(t) = raw.first().and_then(|r| r.tangent) {
                    let _ = t;
                }
                // UVs: bits and exponents
                for (u, set) in me.uv_sets.iter().enumerate() {
                    for c in 0..2 {
                        let max = raw.iter().map(|r| r.uvs[u][c]).max().unwrap_or(0);
                        let min = raw.iter().map(|r| r.uvs[u][c]).min().unwrap_or(0);
                        ml.add(format!(
                            "uv bits = max(1, bit length of the largest): {}{}",
                            set.bits[c] == bitlen(max).max(1),
                            if min == 0 { "" } else { " (smallest not 0)" }
                        ));
                        // the span in UV units
                        let step = 2f32.powi(set.exponent[c] as i32 - 16);
                        let lo = set.base[c] as f32 * step;
                        let hi = (set.base[c] as i64 + max as i64) as f32 * step;
                        let big = lo.abs().max(hi.abs());
                        uv_e.add(format!(
                            "set {u}: e {} | largest |uv| < 2^{:>2} | span < 2^{:>3} | bits {:>2}",
                            set.exponent[c],
                            if big < 1e-9 {
                                -99
                            } else {
                                big.log2().floor() as i32 + 1
                            },
                            if hi - lo < 1e-9 {
                                -99
                            } else {
                                (hi - lo).log2().floor() as i32 + 1
                            },
                            set.bits[c]
                        ));
                    }
                }
                // colours
                for (k, c) in me.colour_sets.iter().enumerate() {
                    let w = me.record[8 + 2 * me.uv_sets.len() + 2 * k];
                    let mask = w & 0xff;
                    let kinds: Vec<&str> = c
                        .channels
                        .iter()
                        .map(|ch| match ch {
                            Channel::Constant(_) => "c",
                            Channel::Unorm8 => "8",
                            Channel::Float => "f",
                        })
                        .collect();
                    ml.add(format!(
                        "colour set {}: mask high {:#x}, channels {}",
                        c.set,
                        mask >> 4,
                        kinds.join("")
                    ));
                    for (ch, kind) in kinds.iter().enumerate() {
                        let vals: Vec<u32> = raw.iter().filter_map(|r| r.colours[k][ch]).collect();
                        if vals.is_empty() {
                            continue;
                        }
                        let all_same = vals.iter().all(|&v| v == vals[0]);
                        let in_unit = *kind != "f"
                            || vals.iter().all(|&v| {
                                let x = f32::from_bits(v);
                                (0.0..=1.0).contains(&x)
                            });
                        let bytes = *kind != "f"
                            || vals.iter().all(|&v| {
                                let x = f32::from_bits(v) * 255.0;
                                (x - x.round()).abs() < 1e-3
                            });
                        ml.add(format!(
                            "colour channel {kind}: all the same {all_same}, in 0..1 {in_unit}, whole bytes {bytes}"
                        ));
                    }
                }
                // the cone
                let w4 = me.record[4];
                let cutoff = (me.record[6] >> 9) & 0x7f;
                ml.add(format!(
                    "cone: {}",
                    if w4 & 0xffff == 0 {
                        format!("none, cutoff {cutoff}")
                    } else {
                        "axis".into()
                    }
                ));
                ml.add(format!("word 7 {}", me.record[7]));
            }
        }
        let _ = last_end;
    }
    for (title, c) in [
        ("files", &file_c),
        ("shapes", &shape_c),
        ("meshlets", &ml),
        ("fill", &fill),
        ("normal bits by size", &normal_bits_by_size),
        ("uv exponents", &uv_e),
    ] {
        println!("== {title}");
        for (k, n) in &c.0 {
            println!("  {n:9}  {k}");
        }
    }
}
