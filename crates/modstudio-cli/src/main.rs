//! `modstudio`: the command-line side of Hookshot Mod Studio.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use modstudio_core::progress::Progress;
use modstudio_core::project::{self, Project};
use modstudio_core::settings::Settings;
use modstudio_core::{build, catalog, model, mods};
use modstudio_formats::bntx::Bntx;
use modstudio_formats::romfs::{self, Romfs};

const USAGE: &str = "\
modstudio: model mods for Hookshot

Usage:
  modstudio inspect <model.bfres.zs> --romfs <romfs>
      meshes, materials, skin counts, vertex formats and textures, as JSON
  modstudio export <model.bfres.zs> <out-dir> --romfs <romfs> [--lod <n>]
      <model>.glb (skeleton, LOD 0 of each mesh, skin weights, UVs, colours), the textures as PNG and
      model.json, for Blender and previews; --lod n: each mesh's LOD n instead (its last if it has fewer)
  modstudio preview <model.bfres.zs> <out.glb> --romfs <romfs>
      the model as the app's 3D view shows it: one .glb with the base colour textures in it (eyes drawn
      as the game's eye shader draws them)
  modstudio replace <model.bfres.zs> <scene.glb> <out-romfs> --romfs <romfs> [--textures <dir>]
      the model rebuilt from Blender's glTF export of its scene (each mesh replaces the game's mesh of its
      name), written compressed under <out-romfs> at the model's path; with --textures, every <name>.png in
      <dir> replaces the game's texture <name> (under <out-romfs>/Tex)
  modstudio decompress <file.zs> <out> --romfs <romfs>
      decompress a .zs file with the game's dictionaries
  modstudio texture <file.bntx.zs> <out.png> --romfs <romfs>
      the first texture's top mip level as PNG
  modstudio --version
  modstudio help

Making mods (what the app does, with the same settings):
  modstudio settings [--hookshot <dir>] [--blender <exe>] [--workspace <dir>]
                     [--mods <dir>] [--game-romfs <dir>]
      shows the settings and whether everything a build needs is there; the options change them
      (--hookshot <dir>: the Hookshot folder, for its romfs and mods folders)
  modstudio models [<part of a name>] [--all]
      the game's models by kind (--all: map pieces too)
  modstudio new <model> [--name <folder>]
      a project in the workspace: the model exported, a Blender scene made from it, project.json
      (<model>: a path in the romfs, or part of a model's name)
  modstudio build <project> [--install] [--no-check] [--skip-blender]
      the mod built from the project's scene (<project>: its folder or project.json); --install copies it
      into Hookshot's mods folder and makes one resource size table for all mods
  modstudio mods [on <folder> | off <folder> | table]
      the mods in Hookshot's mods folder; turn one on or off; make the size table for all mods again

These take --settings <file> (default: settings.json beside modstudio, which is the app's when the two sit in
one folder).

Model paths are relative to the romfs (Model/Npc_X.Npc_X.bfres.zs) or absolute. inspect, export, texture and
decompress take --overlay <mod>/romfs (repeatable): files a mod has are read in place of the game's, as Hookshot
loads them (the last --overlay wins), e.g. to export an installed mod's model with its textures.
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

/// Options that take no value.
const FLAGS: [&str; 4] = ["install", "no-check", "skip-blender", "all"];

/// Positional arguments, `--name value` options and `--flag`s.
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
                if FLAGS.contains(&name) {
                    options.push((name.to_owned(), String::new()));
                    continue;
                }
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

    /// Every value of a repeatable option, in order.
    fn all(&self, name: &str) -> impl Iterator<Item = &str> {
        self.options
            .iter()
            .filter(move |(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    fn flag(&self, name: &str) -> bool {
        self.option(name).is_some()
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

/// The game's romfs (`--romfs`) with the mods given by `--overlay` (each a mod's `romfs` folder) on top.
fn open_romfs(args: &Args) -> Result<Romfs, Failure> {
    let mut romfs = Romfs::open(PathBuf::from(args.required("romfs")?))?;
    for dir in args.all("overlay") {
        romfs = romfs.with_overlay(PathBuf::from(dir));
    }
    Ok(romfs)
}

fn decompress(args: &Args) -> Result<(), Failure> {
    let [input, output] = args.positional(2)? else {
        unreachable!()
    };
    let romfs = open_romfs(args)?;
    let data = romfs::read_maybe_zs(&mut romfs.decoder()?, Path::new(input))?;
    std::fs::write(output, data).map_err(|e| format!("{output}: {e}"))?;
    Ok(())
}

fn texture(args: &Args) -> Result<(), Failure> {
    let [input, output] = args.positional(2)? else {
        unreachable!()
    };
    let romfs = open_romfs(args)?;
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
    let romfs = open_romfs(args)?;
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
    let romfs = open_romfs(args)?;
    let lod = match args.option("lod") {
        Some(n) => n
            .parse()
            .map_err(|_| Failure::Usage(format!("--lod {n}: not a number")))?,
        None => 0,
    };
    let summary = model::export_lod(&romfs, Path::new(model), Path::new(out), lod)?;
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}

fn preview(args: &Args) -> Result<(), Failure> {
    let [model, out] = args.positional(2)? else {
        unreachable!()
    };
    let romfs = open_romfs(args)?;
    let p = modstudio_core::preview::preview(&romfs, Path::new(model), 1024)?;
    std::fs::write(out, &p.glb).map_err(|e| format!("{out}: {e}"))?;
    println!("{}", serde_json::to_string_pretty(&p)?);
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
    let mut report = serde_json::to_value(&summary)?;
    if let Some(dir) = args.option("textures") {
        let (written, warnings) =
            modstudio_core::texture::replace_textures(&romfs, Path::new(dir), Path::new(out))?;
        report["textures"] = serde_json::json!({"written": written, "warnings": warnings});
    }
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

// ---------------------------------------------------------------- making mods

fn settings_path(args: &Args) -> Result<PathBuf, Failure> {
    match args.option("settings") {
        Some(p) => Ok(PathBuf::from(p)),
        None => Settings::default_path().ok_or_else(|| {
            Failure::Failed("can't tell where modstudio is (for settings.json)".into())
        }),
    }
}

fn load_settings(args: &Args) -> Result<Settings, Failure> {
    let path = settings_path(args)?;
    if !path.is_file() {
        return Err(Failure::Failed(format!(
            "no settings at {} yet: run `modstudio settings --hookshot <Hookshot folder> --workspace <dir>` \
             (or the app) first",
            path.display()
        )));
    }
    Ok(Settings::load(&path)?)
}

fn settings(args: &Args) -> Result<(), Failure> {
    args.positional(0)?;
    let path = settings_path(args)?;
    let mut s = if path.is_file() {
        Settings::load(&path)?
    } else {
        Settings::default()
    };
    let mut changed = false;
    if let Some(dir) = args.option("hookshot") {
        let fresh = Settings::for_hookshot(Path::new(dir));
        s.game_romfs = fresh.game_romfs;
        s.install_mods = fresh.install_mods;
        if s.workspace.as_os_str().is_empty() {
            s.workspace = fresh.workspace;
        }
        if s.tools.blender.is_none() {
            s.tools.blender = fresh.tools.blender;
        }
        changed = true;
    }
    for (option, field) in [
        ("game-romfs", &mut s.game_romfs),
        ("mods", &mut s.install_mods),
        ("workspace", &mut s.workspace),
    ] {
        if let Some(v) = args.option(option) {
            *field = PathBuf::from(v);
            changed = true;
        }
    }
    if let Some(v) = args.option("blender") {
        s.tools.blender = Some(PathBuf::from(v));
        changed = true;
    }
    if changed {
        s.save(&path)?;
        println!("Saved {}", path.display());
    } else if !path.is_file() {
        println!("No settings at {} yet", path.display());
    }
    println!("{}", serde_json::to_string_pretty(&s)?);
    let status = s.status();
    println!();
    if let Some(v) = &status.blender {
        println!("{v}");
    }
    if let Some(v) = &status.game_version {
        println!("Game version {v}");
    }
    if status.ready() {
        println!("Ready.");
    } else {
        for p in &status.problems {
            println!("- {p}");
        }
    }
    Ok(())
}

fn models(args: &Args) -> Result<(), Failure> {
    let s = load_settings(args)?;
    let filter = match args.positional.as_slice() {
        [] => None,
        [f] => Some(f.to_lowercase()),
        _ => return Err(Failure::Usage("models takes one filter".into())),
    };
    let models = catalog::list(&s.game_romfs)?;
    for category in catalog::CATEGORIES {
        if category == "Level" && !args.flag("all") {
            continue;
        }
        let names: Vec<&str> = models
            .iter()
            .filter(|m| m.category == category)
            .filter(|m| {
                filter
                    .as_ref()
                    .is_none_or(|f| m.name.to_lowercase().contains(f))
            })
            .map(|m| m.name.as_str())
            .collect();
        if !names.is_empty() {
            println!("{category} ({}):", names.len());
            for n in names {
                println!("  {n}");
            }
        }
    }
    Ok(())
}

/// A model's path in the romfs from a path or part of its name (several matches are listed, not picked).
fn find_model(s: &Settings, what: &str) -> Result<String, Failure> {
    let what = what.replace(std::path::MAIN_SEPARATOR, "/");
    if s.game_romfs.join(&what).is_file() {
        return Ok(what);
    }
    let term = what.rsplit('/').next().unwrap_or(&what).to_lowercase();
    let stem = term.split('.').next().unwrap_or(&term).to_owned();
    let models = catalog::list(&s.game_romfs)?;
    let hits: Vec<&catalog::ModelEntry> = models
        .iter()
        .filter(|m| m.name.to_lowercase().contains(&stem))
        .collect();
    let exact: Vec<&catalog::ModelEntry> = hits
        .iter()
        .copied()
        .filter(|m| m.name.to_lowercase() == stem)
        .collect();
    match (exact.as_slice(), hits.as_slice()) {
        ([m], _) | (_, [m]) => Ok(m.path.clone()),
        (_, []) => Err(Failure::Failed(format!("No model matches {what:?}"))),
        (_, hits) => {
            let mut text = format!("{} models match {what:?}, name one:", hits.len());
            for m in hits.iter().take(60) {
                text.push_str("\n  ");
                text.push_str(&m.name);
            }
            if hits.len() > 60 {
                text.push_str("\n  ...");
            }
            Err(Failure::Failed(text))
        }
    }
}

fn new_project(args: &Args) -> Result<(), Failure> {
    let [what] = args.positional(1)? else {
        unreachable!()
    };
    let s = load_settings(args)?;
    let model = find_model(&s, what)?;
    let folder = match args.option("name") {
        Some(n) => n.to_owned(),
        None => project::model_name(&model),
    };
    println!("Model: {model}");
    let p = project::create(&s, &model, &folder, &mut Progress::stdout())?;
    println!(
        "\nNext: edit {} in Blender, then\n   modstudio build {} --install",
        p.blend().display(),
        p.dir().display()
    );
    Ok(())
}

fn build_project(args: &Args) -> Result<(), Failure> {
    let [path] = args.positional(1)? else {
        unreachable!()
    };
    let s = load_settings(args)?;
    let p = Project::load(Path::new(path))?;
    let options = build::Options {
        install: args.flag("install"),
        check: !args.flag("no-check"),
        skip_blender: args.flag("skip-blender"),
    };
    println!("Project: {} ({})", p.data.name, p.file.display());
    println!("Scene:   {}", p.blend().display());
    let report = build::build(&s, &p, options, &mut Progress::stdout());
    println!(
        "\n{} in {:.0}s",
        if report.ok { "Done" } else { "BUILD FAILED" },
        report.seconds
    );
    match report.error {
        Some(e) => Err(Failure::Failed(e)),
        None => Ok(()),
    }
}

fn mods_command(args: &Args) -> Result<(), Failure> {
    let s = load_settings(args)?;
    let mut progress = Progress::stdout();
    let words: Vec<&str> = args.positional.iter().map(String::as_str).collect();
    match words.as_slice() {
        [] => {}
        [on @ ("on" | "off"), folder] => {
            let new = mods::set_enabled(&s.install_mods, folder, *on == "on")?;
            println!("{folder} -> {new}");
            mods::rebuild_size_table(&s, &mut progress)?;
        }
        ["table"] => {
            mods::rebuild_size_table(&s, &mut progress)?;
        }
        _ => {
            return Err(Failure::Usage(
                "mods [on <folder> | off <folder> | table]".into(),
            ));
        }
    }
    println!("\nMods in {}:", s.install_mods.display());
    for m in mods::list(&s.install_mods)? {
        let mut models = if m.models.is_empty() {
            String::new()
        } else {
            format!(", models {}", m.models.join(", "))
        };
        if m.animations > 0 {
            models.push_str(&format!(", {} animation(s)", m.animations));
        }
        println!(
            "  [{}] {:30} {} file(s){models}",
            if m.enabled { "on " } else { "off" },
            m.folder,
            m.files,
        );
    }
    match mods::size_table(&s.install_mods) {
        Some(t) => println!("Size table: {}", t.display()),
        None => println!("No size table installed"),
    }
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
        "preview" => Args::parse(&raw[1..]).and_then(|a| preview(&a)),
        "settings" => Args::parse(&raw[1..]).and_then(|a| settings(&a)),
        "models" => Args::parse(&raw[1..]).and_then(|a| models(&a)),
        "new" => Args::parse(&raw[1..]).and_then(|a| new_project(&a)),
        "build" => Args::parse(&raw[1..]).and_then(|a| build_project(&a)),
        "mods" => Args::parse(&raw[1..]).and_then(|a| mods_command(&a)),
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
