//! Per shape of a model: skin indices of rigid shapes and their bones' matrix numbers, and the tangent w values
//! (local analysis for the writer).
//!
//! `cargo run --release --example attr_stats -- <romfs> <model.bfres.zs>`

use std::collections::BTreeMap;

use modstudio_formats::bfres::{Bfres, ExternalStrings};
use modstudio_formats::romfs::Romfs;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [romfs, model] = args.as_slice() else {
        eprintln!("usage: attr_stats <romfs> <model>");
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
    let sk = &m.skeleton;
    println!(
        "smooth {} rigid {} matrices {}",
        sk.smooth_count,
        sk.rigid_count,
        sk.matrix_to_bone.len()
    );
    for s in &m.shapes {
        let vb = &m.vertex_buffers[s.vertex_buffer];
        let mut line = format!("{} skin {}", s.name, s.skin_count);
        if s.skin_count == 1 {
            let i0 = vb.read(vb.attribute("_i0").unwrap()).unwrap();
            let mut seen = BTreeMap::new();
            for v in &i0 {
                *seen.entry(v[0] as usize).or_insert(0) += 1;
            }
            for (&mi, n) in &seen {
                let b = sk.matrix_to_bone[mi] as usize;
                let bone = &sk.bones[b];
                line += &format!(
                    " | i0 {mi} x{n} -> bone {b} {} smooth {:?} rigid {:?}",
                    bone.name, bone.smooth_matrix, bone.rigid_matrix
                );
            }
        }
        if let Some(t) = vb.attribute("_t0") {
            let t = vb.read(t).unwrap();
            let mut w = BTreeMap::new();
            for v in &t {
                *w.entry(v[3] as i32).or_insert(0) += 1;
            }
            line += &format!(
                " | _t0 {} w {:?}",
                vb.attribute("_t0").unwrap().format.name(),
                w
            );
        }
        for name in ["_n0", "_p0"] {
            if let Some(a) = vb.attribute(name)
                && a.format.components() == Some(4)
            {
                {
                    let vals = vb.read(a).unwrap();
                    let mut w = BTreeMap::new();
                    for v in &vals {
                        *w.entry(format!("{:.3}", v[3])).or_insert(0) += 1;
                    }
                    line += &format!(
                        " | {name} {} w {:?}",
                        a.format.name(),
                        w.iter().take(4).collect::<Vec<_>>()
                    );
                }
            }
        }
        let names: Vec<&str> = vb.attributes.iter().map(|a| a.name.as_str()).collect();
        line += &format!(" | {names:?}");
        println!("{line}");
    }
}
