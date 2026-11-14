//! Real-data check of the one SDK 2015.1 tagfile of Sekiro (`c1090`'s clip `a000_205000`), extracted from the user's
//! own install (see tests/inputs/README.md); skips without it. No game data is included in the repository.
//!
//! The tagfile names SDK `20150100` and carries its own type table (`TYPE`, with the 2015 section names `TNAM` and
//! `TBOD`). Its classes are laid out differently from the 2016.2 ones (a 2015 `hkReferencedObject` is 16 bytes, a 2016
//! one 24), which the reader takes from the file's type table. The counts are those of a census of the install.
mod inputs;

use havok_formats::Tagfile;
use havok_formats::animation::{Tracks, container};
use havok_formats::spline::SplineAnimation;
use inputs::{Game, OracleSet, Query, expect_found, resolve};

#[test]
fn the_2015_1_clip_reads_with_its_own_layouts() -> Result<(), Box<dyn std::error::Error>> {
    let corpus = Query::Oracle(OracleSet::Corpus, Game::Sekiro, "havok-2015");
    let Some(dir) = resolve(corpus) else {
        return Ok(());
    };
    let mut files = std::fs::read_dir(dir)?
        .map(|f| f.map(|f| f.path()))
        .collect::<Result<Vec<_>, _>>()?;
    files.sort();
    expect_found(corpus, ".hkx files", files.len());
    let bytes = std::fs::read(&files[0])?;
    let tag = Tagfile::read(&bytes, None)?;
    assert_eq!(&tag.version, b"20150100");
    // its own type table (the null type and 192 more), 71 items (the null item and 70 more), and every type hash it
    // stores (25) is the one its description gives
    assert_eq!(tag.types.len(), 193);
    assert_eq!(tag.items.len(), 71);
    assert_eq!(tag.types.iter().filter(|t| t.hash.is_some()).count(), 25);
    // the class layouts are the file's: a 2015 animation is 8 bytes smaller than a 2016.2 one (56, not 64)
    let animation = tag
        .types
        .iter()
        .find(|t| t.name == "hkaAnimation")
        .ok_or("no hkaAnimation")?;
    assert_eq!(animation.size, Some(56));
    let c = container(&tag)?;
    assert_eq!((c.animations.len(), c.bindings.len()), (1, 1));
    let Tracks::Spline(spline) = &c.animations[0].tracks else {
        return Err("not a spline compressed animation".into());
    };
    assert_eq!(spline.num_frames, 36);
    let parsed = SplineAnimation::parse(&c.animations[0])?;
    assert_eq!((parsed.blocks.len(), parsed.blocks[0].len()), (1, 54));
    Ok(())
}
