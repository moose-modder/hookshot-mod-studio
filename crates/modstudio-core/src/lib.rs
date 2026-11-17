//! What the `modstudio` CLI and the app do, on top of `modstudio-formats`:
//!
//! - [`model::inspect`]: a model's meshes, materials, vertex formats and textures, as `model.json`
//! - [`model::export`]: the model as `.glb` (skeleton, LOD 0 of each mesh, skin weights, UVs, colours), its
//!   textures as PNG, and `model.json`, for Blender and previews
//! - [`replace::replace_model`]: the model rebuilt from Blender's glTF export of the scene
//! - [`texture::replace_textures`]: changed textures (PNG) written in the game's format
//!
//! And the workflow the CLI and the app share:
//!
//! - [`settings::Settings`]: where the game, the mods folder, the workspace and the tools are
//! - [`catalog::list`]: the game's models by kind; [`preview::preview`]: one as a self-contained `.glb`
//! - [`project::create`]: a project for a model (export, Blender scene, `project.json`)
//! - [`build::build`]: the mod built from the edited scene, checked, and installed
//! - [`cloth`]: the game's cloth simulated against a test motion, for a preview in Blender
//! - [`anim`]: a model's animations, listed and sampled for playing
//! - [`anim_edit`]: animations into a Blender scene, and the scene's written back into the game's files
//! - [`actor_links`]: which cloth and animation files the game's actors give each model
//! - [`mods`]: the game's mods folder: mods on and off, one resource size table for all of them
//!
//! [`mods`], [`sizetable`], [`progress`] and the error type come from `modstudio-mods`, which the mod manager
//! shares.

pub mod actor_links;
pub mod anim;
pub mod anim_edit;
pub mod build;
pub mod catalog;
pub mod cloth;
pub mod cloth_edit;
pub mod cloth_new;
pub mod eyes;
pub mod gltf;
pub mod look;
pub mod math;
pub mod model;
pub mod preview;
pub mod project;
pub mod replace;
pub mod settings;
pub mod texture;
pub mod tools;

use modstudio_mods::{Context, fsutil};
pub use modstudio_mods::{Error, Result, mods, progress, sizetable};
