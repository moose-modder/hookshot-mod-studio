//! A game's `romfs` folder on disk (the user's own dump, as set up for Hookshot).

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::zs::{Decoder, Dictionaries};

/// Where the dictionaries are, relative to the romfs root.
pub const DICTIONARY_PACK: &str = "Pack/ZsDic.pack.zs";

#[derive(Debug, Clone)]
pub struct Romfs {
    root: PathBuf,
    overlays: Vec<PathBuf>,
    dictionaries: Dictionaries,
}

impl Romfs {
    /// Opens the romfs at `root` and loads its zstd dictionaries.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        let pack = read_file(&root.join(DICTIONARY_PACK))?;
        let dictionaries = Dictionaries::from_pack(&pack)?;
        Ok(Romfs {
            root,
            overlays: Vec::new(),
            dictionaries,
        })
    }

    /// Adds a mod's `romfs` folder on top: a file it has is read in place of the game's, as Hookshot overlays mods
    /// (of several, the last added wins). The zstd dictionaries stay the game's.
    pub fn with_overlay(mut self, dir: impl Into<PathBuf>) -> Self {
        self.overlays.push(dir.into());
        self
    }

    /// The game's romfs folder (without overlays).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where the file at `path` (relative to the romfs) is read from: the last overlay that has it, else the game's
    /// romfs. An absolute path is returned as it is.
    pub fn resolve(&self, path: impl AsRef<Path>) -> PathBuf {
        resolve_in(&self.root, &self.overlays, path.as_ref())
    }

    pub fn dictionaries(&self) -> &Dictionaries {
        &self.dictionaries
    }

    /// A decoder for this romfs's `.zs` files (one per thread).
    pub fn decoder(&self) -> Result<Decoder> {
        self.dictionaries.decoder()
    }

    /// The contents of `path` (relative to the romfs, see [`Romfs::resolve`], or absolute), decompressed if it ends
    /// in `.zs`.
    pub fn read(&self, decoder: &mut Decoder, path: impl AsRef<Path>) -> Result<Vec<u8>> {
        read_maybe_zs(decoder, &self.resolve(path))
    }
}

fn resolve_in(root: &Path, overlays: &[PathBuf], path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_owned();
    }
    overlays
        .iter()
        .rev()
        .map(|o| o.join(path))
        .find(|p| p.exists())
        .unwrap_or_else(|| root.join(path))
}

/// The contents of a file.
pub fn read_file(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|source| Error::Io {
        path: path.to_owned(),
        source,
    })
}

/// The contents of a file, decompressed if its name ends in `.zs`.
pub fn read_maybe_zs(decoder: &mut Decoder, path: &Path) -> Result<Vec<u8>> {
    let data = read_file(path)?;
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("zs"))
    {
        decoder.decompress(&data)
    } else {
        Ok(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlays_win_in_order() {
        let base = std::env::temp_dir().join(format!("modstudio-romfs-{}", std::process::id()));
        let (game, a, b) = (base.join("game"), base.join("a"), base.join("b"));
        for (dir, files) in [
            (&game, &["Model/x.bfres.zs", "Tex/t.bntx.zs"][..]),
            (&a, &["Model/x.bfres.zs", "Tex/t.bntx.zs"][..]),
            (&b, &["Tex/t.bntx.zs"][..]),
        ] {
            for f in files {
                let p = dir.join(f);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(&p, b"").unwrap();
            }
        }
        let overlays = [a.clone(), b.clone()];
        let r = |p: &str| resolve_in(&game, &overlays, Path::new(p));
        assert_eq!(r("Model/x.bfres.zs"), a.join("Model/x.bfres.zs"));
        assert_eq!(r("Tex/t.bntx.zs"), b.join("Tex/t.bntx.zs"));
        assert_eq!(r("Tex/other.bntx.zs"), game.join("Tex/other.bntx.zs"));
        assert_eq!(
            resolve_in(&game, &[], Path::new("Tex/t.bntx.zs")),
            game.join("Tex/t.bntx.zs")
        );
        let abs = base.join("elsewhere.zs");
        assert_eq!(r(abs.to_str().unwrap()), abs);
        std::fs::remove_dir_all(&base).unwrap();
    }
}
