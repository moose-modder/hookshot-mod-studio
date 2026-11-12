//! What the `modstudio` CLI and the app do, on top of `modstudio-formats`:
//!
//! - [`model::inspect`]: a model's meshes, materials, vertex formats and textures, as `model.json`
//! - [`model::export`]: the model as `.glb` (skeleton, LOD 0 of each mesh, skin weights, UVs, colours), its
//!   textures as PNG, and `model.json`, for Blender and previews
//! - [`replace::replace_model`]: the model rebuilt from Blender's glTF export of the scene
//! - [`texture::replace_textures`]: changed textures (PNG) written in the game's format

pub mod gltf;
pub mod math;
pub mod model;
pub mod replace;
pub mod texture;

use std::fmt;

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
pub(crate) trait Context<T> {
    fn context(self, what: impl fmt::Display) -> Result<T>;
}

impl<T, E: fmt::Display> Context<T> for std::result::Result<T, E> {
    fn context(self, what: impl fmt::Display) -> Result<T> {
        self.map_err(|e| Error(format!("{what}: {e}")))
    }
}
