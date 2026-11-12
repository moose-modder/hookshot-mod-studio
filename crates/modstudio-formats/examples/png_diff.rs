//! Compares two PNG files channel by channel (for local checks against other tools' exports).
//!
//! `cargo run --release --example png_diff -- a.png b.png`

use modstudio_formats::image::Image;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [a, b] = args.as_slice() else {
        eprintln!("usage: png_diff <a.png> <b.png>");
        std::process::exit(2);
    };
    let load = |p: &str| Image::from_png(&std::fs::read(p).expect("read")).expect("decode");
    let (a, b) = (load(a), load(b));
    if (a.width, a.height) != (b.width, b.height) {
        println!(
            "size differs: {}x{} vs {}x{}",
            a.width, a.height, b.width, b.height
        );
        return;
    }
    let mut sum = [0u64; 4];
    let mut max = [0u8; 4];
    let mut over2 = [0usize; 4];
    for (pa, pb) in a.rgba.chunks(4).zip(b.rgba.chunks(4)) {
        for c in 0..4 {
            let d = pa[c].abs_diff(pb[c]);
            sum[c] += u64::from(d);
            max[c] = max[c].max(d);
            over2[c] += usize::from(d > 2);
        }
    }
    let n = (a.width * a.height) as f64;
    for (c, name) in ["r", "g", "b", "a"].iter().enumerate() {
        println!(
            "{name}: mean {:.3} max {:3} over 2: {:.3}%",
            sum[c] as f64 / n,
            max[c],
            over2[c] as f64 * 100.0 / n
        );
    }
}
