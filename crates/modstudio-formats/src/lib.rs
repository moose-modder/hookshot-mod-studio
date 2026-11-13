//! The game's model files, read and written without any outside tool.
//!
//! - [`zs`]: zstd frames, with the dictionaries the game keeps in `romfs/Pack/ZsDic.pack.zs`
//! - [`sarc`]: the archive format of that pack (and others)
//! - [`romfs`]: reading files from a game's romfs folder
//! - [`bntx`]: textures, decoded through [`tegra`] (the GPU's block-linear layout) to [`image::Image`]
//! - [`bntx_write`]: a texture written back with a new image, encoded by [`bc_encode`]
//! - [`bfres`]: models: skeleton, shapes with their LOD meshes, vertex and index buffers, materials
//! - [`bfres_write`]: a model written back with new geometry for some shapes
//!
//! The byte-level layouts are ported from Syroot's NintenTools libraries (MIT) as maintained in
//! KillzXGaming/LegacySwitchLibraries; see `THIRD_PARTY.md`.

pub mod bc_encode;
mod bcn;
pub mod bfres;
pub mod bfres_write;
pub mod bntx;
pub mod bntx_write;
mod bytes;
mod error;
pub mod image;
mod math_srgb;
pub mod restbl;
pub mod romfs;
pub mod sarc;
pub mod tegra;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod zs;

pub use error::{Error, Result};
