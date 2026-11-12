//! `modstudio`: the command-line side of Hookshot Mod Studio.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use modstudio_core::model;
use modstudio_formats::bntx::Bntx;
use modstudio_formats::romfs::{self, Romfs};

const USAGE: &str = "\
modstudio: model mods for Hookshot

Usage:
  modstudio inspect <model.bfres.zs> --romfs <romfs>
      meshes, materials, skin counts, vertex formats and textures, as JSON
  modstudio export <model.bfres.zs> <out-dir> --romfs <romfs>
      <model>.glb (skeleton, LOD 0 of each mesh, skin weights, UVs, colours), the textures as PNG and
      model.json, for Blender and previews
  modstudio replace <model.bfres.zs> <scene.glb> <out-romfs> --romfs <romfs>
      the model rebuilt from Blender's glTF export of its scene (each mesh replaces the game's mesh of its
      name), written compressed under <out-romfs> at the model's path
  modstudio decompress <file.zs> <out> --romfs <romfs>
      decompress a .zs file with the game's dictionaries
  modstudio texture <file.bntx.zs> <out.png> --romfs <romfs>
      the first texture's top mip level as PNG
  modstudio --version
  modstudio help

Model paths are relative to the romfs (Model/Npc_X.Npc_X.bfres.zs) or absolute.

To come (docs/plan.md): textures in replace (BC encoding), the resource size table.
";

/// A usage error (exit code 2) or a failure (exit code 1).
enum Failure {
    Usage(String),
    Failed(String),
}

impl<E: std::fmt::Display> From<E> for Failure {
    fn from(e: E) -> Self {
        Failure::Failed(e.to_string())
    }
}

/// Positional arguments and `--name value` options.
struct Args {
    positional: Vec<String>,
    options: Vec<(String, String)>,
}

impl Args {
    fn parse(raw: &[String]) -> Result<Self, Failure> {
        let mut positional = Vec::new();
        let mut options = Vec::new();
        let mut it = raw.iter();
        while let Some(a) = it.next() {
            if let Some(name) = a.strip_prefix("--") {
                let value = it
                    .next()
                    .ok_or_else(|| Failure::Usage(format!("--{name} needs a value")))?;
                options.push((name.to_owned(), value.clone()));
            } else {
                positional.push(a.clone());
            }
        }
        Ok(Args {
            positional,
            options,
        })
    }

    fn option(&self, name: &str) -> Option<&str> {
        self.options
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    fn required(&self, name: &str) -> Result<&str, Failure> {
        self.option(name)
            .ok_or_else(|| Failure::Usage(format!("--{name} is required")))
    }

    fn positional(&self, count: usize) -> Result<&[String], Failure> {
        if self.positional.len() != count {
            return Err(Failure::Usage(format!(
                "expected {count} arguments, got {}",
                self.positional.len()
            )));
        }
        Ok(&self.positional)
    }
}

fn decompress(args: &Args) -> Result<(), Failure> {
    let [input, output] = args.positional(2)? else {
        unreachable!()
    };
    let romfs = Romfs::open(PathBuf::from(args.required("romfs")?))?;
    let data = romfs::read_maybe_zs(&mut romfs.decoder()?, Path::new(input))?;
    std::fs::write(output, data).map_err(|e| format!("{output}: {e}"))?;
    Ok(())
}

fn texture(args: &Args) -> Result<(), Failure> {
    let [input, output] = args.positional(2)? else {
        unreachable!()
    };
    let romfs = Romfs::open(PathBuf::from(args.required("romfs")?))?;
    let data = romfs::read_maybe_zs(&mut romfs.decoder()?, Path::new(input))?;
    let bntx = Bntx::parse(&data)?;
    let texture = bntx
        .textures
        .first()
        .ok_or_else(|| format!("{input}: no textures"))?;
    let mut image = texture.decode(0, 0)?;
    texture.apply_channel_sources(&mut image);
    std::fs::write(output, image.to_png()?).map_err(|e| format!("{output}: {e}"))?;
    println!(
        "{} {}x{} {} mips {} layers {}",
        texture.name,
        texture.width,
        texture.height,
        texture.format.name(),
        texture.mip_count,
        texture.array_length
    );
    Ok(())
}

fn inspect(args: &Args) -> Result<(), Failure> {
    let [model] = args.positional(1)? else {
        unreachable!()
    };
    let romfs = Romfs::open(PathBuf::from(args.required("romfs")?))?;
    let mut decoder = romfs.decoder()?;
    let loaded = model::Loaded::read(&romfs, &mut decoder, Path::new(model))?;
    let inspection = model::inspect(&romfs, &mut decoder, &loaded.bfres()?)?;
    println!("{}", serde_json::to_string_pretty(&inspection)?);
    Ok(())
}

fn export(args: &Args) -> Result<(), Failure> {
    let [model, out] = args.positional(2)? else {
        unreachable!()
    };
    let romfs = Romfs::open(PathBuf::from(args.required("romfs")?))?;
    let summary = model::export(&romfs, Path::new(model), Path::new(out))?;
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}

fn replace(args: &Args) -> Result<(), Failure> {
    let [model, glb, out] = args.positional(3)? else {
        unreachable!()
    };
    let romfs = Romfs::open(PathBuf::from(args.required("romfs")?))?;
    let summary = modstudio_core::replace::replace_model(
        &romfs,
        Path::new(model),
        Path::new(glb),
        Path::new(out),
    )?;
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}

fn main() -> ExitCode {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = raw.first().map(String::as_str) else {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    };
    let result = match command {
        "--version" => {
            println!("modstudio {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "help" | "--help" | "-h" => {
            print!("{USAGE}");
            Ok(())
        }
        "inspect" => Args::parse(&raw[1..]).and_then(|a| inspect(&a)),
        "export" => Args::parse(&raw[1..]).and_then(|a| export(&a)),
        "replace" => Args::parse(&raw[1..]).and_then(|a| replace(&a)),
        "decompress" => Args::parse(&raw[1..]).and_then(|a| decompress(&a)),
        "texture" => Args::parse(&raw[1..]).and_then(|a| texture(&a)),
        other => Err(Failure::Usage(format!("unknown command {other:?}"))),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure::Usage(msg)) => {
            eprintln!("modstudio: {msg}\n\n{USAGE}");
            ExitCode::from(2)
        }
        Err(Failure::Failed(msg)) => {
            eprintln!("modstudio: {msg}");
            ExitCode::FAILURE
        }
    }
}
