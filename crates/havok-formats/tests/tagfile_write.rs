//! Real-data check of the tagfile writer: every loose tagfile of the corpus sets (extracted from the user's own
//! install, see tests/inputs/README.md) is read, written again and compared byte for byte with the file; then one
//! numeric value of it is edited, and so is its first variant's name (a new string item), written and read back.
//! Skips without the sets. No game data is included in the repository.
//!
//! The same read, write and compare ran over every distinct tagfile of both installs, all identical.
mod inputs;

use havok_formats::{Compendium, Tagfile};
use inputs::{Game, OracleSet, Query, expect_found, resolve};

type Error = Box<dyn std::error::Error>;

/// A corpus folder's tagfiles (name and bytes, in name order) and its compendia.
type Folder = (Vec<(String, Vec<u8>)>, Vec<Compendium>);

/// The files of a corpus folder: tagfiles (`.hkx`) and compendia (`.compendium`).
fn folder(corpus: Query) -> Result<Option<Folder>, Error> {
    let Some(dir) = resolve(corpus) else {
        return Ok(None);
    };
    let mut paths = std::fs::read_dir(dir)?
        .map(|f| f.map(|f| f.path()))
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort();
    let mut tagfiles = Vec::new();
    let mut compendia = Vec::new();
    for p in paths {
        let label = p
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let bytes = std::fs::read(&p)?;
        match p.extension().and_then(|e| e.to_str()) {
            Some("hkx") => tagfiles.push((label, bytes)),
            Some("compendium") => compendia.push(Compendium::read(&bytes)?),
            _ => {}
        }
    }
    expect_found(corpus, ".hkx files", tagfiles.len());
    Ok(Some((tagfiles, compendia)))
}

/// What a folder's files came to.
#[derive(Debug, Default, PartialEq, Eq)]
struct Totals {
    files: usize,
    bytes: usize,
    items: usize,
    patches: usize,
    edited_numbers: usize,
    renamed: usize,
}

/// A tagfile of the folder, through the compendium that holds its types when it needs one.
fn read<'a>(
    bytes: &'a [u8],
    compendia: &'a [Compendium],
) -> Result<(Tagfile<'a>, Option<&'a Compendium>), Error> {
    if let Ok(t) = Tagfile::read(bytes, None) {
        return Ok((t, None));
    }
    for c in compendia {
        if let Ok(t) = Tagfile::read(bytes, Some(c)) {
            return Ok((t, Some(c)));
        }
    }
    Ok((Tagfile::read(bytes, compendia.first())?, compendia.first()))
}

/// Read, write and compare every file of a folder, and edit each.
fn check(corpus: Query) -> Result<Option<Totals>, Error> {
    let Some((files, compendia)) = folder(corpus)? else {
        return Ok(None);
    };
    let mut totals = Totals::default();
    for (label, bytes) in &files {
        let (tag, compendium) = read(bytes, &compendia).map_err(|e| format!("{label}: {e}"))?;
        let image = tag.image()?;
        let written = image.write().map_err(|e| format!("{label}: {e}"))?;
        assert!(
            written == *bytes,
            "{label}: the written file differs from the file"
        );
        totals.files += 1;
        totals.bytes += bytes.len();
        totals.items += tag.items.len() - 1;
        totals.patches += tag.patches.iter().map(|(_, o)| o.len()).sum::<usize>();

        // change one numeric value: the first element of the first item of a number type (not a character)
        let number = (1..tag.items.len()).find_map(|i| {
            let v = tag.item(i).ok()?.get(0).ok()?;
            let kind = v.format().ok()? & 15;
            if ![2, 4, 5].contains(&kind) || v.type_name() == "char" {
                return None;
            }
            tag.item_at(v.offset).filter(|&(item, _)| item == i)
        });
        if let Some((item, within)) = number {
            let mut edited = image.clone();
            let byte = edited.items[item]
                .data
                .get_mut(within)
                .ok_or("byte outside its item")?;
            *byte = byte.wrapping_add(1);
            let out = edited.write()?;
            assert_eq!(out.len(), bytes.len(), "{label}");
            // everything else is the file's, the one byte changed
            let differing: Vec<usize> = (0..out.len()).filter(|&i| out[i] != bytes[i]).collect();
            assert_eq!(differing.len(), 1, "{label}: one byte differs");
            let back = Tagfile::read(&out, compendium)?;
            assert_eq!(back.items.len(), tag.items.len(), "{label}");
            for (a, b) in tag.items.iter().zip(&back.items).skip(1) {
                assert_eq!(
                    (a.type_index, a.kind, a.offset, a.count),
                    (b.type_index, b.kind, b.offset, b.count)
                );
            }
            totals.edited_numbers += 1;
        }

        // rename the first variant: a new character run, the name slot pointed at it
        let variants = tag.root()?.field("namedVariants")?.array()?;
        if !variants.is_empty() {
            let name_slot = variants.get(0)?.field("name")?;
            let (item, within) = tag.item_at(name_slot.offset).ok_or("the name is in no item")?;
            let mut edited = image.clone();
            edited.set_string(item, within, "renamed by the test")?;
            let out = edited.write()?;
            let back = Tagfile::read(&out, compendium)?;
            let after = back.root()?.field("namedVariants")?.array()?;
            assert_eq!(after.len(), variants.len(), "{label}");
            for i in 0..variants.len() {
                let (was, now) = (variants.get(i)?, after.get(i)?);
                let want = if i == 0 {
                    "renamed by the test".into()
                } else {
                    was.field("name")?.string()?
                };
                assert_eq!(now.field("name")?.string()?, want, "{label}: name {i}");
                assert_eq!(
                    now.field("className")?.string()?,
                    was.field("className")?.string()?,
                    "{label}: class {i}"
                );
            }
            // the old name's item is left out when nothing else refers to it
            assert!(
                back.items.len() == tag.items.len() || back.items.len() == tag.items.len() + 1,
                "{label}"
            );
            assert_eq!(
                back.image()?.write()?,
                out,
                "{label}: the rewrite is not the same file"
            );
            totals.renamed += 1;
        }
    }
    Ok(Some(totals))
}

/// The folder's files all round trip and come to `want`; a folder that is not configured is skipped.
fn expect(corpus: Query, want: Totals) -> Result<(), Error> {
    if let Some(totals) = check(corpus)? {
        assert_eq!(totals, want, "{corpus:?}");
    }
    Ok(())
}
const fn totals(files: usize, bytes: usize, items: usize, patches: usize) -> Totals {
    Totals {
        files,
        bytes,
        items,
        patches,
        edited_numbers: files,
        renamed: files,
    }
}
#[test]
fn sekiro_map_collision_round_trips() -> Result<(), Error> {
    expect(
        Query::Oracle(OracleSet::Corpus, Game::Sekiro, "havok"),
        totals(173, 228_763_420, 33_491, 33_491),
    )
}
#[test]
fn the_sdk_2015_1_clip_round_trips() -> Result<(), Error> {
    expect(
        Query::Oracle(OracleSet::Corpus, Game::Sekiro, "havok-2015"),
        totals(1, 22_492, 70, 70),
    )
}
#[test]
fn sekiro_cutscene_files_round_trip() -> Result<(), Error> {
    expect(
        Query::Oracle(OracleSet::Corpus, Game::Sekiro, "havok-float"),
        totals(2, 155_144, 1_064, 1_068),
    )
}
#[test]
fn sekiro_low_resolution_collision_round_trips() -> Result<(), Error> {
    expect(
        Query::Oracle(OracleSet::Corpus, Game::Sekiro, "havok-l"),
        totals(1, 21_790_732, 2_470, 2_470),
    )
}
#[test]
fn elden_ring_cutscene_files_round_trip() -> Result<(), Error> {
    expect(
        Query::Oracle(OracleSet::Corpus, Game::Elden, "havok-float"),
        totals(3, 4_009_656, 2_358, 2_423),
    )
}
#[test]
fn elden_ring_low_resolution_collision_round_trips() -> Result<(), Error> {
    expect(
        Query::Oracle(OracleSet::Corpus, Game::Elden, "havok-l"),
        totals(1, 65_234_176, 8_758, 8_757),
    )
}
