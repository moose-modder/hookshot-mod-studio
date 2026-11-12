//! `modstudio`: the command-line side of Hookshot Mod Studio.

use std::process::ExitCode;

const USAGE: &str = "\
modstudio: model mods for Hookshot

Usage:
  modstudio --version
  modstudio help

Commands to come (docs/plan.md):
  inspect <model.bfres.zs>   meshes, materials, skin counts, vertex formats, textures (JSON)
  export <model.bfres.zs>    the model as .glb with its textures, for Blender and previews
  replace                    rebuild a model and its textures from a Blender export
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version") => {
            println!("modstudio {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        None | Some("help" | "--help" | "-h") => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("modstudio: unknown command {other:?}\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}
