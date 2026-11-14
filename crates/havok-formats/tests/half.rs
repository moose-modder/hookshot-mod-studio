//! Real-data check of `Value::half`: the materials (`hknpMaterial`) of the map collision tagfiles store their friction,
//! restitution and mass-changer factors as `hkHalf16`. Reads every material of the loose tagfiles of one Sekiro area and
//! of the one Elden Ring tile the corpus holds; skips without them.
#![allow(
    clippy::unwrap_used,
    reason = "a test fixture that does not parse is the test's failure"
)]
mod inputs;
mod tagbuilder;

use havok_formats::{Compendium, HALF_FORMAT, Tagfile};
use inputs::{Game, OracleSet, Query, expect_found, resolve};
use tagbuilder::{Layout, primitive, record, tagfile};

const HALVES: [&str; 4] = [
    "dynamicFriction",
    "staticFriction",
    "restitution",
    "massChangerHeavyObjectFactor",
];

/// Every material of every tagfile in the corpus folder: how many, and the values read.
type Materials = (usize, Vec<[f32; 4]>);

fn materials(corpus: Query) -> Result<Option<Materials>, Box<dyn std::error::Error>> {
    let Some(dir) = resolve(corpus) else {
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
    let mut count = 0;
    let mut values = Vec::new();
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
        let variants = tag.root()?.field("namedVariants")?.array()?;
        for i in 0..variants.len() {
            let physics = variants.get(i)?.field("variant")?.dereference()?;
            let systems = physics.field("systemDatas")?.array()?;
            for j in 0..systems.len() {
                let system = systems.get(j)?.dereference()?;
                let list = system.field("materials")?.array()?;
                for k in 0..list.len() {
                    let material = list.get(k)?;
                    let mut row = [0.0; 4];
                    for (slot, name) in row.iter_mut().zip(HALVES) {
                        *slot = material.field(name)?.half()?;
                    }
                    values.push(row);
                    count += 1;
                }
            }
        }
    }
    expect_found(corpus, ".hkx files", tagfiles);
    Ok(Some((count, values)))
}

/// Every material of the corpus holds one row: friction 0.5, restitution 0.4 and a heavy-object factor of 1, each cut to
/// the top 16 bits of the `f32` (0.4 is `0x3ECCCCCD`, so the half reads back as `0x3ECC0000`). A reader that took the
/// 16 bits for an IEEE binary16 would read 1.75, 1.75, 1.699 and 1.875.
fn check(name: &str, count: usize, values: &[[f32; 4]]) {
    assert_eq!(values.len(), count);
    let truncated_0_4 = f32::from_bits(0.4f32.to_bits() & 0xFFFF_0000);
    assert_eq!(truncated_0_4, 0.398_437_5);
    for row in values {
        assert_eq!(*row, [0.5, 0.5, truncated_0_4, 1.0], "{name}");
    }
    eprintln!("{name}: {count} materials");
}

#[test]
fn sekiro_materials() -> Result<(), Box<dyn std::error::Error>> {
    let corpus = Query::Oracle(OracleSet::Corpus, Game::Sekiro, "havok");
    let Some((count, values)) = materials(corpus)? else {
        return Ok(());
    };
    assert!(count > 0, "no materials");
    check("sekiro", count, &values);
    Ok(())
}

#[test]
fn elden_materials() -> Result<(), Box<dyn std::error::Error>> {
    let corpus = Query::Oracle(OracleSet::Corpus, Game::Elden, "havok-l");
    let Some((count, values)) = materials(corpus)? else {
        return Ok(());
    };
    assert!(count > 0, "no materials");
    check("elden", count, &values);
    Ok(())
}

/// A root with a half, a float and an integer of the same width as the half: only the half is read as one, and it is the
/// top 16 bits of an `f32` (so `0x3F80` is 1, `0xC000` is -2 and `0x0000` is 0).
#[test]
fn a_half_is_the_top_sixteen_bits_of_a_float() {
    let definitions = vec![
        primitive("hkHalf16", HALF_FORMAT_WORD, 2, None), // 1
        primitive("float", 0x174605, 4, None),            // 2
        primitive("unsigned short", 0x4004, 2, None),     // 3
        record("Root", 8, vec![("h", 0, 1), ("f", 4, 2), ("w", 2, 3)]), // 4
    ];
    for (bits, want) in [
        (0x3F80u16, 1.0f32),
        (0xC000, -2.0),
        (0x0000, 0.0),
        (0x3ECC, 0.398_437_5),
    ] {
        let mut layout = Layout::default();
        let (_, root) = layout.alloc(4, 8, 1, false).unwrap();
        layout.put(root, &bits.to_le_bytes());
        layout.put(root + 2, &bits.to_le_bytes());
        layout.put(root + 4, &1.5f32.to_le_bytes());
        let bytes = tagfile(b"20180100", &definitions, layout.data, &layout.items).unwrap();
        let tag = Tagfile::read(&bytes, None).unwrap();
        let root = tag.root().unwrap();
        assert_eq!(root.field("h").unwrap().half().unwrap(), want, "{bits:#06x}");
        assert!(root.field("f").unwrap().half().is_err());
        assert!(root.field("w").unwrap().half().is_err());
        assert!(root.field("h").unwrap().float().is_err());
        assert_eq!(root.field("f").unwrap().float().unwrap(), 1.5);
    }
}

const HALF_FORMAT_WORD: u32 = HALF_FORMAT as u32;
