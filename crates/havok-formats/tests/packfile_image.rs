//! The packfile image and writer on the games' own files: every packfile of Tony Hawk's SHRED (490, version 7) and of
//! Sekiro and Elden Ring (29 and 6 distinct, versions 9 and 11) is read, imaged and written back, and the bytes are the
//! input's. Skips what is not configured (see tests/inputs/README.md); no game data is in the repository.
mod hostile;
mod inputs;

use std::collections::BTreeMap;

use havok_formats::packfile::image::{Block, Target};
use havok_formats::packfile::{Packfile, Rules};
use inputs::{Game, OracleSet, Query, expect_found, resolve};

/// What a folder of packfiles came to.
#[derive(Debug, Default)]
struct Tally {
    files: usize,
    bytes: usize,
    objects: usize,
    runs: usize,
    nested_runs: usize,
    by_version: BTreeMap<u32, usize>,
}

fn runs(block: &Block, depth: usize, tally: &mut Tally) {
    for slot in &block.slots {
        if let Target::Run(run) = &slot.target {
            tally.runs += 1;
            if depth > 0 {
                tally.nested_runs += 1;
            }
            runs(run, depth + 1, tally);
        }
    }
}

fn round_trip(query: Query) -> Result<Option<Tally>, Box<dyn std::error::Error>> {
    let Some(dir) = resolve(query) else {
        return Ok(None);
    };
    let mut files = std::fs::read_dir(dir)?
        .map(|f| f.map(|f| f.path()))
        .collect::<Result<Vec<_>, _>>()?;
    files.sort();
    let mut tally = Tally::default();
    let mut differing = Vec::new();
    for path in &files {
        let bytes = std::fs::read(path)?;
        let name = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let pack = Packfile::read(&bytes).map_err(|e| format!("{name}: {e}"))?;
        let image = pack.image().map_err(|e| format!("{name}: {e}"))?;
        let written = image.write().map_err(|e| format!("{name}: {e}"))?;
        tally.files += 1;
        tally.bytes += bytes.len();
        tally.objects += image.objects.len();
        *tally.by_version.entry(pack.version).or_default() += 1;
        for object in &image.objects {
            runs(&object.block, 0, &mut tally);
        }
        if written != bytes {
            if let Ok(dir) = std::env::var("PACKFILE_DEBUG_DIR") {
                std::fs::write(format!("{dir}/{name}.written"), &written)?;
            }
            let at = written
                .iter()
                .zip(&bytes)
                .position(|(a, b)| a != b)
                .unwrap_or(written.len().min(bytes.len()));
            differing.push(format!(
                "{name}: first difference at {at:#x}, {} bytes written, {} read",
                written.len(),
                bytes.len()
            ));
        }
    }
    expect_found(query, "packfiles", tally.files);
    assert!(
        differing.is_empty(),
        "{} of {} files differ:\n{}",
        differing.len(),
        tally.files,
        differing.iter().take(8).cloned().collect::<Vec<_>>().join("\n")
    );
    Ok(Some(tally))
}

#[test]
fn shred_packfiles_are_written_back_byte_for_byte() -> Result<(), Box<dyn std::error::Error>> {
    let Some(tally) = round_trip(Query::Oracle(OracleSet::Corpus, Game::Shred, "havok-packfiles"))? else {
        return Ok(());
    };
    eprintln!("shred: {tally:?}");
    assert_eq!(tally.files, 490);
    assert_eq!(tally.by_version.get(&7), Some(&490));
    assert_eq!(tally.objects, 47_915);
    Ok(())
}

#[test]
fn sekiro_packfiles_are_written_back_byte_for_byte() -> Result<(), Box<dyn std::error::Error>> {
    let Some(tally) = round_trip(Query::Oracle(OracleSet::Corpus, Game::Sekiro, "havok-packfiles"))? else {
        return Ok(());
    };
    eprintln!("sekiro: {tally:?}");
    assert_eq!(tally.files, 29);
    assert_eq!(tally.by_version.get(&11), Some(&26));
    assert_eq!(tally.by_version.get(&9), Some(&3));
    Ok(())
}

#[test]
fn elden_packfiles_are_written_back_byte_for_byte() -> Result<(), Box<dyn std::error::Error>> {
    let Some(tally) = round_trip(Query::Oracle(OracleSet::Corpus, Game::Elden, "havok-packfiles"))? else {
        return Ok(());
    };
    eprintln!("elden: {tally:?}");
    assert_eq!(tally.files, 6);
    assert_eq!(tally.by_version.get(&11), Some(&3));
    assert_eq!(tally.by_version.get(&9), Some(&3));
    let _ = Rules::WINDOWS_64;
    Ok(())
}

/// Truncation, flipped bytes and the readers on a few real files of each set: the smallest and the middle of the sizes
/// below 200 kB (a flip costs a read, an image and a write of the whole file).
fn hostile_on(query: Query) -> Result<(), Box<dyn std::error::Error>> {
    let Some(dir) = resolve(query) else {
        return Ok(());
    };
    let mut files = std::fs::read_dir(dir)?
        .map(|f| f.map(|f| f.path()))
        .collect::<Result<Vec<_>, _>>()?;
    files.sort();
    let mut small = Vec::new();
    for path in files {
        let bytes = std::fs::read(&path)?;
        if bytes.len() <= 200_000 {
            small.push(bytes);
        }
    }
    small.sort_by_key(Vec::len);
    let picks = [0, small.len() / 4, small.len() / 2, small.len().saturating_sub(1)];
    let mut done = Vec::new();
    for pick in picks {
        if done.contains(&pick) {
            continue;
        }
        done.push(pick);
        if let Some(bytes) = small.get(pick) {
            hostile::hostile(
                &format!("file {pick} of {} ({} bytes)", small.len(), bytes.len()),
                bytes,
            );
        }
    }
    Ok(())
}

#[test]
fn real_shred_packfiles_are_hostile_proof() -> Result<(), Box<dyn std::error::Error>> {
    hostile_on(Query::Oracle(OracleSet::Corpus, Game::Shred, "havok-packfiles"))
}

#[test]
fn real_sekiro_packfiles_are_hostile_proof() -> Result<(), Box<dyn std::error::Error>> {
    hostile_on(Query::Oracle(OracleSet::Corpus, Game::Sekiro, "havok-packfiles"))
}

#[test]
fn real_elden_packfiles_are_hostile_proof() -> Result<(), Box<dyn std::error::Error>> {
    hostile_on(Query::Oracle(OracleSet::Corpus, Game::Elden, "havok-packfiles"))
}
