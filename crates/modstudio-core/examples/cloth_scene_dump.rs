//! A cloth file as a scene shows it (`modstudio_core::cloth_edit::scene_of`), as JSON on stdout.
//!
//!     cargo run --release -p modstudio-core --example cloth_scene_dump -- <file.bphcl>

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: cloth_scene_dump <file.bphcl>");
        std::process::exit(2);
    };
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let scene =
        modstudio_core::cloth_edit::scene_of(&bytes, &path).unwrap_or_else(|e| panic!("{e}"));
    println!("{}", serde_json::to_string_pretty(&scene).expect("JSON"));
}
