//! The game's mods folder (Hookshot's, Longshot's): the mods in it, turning them on and off, and one resource
//! size table for all of them.
//!
//! The game reads `<mods>/<name>/romfs/...` (the layout emulators use), or deeper as emulators keep them
//! (`<mods>/contents/<title id>/<name>/romfs`). A folder whose name starts with '-' or '_' is off, with every mod
//! in it. When two mods have the same file, the one whose path in the mods folder sorts last (ignoring case) wins.
//! Longshot's loader is `host/mods.c` in its source; [`list`] finds mods as it does.
//!
//! The game reads one resource size table (RESTBL). When several mods each ship one, the game uses the last
//! mod's, so the other mods' grown files get the game's sizes and fail to load (a texture then shows as a
//! default one). So one table is worked out over the files of every enabled mod, as the game overlays them, and
//! installed as its own mod, [`TABLE_MOD`], which sorts after the others. A mod's own table is not lost: its
//! entries that are larger than the shared table's are taken over ([`sizetable::write_table`]).
//!
//! Mod Studio and the mod manager both rebuild that table, and either may be built from an older copy of this
//! crate. A stamp beside the table ([`Stamp`]) says which rules ([`TABLE_RULES`]) and which app built it, and an
//! app with older rules leaves a newer table alone: it would otherwise replace it with one that gives some files
//! too little room.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fsutil::{self, is_size_table};
use crate::progress::Progress;
use crate::{Context, Error, Result, open_game_romfs, sizetable};

/// The mod that holds the size table for all mods.
pub const TABLE_MOD: &str = "zzz-ResourceSizeTable";

/// The version of the table's rules: what [`sizetable`] works out for each file. Bump it with every change that
/// makes the table come out differently for the same mods (a new rule, another margin, a kind of file handled
/// for the first time), so an app built before the change leaves the new tables alone.
///
/// 1: the rules of Mod Studio 0.5.0. 2: mods deeper in the mods folder are counted (`contents/<title id>/<name>`).
pub const TABLE_RULES: u32 = 2;

/// The stamp's file, in [`TABLE_MOD`] beside its `romfs` (the game doesn't read it).
pub const STAMP_FILE: &str = "table-stamp.json";

/// Which rules and which app built the installed table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    /// [`TABLE_RULES`] of the app that built it.
    pub rules: u32,
    /// The app and its version ("Hookshot Mod Studio 0.5.0").
    pub made_by: String,
}

/// The installed table's stamp: `None` when there's no table, or it was built before stamps.
pub fn stamp(mods_dir: &Path) -> Option<Stamp> {
    let text = std::fs::read_to_string(mods_dir.join(TABLE_MOD).join(STAMP_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

#[derive(Debug, Clone, Serialize)]
pub struct InstalledMod {
    /// [`folder`](Self::folder) without the '-' or '_' that turns its folders off.
    pub name: String,
    /// Its folder's path in the mods folder, with '/' (`Name`, or deeper: `contents/<title id>/Name`).
    pub folder: String,
    pub enabled: bool,
    pub files: usize,
    /// Model files it replaces or adds (paths in the romfs); not animations or their skeletons.
    pub models: Vec<String>,
    pub textures: usize,
    /// Animation files (`Model/*.anim.bfres`) it replaces or adds.
    pub animations: usize,
    /// It has its own size table; the shared one replaces it in game and takes over its larger entries.
    pub own_size_table: bool,
}

/// How many folders down the game looks for mods (`contents/<title id>/<name>` is 3 down).
pub const MAX_DEPTH: usize = 4;

/// A folder name that turns off what's in it.
fn is_off(folder_name: &str) -> bool {
    folder_name.starts_with(['-', '_'])
}

/// A folder name without the '-' or '_' that turns it off.
fn on_name(folder_name: &str) -> &str {
    folder_name.trim_start_matches(['-', '_'])
}

/// What a file in `Model/` is (`path` in the romfs, with '/'): an animation (`X.anim.bfres`), an animation's
/// skeleton (`X_Animation.Skeleton.bfres`), a model, or none of them.
#[derive(Debug, PartialEq, Eq)]
enum ModelFile {
    Model,
    Animation,
    Skeleton,
}

fn model_file(path: &str) -> Option<ModelFile> {
    let path = path.to_ascii_lowercase();
    let stem = path.strip_prefix("model/")?.strip_suffix(".bfres.zs")?;
    Some(if stem.ends_with(".anim") {
        ModelFile::Animation
    } else if stem.ends_with(".skeleton") {
        ModelFile::Skeleton
    } else {
        ModelFile::Model
    })
}

/// Every mod in the mods folder with a `romfs`, sorted as the game sorts them (by its path, ignoring case); not
/// the size table's. Found as the game finds them: a mod is a folder holding `romfs`, `exefs` or `cheats`, at most
/// [`MAX_DEPTH`] folders down; the game doesn't look inside a mod for more, nor in folders whose names start with
/// '.'. Mods in a folder that is off are listed, as off.
pub fn list(mods_dir: &Path) -> Result<Vec<InstalledMod>> {
    let mut out = Vec::new();
    find_mods(mods_dir, "", 0, &mut out);
    out.sort_by_key(|m| m.folder.to_ascii_lowercase());
    Ok(out)
}

fn find_mods(dir: &Path, rel: &str, depth: usize, out: &mut Vec<InstalledMod>) {
    if depth > 0
        && ["romfs", "exefs", "cheats"]
            .iter()
            .any(|d| dir.join(d).exists())
    {
        if dir.join("romfs").is_dir() {
            out.push(installed(dir, rel));
        }
        return;
    }
    if depth >= MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !e.path().is_dir() || name.starts_with('.') || (depth == 0 && name == TABLE_MOD) {
            continue;
        }
        let sub = if rel.is_empty() {
            name
        } else {
            format!("{rel}/{name}")
        };
        find_mods(&e.path(), &sub, depth + 1, out);
    }
}

/// The mod in `dir` (`folder`: its path in the mods folder).
fn installed(dir: &Path, folder: &str) -> InstalledMod {
    let files = fsutil::files_under(&dir.join("romfs"));
    let slash = |p: &PathBuf| p.to_string_lossy().replace('\\', "/");
    InstalledMod {
        name: folder.split('/').map(on_name).collect::<Vec<_>>().join("/"),
        enabled: !folder.split('/').any(is_off),
        models: files
            .iter()
            .map(slash)
            .filter(|p| model_file(p) == Some(ModelFile::Model))
            .collect(),
        textures: files
            .iter()
            .map(slash)
            .filter(|p| p.starts_with("Tex/"))
            .count(),
        animations: files
            .iter()
            .filter(|p| model_file(&slash(p)) == Some(ModelFile::Animation))
            .count(),
        own_size_table: files.iter().any(|p| is_size_table(p)),
        files: files.len(),
        folder: folder.to_owned(),
    }
}

/// Turns a mod on or off by renaming its folder (`Name` <-> `-Name`; a mod deeper down, the last folder of its
/// path: `contents/<title id>/Name`). `folder` is its path in the mods folder, with '/', ending in its folder's
/// name either way it is now (`Name` for a mod that is off finds `-Name`). Returns its new path. A mod in a
/// folder that is off can't be turned on by itself. The game reads the mods folder when it starts, so it takes a
/// restart; the size table needs rebuilding.
pub fn set_enabled(mods_dir: &Path, folder: &str, enabled: bool) -> Result<String> {
    let parts: Vec<&str> = folder.split('/').collect();
    let bad_part = |p: &&str| p.is_empty() || *p == "." || *p == ".." || p.contains([':', '\\']);
    if parts.iter().any(bad_part) || parts.len() > MAX_DEPTH || folder == TABLE_MOD {
        return Err(Error(format!("not a mod folder: {folder:?}")));
    }
    let (parents, leaf) = parts.split_at(parts.len() - 1);
    if enabled && let Some(off) = parents.iter().find(|p| is_off(p)) {
        return Err(Error(format!(
            "{folder} is in {off}, a folder that is off: rename that folder to turn it on"
        )));
    }
    let parent: PathBuf = mods_dir.join(parents.iter().collect::<PathBuf>());
    let name = on_name(leaf[0]);
    let candidates = [leaf[0].to_owned(), name.to_owned(), format!("-{name}")];
    let Some(current) = candidates
        .iter()
        .find(|c| parent.join(c).join("romfs").is_dir())
    else {
        return Err(Error(format!(
            "{} is not a mod",
            mods_dir.join(folder).display()
        )));
    };
    let new = if enabled {
        name.to_owned()
    } else {
        format!("-{name}")
    };
    let new_folder = parents
        .iter()
        .copied()
        .chain([new.as_str()])
        .collect::<Vec<_>>()
        .join("/");
    if new == *current {
        return Ok(new_folder);
    }
    let (from, to) = (parent.join(current), parent.join(&new));
    if to.exists() {
        return Err(Error(format!(
            "{} already exists: two copies of {name}",
            to.display()
        )));
    }
    std::fs::rename(&from, &to).context(format!("renaming {}", from.display()))?;
    Ok(new_folder)
}

/// The shared size table, if one is installed.
pub fn size_table(mods_dir: &Path) -> Option<PathBuf> {
    let romfs = mods_dir.join(TABLE_MOD).join("romfs");
    fsutil::files_under(&romfs)
        .into_iter()
        .find(|p| is_size_table(p))
        .map(|p| romfs.join(p))
}

/// Works out one size table for every enabled mod in `mods_dir` ([`sizetable`], against the game's files in
/// `game_romfs`) and installs it as [`TABLE_MOD`], stamped as made by `made_by` (the app and its version).
/// Returns the table, or `None` when no mod needs one (every file fits the game's own table); an old table is
/// then removed. A table built with newer rules than [`TABLE_RULES`] is left as it is, with an error that says
/// which app built it.
pub fn rebuild_size_table(
    mods_dir: &Path,
    game_romfs: &Path,
    made_by: &str,
    progress: &mut Progress,
) -> Result<Option<PathBuf>> {
    if let Some(newer) = stamp(mods_dir).filter(|s| s.rules > TABLE_RULES) {
        return Err(Error(format!(
            "the resource size table in {} was built by {}, whose rules are newer than {made_by}'s: it's \
             left as it is. Update this app, or rebuild the table with {}",
            mods_dir.display(),
            newer.made_by,
            newer.made_by
        )));
    }
    let mods: Vec<InstalledMod> = list(mods_dir)?.into_iter().filter(|m| m.enabled).collect();
    // Each file once, the last mod's (as the game overlays them), by its path ignoring case.
    let mut files: BTreeMap<String, (String, PathBuf)> = BTreeMap::new();
    let mut own_tables: Vec<sizetable::OwnTable> = Vec::new();
    for m in &mods {
        for (rel, path) in sizetable::files_of(&mods_dir.join(&m.folder).join("romfs")) {
            if is_size_table(Path::new(&rel)) {
                own_tables.push((m.folder.clone(), path));
            } else {
                files.insert(rel.to_lowercase(), (rel, path));
            }
        }
    }
    // A mod with its own table that sorts after the shared one would have its table used instead
    let table_key = TABLE_MOD.to_ascii_lowercase();
    for m in mods.iter().filter(|m| m.own_size_table) {
        if m.folder.to_ascii_lowercase() > table_key {
            progress.warn(format!(
                "{} sorts after {TABLE_MOD}, so the game uses its own size table instead of the one for all \
                 mods: rename it so it sorts earlier",
                m.folder
            ));
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
        &open_game_romfs(game_romfs)?,
        &files,
        &own_tables,
        &table_mod.join("romfs"),
        progress,
    )?;
    if report.table.is_none() {
        fsutil::remove_dir(&table_mod)?;
    } else {
        let stamp = Stamp {
            rules: TABLE_RULES,
            made_by: made_by.to_owned(),
        };
        let path = table_mod.join(STAMP_FILE);
        let text = serde_json::to_string_pretty(&stamp).context("the table's stamp")?;
        std::fs::write(&path, text).context(path.display())?;
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
            ("BMod", "Model/Dm_Scene_Player_Adult.anim.bfres.zs"),
            ("BMod", "Model/X__Adult_01_Animation.anim.bfres.zs"),
            ("BMod", "Model/X__Adult_01_Animation.Skeleton.bfres.zs"),
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
        // animations and their skeletons aren't models
        assert_eq!(mods[1].models, ["Model/X.X.bfres.zs"]);
        assert_eq!((mods[1].animations, mods[1].files), (2, 4));
        assert!(size_table(&dir).is_some());

        assert_eq!(set_enabled(&dir, "-AMod", true).unwrap(), "AMod");
        assert_eq!(set_enabled(&dir, "BMod", false).unwrap(), "-BMod");
        assert_eq!(set_enabled(&dir, "AMod", true).unwrap(), "AMod");
        assert!(dir.join("AMod").is_dir() && dir.join("-BMod").is_dir());
        // by its name, whichever way it is now
        assert_eq!(set_enabled(&dir, "BMod", true).unwrap(), "BMod");
        assert_eq!(set_enabled(&dir, "-AMod", false).unwrap(), "-AMod");
        assert!(dir.join("-AMod").is_dir() && dir.join("BMod").is_dir());
        assert!(set_enabled(&dir, TABLE_MOD, false).is_err());
        assert!(set_enabled(&dir, "NotAMod", false).is_err());
        fsutil::remove_dir(&dir).unwrap();
    }

    #[test]
    fn mods_found_as_the_game_finds_them() {
        let dir = std::env::temp_dir().join(format!("modstudio-nested-{}", std::process::id()));
        for (folder, inside) in [
            ("BMod", "romfs/Tex/B.bntx.zs"),
            // as emulators keep them, and as Atmosphere does (the title id's folder is the mod)
            ("contents/0100F2C0115B6000/CMod", "romfs/Tex/C.bntx.zs"),
            (
                "atmosphere/contents/0100F2C0115B6000",
                "romfs/Tex/A.bntx.zs",
            ),
            // a folder that is off turns off every mod in it
            ("-off/DMod", "romfs/Tex/D.bntx.zs"),
            // the game doesn't look inside a mod, nor in hidden folders, nor deeper than MAX_DEPTH
            ("EMod", "romfs/Tex/E.bntx.zs"),
            ("EMod/inner", "romfs/Tex/I.bntx.zs"),
            (".hidden/FMod", "romfs/Tex/F.bntx.zs"),
            ("a/b/c/d/GMod", "romfs/Tex/G.bntx.zs"),
            // a mod with only cheats is a mod (nothing inside it is), but has no files for the table
            ("HMod", "cheats/E0F3CCFA00000000.txt"),
            ("HMod/sub", "romfs/Tex/H.bntx.zs"),
        ] {
            let p = dir.join(folder).join(inside);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        }
        let mods = list(&dir).unwrap();
        let folders: Vec<&str> = mods.iter().map(|m| m.folder.as_str()).collect();
        assert_eq!(
            folders,
            [
                "-off/DMod",
                "atmosphere/contents/0100F2C0115B6000",
                "BMod",
                "contents/0100F2C0115B6000/CMod",
                "EMod",
            ]
        );
        let on: Vec<bool> = mods.iter().map(|m| m.enabled).collect();
        assert_eq!(on, [false, true, true, true, true]);
        assert_eq!(mods[0].name, "off/DMod");

        // a mod deeper down: its own folder is renamed
        let c = "contents/0100F2C0115B6000/CMod";
        let off = "contents/0100F2C0115B6000/-CMod";
        assert_eq!(set_enabled(&dir, c, false).unwrap(), off);
        assert!(dir.join(off).join("romfs").is_dir());
        assert_eq!(set_enabled(&dir, off, true).unwrap(), c);
        // not one in a folder that is off, nor paths outside the mods folder
        let e = set_enabled(&dir, "-off/DMod", true).unwrap_err();
        assert!(e.0.contains("-off, a folder that is off"), "{e}");
        for bad in [
            "../BMod",
            "C:/BMod",
            "/BMod",
            "BMod/",
            "a\\BMod",
            "a/b/c/d/GMod",
        ] {
            assert!(set_enabled(&dir, bad, false).is_err(), "{bad}");
        }
        assert!(dir.join("BMod").is_dir());
        fsutil::remove_dir(&dir).unwrap();
    }

    #[test]
    fn a_table_with_newer_rules_is_left_alone() {
        let dir = std::env::temp_dir().join(format!("modstudio-stamp-{}", std::process::id()));
        let table = dir
            .join(TABLE_MOD)
            .join("romfs/System/Resource/R.rsizetable.zs");
        let texture = dir.join("AMod/romfs/Tex/T.bntx.zs");
        for p in [&table, &texture] {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        }
        let stamped = |rules: u32| {
            let s = Stamp {
                rules,
                made_by: "Mod Manager 9.0.0".into(),
            };
            std::fs::write(
                dir.join(TABLE_MOD).join(STAMP_FILE),
                serde_json::to_string(&s).unwrap(),
            )
            .unwrap();
            assert_eq!(stamp(&dir), Some(s));
        };
        let no_romfs = dir.join("no-romfs");
        let mut progress = Progress::new(|_| {});

        stamped(TABLE_RULES + 1);
        let e = rebuild_size_table(&dir, &no_romfs, "Test 1.0", &mut progress).unwrap_err();
        assert!(e.0.contains("built by Mod Manager 9.0.0, whose rules are newer than Test 1.0's"));
        assert!(table.is_file(), "the newer table is left as it is");

        // the same rules: rebuilt (here as far as the game's files, which aren't there)
        stamped(TABLE_RULES);
        let e = rebuild_size_table(&dir, &no_romfs, "Test 1.0", &mut progress).unwrap_err();
        assert!(e.0.contains("isn't the game's romfs"), "{e}");
        fsutil::remove_dir(&dir).unwrap();
    }
}
