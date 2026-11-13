# Hookshot Mod Studio

Model mods for Hookshot installs, without hand-assembling tools: pick a model from your own game files, get a
Blender scene, edit it, and turn the scene into a mod that Hookshot loads.

> Working name. Status: **early**. The app (`app/`) and the `modstudio` CLI run the whole flow; the game's formats
> and the resource size table are handled in Rust (`crates/`), the editing and baking happen in your Blender
> (`pipeline/`). See [docs/plan.md](docs/plan.md).

## What it does

1. **Find a model** in the `romfs` of your Hookshot install (your own dump; nothing from the game is in this repo),
   and look at it in 3D.
2. **Make a project**: the model exported, a Blender scene made from it (the model, its textures, materials set up
   for editing).
3. **Edit** in your own Blender: shape, new meshes and materials, painted textures.
4. **Build and install**: changed looks are baked into the game's textures, the model is rebuilt with the
   game's formats (unedited meshes kept exactly as the game has them), the result is compared with your scene,
   and the mod goes into Hookshot's `mods` folder with one resource size table for all mods.
5. **Turn mods on and off.**

You need a Hookshot install and [Blender](https://www.blender.org/) 5 or newer.

## The app

The app's first page asks for your Hookshot folder (the game files and mods folder in it), a folder for
projects, and Blender. Then:

- **Models**: the game's models by kind (characters, enemies, items, ...; map pieces hidden unless asked), with a
  3D view. *Make a project* exports the model and makes its Blender scene; the project's page opens it in
  Blender.
- **Projects**: each project's scene and mod. *Build* / *Build and install* show the build's steps as they run
  and its report afterwards: which meshes were kept or replaced, textures changed, warnings, and whether the
  result looks like your scene (with pictures where it doesn't). The 3D view switches between your mod and the
  game's model.
- **Installed mods**: Hookshot's mods folder, mods on and off, the shared size table. Hookshot reads its mods
  when it starts.

## The CLI

The same steps, with the same settings file:

```
modstudio settings --hookshot "C:\Games\OOT_Remake" --workspace "C:\Modding\projects"
modstudio models hylia
modstudio new Npc_Hylia_MN_001 --name townsman
modstudio build <workspace>\townsman --install
modstudio mods off TownsmanMod
```

And the format tools: `inspect`, `export` (`.glb` + PNG + `model.json`), `replace`, `texture`, `decompress`
(`modstudio help`). The Blender side and the project file are described in [pipeline/README.md](pipeline/README.md).

## Layout

| Path | |
|---|---|
| `crates/modstudio-formats` | the game's file formats: zstd with the game's dictionaries, SARC, BNTX, BFRES (reading and writing) |
| `crates/modstudio-core` | what the CLI and the app do: export, replace, previews, settings, projects, builds, installed mods |
| `crates/modstudio-cli` | `modstudio`, the command line |
| `app/` | the app: Tauri (`app/src-tauri`, Rust) with a Svelte page and a three.js 3D view (`app/src`) |
| `pipeline/` | the Python that runs inside Blender (scene creation, baking, the build check); built into the CLI and the app |
| `docs/` | plan and notes |

## Building

```
cargo build --release          # modstudio (target/release/modstudio)
cargo test
```

The app needs [Node.js](https://nodejs.org/) as well:

```
cd app
npm ci
npm run tauri dev              # the app, reloading the page as you edit it
npm run tauri build            # target/release/hookshot-mod-studio, and an installer
```

The Rust toolchain is pinned in `rust-toolchain.toml` (rustup picks it up). Windows and Linux (the app needs
WebKitGTK there: `libwebkit2gtk-4.1-dev`). A `cargo build` of the whole workspace builds the app with an empty page
unless `npm run build` ran first.

## Rules

- **No game files, and nothing made from them**, in the repo, issues or CI: models, textures, archives, exported
  `.png`/`.glb`/`.fbx`, `.blend` scenes, JSON dumps of models, built mods, screenshots of them. Tests use
  synthetic data; checks against a real dump are run locally and reported.
- Every Rust dependency must pass `cargo deny check` (known GPL-compatible license, crates.io, built from source).

## License

GPL-3.0-or-later (see `LICENSE`). Third-party code and references: [THIRD_PARTY.md](THIRD_PARTY.md).
