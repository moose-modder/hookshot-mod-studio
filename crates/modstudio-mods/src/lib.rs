//! A mods folder as the game reads it (Hookshot, Longshot: `mods/<name>/romfs/...`, a folder whose name starts
//! with `-` or `_` is off), and one resource size table for every mod that is on. Mod Studio and the mod manager
//! both use it, so they build the same table.
//!
//! - [`mods`]: the mods in a folder, turning one on or off, and the shared table rebuilt for all that are on
//! - [`sizetable`]: what each mod file needs in the table, worked out from the game's own table
//! - [`progress`]: what a long task tells the user as it goes, and cancelling it
//! - [`fsutil`]: small file helpers

pub mod fsutil;
pub mod mods;
pub mod progress;
pub mod sizetable;

use std::fmt;
use std::path::Path;

use modstudio_formats::romfs::Romfs;

/// A failure, with enough context to show to the user.
#[derive(Debug)]
pub struct Error(pub String);

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<modstudio_formats::Error> for Error {
    fn from(e: modstudio_formats::Error) -> Self {
        Error(e.to_string())
    }
}

/// Adds context to errors: `.context("reading x")?`.
pub trait Context<T> {
    fn context(self, what: impl fmt::Display) -> Result<T>;
}

impl<T, E: fmt::Display> Context<T> for std::result::Result<T, E> {
    fn context(self, what: impl fmt::Display) -> Result<T> {
        self.map_err(|e| Error(format!("{what}: {e}")))
    }
}

/// Whether `dir` is the game's files folder (its romfs: `<game folder>/romfs`).
pub fn is_game_romfs(dir: &Path) -> bool {
    dir.join("Model").is_dir() && dir.join("Pack").join("ZsDic.pack.zs").is_file()
}

/// The game's files folder, opened; an error that says why when `dir` isn't one.
pub fn open_game_romfs(dir: &Path) -> Result<Romfs> {
    if !is_game_romfs(dir) {
        return Err(Error(format!(
            "{} isn't the game's romfs (no Model folder or Pack/ZsDic.pack.zs)",
            dir.display()
        )));
    }
    Ok(Romfs::open(dir)?)
}
