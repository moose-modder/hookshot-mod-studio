# Plan

A standalone app for one job: pick a model from a Hookshot install, make a Blender scene, and turn the edited scene
into a mod that Hookshot loads. No Switch Toolbox, no prebuilt DLLs: the game's formats are handled in Rust, the
editing and baking happen in the user's Blender.

## Pieces

| Piece | Today | Target |
|---|---|---|
| Read a model and its textures | Toolbox `inspect-model` / `export-model` (FBX through Autodesk's SDK) | `modstudio inspect` / `export` → `.glb` + PNG (**done**, phase 1; the pipeline doesn't use it yet) |
| Scene creation, baking, export from Blender | `pipeline/*.py` in Blender | same (glTF instead of FBX) |
| Rebuild the model and textures | Toolbox `replace-model` | `modstudio replace` |
| Resource size table | `restbl.exe` (RESTBL Calculator, AGPL) | own implementation, or Hookshot adjusts the table for mod files itself |
| GUI | none | Tauri: Rust core, web UI, three.js previews from the `.glb` files |

## Phases

1. **Reader**: zstd with the game's dictionaries (`romfs/Pack/ZsDic.pack.zs`, a SARC), SARC, BNTX (block-linear
   layout, BC1/4/5/7 and ASTC decoding), BFRES (skeleton, meshes, vertex attributes in their formats, LODs). Output
   `.glb` + PNG for Blender and previews. Replaces Toolbox's export and its Autodesk FBX SDK dependency.
2. **Writer**: edit in place. Keep the original file, replace vertex and index buffers, mesh headers, bounds and
   textures; keep attribute formats, skin counts and LOD counts as the game expects (see `pipeline/README.md`).
   Texture encoding: BC1/BC4/BC5 written here; BC7 to decide (a source-built crate, or BC3 instead).
3. **RESTBL** or the Hookshot-side alternative.
4. **App**: model list (search, categories; Land hidden), previews, projects, mods on/off.

`.bfres.mc` (MeshCodec-compressed) models are left for later: 5 files in the game use it, and those checked so far
(Impa) also exist as `.bfres.zs`, which is what is read.

## Phase 1 findings (the reader, checked against the whole dump)

- **The files are Switch 2 files.** BNTX platform `Ounc`, shaders `Nin_Ounce_NVN2`. Textures use the Orin GPU's
  block-linear layout: same blocks and mip sizes as the Switch, a different byte order inside each GOB
  (`tegra.rs`). Every texture's mip sizes match the offsets in its file (21,658 files).
- **Textures:** 14,000 of 21,658 are ASTC (4x4 to 12x12), the rest BC1/3/4/5/6/7 and a few RG8. All 21,446 image
  textures decode; the 212 that don't are data (R16 level maps, RGBA16F vertex animation). Against Toolbox's PNGs
  for the Great Fairy and Impa: 92 of 101 identical, the other 9 within 1/255 (rounding).
  **For the writer:** a texture that changes must be encoded as ASTC, or written as BC7/BC1 instead, if the game
  takes a different format in place of ASTC (to try in game before choosing).
- **Models:** all 6,464 parse (version 10.4). 6,406 store names as IDs into `Shader/ExternalBinaryString.bfres`
  (454 strings, all resolve). Up to 10 LODs per mesh, up to 8 influences per vertex (`_i1`/`_w1` beyond 4; Toolbox's
  export drops those). 896 shapes are stored as meshlets, which neither Syroot nor this reader reads: nearly all
  in `Land_` (683 models) and `Obj_` (88), 1 NPC, 3 mask variants; they are left out of exports and listed.
- **Format codes** for vertex attributes and textures are the graphics API's `channels << 8 | type`. Read that way,
  the "16_SNorm" bone indices Syroot names (0x030a) are plain 16-bit unsigned integers (`AttribFormat`).
- **Export checked in Blender** against Toolbox's FBX, mesh by mesh (vertex and face counts, bounds, rest and
  deformed positions, UVs, colours, weights, materials): Great Fairy 15 of 16 identical (the eyelash keeps its 5th
  influence), the Shadow Temple paddle ship identical for every mesh compared. Two deliberate differences: meshes
  that follow one bone (skin count 0) are weighted to that bone (Toolbox used the first bone of the matrix list),
  and vertex colours are written so Blender holds the game's bytes.
- Local checks: `crates/modstudio-formats/tests/romfs.rs` (`MODSTUDIO_ROMFS=... cargo test --release
  -p modstudio-formats --test romfs -- --ignored --nocapture`).

## Risks and how they are contained

- **The game's BFRES version is still being worked out upstream** (v10.2+, meshlets, external strings for this
  game; LegacySwitchLibraries commits from October and November 2026). Port from that repo's source and follow its
  commits.
- **Writing is harder than reading**: wrong offsets or alignment crash the game or draw garbage without an error.
  Edit in place, and require byte-identical output for unchanged models.
- **Only the game proves a writer right.** Local checks before any in-game run: round trips of unchanged models
  across all character models, comparisons with Toolbox's output on the same input (Toolbox stays on the developer's
  PC as a reference, it is never shipped), and the render comparison the pipeline was verified with.
- **Known details** that must survive (found while building the pipeline): attribute format 0x030a holds 16-bit
  integer bone indices (Syroot names it 16_SNorm); colour sets that start at `_c1`; lower LODs' vertices exported as
  loose points; matching LOD counts with dummy LODs; textures that grew 44% in the same format through Toolbox's writer;
  meshes with 5 to 8 influences per vertex (the pipeline limits weights to 4 by default).
