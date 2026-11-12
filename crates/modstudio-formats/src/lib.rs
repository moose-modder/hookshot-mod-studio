//! The game's model files, read and written without any outside tool.
//!
//! Planned modules (see `docs/plan.md`):
//! - `zs`: zstd frames, with the dictionaries the game keeps in `romfs/Pack/ZsDic.pack.zs`
//! - `sarc`: the archive format of that pack (and others)
//! - `bntx`: textures, including the Switch's block-linear layout
//! - `bfres`: models: skeleton, meshes, vertex and index buffers, LODs, bounds; materials passed through
//!
//! The byte-level layouts are ported from Syroot's NintenTools libraries (MIT) as maintained in
//! KillzXGaming/LegacySwitchLibraries; see `THIRD_PARTY.md`.
