//! Where things are: the game's romfs, Hookshot's mods folder, the workspace that holds projects, and the tools
//! (Blender, the resource size table calculator). One file, shared by the CLI and the app; its keys are the ones
//! the pipeline's `config.json` had, so an old `config.json` works as it is.

use std::path::{Path, PathBuf};

use modstudio_formats::romfs::Romfs;
use serde::{Deserialize, Serialize};

use crate::{Context, Error, Result, tools};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// The game, as the resource size table calculator names it.
    #[serde(default = "default_game")]
    pub game: String,
    /// The game's files: `<Hookshot>/romfs`.
    #[serde(default)]
    pub game_romfs: PathBuf,
    /// Hookshot's mods folder: `<Hookshot>/mods`.
    #[serde(default)]
    pub install_mods: PathBuf,
    /// Where projects are kept, one folder each.
    #[serde(default)]
    pub workspace: PathBuf,
    #[serde(default)]
    pub tools: Tools,
    /// The game's version for the resource size table ("111" for 1.1.1); found from the game's own table if
    /// not set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game_version: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tools {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blender: Option<PathBuf>,
    /// RESTBL Calculator's `restbl` (until the size table is done here, docs/plan.md phase 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restbl: Option<PathBuf>,
}

fn default_game() -> String {
    "oot".into()
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            game: default_game(),
            game_romfs: PathBuf::new(),
            install_mods: PathBuf::new(),
            workspace: PathBuf::new(),
            tools: Tools::default(),
            game_version: None,
        }
    }
}

/// What works and what doesn't, for the app's setup page.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Status {
    pub romfs: bool,
    pub mods: bool,
    pub workspace: bool,
    /// Blender's version line ("Blender 5.2.0"), if it runs.
    pub blender: Option<String>,
    pub restbl: bool,
    pub game_version: Option<String>,
    /// One line per problem, for the user.
    pub problems: Vec<String>,
}

impl Status {
    /// Everything a build needs is there.
    pub fn ready(&self) -> bool {
        self.problems.is_empty()
    }
}

impl Settings {
    /// The settings file the CLI and the app share: `%APPDATA%\HookshotModStudio\settings.json` on Windows,
    /// `$XDG_CONFIG_HOME/hookshot-mod-studio/settings.json` (or `~/.config/...`) elsewhere.
    /// `MODSTUDIO_SETTINGS`, if set, names another file (to try things without changing the real settings).
    pub fn default_path() -> Option<PathBuf> {
        if let Some(path) = std::env::var_os("MODSTUDIO_SETTINGS") {
            return Some(PathBuf::from(path));
        }
        if cfg!(windows) {
            std::env::var_os("APPDATA").map(|d| {
                PathBuf::from(d)
                    .join("HookshotModStudio")
                    .join("settings.json")
            })
        } else {
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
                .map(|d| d.join("hookshot-mod-studio").join("settings.json"))
        }
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).context(path.display())?;
        serde_json::from_str(&text).context(path.display())
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).context(dir.display())?;
        }
        let text = serde_json::to_string_pretty(self).context("settings")?;
        std::fs::write(path, text).context(path.display())
    }

    /// Settings for a Hookshot install folder (the one with `hookshot.exe`, `romfs` and `mods`), with the
    /// Blender that's found and a workspace in the user's home folder.
    pub fn for_hookshot(install: &Path) -> Self {
        let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .map(PathBuf::from)
            .unwrap_or_default();
        Settings {
            game_romfs: install.join("romfs"),
            install_mods: install.join("mods"),
            workspace: home.join("HookshotModStudio"),
            tools: Tools {
                blender: tools::find_blender().into_iter().next(),
                restbl: None,
            },
            ..Settings::default()
        }
    }

    pub fn romfs(&self) -> Result<Romfs> {
        if !is_romfs(&self.game_romfs) {
            return Err(Error(format!(
                "{} isn't the game's romfs (no Model folder or Pack/ZsDic.pack.zs)",
                self.game_romfs.display()
            )));
        }
        Ok(Romfs::open(&self.game_romfs)?)
    }

    pub fn blender(&self) -> Result<&Path> {
        tool(self.tools.blender.as_deref(), "Blender")
    }

    pub fn restbl(&self) -> Result<&Path> {
        tool(
            self.tools.restbl.as_deref(),
            "the resource size table calculator (restbl)",
        )
    }

    /// Where the app keeps what it makes for itself (the Blender scripts, previews): `<workspace>/.modstudio`.
    pub fn cache_dir(&self) -> PathBuf {
        self.workspace.join(".modstudio")
    }

    /// The game's version, from `game_version` or the name of the game's resource size table.
    pub fn game_version(&self) -> Result<String> {
        if let Some(v) = &self.game_version {
            return Ok(v.replace('.', ""));
        }
        let dir = self.game_romfs.join("System").join("Resource");
        let mut versions: Vec<String> = std::fs::read_dir(&dir)
            .context(dir.display())?
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let rest = name.strip_prefix("ResourceSizeTable.Product.")?;
                let (v, ext) = rest.split_once('.')?;
                (ext.contains("rsizetable")
                    && !v.is_empty()
                    && v.bytes().all(|b| b.is_ascii_digit()))
                .then(|| v.to_owned())
            })
            .collect();
        versions.sort();
        versions.dedup();
        match versions.as_slice() {
            [v] => Ok(v.clone()),
            _ => Err(Error(format!(
                "Could not tell the game's version from its resource size table in {} (set game_version, e.g. \"111\")",
                dir.display()
            ))),
        }
    }

    /// Checks every path and tool (runs Blender once for its version).
    pub fn status(&self) -> Status {
        let mut s = Status::default();
        if self.game_romfs.as_os_str().is_empty() {
            s.problems.push("Choose your Hookshot folder".into());
        } else if is_romfs(&self.game_romfs) {
            s.romfs = true;
            match self.game_version() {
                Ok(v) => s.game_version = Some(v),
                Err(e) => s.problems.push(e.0),
            }
        } else {
            s.problems.push(format!(
                "{} isn't the game's romfs: choose the Hookshot folder that holds romfs and mods",
                self.game_romfs.display()
            ));
        }
        if self.install_mods.as_os_str().is_empty() {
            s.problems.push("Choose Hookshot's mods folder".into());
        } else if self.install_mods.is_dir() || self.install_mods.parent().is_some_and(Path::is_dir)
        {
            // (a missing mods folder is made by the first install)
            s.mods = true;
        } else {
            s.problems
                .push(format!("{} can't be made", self.install_mods.display()));
        }
        if self.workspace.as_os_str().is_empty() {
            s.problems.push("Choose a folder for your projects".into());
        } else {
            s.workspace = true;
        }
        match self.blender().and_then(tools::blender_version) {
            Ok(v) => s.blender = Some(v),
            Err(e) => s.problems.push(e.0),
        }
        match self.restbl() {
            Ok(_) => s.restbl = true,
            Err(e) => s.problems.push(e.0),
        }
        s
    }
}

fn tool<'a>(path: Option<&'a Path>, name: &str) -> Result<&'a Path> {
    match path {
        Some(p) if p.is_file() => Ok(p),
        Some(p) => Err(Error(format!("{name} not found: {}", p.display()))),
        None => Err(Error(format!("Choose where {name} is"))),
    }
}

fn is_romfs(dir: &Path) -> bool {
    dir.join("Model").is_dir() && dir.join("Pack").join("ZsDic.pack.zs").is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_pipeline_config() {
        let old = r#"{"game": "oot", "game_romfs": "C:/Games/X/romfs", "workspace": "C:/w",
            "install_mods": "C:/Games/X/mods", "tools": {"blender": "C:/b/blender.exe",
            "modstudio": "C:/m/modstudio.exe", "restbl": "C:/r/restbl.exe"}}"#;
        let s: Settings = serde_json::from_str(old).unwrap();
        assert_eq!(s.game_romfs, PathBuf::from("C:/Games/X/romfs"));
        assert_eq!(s.tools.restbl, Some(PathBuf::from("C:/r/restbl.exe")));
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(s, Settings::default());
        assert_eq!(s.game, "oot");
    }

    #[test]
    fn version_from_the_table_name() {
        let dir = std::env::temp_dir().join(format!("modstudio-version-{}", std::process::id()));
        let res = dir.join("System").join("Resource");
        std::fs::create_dir_all(&res).unwrap();
        std::fs::write(
            res.join("ResourceSizeTable.Product.131.Nin_Ounce_NVN2.rsizetable.zs"),
            b"",
        )
        .unwrap();
        let s = Settings {
            game_romfs: dir.clone(),
            ..Settings::default()
        };
        assert_eq!(s.game_version().unwrap(), "131");
        let s = Settings {
            game_version: Some("1.2.0".into()),
            ..s
        };
        assert_eq!(s.game_version().unwrap(), "120");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
