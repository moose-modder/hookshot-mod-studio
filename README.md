# Hookshot Mod Studio

Model mods for Hookshot installs, without hand-assembling tools: pick a model from your own
game files, get a Blender scene, edit it, and turn the scene into a mod that Hookshot loads.

> Working name. Status: **early**. The model pipeline works today with Blender and a Switch Toolbox build
> (`pipeline/`); the Rust tool that replaces Toolbox is being written (`crates/`), and the app comes after it.
> See [docs/plan.md](docs/plan.md).

## What it does

1. **Find a model** in the `romfs` of your Hookshot install (your own dump; nothing from the game is in this repo).
2. **Create a Blender scene** from it: the model, its textures, materials set up for editing.
3. **Edit** in your own Blender: shape, new materials, painted textures.
4. **Create the mod** from the saved scene, with Blender still open: changed looks are baked into the game's
   textures, the model is rebuilt with the game's formats, and the mod is installed into Hookshot's `mods` folder
   with one resource size table for all mods.

## Layout

| Path | |
|---|---|
| `crates/modstudio-formats` | the game's file formats in Rust: zstd with the game's dictionaries, SARC, BNTX, BFRES |
| `crates/modstudio-cli` | `modstudio`, the command-line tool the pipeline and the app call |
| `pipeline/` | Python: Blender scene creation, baking and export (runs inside Blender), build and install |
| `docs/` | plan and notes |

## Building

```
cargo build --release
cargo test
```

The toolchain is pinned in `rust-toolchain.toml` (rustup picks it up). Windows and Linux.

## Rules

- **No game files, and nothing made from them**, in the repo, issues or CI: models, textures, archives, exported
  `.png`/`.glb`/`.fbx`, `.blend` scenes, JSON dumps of models, built mods. Tests use synthetic data; checks against a
  real dump are run locally and reported.
- Every dependency must pass `cargo deny check` (known GPL-compatible license, crates.io, built from source).

## License

GPL-3.0-or-later (see `LICENSE`). Third-party code and references: [THIRD_PARTY.md](THIRD_PARTY.md).
