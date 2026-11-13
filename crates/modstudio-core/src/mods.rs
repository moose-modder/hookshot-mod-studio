//! Hookshot's mods folder: the mods in it, turning them on and off, and one resource size table for all of them.
//!
//! Hookshot reads `<mods>/<name>/romfs/...` (the layout emulators use). A folder whose name starts with '-' or
//! '_' is off. When two mods have the same file, the one whose folder sorts last wins.
//!
//! The game reads one resource size table (RESTBL). When several mods each ship one, Hookshot uses the last
//! mod's, so the other mods' grown files get the game's sizes and fail to load (a texture then shows as a
//! default one). So one table is worked out over the files of every enabled mod, as Hookshot overlays them, and
//! installed as its own mod, [`TABLE_MOD`], which sorts after the others.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::fsutil::{self, is_size_table};
use crate::progress::Progress;
use crate::settings::Settings;
use crate::{Context, Error, Result, sizetable};

/// The mod that holds the size table for all mods.
pub const TABLE_MOD: &str = "zzz-ResourceSizeTable";

#[derive(Debug, Clone, Serialize)]
pub struct InstalledMod {
    /// The name without the '-' or '_' that turns it off.
    pub name: String,
    /// The folder's name in the mods folder.
    pub folder: String,
    pub enabled: bool,
    pub files: usize,
    /// Model files it replaces or adds (paths in the romfs).
    pub models: Vec<String>,
    pub textures: usize,
    /// It has its own size table, which is not used (the shared one is).
    pub own_size_table: bool,
}

fn is_off(folder: &str) -> bool {
    folder.starts_with(['-', '_'])
}

/// Every mod in the mods folder (folders with a `romfs`), sorted as Hookshot sorts them; not the size table's.
pub fn list(mods_dir: &Path) -> Result<Vec<InstalledMod>> {
    let Ok(entries) = std::fs::read_dir(mods_dir) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for e in entries.flatten() {
        let folder = e.file_name().to_string_lossy().into_owned();
        let romfs = e.path().join("romfs");
        if folder == TABLE_MOD || !romfs.is_dir() {
            continue;
        }
        let files = fsutil::files_under(&romfs);
        let slash = |p: &PathBuf| p.to_string_lossy().replace('\\', "/");
        out.push(InstalledMod {
            name: folder.trim_start_matches(['-', '_']).to_owned(),
            enabled: !is_off(&folder),
            models: files
                .iter()
                .map(slash)
                .filter(|p| p.starts_with("Model/") && p.ends_with(".bfres.zs"))
                .collect(),
            textures: files
                .iter()
                .map(slash)
                .filter(|p| p.starts_with("Tex/"))
                .count(),
            own_size_table: files.iter().any(|p| is_size_table(p)),
            files: files.len(),
            folder,
        });
    }
    out.sort_by_key(|m| m.folder.to_lowercase());
    Ok(out)
}

/// Turns a mod on or off by renaming its folder (`Name` <-> `-Name`). Returns the new folder name. Hookshot
/// reads the mods folder when it starts, so it takes a restart; the size table needs rebuilding.
pub fn set_enabled(mods_dir: &Path, folder: &str, enabled: bool) -> Result<String> {
    if folder.is_empty() || folder.contains(['/', '\\']) || folder == TABLE_MOD {
        return Err(Error(format!("not a mod folder: {folder:?}")));
    }
    let from = mods_dir.join(folder);
    if !from.join("romfs").is_dir() {
        return Err(Error(format!("{} is not a mod", from.display())));
    }
    let name = folder.trim_start_matches(['-', '_']);
    let new = if enabled {
        name.to_owned()
    } else {
        format!("-{name}")
    };
    if new == folder {
        return Ok(new);
    }
    let to = mods_dir.join(&new);
    if to.exists() {
        return Err(Error(format!(
            "{} already exists: two copies of {name}",
            to.display()
        )));
    }
    std::fs::rename(&from, &to).context(format!("renaming {}", from.display()))?;
    Ok(new)
}

/// The shared size table, if one is installed.
pub fn size_table(mods_dir: &Path) -> Option<PathBuf> {
    let romfs = mods_dir.join(TABLE_MOD).join("romfs");
    fsutil::files_under(&romfs)
        .into_iter()
        .find(|p| is_size_table(p))
        .map(|p| romfs.join(p))
}

/// Works out one size table for every enabled mod ([`sizetable`]) and installs it as [`TABLE_MOD`]. Returns the
/// table, or `None` when no mod needs one (every file fits the game's own table); an old table is then removed.
pub fn rebuild_size_table(settings: &Settings, progress: &mut Progress) -> Result<Option<PathBuf>> {
    let mods_dir = &settings.install_mods;
    let mods: Vec<InstalledMod> = list(mods_dir)?.into_iter().filter(|m| m.enabled).collect();
    // Each file once, the last mod's (as Hookshot overlays them), by its path ignoring case.
    let mut files: BTreeMap<String, (String, PathBuf)> = BTreeMap::new();
    let mut own_tables = Vec::new();
    for m in &mods {
        for (rel, path) in sizetable::files_of(&mods_dir.join(&m.folder).join("romfs")) {
            if is_size_table(Path::new(&rel)) {
                own_tables.push(m.folder.clone());
            } else {
                files.insert(rel.to_lowercase(), (rel, path));
            }
        }
    }
    let names: Vec<&str> = mods.iter().map(|m| m.folder.as_str()).collect();
    let table_mod = mods_dir.join(TABLE_MOD);
    fsutil::remove_dir(&table_mod)?;
    if files.is_empty() {
        progress.line("No mod files: no resource size table needed");
        return Ok(None);
    }
    progress.line(format!(
        "{} file(s) of {} mod(s) {names:?}",
        files.len(),
        mods.len()
    ));
    let files: BTreeMap<String, PathBuf> = files.into_values().collect();
    let report = sizetable::write_table(
        &settings.romfs()?,
        &files,
        &table_mod.join("romfs"),
        progress,
    )?;
    if report.table.is_none() {
        fsutil::remove_dir(&table_mod)?;
    }
    if !own_tables.is_empty() {
        own_tables.sort();
        own_tables.dedup();
        progress.line(format!("(their own tables are not used: {own_tables:?})"));
    }
    Ok(report.table)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_and_off() {
        let dir = std::env::temp_dir().join(format!("modstudio-mods-{}", std::process::id()));
        for (folder, file) in [
            ("BMod", "Model/X.X.bfres.zs"),
            ("-AMod", "Tex/T.bntx.zs"),
            (TABLE_MOD, "System/Resource/R.rsizetable.zs"),
        ] {
            let p = dir.join(folder).join("romfs").join(file);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        }
        std::fs::create_dir_all(dir.join("NotAMod")).unwrap();
        let mods = list(&dir).unwrap();
        let folders: Vec<&str> = mods.iter().map(|m| m.folder.as_str()).collect();
        assert_eq!(folders, ["-AMod", "BMod"]);
        assert!(!mods[0].enabled && mods[0].name == "AMod" && mods[0].textures == 1);
        assert_eq!(mods[1].models, ["Model/X.X.bfres.zs"]);
        assert!(size_table(&dir).is_some());

        assert_eq!(set_enabled(&dir, "-AMod", true).unwrap(), "AMod");
        assert_eq!(set_enabled(&dir, "BMod", false).unwrap(), "-BMod");
        assert_eq!(set_enabled(&dir, "AMod", true).unwrap(), "AMod");
        assert!(dir.join("AMod").is_dir() && dir.join("-BMod").is_dir());
        assert!(set_enabled(&dir, TABLE_MOD, false).is_err());
        assert!(set_enabled(&dir, "NotAMod", false).is_err());
        fsutil::remove_dir(&dir).unwrap();
    }
}
