//! The resource size table for mods, worked out here (no outside tool). The game reserves memory for each file
//! by its table (`System/Resource/ResourceSizeTable.Product.*.rsizetable.zs`); a mod file larger than the game's
//! needs a larger entry, or the game doesn't load it: a texture then shows as a default one, and a parameter
//! file missing from an actor's pack crashes the game when it makes the actor (seen in game: a mod that turns
//! Link into Malon adds `Player_Malon_*` parameters to `Pack/Actor/Player_Adult.pack`).
//!
//! The table has entries for the files inside packs too (`Pack/Actor/X.pack` holds `Component/...bgyml` files,
//! each with an entry of its own), so a mod's packs are opened and their files compared with the game's same
//! pack, one by one.
//!
//! What a file needs, from the game's own table (local analysis: `examples/restbl_probe.rs` and `restbl_fit.rs`
//! over every model and texture of the game, `restbl_packs.rs` over every file in its packs; docs/plan.md
//! phase 3):
//!
//! - Textures (`.bntx`): the file's size plus a constant (280 bytes for most, 288 for the rest).
//! - Models (`.bfres`): the file's size plus memory that follows the model's structure (shapes, meshes,
//!   materials' parameters, bones), not its vertex and index data, which the game uses in place. Edited models
//!   keep their structure, so they need the game's entry plus the bytes the file grew (with 1% of the growth
//!   to spare).
//! - Animations (`.anim.bfres`): unlike models, their extra grows with the file; all 1,427 of the game's are
//!   within 16 KB plus 1.04 times the size.
//! - Parameter files (`.bgyml`, in packs): 5056 bytes and more, up to 9.2 times the file's size (the file is
//!   read into objects); all 57,752 of the game's are within 6500 plus 10 times the size.
//! - Animation state files (`AS/*.asb`): up to 1.38 times the size plus 1 KB. The rest of what packs hold (AI
//!   `.ainb`, physics, `.byml`): the file's size plus at most 573.
//!
//! So a file the game has gets its game entry, plus its growth if it grew; one it doesn't have (added by a
//! mod) gets an estimate ([`estimate`]): textures their size plus 288, models their size plus 1.25 times a
//! structure estimate (never below the game's own value for any of its models), animations 16 KB plus 1.04
//! times their size, parameter files 6500 plus 10 times their size, other kinds 1.5 times their size plus 1 KB.
//!
//! A mod made with another tool may come with its own table, which the shared one replaces in game. Its
//! entries that are larger than the shared table's are taken over: its tool knew what the mod needs, perhaps
//! for kinds of file not worked out here.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use modstudio_formats::bfres::Bfres;
use modstudio_formats::restbl::Restbl;
use modstudio_formats::romfs::{self, Romfs};
use modstudio_formats::sarc::Sarc;
use modstudio_formats::zs::{self, Decoder};
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
    /// The pack the file is in (`Pack/Actor/X.pack`), for a file inside one.
    pub pack: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct TableReport {
    /// The table written, or `None` when every file fits the game's sizes (no table needed).
    pub table: Option<PathBuf>,
    pub changes: Vec<Change>,
    /// Files whose game size is enough (files in packs included).
    pub fits: usize,
    /// Mods' own tables: the mod, and how many of its entries were larger than ours (taken over).
    pub merged: Vec<(String, usize)>,
}

/// A mod's own size table: the mod's name and the table's file.
pub type OwnTable = (String, PathBuf);

/// A file in one of the mods' packs.
struct InPack {
    pack: String,
    path: String,
    size: u64,
    estimate: u64,
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

/// The table for `files` (romfs-relative path, as the mod spells it -> the file), with the larger entries of
/// the mods' own tables in `own_tables` taken over, written compressed at the game's table path under
/// `out_romfs`; when every file fits the game's sizes, no table (an old one there is removed).
pub fn write_table(
    romfs: &Romfs,
    files: &BTreeMap<String, PathBuf>,
    own_tables: &[OwnTable],
    out_romfs: &Path,
    progress: &mut Progress,
) -> Result<TableReport> {
    let (table_rel, game) = game_table(romfs)?;
    let mut decoder = romfs.decoder()?;
    let mut spelling = Spelling::default();
    let mut report = TableReport::default();
    // path -> what it needs (the most, when several packs hold the file)
    let mut needs: BTreeMap<String, Change> = BTreeMap::new();
    let mut keep = |c: Change| {
        if needs.get(&c.path).is_none_or(|old| old.size < c.size) {
            needs.insert(c.path.clone(), c);
        }
    };
    // the files in the mods' packs, and the sizes of the files in the game's same packs
    let mut in_packs: Vec<InPack> = Vec::new();
    let mut game_in_packs: HashMap<String, u64> = HashMap::new();
    for (rel, file) in files {
        if is_size_table(Path::new(rel)) {
            continue;
        }
        let game_rel = spelling.of(romfs.root(), rel);
        let key = game_rel.trim_end_matches(".zs").to_owned();
        let data = romfs::read_maybe_zs(&mut decoder, file).context(file.display())?;
        let entry = game.get(&key);
        let game_file = romfs.root().join(&game_rel);
        let game_data = if game_file.is_file() {
            Some(romfs::read_maybe_zs(&mut decoder, &game_file).context(game_file.display())?)
        } else {
            None
        };
        let game_size = game_data
            .as_ref()
            .filter(|_| entry.is_some())
            .map(|d| d.len() as u64);
        match need(&key, data.len() as u64, entry, game_size, || {
            estimate(&key, &data)
        })? {
            Some((size, how)) => keep(Change {
                path: key.clone(),
                game: entry,
                size,
                how,
                pack: None,
            }),
            None => report.fits += 1,
        }
        if key.ends_with(".pack") {
            pack_files(
                &key,
                &data,
                game_data.as_deref(),
                &mut decoder,
                &mut in_packs,
                &mut game_in_packs,
            )
            .context(file.display())?;
        }
    }
    for f in in_packs {
        let entry = game.get(&f.path);
        let game_size = game_in_packs
            .get(&f.path)
            .copied()
            .filter(|_| entry.is_some());
        match need(&f.path, f.size, entry, game_size, || f.estimate)? {
            Some((size, how)) => keep(Change {
                path: f.path,
                game: entry,
                size,
                how,
                pack: Some(f.pack),
            }),
            None => report.fits += 1,
        }
    }

    let mut table = game.clone();
    for c in needs.values() {
        table.set(&c.path, c.size, c.game.is_some());
    }
    report.changes = needs.into_values().collect();
    for c in &report.changes {
        let pack = c
            .pack
            .as_ref()
            .map(|p| format!(" (in {p})"))
            .unwrap_or_default();
        match c.game {
            Some(g) => progress.line(format!("^ {}{pack}  {g} -> {}", c.path, c.size)),
            None => progress.line(format!(
                "+ {}{pack}  {} (the game doesn't have it: estimated)",
                c.path, c.size
            )),
        }
    }
    for (name, path) in own_tables {
        if path.file_name() != table_rel.file_name() {
            progress.line(format!(
                "{name}: its own size table is for another version of the game ({}): left out",
                path.file_name().unwrap_or_default().to_string_lossy()
            ));
            continue;
        }
        let data = romfs::read_maybe_zs(&mut decoder, path).context(path.display())?;
        let theirs = Restbl::parse(&data).context(path.display())?;
        let taken = take_larger(&mut table, &theirs);
        progress.line(format!(
            "{name}: {taken} entries of its own size table are larger than ours: taken over"
        ));
        report.merged.push((name.clone(), taken));
    }

    let out = out_romfs.join(&table_rel);
    fsutil::remove_file(&out)?;
    if report.changes.is_empty() && report.merged.iter().all(|(_, n)| *n == 0) {
        progress.line(format!(
            "No resource size table needed: {} file(s) fit the game's sizes",
            report.fits
        ));
        return Ok(report);
    }
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).context(dir.display())?;
    }
    std::fs::write(&out, zs::compress(&table.to_bytes())).context(out.display())?;
    progress.line(format!(
        "{} file(s) need more than the game's size, {} fit: {}",
        report.changes.len(),
        report.fits,
        out.display()
    ));
    report.table = Some(out);
    Ok(report)
}

/// What a file needs, when the game's size isn't enough: `size` bytes now, the game's entry for its path and
/// the game's file's size (when the game has the file), and an estimate for a file the game doesn't have.
fn need(
    path: &str,
    size: u64,
    entry: Option<u32>,
    game_size: Option<u64>,
    estimate: impl FnOnce() -> u64,
) -> Result<Option<(u32, &'static str)>> {
    let (need, how) = match (entry, game_size) {
        (Some(entry), Some(old)) => {
            if size <= old {
                return Ok(None);
            }
            (grown(kind_of(path), entry, old, size), "grown")
        }
        (entry, _) => {
            let estimate = estimate();
            if entry.is_some_and(|e| u64::from(e) >= estimate) {
                return Ok(None);
            }
            (estimate, "added")
        }
    };
    let need =
        u32::try_from(need).map_err(|_| Error(format!("{path}: too large for the table")))?;
    Ok(Some((need, how)))
}

/// The files in a mod's pack (`data`, decompressed) into `out`, and the sizes of the files in the game's same
/// pack (`game`) into `game_sizes`. Data that isn't an archive has no files.
fn pack_files(
    pack: &str,
    data: &[u8],
    game: Option<&[u8]>,
    decoder: &mut Decoder,
    out: &mut Vec<InPack>,
    game_sizes: &mut HashMap<String, u64>,
) -> Result<()> {
    if let Some(game) = game.and_then(|g| Sarc::parse(g).ok()) {
        for f in game.files() {
            let Some(name) = f.name else { continue };
            let size = if name.ends_with(".zs") {
                zs::frame_info(f.data)?
                    .content_size
                    .unwrap_or(f.data.len() as u64)
            } else {
                f.data.len() as u64
            };
            let e = game_sizes
                .entry(name.trim_end_matches(".zs").to_owned())
                .or_default();
            *e = (*e).max(size);
        }
    }
    let Ok(sarc) = Sarc::parse(data) else {
        return Ok(());
    };
    for f in sarc.files() {
        let Some(name) = f.name else { continue };
        let path = name.trim_end_matches(".zs").to_owned();
        let data = if name.ends_with(".zs") {
            std::borrow::Cow::Owned(decoder.decompress(f.data).context(name)?)
        } else {
            std::borrow::Cow::Borrowed(f.data)
        };
        out.push(InPack {
            pack: pack.to_owned(),
            estimate: estimate(&path, &data),
            size: data.len() as u64,
            path,
        });
    }
    Ok(())
}

/// Takes the entries of `theirs` that are larger than `table`'s, or that `table` doesn't have. Returns how many.
fn take_larger(table: &mut Restbl, theirs: &Restbl) -> usize {
    let mut taken = 0;
    for (hash, &size) in &theirs.hashes {
        if table.hashes.get(hash).is_none_or(|&s| s < size) {
            table.hashes.insert(*hash, size);
            taken += 1;
        }
    }
    for (name, &size) in &theirs.names {
        if table.names.get(name).is_none_or(|&s| s < size) {
            table.names.insert(name.clone(), size);
            taken += 1;
        }
    }
    taken
}

/// A path's kind of file for the table: its extension, in lower case (`bfres`, `bgyml`).
pub fn kind_of(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.').map_or("", |(_, ext)| ext)
}

/// What a file the game has needs after growing from `old` to `new` bytes.
fn grown(kind: &str, entry: u32, old: u64, new: u64) -> u64 {
    let entry = u64::from(entry);
    let growth = new - old;
    let size = match kind.to_ascii_lowercase().as_str() {
        // the extra over the file size is a per-file constant
        "bntx" => entry + growth,
        // the extra follows the structure, which edits keep (1% of the growth to spare)
        "bfres" => entry + growth + growth.div_ceil(100),
        // read into objects: up to 9.2 times the file (restbl_packs)
        "bgyml" => entry + growth * 10,
        // not worked out: at least the growth, and the game's extra in proportion
        _ => (entry + growth).max((entry * new).div_ceil(old.max(1))),
    };
    size.next_multiple_of(8)
}

/// What a file the game doesn't have needs (`path` in the table, `data` decompressed). Each rule is at least
/// the game's own value for every file of its kind in the game (`examples/restbl_estimates.rs`,
/// `restbl_packs.rs`).
pub fn estimate(path: &str, data: &[u8]) -> u64 {
    let size = data.len() as u64;
    let kind = kind_of(path).to_ascii_lowercase();
    let estimate = match kind.as_str() {
        "bntx" => size + 288,
        // animations: their extra grows with the file (up to 3.2% of 16 KB-and-larger files)
        "bfres" if path.to_ascii_lowercase().ends_with(".anim.bfres") => {
            16384 + size + size.div_ceil(25)
        }
        "bfres" => match Bfres::parse(data, None) {
            Ok(b) => size + structure_estimate(&b) * 5 / 4,
            Err(_) => size * 3 / 2,
        },
        "bgyml" => 6500 + size * 10,
        // the game's `.asb` need up to 1.38 times their size and 1 KB, its other files in packs 573 bytes more
        _ => size * 3 / 2 + 1024,
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
        // parameter files: ten times the growth
        assert_eq!(grown("bgyml", 6232, 776, 1096), 9432);
        // others: at least in proportion
        assert_eq!(grown("ainb", 2000, 1000, 1500), 3000);
        assert_eq!(grown("ainb", 1008, 1000, 1500), 1512);
    }

    #[test]
    fn added_files() {
        assert_eq!(estimate("Tex/T.bntx", &[0; 1000]), 1288);
        assert_eq!(estimate("A/P.x__ModelInfo.bgyml", &[0; 140]), 7904);
        assert_eq!(estimate("A/P.BGYML", &[0; 140]), 7904);
        // animations: 16 KB and 1.04 times their size
        assert_eq!(estimate("Model/X_Animation.anim.bfres", &[0; 2500]), 18984);
        // others: half again their size and 1 KB
        assert_eq!(estimate("AI/X.root.ainb", &[0; 290]), 1464);
        assert_eq!(estimate("Sound/X.bwav", &[0; 4000]), 7024);
        // a model that doesn't parse: half again its size
        assert_eq!(estimate("Model/X.X.bfres", &[0; 4000]), 6000);
    }

    #[test]
    fn kinds() {
        assert_eq!(kind_of("Model/A.B.bfres"), "bfres");
        assert_eq!(
            kind_of("Component/ModelInfo/P.engine__component__ModelInfo.bgyml"),
            "bgyml"
        );
        assert_eq!(kind_of("Dir.x/NoExtension"), "");
    }

    #[test]
    fn larger_entries_are_taken() {
        let mut ours = Restbl {
            version: 1,
            name_len: 160,
            hashes: BTreeMap::from([(1, 100), (2, 200)]),
            names: BTreeMap::from([("A".to_owned(), 10)]),
        };
        let theirs = Restbl {
            version: 1,
            name_len: 160,
            hashes: BTreeMap::from([(1, 150), (2, 150), (3, 30)]),
            names: BTreeMap::from([("A".to_owned(), 5), ("B".to_owned(), 7)]),
        };
        assert_eq!(take_larger(&mut ours, &theirs), 3);
        assert_eq!(ours.hashes, BTreeMap::from([(1, 150), (2, 200), (3, 30)]));
        assert_eq!(
            ours.names,
            BTreeMap::from([("A".to_owned(), 10), ("B".to_owned(), 7)])
        );
    }

    /// A mod's pack with a file the game's pack has (unchanged, and grown) and one it doesn't: the new one gets
    /// an entry by its own path, and the pack its own.
    #[test]
    fn files_in_packs() {
        let game = modstudio_formats::sarc::write(
            &[("A/same.bgyml", &[1; 100]), ("A/grows.bgyml", &[2; 100])],
            8,
        );
        let ours = modstudio_formats::sarc::write(
            &[
                ("A/same.bgyml", &[1; 100]),
                ("A/grows.bgyml", &[2; 120]),
                ("A/new.bgyml", &[3; 140]),
            ],
            8,
        );
        let (mut out, mut sizes) = (Vec::new(), HashMap::new());
        pack_files(
            "Pack/Actor/X.pack",
            &ours,
            Some(&game),
            &mut Decoder::plain(),
            &mut out,
            &mut sizes,
        )
        .unwrap();
        assert_eq!(sizes.get("A/grows.bgyml"), Some(&100));
        let found: Vec<(&str, u64, u64)> = out
            .iter()
            .map(|f| (f.path.as_str(), f.size, f.estimate))
            .collect();
        assert_eq!(found.len(), 3);
        assert!(found.contains(&("A/new.bgyml", 140, 7904)));
        assert!(out.iter().all(|f| f.pack == "Pack/Actor/X.pack"));
        // not an archive: no files
        out.clear();
        pack_files(
            "Pack/Y.pack",
            b"not a sarc",
            None,
            &mut Decoder::plain(),
            &mut out,
            &mut sizes,
        )
        .unwrap();
        assert!(out.is_empty());

        // what each needs, with the game's entries 5600 for both of the game's files
        let need_of = |path: &str, size: u64, entry: Option<u32>| {
            need(
                path,
                size,
                entry,
                sizes.get(path).copied().filter(|_| entry.is_some()),
                || estimate(path, &vec![0; size as usize]),
            )
            .unwrap()
        };
        assert_eq!(need_of("A/same.bgyml", 100, Some(5600)), None);
        assert_eq!(
            need_of("A/grows.bgyml", 120, Some(5600)),
            Some((5800, "grown"))
        );
        assert_eq!(need_of("A/new.bgyml", 140, None), Some((7904, "added")));
        // a file the game has in another pack: its entry is kept when the estimate is no more, else raised
        assert_eq!(need_of("B/elsewhere.bgyml", 40, Some(7000)), None);
        assert_eq!(
            need_of("B/elsewhere.bgyml", 40, Some(6000)),
            Some((6904, "added"))
        );
    }
}
