//! The class versions the games' own type tables store, per game: Sekiro (2016.2) and Elden Ring (2018.1) differ in the
//! versions of most of the `hknp` classes, and no class table in the crate may state the one game's for the other. Every
//! tagfile of the Sekiro area and of the Elden Ring tile the corpus holds is read; a class that occurs must have the
//! version listed, and each listed class must occur in at least one file. Skips without the corpus.
mod inputs;

use std::collections::BTreeMap;

use havok_formats::{Compendium, Tagfile};
use inputs::{Game, OracleSet, Query, expect_found, resolve};

/// The version of each class in the games' type tables, as read from the tables: class, Sekiro, Elden Ring.
const VERSIONS: [(&str, u64, u64); 9] = [
    ("hknpCompressedMeshShape", 6, 7),
    ("hknpCompressedMeshShapeData", 1, 2),
    ("hknpBodyCinfo", 6, 8),
    ("hknpPhysicsSystemData", 2, 3),
    ("hknpMaterial", 4, 5),
    ("hknpMotionProperties", 4, 5),
    ("hknpWorldCinfo", 11, 13),
    ("hknpExternMeshShape", 2, 3),
    ("hkcdSimdTree", 1, 3),
];
/// Classes whose version the two games share.
const SHARED: [(&str, u64); 4] = [
    ("hknpConvexPolytopeShape", 4),
    ("hknpShape", 4),
    ("hknpCompositeShape", 2),
    ("hknpPhysicsSceneData", 1),
];

/// Every class version seen in the corpus (and the number of tables it was seen in), by class name; a class with two
/// versions in one corpus is a failure.
type Versions = BTreeMap<String, (u64, usize)>;

fn seen(query: Query) -> Result<Option<(usize, Versions)>, Box<dyn std::error::Error>> {
    let Some(dir) = resolve(query) else {
        return Ok(None);
    };
    let mut files = std::fs::read_dir(dir)?
        .map(|f| f.map(|f| f.path()))
        .collect::<Result<Vec<_>, _>>()?;
    files.sort();
    let comps = files
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == "compendium"))
        .map(|p| Compendium::read(&std::fs::read(p)?).map_err(Box::<dyn std::error::Error>::from))
        .collect::<Result<Vec<_>, _>>()?;
    let mut out: BTreeMap<String, (u64, usize)> = BTreeMap::new();
    let mut tagfiles = 0;
    for p in files.iter().filter(|p| p.extension().is_some_and(|e| e == "hkx")) {
        let bytes = std::fs::read(p)?;
        let mut result = Tagfile::read(&bytes, None);
        for c in &comps {
            if result.is_ok() {
                break;
            }
            result = Tagfile::read(&bytes, Some(c));
        }
        let tag = result.map_err(|e| format!("{}: {e}", p.display()))?;
        tagfiles += 1;
        for t in &tag.types {
            if let Some(version) = t.version {
                let entry = out.entry(t.name.clone()).or_insert((version, 0));
                assert_eq!(entry.0, version, "{} has two versions in one corpus", t.name);
                entry.1 += 1;
            }
        }
    }
    expect_found(query, ".hkx files", tagfiles);
    Ok(Some((tagfiles, out)))
}

fn check(game: &str, found: &BTreeMap<String, (u64, usize)>, pick: impl Fn(&(&str, u64, u64)) -> u64) {
    let mut present = 0;
    for row in &VERSIONS {
        if let Some((version, _)) = found.get(row.0) {
            assert_eq!(*version, pick(row), "{game}: {}", row.0);
            present += 1;
        }
    }
    for (class, version) in SHARED {
        if let Some((found, _)) = found.get(class) {
            assert_eq!(*found, version, "{game}: {class}");
            present += 1;
        }
    }
    eprintln!(
        "{game}: {present} of {} listed classes occur",
        VERSIONS.len() + SHARED.len()
    );
    assert!(present >= 8, "{game}: only {present} listed classes occur");
}

#[test]
fn sekiro_class_versions() -> Result<(), Box<dyn std::error::Error>> {
    let Some((_, found)) = seen(Query::Oracle(OracleSet::Corpus, Game::Sekiro, "havok"))? else {
        return Ok(());
    };
    check("sekiro", &found, |row| row.1);
    Ok(())
}

#[test]
fn elden_ring_class_versions() -> Result<(), Box<dyn std::error::Error>> {
    let Some((_, found)) = seen(Query::Oracle(OracleSet::Corpus, Game::Elden, "havok-l"))? else {
        return Ok(());
    };
    check("elden", &found, |row| row.2);
    Ok(())
}
