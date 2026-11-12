# Plan

A standalone app for one job: pick a model from a Hookshot install, make a Blender scene, and turn the edited scene
into a mod that Hookshot loads. No Switch Toolbox, no prebuilt DLLs: the game's formats are handled in Rust, the
editing and baking happen in the user's Blender.

## Pieces

| Piece | Today | Target |
|---|---|---|
| Read a model and its textures | Toolbox `inspect-model` / `export-model` (FBX through Autodesk's SDK) | `modstudio inspect` / `export` → `.glb` + PNG |
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

`.bfres.mc` (MeshCodec-compressed) models are left for later: 5 models in the game use them.

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
  loose points; matching LOD counts with dummy LODs; textures that grew 44% in the same format through Toolbox's writer.
