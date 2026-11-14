//! Real-data checks of the XML form of a tagfile's objects (`havok_formats::xml`) on the behaviour tagfiles of the Sekiro
//! and Elden Ring installs, extracted from the user's own install (see tests/inputs/README.md); each skips without its input.
//! No game data is included in the repository.
//!
//! - Against HKLib: HKLib's own XML of twelve Elden Ring behaviour files (the biggest of them, 36,527 objects, and
//!   eleven others from 3 to 10,355) is parsed, brought to the identity form, and compared with this crate's objects of the
//!   same file: no difference, except that HKLib reads a string's bytes as ISO 8859-1, which the one file with a
//!   non-ASCII string shows until [`latin1_strings`] makes this crate's tree say the same. (HKLib reads only the 2018.1
//!   tagfiles, so it has no XML of Sekiro's.)
//! - Against a second writer: a census written in another language, from the type tables and item tables of the same
//!   files, wrote the same XML of every behaviour tagfile of each install (its rules checked against HKLib on every
//!   Elden Ring file); the CRC-32 of the text of all of a game's files, the byte count and the number of elements of each
//!   name are what it found.
mod inputs;
mod xml_expected;

use std::collections::BTreeMap;

use havok_formats::Tagfile;
use havok_formats::xml::{Element, diff, identity_form, latin1_strings, objects};
use inputs::{Game, OracleSet, Query, expect_found, resolve};
use xml_expected::{Digest, ELDEN, HKLIB_SAMPLE, SEKIRO};

type Failure = Box<dyn std::error::Error>;

fn files_with(dir: &std::path::Path, suffix: &str) -> Result<Vec<std::path::PathBuf>, Failure> {
    let mut paths = std::fs::read_dir(dir)?
        .map(|f| f.map(|f| f.path()))
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort();
    paths.retain(|p| p.to_string_lossy().ends_with(suffix));
    Ok(paths)
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

#[test]
fn hklib_writes_the_same_objects_for_the_elden_ring_files_it_reads() -> Result<(), Failure> {
    let xml_corpus = Query::Oracle(OracleSet::Corpus, Game::Elden, "havok-behavior-xml");
    let hkx_corpus = Query::Oracle(OracleSet::Corpus, Game::Elden, "havok-behavior");
    let (Some(xml_dir), Some(hkx_dir)) = (resolve(xml_corpus), resolve(hkx_corpus)) else {
        return Ok(());
    };
    let xml_files = files_with(&xml_dir, ".hkx.xml")?;
    expect_found(xml_corpus, ".hkx.xml files", xml_files.len());
    assert_eq!(xml_files.len(), HKLIB_SAMPLE.len());
    let mut compared = 0usize;
    for (path, &(expected_name, expected_objects, plain_differences)) in xml_files.iter().zip(HKLIB_SAMPLE) {
        let name = file_name(path);
        assert_eq!(name, expected_name);
        let hkx = hkx_dir.join(name.trim_end_matches(".xml"));
        let bytes = std::fs::read(&hkx).map_err(|e| format!("{}: {e}", hkx.display()))?;
        let tag = Tagfile::read(&bytes, None)?;
        let ours = objects(&tag)?;
        assert_eq!(ours.children.len(), expected_objects, "{name}: objects");
        let theirs = identity_form(&Element::parse(&std::fs::read_to_string(path)?)?)?;
        assert_eq!(theirs.children.len(), expected_objects, "{name}: HKLib's objects");

        // HKLib's strings are one character a byte
        let differences = diff(&ours, &theirs, 5);
        assert_eq!(differences.len(), plain_differences, "{name}: {differences:?}");
        let marked = diff(&latin1_strings(&ours), &theirs, 5);
        assert!(marked.is_empty(), "{name}: {marked:?}");

        // the text writes back to the same tree
        if expected_objects < 5000 {
            assert_eq!(Element::parse(&ours.write())?, ours, "{name}: text round trip");
        }
        compared += expected_objects;
    }
    assert_eq!(compared, HKLIB_SAMPLE.iter().map(|&(_, n, _)| n).sum::<usize>());
    Ok(())
}

/// The CRC-32 table of the polynomial zlib uses (reflected 0xEDB88320).
fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    for (i, slot) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
        *slot = c;
    }
    table
}

/// zlib's `crc32(data, crc)`.
fn crc32(table: &[u32; 256], crc: u32, data: &[u8]) -> u32 {
    let mut c = !crc;
    for &b in data {
        c = table[((c ^ u32::from(b)) & 0xff) as usize] ^ (c >> 8);
    }
    !c
}

/// Count the elements of a tree by name.
fn count_elements(root: &Element, counts: &mut BTreeMap<String, usize>) {
    let mut stack = vec![root];
    while let Some(element) = stack.pop() {
        *counts.entry(element.name.clone()).or_default() += 1;
        stack.extend(element.children.iter());
    }
}

fn digest(game: Game, expected: &Digest) -> Result<(), Failure> {
    let corpus = Query::Oracle(OracleSet::Corpus, game, "havok-behavior");
    let Some(dir) = resolve(corpus) else {
        return Ok(());
    };
    let files = files_with(&dir, ".hkx")?;
    expect_found(corpus, ".hkx files", files.len());
    let table = crc_table();
    let (mut crc, mut bytes) = (0u32, 0u64);
    let mut counts = BTreeMap::new();
    for path in &files {
        let data = std::fs::read(path)?;
        let tag = Tagfile::read(&data, None).map_err(|e| format!("{}: {e}", file_name(path)))?;
        let tree = objects(&tag).map_err(|e| format!("{}: {e}", file_name(path)))?;
        count_elements(&tree, &mut counts);
        let text = tree.write();
        crc = crc32(&table, crc, text.as_bytes());
        bytes += text.len() as u64;
    }
    assert_eq!(files.len(), expected.files);
    assert_eq!(bytes, expected.bytes, "bytes of XML text");
    assert_eq!(
        counts.into_iter().collect::<Vec<_>>(),
        expected
            .elements
            .iter()
            .map(|&(n, c)| (n.to_owned(), c))
            .collect::<Vec<_>>()
    );
    assert_eq!(crc, expected.crc32, "CRC-32 of the XML text");
    Ok(())
}

#[test]
fn the_objects_of_every_sekiro_behaviour_file_are_what_a_second_writer_wrote() -> Result<(), Failure> {
    digest(Game::Sekiro, &SEKIRO)
}

#[test]
fn the_objects_of_every_elden_ring_behaviour_file_are_what_a_second_writer_wrote() -> Result<(), Failure> {
    digest(Game::Elden, &ELDEN)
}
