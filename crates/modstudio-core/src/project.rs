//! A project: one game model being modded, in its own folder of the workspace:
//!
//! - `project.json`: the mod's name, the model, the scene, and settings for the build ([`ProjectFile`])
//! - the export of the game's model the scene was made from: `<model>.glb`, its textures as PNG, `model.json`
//! - `<folder>.blend`: the Blender scene the user edits
//! - `build/`: the build's work files and logs; `build/build_report.json` is the last build's [`BuildReport`]
//! - `mod/romfs`: the built mod, as it's installed into `<mods>/<name>/romfs`

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::build::{BuildReport, REPORT_FILE};
use crate::progress::Progress;
use crate::settings::Settings;
use crate::{Context, Error, Result, args, fsutil, model, tools};

pub const PROJECT_FILE: &str = "project.json";

/// `project.json`. Paths are relative to the project's folder (absolute ones work too). Keys the pipeline's
/// project files had for tools and the game (`tools`, `game_romfs`, ...) are ignored: those are settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectFile {
    /// The mod's name: its folder in Hookshot's mods folder.
    pub name: String,
    /// The game's model, in the romfs (`Model/<name>.<name>.bfres.zs`).
    pub model: String,
    /// The Blender scene.
    pub blend: String,
    /// The build's work files (default `build`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_dir: Option<String>,
    /// The built mod, with its `romfs` (default `mod`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mod_dir: Option<String>,
    /// Where it's installed (default `<mods>/<name>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_dir: Option<String>,
    /// Settings for preparing the scene (prep_for_game.py; pipeline/README.md).
    #[serde(default)]
    pub prep: Map<String, Value>,
    /// Settings for the build check (check_result.py).
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub check: Map<String, Value>,
}

#[derive(Debug, Clone)]
pub struct Project {
    /// The `project.json` file.
    pub file: PathBuf,
    pub data: ProjectFile,
}

impl Project {
    /// Reads `path`: a `project.json` (or an old pipeline project file), or a project folder.
    pub fn load(path: &Path) -> Result<Self> {
        let file = if path.is_dir() {
            path.join(PROJECT_FILE)
        } else {
            path.to_owned()
        };
        let text = std::fs::read_to_string(&file).context(file.display())?;
        let data = serde_json::from_str(&text).context(file.display())?;
        Ok(Project { file, data })
    }

    pub fn save(&self) -> Result<()> {
        let text = serde_json::to_string_pretty(&self.data).context("project")?;
        std::fs::write(&self.file, text).context(self.file.display())
    }

    pub fn dir(&self) -> &Path {
        self.file.parent().unwrap_or(Path::new("."))
    }

    fn path(&self, p: &str) -> PathBuf {
        let p = Path::new(p);
        if p.is_absolute() {
            p.to_owned()
        } else {
            self.dir().join(p)
        }
    }

    pub fn blend(&self) -> PathBuf {
        self.path(&self.data.blend)
    }

    pub fn work_dir(&self) -> PathBuf {
        self.path(self.data.work_dir.as_deref().unwrap_or("build"))
    }

    pub fn mod_dir(&self) -> PathBuf {
        self.path(self.data.mod_dir.as_deref().unwrap_or("mod"))
    }

    pub fn mod_romfs(&self) -> PathBuf {
        self.mod_dir().join("romfs")
    }

    pub fn install_dir(&self, settings: &Settings) -> PathBuf {
        match &self.data.install_dir {
            Some(p) => self.path(p),
            None => settings.install_mods.join(&self.data.name),
        }
    }

    /// The last build's report, if there was one.
    pub fn last_build(&self) -> Option<BuildReport> {
        let text = std::fs::read_to_string(self.work_dir().join(REPORT_FILE)).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn summary(&self, settings: &Settings) -> Summary {
        let last = self.last_build();
        let install = self.install_dir(settings);
        let off = install
            .file_name()
            .map(|n| install.with_file_name(format!("-{}", n.to_string_lossy())));
        Summary {
            file: self.file.clone(),
            dir: self.dir().to_owned(),
            folder: self
                .dir()
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            name: self.data.name.clone(),
            model: self.data.model.clone(),
            model_name: model_name(&self.data.model),
            blend: self.blend(),
            blend_saved: fsutil::modified(&self.blend()),
            last_build: last.as_ref().map(|r| r.finished),
            last_build_ok: last.as_ref().map(|r| r.ok),
            installed: if install.join("romfs").is_dir() {
                Some(true)
            } else if off.is_some_and(|o| o.join("romfs").is_dir()) {
                Some(false)
            } else {
                None
            },
            install_dir: install,
        }
    }
}

/// A project as the app lists it.
#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub file: PathBuf,
    pub dir: PathBuf,
    pub folder: String,
    pub name: String,
    pub model: String,
    pub model_name: String,
    pub blend: PathBuf,
    /// When the scene was last saved (seconds since 1970).
    pub blend_saved: Option<u64>,
    pub last_build: Option<u64>,
    pub last_build_ok: Option<bool>,
    /// `Some(true)` installed and on, `Some(false)` installed but turned off, `None` not installed.
    pub installed: Option<bool>,
    pub install_dir: PathBuf,
}

/// `Npc_Hylia_MN_001` from `Model/Npc_Hylia_MN_001.Npc_Hylia_MN_001.bfres.zs` (the model, not its group).
pub fn model_name(model: &str) -> String {
    crate::catalog::split_name(model).1.to_owned()
}

/// The projects in the workspace (folders with a `project.json`), newest scene first.
pub fn list(settings: &Settings) -> Vec<Summary> {
    let Ok(entries) = std::fs::read_dir(&settings.workspace) else {
        return Vec::new();
    };
    let mut out: Vec<Summary> = entries
        .flatten()
        .filter(|e| e.path().join(PROJECT_FILE).is_file())
        .filter_map(|e| Project::load(&e.path()).ok())
        .map(|p| p.summary(settings))
        .collect();
    out.sort_by_key(|s| std::cmp::Reverse(s.blend_saved.max(s.last_build)));
    out
}

/// A folder name for a project: letters, digits, '_' and '-'.
pub fn check_folder_name(folder: &str) -> Result<()> {
    if folder.is_empty()
        || folder.starts_with(['-', '_', '.'])
        || !folder
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(Error(format!(
            "{folder:?}: use letters, digits, '_' and '-' (not first) for a project's name"
        )));
    }
    Ok(())
}

/// The mod's name for a project folder: `townsman` -> `TownsmanMod`.
pub fn mod_name_for(folder: &str) -> String {
    let mut c = folder.chars();
    match c.next() {
        Some(first) => format!("{}{}Mod", first.to_uppercase(), c.as_str()),
        None => "Mod".into(),
    }
}

/// Makes a project for `model` (a path in the romfs) in `<workspace>/<folder>`: exports the model, makes the
/// Blender scene from it and writes `project.json`. A folder that already has a project is left alone; one
/// with an earlier, unfinished start is used again (an existing scene is kept).
pub fn create(
    settings: &Settings,
    model_path: &str,
    folder: &str,
    progress: &mut Progress,
) -> Result<Project> {
    check_folder_name(folder)?;
    let romfs = settings.romfs()?;
    let blender = settings.blender()?;
    let model_path = model_path.replace('\\', "/");
    if !romfs.root().join(&model_path).is_file() {
        return Err(Error(format!(
            "{model_path} isn't in {}",
            romfs.root().display()
        )));
    }
    let dir = settings.workspace.join(folder);
    let file = dir.join(PROJECT_FILE);
    if file.exists() {
        return Err(Error(format!(
            "There's already a project called {folder} ({})",
            dir.display()
        )));
    }
    std::fs::create_dir_all(&dir).context(dir.display())?;

    progress.step("Exporting the model");
    let export = model::export(&romfs, Path::new(&model_path), &dir)?;
    progress.line(format!(
        "{} meshes, {} textures",
        export.meshes,
        export.textures.len()
    ));
    for s in &export.skipped {
        progress.warn(format!("left out: {s}"));
    }
    if !export.missing_textures.is_empty() {
        progress.line(format!(
            "textures the game doesn't have (left out): {}",
            export.missing_textures.join(", ")
        ));
    }
    progress.check()?;

    let blend = dir.join(format!("{folder}.blend"));
    if blend.exists() {
        progress.step("Blender scene");
        progress.line(format!("{} exists, kept", blend.display()));
    } else {
        progress.step("Making the Blender scene");
        let scripts = tools::scripts_dir(&settings.cache_dir())?;
        tools::run(
            blender,
            &args![
                "--background",
                "--factory-startup",
                "--python-exit-code",
                "1",
                "--python",
                scripts.join("make_scene.py"),
                "--",
                "--glb",
                &export.glb,
                "--blend",
                &blend,
            ],
            &dir.join("logs").join("new_scene.log"),
            &|l| l.starts_with("[scene]") || l.contains("Error"),
            progress,
        )?;
    }

    let mut prep = Map::new();
    prep.insert("armature".into(), json!("Armature"));
    prep.insert("max_weights".into(), json!(8));
    prep.insert("merge".into(), json!([]));
    let project = Project {
        file,
        data: ProjectFile {
            name: mod_name_for(folder),
            model: model_path,
            blend: format!("{folder}.blend"),
            work_dir: None,
            mod_dir: None,
            install_dir: None,
            prep,
            check: Map::new(),
        },
    };
    project.save()?;
    progress.line(format!("Project {}", project.file.display()));
    Ok(project)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(mod_name_for("townsman"), "TownsmanMod");
        assert_eq!(
            model_name("Model/Npc_Hylia_MN_001.Npc_Hylia_MN_001.bfres.zs"),
            "Npc_Hylia_MN_001"
        );
        assert!(check_folder_name("great_fairy-2").is_ok());
        for bad in ["", "-x", "a b", "a/b", "..", "é"] {
            assert!(check_folder_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn paths_from_the_project_folder() {
        // An old pipeline project file: absolute paths, tools and the game's paths (ignored).
        let root = if cfg!(windows) { "C:/w" } else { "/w" };
        let old = r#"{"name": "TownsmanMod", "game": "oot", "game_romfs": "C:/g/romfs",
            "model": "Model/Npc_Hylia_MN_001.Npc_Hylia_MN_001.bfres.zs",
            "blend": "ROOT/townsman/townsman.blend", "work_dir": "ROOT/townsman/build",
            "mod_dir": "ROOT/TownsmanMod", "install_dir": "C:/g/mods/TownsmanMod",
            "tools": {"blender": "b"}, "prep": {"armature": "Armature"}, "toolbox_options": []}"#
            .replace("ROOT", root);
        let data: ProjectFile = serde_json::from_str(&old).unwrap();
        let p = Project {
            file: PathBuf::from(format!("{root}/elsewhere/project.json")),
            data,
        };
        assert_eq!(
            p.blend(),
            PathBuf::from(format!("{root}/townsman/townsman.blend"))
        );
        assert_eq!(
            p.mod_romfs(),
            PathBuf::from(format!("{root}/TownsmanMod")).join("romfs")
        );

        let p = Project {
            file: PathBuf::from("/w/townsman/project.json"),
            data: ProjectFile {
                blend: "townsman.blend".into(),
                work_dir: None,
                mod_dir: None,
                install_dir: None,
                ..p.data
            },
        };
        let settings = Settings {
            install_mods: PathBuf::from("/g/mods"),
            ..Settings::default()
        };
        assert_eq!(p.blend(), Path::new("/w/townsman").join("townsman.blend"));
        assert_eq!(p.work_dir(), Path::new("/w/townsman").join("build"));
        assert_eq!(
            p.mod_romfs(),
            Path::new("/w/townsman").join("mod").join("romfs")
        );
        assert_eq!(
            p.install_dir(&settings),
            Path::new("/g/mods").join("TownsmanMod")
        );
    }
}
