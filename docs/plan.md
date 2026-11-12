# Plan

A standalone app for one job: pick a model from a Hookshot install, make a Blender scene, and turn the edited scene
into a mod that Hookshot loads. No Switch Toolbox, no prebuilt DLLs: the game's formats are handled in Rust, the
editing and baking happen in the user's Blender.

## Pieces

| Piece | Today | Target |
|---|---|---|
| Read a model and its textures | Toolbox `inspect-model` / `export-model` (FBX through Autodesk's SDK) | `modstudio inspect` / `export` → `.glb` + PNG (**done**, phase 1; `extract_model.py` uses it) |
| Scene creation, baking, export from Blender | `pipeline/*.py` in Blender | scenes made from the `.glb` (**done**); the build still exports FBX for Toolbox, glTF once `modstudio replace` exists |
| Rebuild the model and textures | Toolbox `replace-model` | `modstudio replace` (**done**; `build_mod.py` uses it) |
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
  for the Great Fairy and Impa: 100 of 101 identical (BC1-3 are decoded the way DirectXTex does, which Toolbox
  uses). The other is one ASTC block of Impa's cloth normal map where Toolbox's decoder gives an impossible normal
  (red 254 with green 0) and ours a plausible one.
  **For the writer:** a changed texture doesn't have to be ASTC. Toolbox can't encode ASTC and writes BC7 (BC5 for
  `_Nrm`) instead; the Great Fairy mod built that way has its skin normal map as BC5 where the game has ASTC 5x5,
  and it shows right in game.
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
- **Scenes from the export** (`make_scene.py --glb`) build through Toolbox like scenes from its FBX. Round trip
  of the unedited Great Fairy (extract, build, read the result back): same faces, bounds, UVs, colours and weights
  as the game's model, as good as the Toolbox-made scene or better (5th influences kept). Two lessons:
  - Weights must stay on exact 1/255 steps: Toolbox's writer truncates each weight and gives the remainder to an
    empty slot on matrix 0, so 162.99999/255 became a 4th influence. The export keeps the game's bytes exactly.
  - Toolbox's FBX import splits a few more vertices (21 of 6,544 on one mesh) from the glTF-made scene than from
    its own: differences below 1e-5 in stored normals. Nothing visible; our writer replaces that step.
- Local checks: `crates/modstudio-formats/tests/romfs.rs` (`MODSTUDIO_ROMFS=... cargo test --release
  -p modstudio-formats --test romfs -- --ignored --nocapture`).

## Phase 2: the writer (`bfres_write.rs`), status

Done: the model file written back with new geometry for chosen shapes. Everything before the GPU data stays in
place (fields patched); the GPU block is laid out again; the memory pool block and the relocation table move
with its end. Checked on the whole dump: all 5,627 models with ordinary meshes write back byte-identical, and
written with every shape's own decoded geometry, nothing changes but the recomputed bounds.

Rules taken from the files (all checked against the dump):
- Index buffers in shape and LOD order, each on 256 bytes; then vertex buffers, each on its buffer alignment;
  the GPU block padded to 4 KiB. Relocation sections: 0 the CPU part, 1 index buffers, 2 vertex buffers, then the
  memory pool block and the table itself (pointers into those move with them).
- Each LOD has its own vertices in the shape's vertex buffer (`first_vertex`).
- Bounding boxes per LOD: each sub-mesh, then the whole mesh, around every vertex in the local space of the bone
  of each skin slot, empty slots included (they name matrix 0). Matches the game's LOD 0 boxes exactly; some lower
  LODs in the game are looser than their geometry.
- Spheres: one per skin bone, the smallest sphere around the vertices weighted to it (all LODs, its local space),
  radius + 0.0001; shapes without skin bones have one sphere (Syroot's "one per mesh" reads past the array).

Also done: `modstudio replace` reads Blender's glTF export (each mesh replaces the game shape of its name;
skinning back to the shape's matrices and skin count, rigid and unskinned vertices back to their bone's space;
every LOD gets the new mesh), and writes changed textures (BC1/BC3/BC4/BC5/BC7 encoders, BNTX writer; ASTC as
BC5/BC4/BC7). `build_mod.py` uses it, so the pipeline no longer needs Toolbox.

Checked locally: the unedited Great Fairy round trip matches the game's model as well as the Toolbox build did
(better for 5-influence meshes); the edited Great Fairy scene builds, and matches the Toolbox build of the same
scene mesh by mesh; textures come out closer to their PNGs than Toolbox's. **Not yet checked in the game.**

**The game needs the zstd frame to state its decompressed size.** The first in-game test (the townsman,
`Npc_Hylia_MN_001`) showed the model opened (Hookshot's `mods:` line) and nothing more: its textures were never
asked for, its characters never appeared, and nothing in the log said why. ruzstd's encoder leaves the size out
of the frame header (libzstd's `ZSTD_getFrameContentSize` says "unknown") and adds a checksum; every file the game
ships, Toolbox's and restbl's have the size and no checksum. `zs::compress` now rewrites the header that way.

Left: simplified LODs (every LOD is the full mesh now), new meshes and bones (in-place writing can't add them),
meshlet shapes, the resource size table (phase 3).

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
