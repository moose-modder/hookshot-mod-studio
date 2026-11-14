//! A model's animations as the app lists them (`anim::list`), how long that takes, and a few opened as clips: their
//! tracks, the bones they move that the model hasn't, and the size of what the app is sent.
//!
//!     cargo run --release -p modstudio-core --example anim_list -- <romfs> <model.bfres.zs> [name filter]

use std::time::Instant;

use modstudio_core::anim::{self, Clip};
use modstudio_formats::romfs::Romfs;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [root, model, rest @ ..] = args.as_slice() else {
        eprintln!("usage: anim_list <romfs> <model> [name filter]");
        std::process::exit(2);
    };
    let filter = rest.first().map(String::as_str).unwrap_or("");
    let romfs = Romfs::open(root).expect("romfs");
    let t = Instant::now();
    let sources = anim::anim_files(romfs.root(), None, model);
    println!(
        "{} file(s) in {:.2} s: {} actor, {} own, {} cutscene",
        sources.len(),
        t.elapsed().as_secs_f32(),
        sources.iter().filter(|s| s.kind == "actor").count(),
        sources.iter().filter(|s| s.kind == "own").count(),
        sources.iter().filter(|s| s.kind == "cutscene").count()
    );
    let t = Instant::now();
    let list = anim::list(&romfs, None, model).expect("list");
    println!(
        "{} animation(s) listed in {:.2} s",
        list.len(),
        t.elapsed().as_secs_f32()
    );
    let longest = list.iter().max_by_key(|a| a.frames).cloned();
    let picked: Vec<_> = list
        .iter()
        .filter(|a| !filter.is_empty() && a.name.contains(filter))
        .take(3)
        .cloned()
        .chain(longest)
        .collect();
    for a in picked {
        let t = Instant::now();
        let clip = Clip::open(&romfs, model, &a.file, &a.name).expect("clip");
        let bytes = clip.to_bytes(0);
        println!(
            "{} / {}: {} frames{}, {} tracks, {} unmatched {:?}, {:.1} MB, opened in {:.2} s",
            a.file.trim_start_matches("Model/"),
            a.name,
            a.frames,
            if a.looping { " looping" } else { "" },
            clip.tracks.len(),
            clip.unmatched.len(),
            &clip.unmatched[..clip.unmatched.len().min(4)],
            bytes.len() as f32 / 1e6,
            t.elapsed().as_secs_f32()
        );
    }
}
