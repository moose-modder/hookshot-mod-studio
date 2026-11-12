//! Counts vertices whose influences name the same bone twice, per shape (for local checks of built models).
//!
//! `cargo run --release --example skin_check -- <romfs> <model.bfres.zs>`

use modstudio_formats::bfres::{Bfres, ExternalStrings};
use modstudio_formats::romfs::Romfs;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [romfs, model] = args.as_slice() else {
        eprintln!("usage: skin_check <romfs> <model>");
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
    for s in &m.shapes {
        let vb = &m.vertex_buffers[s.vertex_buffer];
        let (Some(i0), Some(w0)) = (vb.attribute("_i0"), vb.attribute("_w0")) else {
            continue;
        };
        let (i0, w0) = (vb.read(i0).unwrap(), vb.read(w0).unwrap());
        let (mut dup, mut low_sum, mut example) = (0, 0, None);
        for v in 0..i0.len() {
            let n = (s.skin_count as usize).min(4);
            let mut seen: Vec<f32> = Vec::new();
            let mut sum = 0.0;
            for k in 0..n {
                if w0[v][k] > 0.0 {
                    if seen.contains(&i0[v][k]) {
                        dup += 1;
                        example.get_or_insert((v, i0[v], w0[v]));
                    }
                    seen.push(i0[v][k]);
                }
                sum += w0[v][k];
            }
            if s.skin_count <= 4 && s.skin_count > 1 && (sum - 1.0).abs() > 0.01 {
                low_sum += 1;
            }
        }
        println!(
            "{:40} skin {} vertices {:6} duplicate bones {dup:4} weights not summing to 1 {low_sum:4} {:?}",
            s.name,
            s.skin_count,
            i0.len(),
            example
        );
    }
}
