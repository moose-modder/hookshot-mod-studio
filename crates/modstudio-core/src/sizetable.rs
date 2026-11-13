//! The resource size table for mods, worked out here (no outside tool). The game reserves memory for each file
//! by its table (`System/Resource/ResourceSizeTable.Product.*.rsizetable.zs`); a mod file larger than the game's
//! needs a larger entry, or the game doesn't load it (a texture then shows as a default one).
//!
//! What a file needs, from the game's own table (local analysis over every model and texture of the game,
//! `examples/restbl_probe.rs`, `restbl_fit.rs`; docs/plan.md phase 3):
//!
//! - Textures (`.bntx`): the file's size plus a constant (280 bytes for most, 288 for the rest).
//! - Models (`.bfres`): the file's size plus memory that follows the model's structure (shapes, meshes,
//!   materials' parameters, bones), not its vertex and index data, which the game uses in place. Edited models
//!   keep their structure, so they need the game's entry plus the bytes the file grew (with 1% of the growth
//!   to spare).
//!
//! So a file the game has gets its game entry, plus its growth if it grew; one it doesn't have (added by a
//! mod) gets an estimate: textures their size plus 288, models their size plus 1.25 times a structure estimate
//! (never below the game's own value for any of its models), other kinds 1.5 times their size, with a note.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use modstudio_formats::bfres::Bfres;
use modstudio_formats::restbl::Restbl;
use modstudio_formats::romfs::{self, Romfs};
use serde::Serialize;

use crate::fsutil::{self, is_size_table};
use crate::progress::Progress;
use crate::{Context, Error, Result};

#[derive(Debug, Clone, Serialize)]
pub struct Change {
    /// The file's path in the table (`Model/X.X.bfres`).
    pub path: String,
    /// The game's size for it (`None`: a file the game doesn't have).
    pub game: Option<u32>,
    pub size: u32,
    /// "grown", or "added" (estimated)
    pub how: &'static str,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct TableReport {
    /// The table written, or `None` when every file fits the game's sizes (no table needed).
    pub table: Option<PathBuf>,
    pub changes: Vec<Change>,
    /// Files whose game size is enough.
    pub fits: usize,
}

/// The game's table: its path in the romfs and its entries.
pub fn game_table(romfs: &Romfs) -> Result<(PathBuf, Restbl)> {
    let dir = romfs.root().join("System").join("Resource");
    let mut tables: Vec<PathBuf> = std::fs::read_dir(&dir)
        .context(dir.display())?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().contains(".rsizetable"))
        })
        .collect();
    let [path] = tables.as_mut_slice() else {
        return Err(Error(format!(
            "expected one resource size table in {}, found {}",
            dir.display(),
            tables.len()
        )));
    };
    let data = romfs
        .read(&mut romfs.decoder()?, &*path)
        .context(path.display())?;
    let table = Restbl::parse(&data).context(path.display())?;
    let rel = path
        .strip_prefix(romfs.root())
        .map(Path::to_owned)
        .unwrap_or_else(|_| path.clone());
    Ok((rel, table))
}

/// The table for `files` (romfs-relative path, as the mod spells it -> the file), written compressed at the
/// game's table path under `out_romfs`; when every file fits the game's sizes, no table (an old one there is
/// removed).
pub fn write_table(
    romfs: &Romfs,
    files: &BTreeMap<String, PathBuf>,
    out_romfs: &Path,
    progress: &mut Progress,
) -> Result<TableReport> {
    let (table_rel, mut table) = game_table(romfs)?;
    let mut decoder = romfs.decoder()?;
    let mut spelling = Spelling::default();
    let mut report = TableReport::default();
    for (rel, file) in files {
        if is_size_table(Path::new(rel)) {
            continue;
        }
        let game_rel = spelling.of(romfs.root(), rel);
        let key = game_rel.trim_end_matches(".zs").to_owned();
        let data = romfs::read_maybe_zs(&mut decoder, file).context(file.display())?;
        let new_size = data.len() as u64;
        let game_entry = table.get(&key);
        let game_file = romfs.root().join(&game_rel);
        let game_size = if game_entry.is_some() && game_file.is_file() {
            Some(
                romfs::read_maybe_zs(&mut decoder, &game_file)
                    .context(game_file.display())?
                    .len() as u64,
            )
        } else {
            None
        };
        let kind = key.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        let (size, how) = match (game_entry, game_size) {
            (Some(entry), Some(old)) => {
                if new_size <= old {
                    report.fits += 1;
                    continue;
                }
                (grown(&kind, entry, old, new_size), "grown")
            }
            (entry, _) => {
                let estimate = added(&kind, &data, new_size);
                match entry {
                    Some(e) if u64::from(e) >= estimate => {
                        report.fits += 1;
                        continue;
                    }
                    _ => (estimate, "added"),
                }
            }
        };
        let size =
            u32::try_from(size).map_err(|_| Error(format!("{key}: too large for the table")))?;
        table.set(&key, size, game_entry.is_some());
        report.changes.push(Change {
            path: key,
            game: game_entry,
            size,
            how,
        });
    }

    let out = out_romfs.join(&table_rel);
    fsutil::remove_file(&out)?;
    for c in &report.changes {
        match c.game {
            Some(g) => progress.line(format!("^ {}  {g} -> {}", c.path, c.size)),
            None => progress.line(format!(
                "+ {}  {} (the game doesn't have it: estimated)",
                c.path, c.size
            )),
        }
    }
    if report.changes.is_empty() {
        progress.line(format!(
            "No resource size table needed: {} file(s) fit the game's sizes",
            report.fits
        ));
        return Ok(report);
    }
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).context(dir.display())?;
    }
    std::fs::write(&out, modstudio_formats::zs::compress(&table.to_bytes()))
        .context(out.display())?;
    progress.line(format!(
        "{} file(s) need more than the game's size, {} fit: {}",
        report.changes.len(),
        report.fits,
        out.display()
    ));
    report.table = Some(out);
    Ok(report)
}

/// What a file the game has needs after growing from `old` to `new` bytes.
fn grown(kind: &str, entry: u32, old: u64, new: u64) -> u64 {
    let entry = u64::from(entry);
    let growth = new - old;
    let size = match kind {
        // the extra over the file size is a per-file constant
        "bntx" => entry + growth,
        // the extra follows the structure, which edits keep (1% of the growth to spare)
        "bfres" => entry + growth + growth.div_ceil(100),
        // not worked out: at least the growth, and the game's extra in proportion
        _ => (entry + growth).max((entry * new).div_ceil(old.max(1))),
    };
    size.next_multiple_of(8)
}

/// An estimate for a file the game doesn't have (`data` decompressed).
fn added(kind: &str, data: &[u8], size: u64) -> u64 {
    let estimate = match kind {
        "bntx" => size + 288,
        "bfres" => match Bfres::parse(data, None) {
            Ok(b) => size + structure_estimate(&b) * 5 / 4,
            Err(_) => size * 3 / 2,
        },
        _ => size * 3 / 2,
    };
    estimate.next_multiple_of(8)
}

/// Memory a model needs besides its file, from its structure: a fit over the game's models with the terms that
/// came out negative left out, so that it is at least the game's own value for every model of the game.
fn structure_estimate(b: &Bfres) -> u64 {
    let mut n = [0u64; 8];
    for m in &b.models {
        n[0] += m.shapes.len() as u64;
        n[1] += m.shapes.iter().map(|s| s.meshes.len() as u64).sum::<u64>();
        n[6] += m.skeleton.bones.len() as u64;
        n[7] += m.materials.len() as u64;
        for mat in &m.materials {
            if let Some(s) = &mat.shader {
                n[2] += s.params.len() as u64;
                n[3] += s.render_info.len() as u64;
            }
        }
        n[4] += m
            .vertex_buffers
            .iter()
            .map(|v| v.attributes.len() as u64)
            .sum::<u64>();
        n[5] += m
            .shapes
            .iter()
            .map(|s| s.skin_bone_indices.len() as u64)
            .sum::<u64>();
    }
    let [
        shapes,
        meshes,
        params,
        render_info,
        attrs,
        skin_bones,
        bones,
        materials,
    ] = n;
    7850 + 7662 * shapes
        + 624 * meshes
        + 417 * params
        + 42 * render_info
        + 47 * attrs
        + 32 * bones
        + 30 * materials
        + 11 * skin_bones
}

/// A mod file's path as the game spells it (the table is keyed by the game's spelling; Hookshot matches mod
/// files to the game's ignoring case).
#[derive(Default)]
struct Spelling {
    dirs: HashMap<PathBuf, HashMap<String, String>>,
}

impl Spelling {
    fn of(&mut self, root: &Path, rel: &str) -> String {
        let rel = rel.replace('\\', "/");
        if root.join(&rel).exists() {
            return rel;
        }
        let (dir, name) = rel.rsplit_once('/').unwrap_or(("", &rel));
        let names = self.dirs.entry(root.join(dir)).or_insert_with_key(|d| {
            std::fs::read_dir(d)
                .map(|it| {
                    it.flatten()
                        .map(|e| {
                            let n = e.file_name().to_string_lossy().into_owned();
                            (n.to_lowercase(), n)
                        })
                        .collect()
                })
                .unwrap_or_default()
        });
        match names.get(&name.to_lowercase()) {
            Some(n) if dir.is_empty() => n.clone(),
            Some(n) => format!("{dir}/{n}"),
            None => rel.clone(),
        }
    }
}

/// The files of a mod's romfs, by their romfs path.
pub fn files_of(romfs: &Path) -> BTreeMap<String, PathBuf> {
    fsutil::files_under(romfs)
        .into_iter()
        .map(|rel| (rel.to_string_lossy().replace('\\', "/"), romfs.join(rel)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grown_files() {
        // textures: the game's extra stays
        assert_eq!(grown("bntx", 127_368, 127_088, 458_864), 459_144);
        // models: the growth and 1% of it
        assert_eq!(grown("bfres", 1_647_928, 1_492_464, 1_504_752), 1_660_344);
        // others: at least in proportion
        assert_eq!(grown("bgyml", 2000, 1000, 1500), 3000);
        assert_eq!(grown("bgyml", 1008, 1000, 1500), 1512);
    }

    #[test]
    fn added_files() {
        assert_eq!(added("bntx", &[], 1000), 1288);
        assert_eq!(added("bgyml", &[], 1000), 1504);
        // a model that doesn't parse: half again its size
        assert_eq!(added("bfres", &[0; 16], 1000), 1504);
    }
}
