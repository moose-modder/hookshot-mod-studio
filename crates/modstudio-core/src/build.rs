//! Building a project's mod from its Blender scene, in one go:
//!
//! 1. The game's model exported to `build/original`: `model.json`, its textures (and every colour version of
//!    clothes textures, `<name>.1` ...) as PNG.
//! 2. Blender, in the background (the `.blend` is not changed): `prep_for_game.py` bakes changed looks into the
//!    game's textures and exports the scene as `build/model.glb`.
//! 3. The mod's model and changed textures, written under `mod/romfs`.
//! 4. The result read back: every game mesh must keep its material, skin count and LOD count.
//! 5. The build check (`check_result.py`): the result rendered as the scene is, and compared with it.
//! 6. The mod's own resource size table (for using the mod on its own).
//! 7. Install, if asked: the mod's romfs into `<mods>/<name>/romfs`, and one size table for all mods
//!    ([`mods::rebuild_size_table`]).
//!
//! Every step's tool output is in `build/` (`2_blender.log`, ...); the report is `build/build_report.json`.

use std::path::{Path, PathBuf};
use std::time::Instant;

use modstudio_formats::bntx::Bntx;
use modstudio_formats::romfs::{self, Romfs};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::fsutil::{self, is_size_table};
use crate::model::{self, Loaded, TEXTURE_DIR};
use crate::progress::Progress;
use crate::project::Project;
use crate::settings::Settings;
use crate::{Context, Error, Result, args, mods, replace, texture, tools};

/// The last build's report, in the work folder.
pub const REPORT_FILE: &str = "build_report.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// Install into Hookshot's mods folder afterwards.
    pub install: bool,
    /// Compare the result with the scene (step 5).
    pub check: bool,
    /// Reuse the last build's export of the scene (step 2 skipped).
    pub skip_blender: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            install: false,
            check: true,
            skip_blender: false,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BuildReport {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// When it finished (seconds since 1970).
    pub finished: u64,
    pub seconds: f64,
    pub install: bool,
    /// Each game mesh, and what the build did with it.
    pub meshes: Vec<MeshRow>,
    /// Textures written (changed in the scene).
    pub textures: Vec<TextureRow>,
    pub warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<CheckSummary>,
    /// The mod's files, relative to its romfs.
    pub files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_table: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshRow {
    pub name: String,
    /// "kept" (the game's own data), "replaced", "removed" (not in the scene: drawn as nothing)
    pub status: String,
    pub material: String,
    pub game_vertices: u64,
    pub vertices: u64,
    pub triangles: u64,
    pub skin_count: u64,
    pub lods: u64,
    /// New meshes of the scene joined into this one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub joined: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextureRow {
    pub name: String,
    pub from: String,
    pub to: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckSummary {
    pub matches: bool,
    pub differences: Vec<Difference>,
    /// Pictures of each view that differs: scene | result | differences in red.
    pub images: Vec<PathBuf>,
    /// Materials the check couldn't render exactly as Blender shows them.
    pub not_compared: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Difference {
    pub view: String,
    pub pose: String,
    /// "shape" or "colour"
    pub kind: String,
    pub pixels: u64,
    pub meshes: Vec<String>,
}

/// Builds the project's mod (see the module's notes). The report is also saved as `build/build_report.json`;
/// a failed build's report has `ok: false` and the error.
pub fn build(
    settings: &Settings,
    project: &Project,
    options: Options,
    progress: &mut Progress,
) -> BuildReport {
    let start = Instant::now();
    let mut report = BuildReport {
        install: options.install,
        ..BuildReport::default()
    };
    let result = run(settings, project, options, &mut report, progress);
    report.ok = result.is_ok();
    report.error = result.err().map(|e| e.0);
    report.seconds = (start.elapsed().as_secs_f64() * 10.0).round() / 10.0;
    report.finished = fsutil::now();
    let work = project.work_dir();
    if std::fs::create_dir_all(&work).is_ok()
        && let Ok(text) = serde_json::to_string_pretty(&report)
    {
        let _ = std::fs::write(work.join(REPORT_FILE), text);
    }
    report
}

fn run(
    settings: &Settings,
    project: &Project,
    options: Options,
    report: &mut BuildReport,
    progress: &mut Progress,
) -> Result<()> {
    let romfs = settings.romfs()?;
    let blender = if options.check || !options.skip_blender {
        Some(settings.blender()?)
    } else {
        None
    };
    let restbl = settings.restbl()?;
    let model_rel = Path::new(&project.data.model);
    if !romfs.root().join(model_rel).is_file() {
        return Err(Error(format!(
            "The game's model {} isn't in {}",
            project.data.model,
            romfs.root().display()
        )));
    }
    let blend = project.blend();
    if !blend.is_file() {
        return Err(Error(format!(
            "The Blender scene {} isn't there",
            blend.display()
        )));
    }
    let work = project.work_dir();
    std::fs::create_dir_all(&work).context(work.display())?;
    let mod_romfs = project.mod_romfs();
    let glb = work.join("model.glb");
    let scripts = tools::scripts_dir(&settings.cache_dir())?;
    let prep = &project.data.prep;

    // 1. The game's model
    progress.step("Reading the game's model");
    let original = work.join("original");
    let vanilla_json = original.join("model.json");
    model::export(&romfs, model_rel, &original)?;
    let vanilla = read_json(&vanilla_json)?;
    decode_colour_versions(&romfs, &vanilla, &original, progress)?;
    progress.check()?;

    // 2. Blender
    let prep_report_path = work.join("prep_report.json");
    if let Some(blender) = blender.filter(|_| !options.skip_blender) {
        progress.step("Preparing the scene in Blender");
        let mut config = prep.clone();
        config.insert("vanilla_json".into(), json!(vanilla_json));
        config.insert("source_textures".into(), json!(original));
        config.insert("output".into(), json!(work));
        config.insert("glb_name".into(), json!("model.glb"));
        // Each mesh keeps its own skin count (up to 8).
        let max_weights = prep.get("max_weights").and_then(Value::as_u64).unwrap_or(8);
        config.insert("max_weights".into(), json!(max_weights.max(8)));
        let config_path = work.join("prep_config.json");
        write_json(&config_path, &Value::Object(config))?;
        fsutil::remove_file(&glb)?;
        fsutil::remove_file(&prep_report_path)?;
        let ran = tools::run(
            blender,
            &args![
                "--background",
                &blend,
                "--python",
                scripts.join("prep_for_game.py"),
                "--",
                "--config",
                &config_path,
            ],
            &work.join("2_blender.log"),
            &|l| l.starts_with("[prep]") || l.contains("Error"),
            &mut tagged(progress, "[prep] "),
        );
        // Problems the scene has are clearer than Blender's exit code.
        if let Ok(prep_report) = read_json(&prep_report_path) {
            let errors = strings(&prep_report["errors"]);
            if !errors.is_empty() {
                return Err(Error(format!(
                    "Blender found problems in the scene:\n  {}",
                    errors.join("\n  ")
                )));
            }
            report.warnings.extend(strings(&prep_report["warnings"]));
        }
        ran?;
    } else if !glb.is_file() {
        return Err(Error(format!(
            "No {} from an earlier build to reuse",
            glb.display()
        )));
    }
    let prep_report = read_json(&prep_report_path).unwrap_or(Value::Null);
    progress.check()?;

    // 3. The mod's model and textures. An earlier build's files go first: a texture no longer changed must
    // not stay in the mod.
    progress.step("Building the model and textures");
    let model_out = mod_romfs.join(model_rel);
    fsutil::remove_file(&model_out)?;
    for rel in fsutil::files_under(&mod_romfs.join(TEXTURE_DIR)) {
        if rel.to_string_lossy().ends_with(".bntx.zs") {
            fsutil::remove_file(&mod_romfs.join(TEXTURE_DIR).join(rel))?;
        }
    }
    let summary = replace::replace_model(&romfs, model_rel, &glb, &mod_romfs)?;
    for s in &summary.geometry.shapes {
        progress.line(format!(
            "{:44} {:9} {:>6} vertices {:>6} triangles",
            s.name, s.status, s.vertices, s.triangles
        ));
    }
    report
        .warnings
        .extend(summary.geometry.warnings.iter().cloned());
    for w in &summary.geometry.warnings {
        progress.warn(w.clone());
    }
    let textures = work.join("textures");
    if fsutil::files_under(&textures)
        .iter()
        .any(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("png")))
    {
        let (written, warnings) = texture::replace_textures(&romfs, &textures, &mod_romfs)?;
        for t in &written {
            progress.line(format!(
                "texture {:40} {} -> {} {}x{}",
                t.name, t.from, t.to, t.width, t.height
            ));
        }
        report.textures = written
            .into_iter()
            .map(|t| TextureRow {
                name: t.name,
                from: t.from,
                to: t.to,
                width: t.width,
                height: t.height,
            })
            .collect();
        for w in warnings {
            progress.warn(w.clone());
            report.warnings.push(w);
        }
    }
    progress.check()?;

    // 4. The result, read back (with the mod's textures)
    progress.step("Reading the result");
    let with_mod = Romfs::open(romfs.root())?.with_overlay(&mod_romfs);
    let mut decoder = with_mod.decoder()?;
    let loaded = Loaded::read(&with_mod, &mut decoder, model_rel)?;
    let result = serde_json::to_value(model::inspect(
        &with_mod,
        &mut decoder,
        &loaded.bfres().context("the built model")?,
    )?)
    .context("result.json")?;
    write_json(&work.join("result.json"), &result)?;
    let (problems, rows) =
        compare_models(&vanilla, &result, &summary.geometry.shapes, &prep_report);
    for r in &rows {
        progress.line(format!(
            "{:34} {:16} vertices {:>6} -> {:<6} skin {}  LODs {}  {}",
            r.name, r.material, r.game_vertices, r.vertices, r.skin_count, r.lods, r.status
        ));
    }
    report.meshes = rows;
    if !problems.is_empty() {
        return Err(Error(format!(
            "The result doesn't match the game's model:\n  {}",
            problems.join("\n  ")
        )));
    }
    progress.check()?;

    // 5. Does it look like the scene?
    if let Some(blender) = blender.filter(|_| options.check) {
        report.check = Some(check_scene(
            blender,
            &scripts,
            project,
            &romfs,
            &mod_romfs,
            &vanilla,
            &prep_report,
            progress,
        )?);
    }

    // 6. The mod's own size table (the last build's goes first: restbl would count it as one of the mod's files)
    let resource = mod_romfs.join("System").join("Resource");
    for rel in fsutil::files_under(&resource) {
        if rel.to_string_lossy().to_lowercase().contains(".rsizetable") {
            fsutil::remove_file(&resource.join(rel))?;
        }
    }
    let version = settings.game_version()?;
    progress.step(format!("Resource size table ({} {version})", settings.game));
    tools::run(
        restbl,
        &args![
            project.mod_dir(),
            "--game",
            &settings.game,
            "--version",
            &version
        ],
        &work.join("6_restbl.log"),
        &|_| true,
        progress,
    )?;

    // 7. Install
    if options.install {
        progress.step("Installing");
        let target = install(settings, project, &mod_romfs, progress)?;
        report.size_table = mods::rebuild_size_table(settings, &work, progress)?;
        report.installed = Some(target);
    }

    report.files = fsutil::files_under(&mod_romfs)
        .iter()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .collect();
    progress.step(format!("Built {}", project.data.name));
    for f in &report.files {
        progress.line(format!("romfs/{f}"));
    }
    if let Some(check) = report.check.as_ref().filter(|c| !c.matches) {
        let mut meshes: Vec<&str> = check
            .differences
            .iter()
            .flat_map(|d| d.meshes.iter().map(String::as_str))
            .collect();
        meshes.sort();
        meshes.dedup();
        progress.warn(format!(
            "the result doesn't look like the scene in {} place(s), on {}",
            check.differences.len(),
            meshes.join(", ")
        ));
    }
    Ok(())
}

/// Lines from a Blender script, without its tag.
fn tagged<'p, 'a: 'p>(progress: &'p mut Progress<'a>, tag: &'static str) -> Progress<'p> {
    progress.map_lines(move |l| match l.strip_prefix(tag) {
        Some(rest) => rest.to_owned(),
        None => l,
    })
}

/// A texture `<name>.0` is one of several colour versions (`Tex/<name>.1.bntx.zs`, ...; the game picks one
/// per character). The export has version 0; the bake needs the others too.
fn decode_colour_versions(
    romfs: &Romfs,
    vanilla: &Value,
    folder: &Path,
    progress: &mut Progress,
) -> Result<()> {
    let mut decoder = romfs.decoder()?;
    let mut made = Vec::new();
    for t in vanilla["textures"].as_array().into_iter().flatten() {
        let Some(base) = t["name"].as_str().and_then(|n| n.strip_suffix(".0")) else {
            continue;
        };
        let mut k = 1;
        loop {
            let path = romfs
                .root()
                .join(TEXTURE_DIR)
                .join(format!("{base}.{k}.bntx.zs"));
            if !path.is_file() {
                break;
            }
            let data = romfs::read_maybe_zs(&mut decoder, &path).context(path.display())?;
            let bntx = Bntx::parse(&data).context(path.display())?;
            let texture = bntx
                .textures
                .first()
                .ok_or_else(|| Error(format!("{}: no texture", path.display())))?;
            let mut image = texture.decode(0, 0).context(path.display())?;
            texture.apply_channel_sources(&mut image);
            let out = folder.join(format!("{base}.{k}.png"));
            std::fs::write(&out, image.to_png()?).context(out.display())?;
            k += 1;
        }
        made.push(format!("{base}.0 and {} more", k - 1));
    }
    if !made.is_empty() {
        progress.line(format!("colour versions: {}", made.join(", ")));
    }
    Ok(())
}

/// Each game mesh with what the build did to it, and problems: a game mesh missing from the result, or with
/// another material, skin count or LOD count.
fn compare_models(
    vanilla: &Value,
    result: &Value,
    shapes: &[replace::ShapeReport],
    prep_report: &Value,
) -> (Vec<String>, Vec<MeshRow>) {
    let meshes = |v: &Value| -> Vec<Value> {
        v["models"][0]["meshes"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    };
    let (vm, rm) = (meshes(vanilla), meshes(result));
    let mut problems = Vec::new();
    let mut rows = Vec::new();
    for v in &vm {
        let name = v["name"].as_str().unwrap_or_default();
        let Some(r) = rm.iter().find(|r| r["name"] == v["name"]) else {
            problems.push(format!("game mesh {name} is missing from the result"));
            continue;
        };
        for (key, what) in [
            ("material", "material"),
            ("skinCount", "skin count"),
            ("lods", "LOD count"),
        ] {
            if r[key] != v[key] {
                problems.push(format!("{name}: {what} {} instead of {}", r[key], v[key]));
            }
        }
        let shape = shapes.iter().find(|s| s.name == name);
        rows.push(MeshRow {
            name: name.to_owned(),
            status: shape.map_or("kept".into(), |s| s.status.clone()),
            material: r["material"].as_str().unwrap_or_default().to_owned(),
            game_vertices: v["vertices"].as_u64().unwrap_or(0),
            vertices: r["vertices"].as_u64().unwrap_or(0),
            triangles: shape.map_or(r["triangles"].as_u64().unwrap_or(0), |s| {
                u64::from(s.triangles)
            }),
            skin_count: r["skinCount"].as_u64().unwrap_or(0),
            lods: r["lods"].as_u64().unwrap_or(0),
            joined: strings(&prep_report["meshes"][name]["joined"]),
        });
    }
    for r in &rm {
        if !vm.iter().any(|v| v["name"] == r["name"]) {
            problems.push(format!(
                "{} is a new mesh; its material may be wrong",
                r["name"].as_str().unwrap_or_default()
            ));
        }
    }
    (problems, rows)
}

/// Step 5: renders the scene and the built model the same way and compares them (check_result.py).
#[allow(clippy::too_many_arguments)]
fn check_scene(
    blender: &Path,
    scripts: &Path,
    project: &Project,
    romfs: &Romfs,
    mod_romfs: &Path,
    vanilla: &Value,
    prep_report: &Value,
    progress: &mut Progress,
) -> Result<CheckSummary> {
    progress.step("Comparing the result with the scene");
    let work = project.work_dir();
    let check = work.join("check");
    fsutil::remove_dir(&check)?;
    let result_dir = check.join("result");
    let with_mod = Romfs::open(romfs.root())?.with_overlay(mod_romfs);
    let export = model::export(&with_mod, Path::new(&project.data.model), &result_dir)?;

    let mut joined = Map::new();
    if let Some(meshes) = prep_report["meshes"].as_object() {
        for (target, m) in meshes {
            for n in strings(&m["joined"]) {
                joined.insert(n, json!(target));
            }
        }
    }
    let prep = &project.data.prep;
    let mut config = project.data.check.clone();
    config.insert("out".into(), json!(check));
    config.insert("result_glb".into(), json!(export.glb));
    config.insert("game_bones".into(), vanilla["models"][0]["bones"].clone());
    config.insert("joined".into(), Value::Object(joined));
    config.insert(
        "armature".into(),
        prep.get("armature").cloned().unwrap_or(json!("Armature")),
    );
    config.insert(
        "work_suffix".into(),
        prep.get("work_suffix").cloned().unwrap_or(json!("_WORK")),
    );
    config.insert(
        "exclude".into(),
        prep.get("exclude").cloned().unwrap_or(json!([])),
    );
    let config_path = check.join("check_config.json");
    write_json(&config_path, &Value::Object(config))?;
    let script = scripts.join("check_result.py");
    let show = |l: &str| l.starts_with("[check]") || l.contains("Error");
    progress.line("rendering the scene");
    tools::run(
        blender,
        &args![
            "--background",
            project.blend(),
            "--python",
            &script,
            "--",
            "--config",
            &config_path,
            "--side",
            "scene"
        ],
        &work.join("5_check_scene.log"),
        &show,
        &mut tagged(progress, "[check] "),
    )?;
    progress.line("rendering the result");
    tools::run(
        blender,
        &args![
            "--background",
            "--factory-startup",
            "--python",
            &script,
            "--",
            "--config",
            &config_path,
            "--side",
            "result"
        ],
        &work.join("5_check_result.log"),
        &show,
        &mut tagged(progress, "[check] "),
    )?;
    let report = read_json(&check.join("check_report.json"))?;
    let differences: Vec<Difference> = report["differences"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|d| Difference {
            view: d["view"].as_str().unwrap_or_default().to_owned(),
            pose: d["pose"].as_str().unwrap_or_default().to_owned(),
            kind: d["kind"].as_str().unwrap_or_default().to_owned(),
            pixels: d["pixels"].as_u64().unwrap_or(0),
            meshes: d["meshes"]
                .as_object()
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default(),
        })
        .collect();
    let summary = CheckSummary {
        matches: report["matches"].as_bool().unwrap_or(false),
        images: strings(&report["images"])
            .into_iter()
            .map(PathBuf::from)
            .collect(),
        not_compared: strings(&report["not_compared"]),
        differences,
    };
    if summary.matches {
        progress
            .line("The result looks like the scene (base colour, every view, rest and test pose).");
    } else {
        let mut by_mesh: std::collections::BTreeMap<&str, Vec<String>> = Default::default();
        for d in &summary.differences {
            for m in &d.meshes {
                by_mesh.entry(m).or_default().push(format!(
                    "{} {} ({}, {} px)",
                    d.view, d.pose, d.kind, d.pixels
                ));
            }
        }
        for (mesh, where_) in by_mesh {
            progress.warn(format!(
                "{mesh} looks different from the scene: {}",
                where_.join(", ")
            ));
        }
    }
    for item in &summary.not_compared {
        progress.line(format!(
            "Note: not compared exactly (a shader the check can't flatten): {item}"
        ));
    }
    Ok(summary)
}

/// Copies the mod into Hookshot's mods folder: the folder gets exactly this build (files of an earlier one
/// go), without the mod's own size table (one is made for all mods). A mod that was turned off is turned on.
fn install(
    settings: &Settings,
    project: &Project,
    mod_romfs: &Path,
    progress: &mut Progress,
) -> Result<PathBuf> {
    let target = project.install_dir(settings);
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if name.is_empty() || name == "mods" || name == "load" {
        // Hookshot reads <mods>/<name>/romfs, never <mods>/romfs.
        return Err(Error(format!(
            "The install folder is the mods folder itself ({}); name the mod's folder in it",
            target.display()
        )));
    }
    let off = target.with_file_name(format!(
        "-{}",
        target.file_name().unwrap_or_default().to_string_lossy()
    ));
    if !target.exists() && off.join("romfs").is_dir() {
        std::fs::rename(&off, &target).context(format!("turning on {}", off.display()))?;
        progress.line(format!("{} was turned off: turned on", project.data.name));
    }
    let romfs_dir = target.join("romfs");
    fsutil::remove_dir(&romfs_dir)?;
    let n = fsutil::copy_tree(mod_romfs, &romfs_dir, |rel| !is_size_table(rel))?;
    progress.line(format!("{n} file(s) to {}", romfs_dir.display()));
    Ok(target)
}

fn read_json(path: &Path) -> Result<Value> {
    let text = std::fs::read_to_string(path).context(path.display())?;
    serde_json::from_str(&text).context(path.display())
}

fn write_json(path: &Path, value: &Value) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).context(dir.display())?;
    }
    let text = serde_json::to_string_pretty(value).context(path.display())?;
    std::fs::write(path, text).context(path.display())
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s.as_str().map(str::to_owned))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mesh(name: &str, material: &str, skin: u64, lods: u64) -> Value {
        json!({"name": name, "material": material, "skinCount": skin, "lods": lods, "vertices": 10, "triangles": 4})
    }

    #[test]
    fn compares_meshes_with_the_game() {
        let vanilla =
            json!({"models": [{"meshes": [mesh("A", "Mt_A", 4, 3), mesh("B", "Mt_B", 1, 2)]}]});
        let same = vanilla.clone();
        let shapes = vec![replace::ShapeReport {
            name: "A".into(),
            status: "replaced".into(),
            vertices: 12,
            triangles: 6,
        }];
        let prep = json!({"meshes": {"A": {"joined": ["Hat"]}}});
        let (problems, rows) = compare_models(&vanilla, &same, &shapes, &prep);
        assert!(problems.is_empty());
        assert_eq!(rows[0].status, "replaced");
        assert_eq!(rows[0].triangles, 6);
        assert_eq!(rows[0].joined, ["Hat"]);
        assert_eq!(rows[1].status, "kept");

        let changed =
            json!({"models": [{"meshes": [mesh("A", "Mt_B", 4, 1), mesh("C", "Mt_C", 0, 1)]}]});
        let (problems, _) = compare_models(&vanilla, &changed, &shapes, &Value::Null);
        assert_eq!(problems.len(), 4, "{problems:?}");
        assert!(problems.iter().any(|p| p.contains("B is missing")));
        assert!(problems.iter().any(|p| p.contains("C is a new mesh")));
    }
}
