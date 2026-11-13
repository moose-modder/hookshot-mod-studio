# Plan

A standalone app for one job: pick a model from a Hookshot install, make a Blender scene, and turn the edited scene
into a mod that Hookshot loads. No Switch Toolbox, no prebuilt DLLs: the game's formats are handled in Rust, the
editing and baking happen in the user's Blender.

## Pieces

| Piece | Today | Target |
|---|---|---|
| Read a model and its textures | Toolbox `inspect-model` / `export-model` (FBX through Autodesk's SDK) | `modstudio inspect` / `export` → `.glb` + PNG (**done**, phase 1) |
| Scene creation, baking, export from Blender | `pipeline/*.py` in Blender | scenes made from the `.glb`, glTF export for the build (**done**) |
| Rebuild the model and textures | Toolbox `replace-model` | `modstudio replace` (**done**) |
| Run the steps (new project, build, install) | Python scripts (`extract_model.py`, `build_mod.py`, `restbl_all.py`) | `modstudio-core` (`project`, `build`, `mods`), used by the CLI and the app (**done**; the Python scripts are gone) |
| Resource size table | `restbl.exe` (RESTBL Calculator, AGPL) | own implementation, or Hookshot adjusts the table for mod files itself |
| GUI | none | Tauri: Rust core, web UI, three.js previews (**first version done**, phase 4) |

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
BC5/BC4/BC7). The build uses it, so the pipeline no longer needs Toolbox.

Checked locally: the unedited Great Fairy round trip matches the game's model as well as the Toolbox build did
(better for 5-influence meshes); the edited Great Fairy scene builds, and matches the Toolbox build of the same
scene mesh by mesh; textures come out closer to their PNGs than Toolbox's. **Not yet checked in the game.**

**The game needs the zstd frame to state its decompressed size.** The first in-game test (the townsman,
`Npc_Hylia_MN_001`) showed the model opened (Hookshot's `mods:` line) and nothing more: its textures were never
asked for, its characters never appeared, and nothing in the log said why. ruzstd's encoder leaves the size out
of the frame header (libzstd's `ZSTD_getFrameContentSize` says "unknown") and adds a checksum; every file the game
ships, Toolbox's and restbl's have the size and no checksum. `zs::compress` now rewrites the header that way.

Checked in game since: the townsman (`Npc_Hylia_MN_001`, a new goatee joined into the hair, a lip edit baked
into the face) loads and shows the edit.

## Phase 2b: more kinds of edits (done)

- **Build check** (`check_result.py`): the built model rendered as the scene is (base colour, four sides, rest
  and a test pose) and compared; differences reported by mesh and view, with images.
- **Unedited meshes kept** as the game has them, LODs and tangents included (triangle-by-triangle comparison).
- **Lower LODs** of edited meshes decimated in the game's own proportions.
- **Material changes** of any kind noticed (a fingerprint stamped by `make_scene.py`), and baked.
- **Colour versions** of clothes textures (`_Alb.0` ... `.13`) all baked.
- **New meshes** joined into a game mesh: the one with their material, else the nearest with texture room.
- **Bones**: weights on new bones go to their nearest game parent; moved bones are reported (the animations set
  bone positions, so the skeleton is kept).

Left: more bone influences than a mesh's skin count (new vertex attributes; the shader may not have the
variant), real new meshes, materials or bones in the file (writing the whole file, names in the shared string
table), meshlet shapes, the resource size table (phase 3).

## Phase 4: the app (first version)

The steps the Python scripts ran (a new project, the build, install, one size table for all mods) are in
`modstudio-core` now, so the CLI (`modstudio new`, `build`, `mods`, `settings`, `models`) and the app share them
and nobody needs Python outside Blender. Blender's scripts are built in and written out before a run. Checked
locally: the townsman built this way is byte-identical to the Python build (model, five textures, size table), and
an install plus turning mods off and on gives the same shared table.

The app (`app/`, Tauri 2, Svelte 5, three.js):
- **Settings**: the Hookshot folder (romfs and mods), the projects folder, Blender (found if installed in the usual
  places), `restbl`; each checked (Blender's version, the game's version from its own size table).
- **Models**: the game's models by kind, searchable (map pieces hidden unless asked), mods and projects marked; a
  3D view (`preview`: one `.glb` with the base colour textures in it, cut-outs alpha-masked; about 0.1 s for a
  townsman) and the model's meshes; *Make a project*.
- **Projects**: open the scene in Blender, *Build* / *Build and install* with the steps shown as they run (and
  Cancel), the last build's report (meshes kept or replaced, textures, warnings, the build check with its
  pictures), the 3D view of the mod or the game's model.
- **Installed mods**: on and off (the folder renamed with '-'), the shared size table made again.

Checked by driving the page through WebView2's DevTools port (`MODSTUDIO_WEBVIEW_ARGS`; settings pointed elsewhere
with `MODSTUDIO_SETTINGS`): a new project from the model list, a build with install into a scratch mods folder, the
check's result, mods off and on. Not yet: an installer people download (`npm run tauri build` makes one locally;
releases need a CI job), thumbnails in the model list, friendlier model names.

## Materials: shader parameters and eyes

Materials are read with their shader parameters now (`bfres::ShaderAssign`, from LegacySwitchLibraries'
`MaterialParserV10.cs`; all models still parse and write back unchanged). Characters' materials use generic
slots (`ns_param_scalar0..7`, `ns_param_color0..5`, `ns_tex_mtx0..5`); `examples/material_params.rs` lists them
for a model, or one material's across every model.

Each material gets a `look` in `model.json` (`look.rs`): how to draw it with one base colour, which the app's 3D
view and the Blender scene follow. The colour comes from the texture on the colour sampler (`_a0`), not from
texture names: eye shadows (about 75 materials) bind a texture named `..._Alb` to a mask sampler (`_gn3`), and
drawn as colour it covered the eyes in red.

Eyes (`Mt_Eye_Inner`, `eyes.rs`): iris `_a0`, sclera `_a1`, iris mask `_gn3`. The iris and its mask are sampled
at the eye's UVs scaled about the centre by `ns_param_scalar3` (larger: a smaller iris; 0.55 for Gorons and
horses up to 3.3 for Kotake; the cornea material has the same value in its `ns_param_scalar4`), the sclera at the
eye's own UVs. Worked out from the textures and values, not from the shader's code, and checked by rendering:
with the mask in the eye's own UV space instead, the iris covers the whole visible eye. The cornea
(`Mt_Eye_Outer`) and eye shadows are left out of the base colour (see-through).

Render settings are read too (`RenderInfo`: `gsys_render_state_mode` opaque / mask / translucent,
`gsys_alpha_test_*`, `gsys_render_state_display_face`), and decide alpha (`model.json`'s `alpha`): opaque
materials keep data in their colour texture's alpha (the Zoras' skin: alpha 0 over 64% of the texture), so
treating alpha as opacity made their bodies disappear. In Blender the textures are loaded "channel packed" (no
premultiplying, which drew that colour black), and the build carries such alpha over as data when it re-bakes
faces (checked: an edited patch on Ruto's skin keeps the original's alpha values).

Tints (`model.json`'s `tint`): a grey colour texture, or one the game doesn't have, takes the material's
`ns_param_color0` (linear RGB). Across the characters, 122 grey textures have a colour there and they are each
character's colours (shared eyebrow and eyelash textures: Zelda's eyebrows blond, Impa's silver); 30 materials
name a colour texture the game doesn't have (child Link's hair: the tint is the colour); coloured textures with
a tint (748, e.g. Zelda's hair) are left alone. `examples/colour_survey.rs` lists them. A re-baked texture the
game tints gets a warning (the bake's colour would be tinted again).

Found on the way: 154 meshes (128 on characters, e.g. child Zelda's hair) keep data in 16-bit float vertex colours
with values above 1, and 25 in signed ones. Blender holds imported vertex colours as 8-bit sRGB, so these come
back rounded and clamped: the build counts such a mesh as edited and writes the changed values. To fix: carry
those colour sets through Blender as float attributes.

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
