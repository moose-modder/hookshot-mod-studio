//! A game's `romfs` folder on disk (the user's own dump, as set up for Hookshot).

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::zs::{Decoder, Dictionaries};

/// Where the dictionaries are, relative to the romfs root.
pub const DICTIONARY_PACK: &str = "Pack/ZsDic.pack.zs";

#[derive(Debug, Clone)]
pub struct Romfs {
    root: PathBuf,
    dictionaries: Dictionaries,
}

impl Romfs {
    /// Opens the romfs at `root` and loads its zstd dictionaries.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        let pack = read_file(&root.join(DICTIONARY_PACK))?;
        let dictionaries = Dictionaries::from_pack(&pack)?;
        Ok(Romfs { root, dictionaries })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn dictionaries(&self) -> &Dictionaries {
        &self.dictionaries
    }

    /// A decoder for this romfs's `.zs` files (one per thread).
    pub fn decoder(&self) -> Result<Decoder> {
        self.dictionaries.decoder()
    }

    /// The contents of `path` (relative to the root, or absolute), decompressed if it ends in `.zs`.
    pub fn read(&self, decoder: &mut Decoder, path: impl AsRef<Path>) -> Result<Vec<u8>> {
        read_maybe_zs(decoder, &self.root.join(path))
    }
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
