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
| Resource size table | `restbl.exe` (RESTBL Calculator, AGPL) | `restbl.rs` (the table) and `sizetable.rs` (the sizes), from the game's own table (**done**, phase 3) |
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
5. **Physics, animations, new bones** (planned, in that order: see the last sections).

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
  export drops those). 896 shapes are stored as meshlets, which Syroot doesn't read: nearly all in `Land_`
  (683 models) and `Obj_` (88), 4 cubemaps, 1 NPC equipment model, 3 mask variants. `meshlet.rs` reads them (see the meshlets section below).
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

## Phase 3: the resource size table (done)

The table (`restbl.rs`): `RESTBL`, version 1, 160-byte names; CRC-32 hash entries (path without `.zs` -> size,
sorted) and name entries for paths that share a hash (10 in the game's table). The game's table reads and writes
back byte-identical, and every model and texture has an entry.

The sizes (`sizetable.rs`), from the game's own table over all its models and textures
(`examples/restbl_probe.rs`, `restbl_fit.rs`):
- Textures: the file's size plus a constant (280 bytes for 18,942 of 20,667, 288 for 1,721).
- Models: the file's size plus memory that follows the structure: a fit over 6,464 models gives about 7,650 bytes
  per shape, 620 per mesh, 410 per shader parameter, ...; vertex and index data add nothing (0.0002 bytes per
  byte; the game uses them in place), and within groups of models of the same structure the extra moves by 0.0004
  bytes per byte of file. No rounding (sizes are multiples of 8, not of a page).

So a file the game has needs its game entry plus what it grew (models: and 1% of that to spare); a file it
doesn't have gets an estimate (textures +288; models 1.25 times a structure estimate, which is never below the
game's own value for any of its models; others 1.5 times their size). Compared with RESTBL Calculator over the
installed mods: every entry the same but the edited townsman model, 1,660,344 here against its 1,751,112 (a
general formula; the game's own value was 1,647,928). Checked in game: with this table installed, the townsman
and an edited Malon both load.

**Packs and other mods' tables (2026-11-13).** A GameBanana mod that makes Link into Malon crashed the game on
loading a save (a write to address 0 while the player actor was made, right after its
`Pack/Actor/Player_Adult.pack` was read). The table has entries for the files *inside* packs too, and the mod adds
`Player_Malon_*` parameter files (`.bgyml`) to the player's packs; its own table had entries for them, but the
shared table replaced it in game and knew only loose files. Now:
- A mod's packs are opened and their files compared with the game's same pack (`pack_files`): grown or added ones
  get entries of their own.
- Sizes for more kinds, each covering every file of its kind in the game (`examples/restbl_packs.rs`,
  `crates/modstudio-core/examples/restbl_estimates.rs`): parameter files 6500 plus 10 times their size (the game's
  need 5,056 bytes and up, up to 9.2 times the size: they are read into objects; 57,752 files), animations
  (`.anim.bfres`) 16 KB plus 1.04 times their size (unlike models, their extra grows with the file; 418 of 1,427
  were above the model formula), everything else 1.5 times their size plus 1 KB (`.asb`: up to 1.38 times plus
  1 KB). Grown parameter files: the game's entry plus 10 times the growth.
- A mod's own table (for the same game version) is merged in: its entries larger than the shared table's are
  taken over. With the Malon mod: before, 43 of the 102 entries its table changes were missing or smaller in the
  shared one; now none (and none had to be taken over).

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

Vertex colours: 154 meshes (128 on characters, e.g. child Zelda's hair) keep data in 16-bit float vertex colours
with values above 1, and 25 in signed ones. Blender holds imported vertex colours as 8-bit sRGB and its glTF export
clamps them to 0..1, so these came back changed and the mesh counted as edited. Every colour set now travels as
the custom glTF attribute `_C<n>` (named after the game's `_c<n>`) with the game's values: Blender imports it as a
float colour layer of that name, exports it exactly with `export_attributes`, and the build reads it by name
(scenes made before still build from `COLOR_n`). Checked: an unedited child Zelda keeps every mesh (her hair was
rebuilt before), and with a hair vertex moved the rebuilt hair has exactly the game's colour values, the 9 above 1
included.

## Next: physics, animations, new bones (planned)

Asked for by testers (2026-11-13): editing the physics files (`.bphcl`), custom skeletons, and animations. A mod
made here changes meshes and textures; the game's skeleton, cloth and animations stay as they are. All three are
edited in Blender, as meshes and textures are. Physics comes first (its format work is furthest along), then
animations; new bones last, because what moves a new bone in game is cloth or an animation.

### How the game moves a character (found so far)

- **Bones.** The skeleton is the model's (child Link: 309 bones). Animations set the animated bones' places
  (`examples/anim_probe.rs`: the townsman's hold the rest positions of most of his bones), so a bone moved in
  Blender is put back in game, and a new bone has no animation.
- **Cloth.** Hair, hoods, belts and capes are simulated with Havok Cloth, from
  `Phive/Cloth/<name>.Nin_Ounce_NVN2.bphcl` (171 files; the model's actor pack names which). Each simulated part (child Link has 20, adult Link 22)
  runs the same operators: a particle mesh skinned to the animated bones (`hclObjectSpaceSkinPOperator`), moved and
  simulated (`hclMoveParticlesOperator`, `hclSimulateOperator`), then bones set from the simulated triangles
  (`hclSimpleMeshBoneDeformOperator`). Those are the `Ph_` bones (178 of child Link's 309; 131 are named in his
  cloth file), and the drawn meshes are skinned to them. So reshaped hair already swings in game as long as it
  keeps its weights on the `Ph_` bones.
- **The cloth file**: a Phive header (offsets and sizes of the two parts), a Havok tagfile (`TAG0`: every object
  described by the file's own type table, arrays and pointers as item numbers, a patch list of where pointers are)
  and after it an AAMP parameter archive that lists the simulated meshes and colliders by name. The actor's packs
  hold `Phive/ClothParam/<model>.phive__ClothParam.bgyml` (`MeshList`, `CollidableList`: which colliders are on).
- **Colliders** belong to each simulated mesh: its list of collidables (a shape on a bone, at an offset from it)
  and, per particle, a mask of the colliders it meets. Child Link's hood collides with the shield and sword on his
  back through colliders on bone `Pod_A`.
- **The game takes all of this from a mod**: the GameBanana PlayableMalonTunic mod (made with other tools) ships
  its own cloth files, cloth parameters in the player's packs, and 190 animation files.

### What exists

Prototype scripts from fixing that mod (adult Malon's back hair given the gear colliders Link's hood has; in game:
not yet confirmed), outside this repository: a tagfile reader that decodes every object by its types, an editor
that grows arrays in place (later data, items and pointer patches move), an AAMP reader and writer, a BYML writer
and a pack (SARC) file replacer. Checked on the dump:
- The game's 171 cloth files rebuilt without edits: the content is identical in all of them. 121 headers give the
  tagfile's size rounded up to 16 where the editor writes the exact size (as the Malon mod's files do, which load).
- AAMP: every cloth file's archive round-trips. BYML: 1,415 of the player packs' 1,434 parameter files byte-identical,
  the same content in the rest.

**Simulation for previews (2026-11-13, branch `feat/cloth-preview`):** havok-formats and havok-cloth (Kalmarv,
GPL-3.0-or-later, written against Sekiro and Elden Ring) are vendored in `crates/` (THIRD_PARTY.md) and read SDK
2025.2 now: the version, `0xFF`-padded string tables, member records with flag `0x80` and one more value, type hashes
over a type's own members (all 21,297 stored hashes reproduce), `hkPreciseTransform`, renamed members (`bufferName`,
`shapeRadius`), `localStiffnessConstraints` (per-entry stiffness, 240 of the 1,124 local range sets), one-component
transform set flags and `batchOffset`; each crate's README lists the changes. `modstudio-formats::phive` splits a
cloth file. `examples/cloth_probe.rs`: every cloth of the 171 files runs. `examples/cloth_rest.rs`, child Link at
rest: the skeleton's reference pose is every cloth's bind pose (most hair cloths' meshes are stored relative to a
fixed frame, a translation), collidables sit exactly on their bones, and cloths not touching a collidable stay in
place with their bones on the rest pose. With gravity every part settles within a second; the motion it kept at
rest in the first tests came from leaving gravity out (nothing presses the cloth onto its collidables, and the files'
damping is 0.001 a second). Not simulated: `hclVolumeConstraint` (12 files: adult Zelda, child Ganondorf, Skull Kid,
...), `hclBonePlanesConstraintSet` (3 horse files), aerodynamics (wind).

**Cloth preview (2026-11-14, same branch):** `modstudio-core::cloth` runs a model's cloth file
(the one its actors give it: `actor_links`, below; at first the file named after the model's archive), read through
the romfs with a mod's overlay (a mod's own cloth file is the one simulated), against a test motion of the whole model
(no animation is read yet): it stands, turns 60 degrees each way, runs 1.5 m and stops, jumps 35 cm and stands again
(6.5 s for a model of child Link's size, 1.18 m; larger models move further and slower, in proportion and by the square
root, up to 4 times). The cloth first settles at rest for 2 s, unrecorded. Out come the places, frame by frame, of the
bones the cloth sets (`hclSimpleMeshBoneDeformOperator`; child Link: 108, simulated in 0.14 s).
- **In the app** (the main way), live: *Cloth physics* in the 3D view of the Models page and of a project's tabs.
  `cloth::LiveCloth` (settled at rest, then stepped wherever the caller has the model) is kept by the app
  (`cloth_open`, `cloth_step` at 1/60 s a step, `cloth_close`); the page sends where the model is for each step and
  poses the model's own skeleton (the preview `.glb`'s) with the answer: each cloth bone at its rest place moved as
  the simulation moved it, every other bone following its parent. A drag that starts on the model turns it (sideways)
  and lifts it (up; let go, it falls); a drag elsewhere moves the camera. *Test cloth animation* sends the test
  motion instead, from where the model faces, the run shown in place. On *Your mod* it runs the built model with the
  mod's cloth file, so it checks what a build wrote. Checked in the app (DevTools screenshots, synthetic drags in the
  page): child Link turned by a drag, lifted and dropped, and through the test motion, the cap swinging and
  settling; physics off puts him back at rest.
- **In Blender** before a build: `modstudio cloth <project> --open` writes `build/cloth_preview.json` and
  `pipeline/cloth_preview.py` makes the action "Cloth preview" of it on the scene's armature; the build drops any
  action and pose before it reads the scene. The posed bones are within 0.03 mm of the simulation, in a scene at the
  game's size and at 100 times it.

All 171 files run (the largest swings are a horse's tail in the turn, about 2 m, and the Deku Tree's beard). A part
that starts more than a tenth of the model's size inside a collidable is named in the notes (2 files: King Zora's
floor and ground collidables, and the `Skl_Root` and floor collidables of a townsman, `Npc_Hylia_MU_005`; they most
likely fit the models' animated poses, not the rest pose). It is havok-cloth's simulation of the game's data, not the
game's: no capture from the game has been compared yet.

How this serves the editing steps: the preview reads cloth files with the same reader the physics editing step will
write them for, and it simulates the file the build wrote. A written cloth file that the reader refuses (type hashes,
layout) or that simulates wrongly shows in the app before the game. Step 1's check list adds: every edited cloth file
read back and simulated, as the game's are.

### 1. Physics editing

In Blender: the scene gets the model's cloth (`make_scene.py`, from the cloth file) and the build reads it back
(`prep_for_game.py`), written into the mod with the model:
- **Colliders** as objects on their bones (a capsule or sphere parented to the bone, named after the collider),
  moved and scaled like anything else. Which colliders each simulated part meets: a custom property on the part (an
  empty per simulated part, named after it), e.g. the gear colliders on Malon's hair. Colliders a file doesn't
  have (child Malon's hair lacks the gear colliders child Link's hood has) become new objects in the tagfile: a
  collidable and its shape.
- **Simulation settings** of each part (gravity, damping, constraint stiffness) as custom properties on the same
  empty. Which ones the files use and what they do is to be found out file by file.
- How a change swings: *Cloth physics* on the project's *Your mod* tab runs the cloth file the build wrote (above);
  the game has the last word. The build report lists what changed in each part.

**Done (2026-11-14, branch `feat/cloth-preview`): edits in place.** `modstudio-core::cloth_edit`: `scene_of` reads a
cloth file as the scene shows it (each collider: its item, bone, place at rest and shape; each part, a sim cloth:
gravity, damping, substeps, solve iterations, scales of particle radius, friction, stretch and bend stiffness and
local range distances, and whether its particles meet each of its colliders); `apply` writes a scene's differences
into the file through havok-formats' `TagfileImage` (values patched where they are: the collidable's
`hkPreciseTransform`, each part's `collidableTransformMap` bone index and offset, the shape's fields with a capsule's
`dir` and `capLenSqrdInv` worked out, `simulationInfo`, `particleDatas`, `maxParticleRadius`, the constraint sets'
link stiffnesses and local range distances, `staticCollisionMasks`, `simulateOpConfigs`) and
`modstudio-formats::phive::with_tagfile`. Facts it rests on: havok-formats' writer gives all 171 tagfiles back byte
for byte and `with_tagfile` the whole files; colliders are shared between a model's parts (134 files), so a
collider is one object and every part using it is written; the files' shapes are capsules (1,884), spheres (211)
and planes (46); local range limits of the largest float mean none and aren't scaled. `cloth_probe --edit`: on
all 171 files an unchanged scene writes nothing, and a scene with every collider moved and resized and every
part's settings changed writes a file that reads back so (the scales checked value by value) and simulates.
Blender: `pipeline/cloth_scene.py` (the collection "Cloth": colliders as wireframe meshes on their bones, parts as
empties with custom properties; read back at rest with the scene's size taken out, an object's scale turned into
the shape's), new projects get it from `make_scene.py`, older ones with *Add cloth to the scene*
(`cloth_edit::add_to_scene`). The build writes the file only when something changed, after reading it back and
simulating it. Checked on child Link: the colliders sit on his body in the scene (renders), an unchanged scene
builds with no cloth file, and a scene with a collider moved, two resized, one moved to another bone and two parts'
settings changed builds a file with exactly those changes, which *Cloth physics* on *Your mod* runs. Not yet
checked in game.

**Done (2026-11-14): new and deleted colliders.** Facts from the 171 files: each of the 1,134 sim cloths keeps four
per-collider arrays as long as its collider list (`perInstanceCollidables`, the map's `transformIndices` and
`offsets`, `setupCollidableSidedness`, `collidablePinchingDatas`); every simulate configuration uses all colliders
(`useAllInstanceCollidables`, no `instanceCollidablesUsed`); at most 22 colliders a part (the masks hold 30 and the
landscape's bit 31); the container lists every collider the parts use, and 11 that none uses; the parameter
archive's list of colliders keys each entry `crc32("collidable_<n>")` with a `Name` and `ForReplace` off (2,130 of
2,130). So a **deleted** collider is switched off in every part (its mask bits cleared; the object stays, unused);
a collider **added to a part** gets a new run for each of the part's per-collider arrays with one more element, and
its bit on the part's free particles; a **new** collider is a copy of one of the file's collidables and of a shape of
its class (a sphere as a capsule with no length where the file has no sphere), named, placed, sized, added to the
container and to the parameter archive (`modstudio-formats::aamp`, read and written byte for byte for all 171;
`phive::with_parts` pads it to 16 as every file has it). In the scene: `cloth_parts` on each collider (which parts
have it), a copy of a collider (Shift+D) or a mesh in the colliders' collection (its shape from its size) is new,
a deleted object a deleted collider. `cloth_probe --structure`: on 163 files two new colliders, one collider added
to a part, one deleted and one taken from a part are written, read back, listed in the archive and simulated (7
files have no collider on a bone to copy, 1 nothing to copy a sphere from: said so). On child Link, through Blender
and a build: a copied head collider, a new sphere on the back for the hood, the sword collider deleted and the neck
collider given to the hood, all in the written file. Not yet checked in game.

Checked in game (2026-11-14): a new sphere collider behind adult Link's back, given to his hood, holds the cap out.
The actor pack's `ClothParam` lists the colliders too (the same names as the parameter archive); the build doesn't
change it, and the game didn't need it to.

**Done (2026-11-14): every model's cloth file, from the actor packs.** Matching names found the wrong file or none
for 17 models. `modstudio-core::actor_links` (first named `cloth_links`) reads each actor pack (`Pack/Actor/<actor>.pack.zs`, 8,373; BYML read
by `modstudio-formats::byml`): the actor's `ActorParam` names its model info (`ModelProjectName`.`FmdbName`, the
model) and its physics, whose controller set's `ClothList` names the cloth files; each file takes what it leaves out
from its `$parent`, in the same pack. The links are kept in `<workspace>/.modstudio/actor_links.json` and read
again when the packs change (reading them takes 0.5 s here). Every lookup (the app's *Cloth physics*, a project's
cloth scene, the Blender preview, the build) uses them, and falls back to a file named after the model only when
the packs give it none. From the game (`examples/actor_links.rs`): 176 models get cloth from 170 files
(`Horse_Link_Body` has no actor and no model). Against the old guess: 163 agree; 9 are new, among them the Great
Fairy (`Npc_U_GreantFairy`), Jabu-Jabu, Gibdo (ReDead's file) and Dark Link (adult Link's); 4 had the group's file
where their own exists (Kotake, the Gerudo leader, a Goron, Nabooru's Iron Knuckle in cutscenes); 4 sub-models
(Sheik's knife, Ganon's crystal, ...) had the group's cloth, which no actor gives them. 7 files are shared by two
models (adult Link and Dark Link, ReDead and Gibdo, the Poes, ...): a build that changes one says which other
model changes with it. Adult Zelda has two files (the second, a little smaller, for the actor
`Npc_Hylia_Zelda_Adult_Scalable`): the main one is edited, and the build says the other is left as it is.

**Next (put off for now):** in game, the other kinds of edit (moved, resized and deleted colliders, a collider given
to or taken from a part, part settings).

Work: the reader and editor ported to Rust (`modstudio-formats`: tagfile, AAMP, BYML writing, SARC writing), with
the round trips above as dump tests; new objects in a tagfile (items, patches, and the type table when a type isn't
in the file yet); the parameter archive and the packs kept in step. The size table already handles `.bphcl` files
and packs. Checks: every game cloth file rebuilt unchanged; in game, each kind of edit on one character.

Not in this step: a cloth whose particle mesh changes shape (a longer cape needs a new simulation mesh, its
constraints and skinning; Havok's own tools make those). It comes with new bones (step 3).

### 2. Animations

- 1,427 `.anim.bfres` files. The player's are in bundles (`Player_Child_01_Animation` ... `_04`, `_Cba`, `_Demo`),
  cutscenes in their own files (`Dm_*`).
- In Blender: the model's animations as actions on the scene's armature (a bundle holds many; the scene gets the
  ones picked), edited with Blender's tools; the build writes the changed actions back into their files. Cloth
  follows edited animations as it is: the simulated parts are skinned to the animated bones.
- Work: skeletal animations read (curves per bone: keys, interpolation, quantised values; LegacySwitchLibraries
  has the format), exported to glTF for Blender, Blender's export (sampled per frame) read back and written as
  curves. To find out first: whether animations bind bones by name or by number (new bones depend on it too). The
  size table already estimates animations. Checks: every game animation round-tripped unchanged; in game, an
  edited idle.
- The largest of the three. Other tools already write animations (the Malon mod's 190), so the format is known
  to be writable.

**Done (2026-11-14): animations read and played in the app.** `modstudio-formats::bfres_anim` reads the skeletal
animations (FSKA, ported from LegacySwitchLibraries): all 1,427 files, 22,875 animations, 3.1 million bone tracks
(`examples/anim_survey.rs`). What the game's files hold:
- Every bind index is unset (0xFFFF): animations bind to bones **by name** when played. New bones (step 3) only
  need a name an animation moves, or none.
- Every curve is "value-keyed" Hermite (flag 0x4000): frames as u8 or 10.5 fixed point (some scaled), values
  quantised to 8 or 16 bits, unsigned, times the curve's scale plus its minimum, each with a slope.
- Rotations are Euler angles XYZ, one curve per angle, except on bones whose flags have bit 20 (0x100000; 12,661
  tracks, the library doesn't know it): their base rotation is a quaternion, and one curve on the X target with flag
  0x80 holds a whole quaternion (four f32) per key, blended without slopes. The animation's rotation mode (0x1000,
  or 0x2000 where such bones occur) says no more than that.
- Checked: at frame 0 bones sit at their rest places; the clavicles of an idle, quaternion bones, read as Euler
  had tipped the whole body over; read as quaternions every bone but the fingers and arms of the idle stays within
  20 degrees of rest. Rigid bones keep their length between keys (`examples/anim_check.rs`).

`modstudio-core::actor_links` (the actor packs, as for cloth) now also reads each actor's `AnimationParam`: 580
models get 445 animation files from their actors (the player's six bundles). `anim::anim_files` adds the model's own
file and its cutscenes (`Dm_<scene>_<model>`): child Link has 78 files, 2,262 animations, listed in 0.4 s.
`anim::Clip` reads one with the model's skeleton, sampled at every frame (30 a second; a 2,320-frame cutscene is
4 MB for the app). In the app's 3D view, *Animations* lists them by name (the actors', its own, cutscenes) and
plays one in a loop with a frame slider. A clip that carries the model (a run: the root bone's travel over the
ground) is shown in place.

The cloth follows: `LiveCloth::step_posed` takes the animation's pose (bone places in the model, by name; the cloth
file's skeleton fills in bones the model lacks); the cloth settles on a clip's first frame for a second before it
plays, and a run's travel goes on in the simulation's world, loop after loop, so the cap trails behind. Child Link:
the cloth file's skeleton and the model's agree at rest to 0.00 mm; through a run and an idle the 21 chains' first
bones stay within 6.5 cm of the animation (`examples/cloth_anim.rs`).

**Done (2026-11-14): animations edited in Blender and written back.**
- `modstudio-formats::bfres_anim_write`: an animation file with some bones' curves replaced. The file stays as it is
  up to the end of its first relocation section (patched in place); new curves (value-keyed Hermite, a key at every
  frame, 16-bit values and slopes; quaternion bones' as four f32 a key), curve arrays and base values go after it,
  and the relocation table (`_RLT`: sections, then entries of pointers per structure) is written again with entries
  for the new pointers. Counted again: each bone's first curve, the animation's curves and its baked size, which in
  all 22,875 of the game's animations is curves x (4 x frames + 32) rounded up to 8 (`examples/anim_baked.rs`).
  Checked on all 1,427 files (`examples/anim_rewrite.rs`): every bone of every animation written again, every other
  curve new: each file reads back, every pointer the table lists stays inside the file (34 million), the counts are
  right, scalar curves match at every frame to 0.0015% of their range. One boss's quaternion curves have keys between
  frames (four a frame); written a key a frame, they differ between frames by up to 5 degrees.
- `modstudio-core::anim_edit`: an animation written for the scene (its animated bones' places in the model at every
  frame) and, from the scene's export, each bone's own translation, rotation and scale compared with the game's
  (rotations by the length of the quaternions' difference: an angle from the cosine of their dot product reads 0.04
  degrees for equal rotations in f32). Changed parts get new curves (Euler angles kept near the frame before's), the
  rest are kept. `examples/anim_roundtrip.rs` (no Blender): unchanged, nothing written; child Link's head turned 30
  degrees about its own x axis with its 112 descendants carried along: only the head changes, and the written file has
  it turned by 30 degrees to 0.08 degrees over all 601 frames of *Wait*.
- `pipeline/anim_scene.py`: the action made (the cloth preview's conversion), and read back in the build
  (`prep_for_game.py`, before the armature goes to rest; the meshes' armature modifiers muted while it plays). Through
  Blender on a new child Link project: *Wait* added and built unchanged gives no change; the head turned in Blender
  (every key) gives "Wait: 1 bone(s) changed: Head", and *Animations* on *Your mod* shows the head turned; a second
  animation added from the app (*Add to the scene* in the player bar) builds unchanged beside it.
- `modstudio anim <project> --list | --add`; the app's 3D view has *Add to the scene* on a project's page; the project
  keeps which animations it added (`scene_anims`); the build report lists "Animations changed".

Checked in game (2026-11-14): adult Link's three standing idles (`Wait`, `Wait_Sword`, `Wait_Sword_Shield`, in
`Player_Adult_04_Animation`) with the head turned 30 degrees in Blender: the tilt plays in game, and nothing else
changes.

**Next:** fewer keys than one a frame; animations of another length. (Bones the game's animation doesn't move:
done with new bones, step 3 (4).)

### 3. New bones (custom skeletons)

- In Blender: added in edit mode and weighted, as today; the build writes them into the skeleton instead of
  giving their weights to the nearest game parent.
- A new bone moves in game only if something drives it, hence last:
  - an **animation**: its curves added in the actions edited in step 2; in the others it keeps its place on its
    parent (to check: what the game does with a bone an animation has no curve for);
  - **cloth**: new `Ph_` bones driven by a new simulated part (a ponytail, a longer cape), on step 1. The simulated
    part is the hardest piece (particle mesh, constraints, operators): most likely an existing part of the same
    kind (a hair strand) copied and fitted to the new bones, its particle mesh a mesh in the scene.
- Moving existing bones would mean editing every animation that sets them: not planned (reshape the mesh instead).

**Found (2026-11-14), `examples/skeleton_probe.rs`, `dict_check.rs`, `rigid_probe.rs`:**
- **Bone names are strings in the file**, in all 6,464 models, the 6,406 that keep other names as IDs into the
  shared `ExternalBinaryString.bfres` too. A new bone's name is a new string in the file, its pointer listed in the
  relocation table as the others' are (the table lists null pointers too). Nothing shared changes.
- **The skeleton** (`FSKL`): the bone array (0x58 a bone: name, user data pointers, index, parent, smooth and rigid
  matrix indices, billboard index, flags, scale, rotation, translation), the matrix list (smooth matrices, then rigid
  ones), the inverse bind matrices (one per smooth matrix, 3x4), and the name dictionary lie one after another,
  before the string table; the GPU data follows on the next 4 KB. Added bones mean new arrays after the string table
  (the GPU data moved down, as `bfres_write` already moves it), the skeleton's pointers and counts patched.
- **The name dictionary** (a Patricia trie over the names' bits, counted from the end of a name) can be built:
  `modstudio-formats::res_dict`, ported from LegacySwitchLibraries, builds all 7,748 of the game's (every model's bones,
  every animation file's animations) exactly as the game has them, and looks up all 74,811 names through them.
- **Vertices index the matrix list**, smooth matrices first. New bones' smooth matrices go after the game's smooth
  ones, so the rigid ones move: in 446 models (characters' eyes; child Link's head band too) the rigidly skinned
  shapes' vertex indices past the smooth part go up by the number of new bones, and so do the bones' rigid indices.
- **Shapes list the bones they're skinned to** (with a bounding sphere each). A shape weighted to a new bone gets it
  in its list. (The build today doesn't extend a replaced shape's list when its weights move to other game bones.)
- **Limits:** the game's largest palette is Ganon's, 729 matrices (757 bones); adult Link has 281.
- **`*_Animation.Skeleton` files** (55): models named `Skeleton` without meshes, beside an animation resource of the
  same name. Most are copies of their model's skeleton, bone for bone (the player's two `_Demo` ones among them);
  the others are a family's skeleton (`Npc_Hylia_MN`, `_WF`, Gorons) whose models have more bones besides (up to 78
  more): models with bones their animations don't know are the game's own case.
- **Animations bind by name** (step 2): a bone no animation moves stays at rest on its parent.

**Plan:** (1) the model written with added bones (bone records, strings, matrix list, inverse bind matrices,
dictionary, counts, rigid indices, shapes' lists; checked by reading back and on every model with bones added);
(2) the build making a scene's new bones into bones instead of handing their weights to a parent; (3) in game, a mesh
on a new bone; (4) animations moving new bones (bone animations added to animation files, the `_Demo` skeletons
too); (5) cloth on new bones.

**Done (2026-11-14): (1) and (2).**
- `modstudio-formats::bfres_bones::with_bones`: the skeleton with bones after its own; each a smooth matrix after the
  game's smooth ones (the rigid ones, and the bones' rigid indices, move up), an inverse bind matrix from its rest
  place (the same computation reproduces all 37,781 of the game's smooth bones' to 1e-5, `examples/inverse_check.rs`),
  flags: visible, the rotation mode and bit 23 from its parent, and bits 24 to 27 from its own transform (scale
  uniform, scale volume one, no rotation, no translation: exactly so on all 51,936 game bones; bits 28 to 31 are 0
  on them, `examples/bone_flags_check.rs`).
- `bfres_write::write_edited`: with bones added, the names (strings: a u16 length, the bytes, a zero), the bone array
  (the game's records as they are, then the new ones), the matrix list, the inverse bind matrices and the dictionary
  go after the file's data (the first relocation section grows over them); the GPU data starts on the next 4 KiB
  after (its start pointer, the only pointer into it, follows; the memory pool and the table after it move as
  before); the rigidly skinned shapes' vertices get their matrix indices moved up; the relocation table is written
  again with entries for every new pointer. Shapes given a new list of skin bones get it after the data too, with a
  bounding sphere per bone. A skin index attribute that can't hold an index (a new bone's on adult Link is 279; 8-bit
  formats hold 255) is widened to the 16-bit format the game's own models use past 255 (`16_UInt`, `16_16_UInt`,
  `16_16_16_16_UInt`), its buffer packed again (each attribute on 4 bytes) and the stride patched. Checked on every
  model the writer takes (5,627; skeleton-only and meshlet models it doesn't), two bones added to each
  (`examples/bones_add.rs`): every pointer inside the file, read back with the bones, the dictionary the one `res_dict`
  builds, every name found, rigid vertices on the same bones as before (446 models), every other buffer byte for byte
  the game's, written again unchanged.
- A shape's list of skin bones is exactly the bones its vertices are weighted to, ascending, in all 2,860 of the
  game's skinned shapes (`examples/skin_list_check.rs`): a replaced skinned shape gets that list where it isn't the
  game's (with new bones, or weights moved to other bones, which the build left unlisted before).
- `replace`: the skins' joints the game's skeleton hasn't are added as bones (parent: the nearest bone above; place:
  relative to it as the scene has it; Euler angles for the game's skeletons), their weights kept; the build lists
  them ("Bones added"). Through Blender, adult Link's feather put on a new bone under `Ph_Feather_1`: the build's
  check finds the result like the scene at rest and in the test pose. (First the 8-bit indices wrapped the new bone's
  279 onto another bone; the check caught it.)

**Done (2026-11-14): (3).** In game (Longshot): adult Link's feather moved into his right hand, four times larger,
on a new bone under `Wrist_R` (its skin index 279, so the 16-bit format): it stays in his hand as he moves. The
game takes a skeleton with bones after its own, their inverse bind matrices, and the widened skin indices.

**Done (2026-11-14): (4) for the game's actor animations: tracks added to animations.**
- The game's animation files keep their tracks' names as strings in the file (all 3.1 million), and its animations
  name bones their models don't have: 204 of the 580 models given animation files by their actors get tracks for
  bones they lack (`examples/anim_names_probe.rs`), so the game skips names it doesn't find, and a file shared by two
  models (adult Link's with Dark Link) can take a bone only one has.
- A track's flag bits 24 to 27 (scale uniform, scale volume one, no rotation, no translation) come from its base
  values, each cleared where a curve moves that part: so in 99.8% of the game's tracks, the others with fewer set.
  The writer worked them out for new tracks, and edited tracks now keep only the ones that still hold (before, a bone
  that didn't turn and was made to kept "no rotation").
- `bfres_anim_write::AnimWriter::add_bones`: the animation's track array written again after the file's data (the
  game's tracks copied with their pointers listed as they were, the new ones after), its bind indices too (unset:
  bound by name), the names after the data. Checked on all 1,427 files (22,875 animations, one track added to each,
  Euler or quaternion, before or after edits to the game's tracks): every one reads back, the new track's values
  as written, the game's tracks as before (`examples/anim_rewrite.rs --add`).
- `anim_edit::apply`: a bone the scene's action moves that the animation has no track for gets one (its rotation
  kept as its nearest animated parent's is, bit 23 from it), against the built model's skeleton (with its new bones).
  Through Blender: the plume in adult Link's hand on a new bone, swung 60 degrees each way ten times over his three
  standing idles: the tracks read back as that (`examples/anim_track_check.rs`).

Checked in game (2026-11-14): the plume swings as written in adult Link's standing idles, and stays still in his hand
when he runs or jumps (animations without its track). A new bone moves in the game's animations given a track for it.

**Next:** (5) cloth on new bones; the `_Demo` skeletons (cutscene animations: whether a bone their skeleton files
don't have plays) after.

**Found for (5) (2026-11-14), `examples/cloth_parts_probe.rs`, `cloth_patterns.rs`, `pack_dump.rs`:**
- **Each simulated part is a cloth data of its own** (`hclClothData`; adult Link has 22), and all 1,134 parts of the
  171 files are built alike: one sim cloth; operators skin (object space 911, bone space 223), move the fixed
  particles, simulate, set bones from triangles (`hclSimpleMeshBoneDeformOperator`), and two vertex copies (copy or
  gather); states `Default` [0, 1, 2, 3], `AnimToSimCurrent` [0, 4], `AnimToSimPrev` [0, 5]; buffers scratch, sim
  current, sim previous; one transform set, the part's own skeleton (`cloth_skeleton_<part>`: the bones it uses and
  their parents up to `Root`, 7 to 56 bones; matched to the model's by name: only `Root` has the model's index).
- **Adult Link's feather**, the smallest: 3 particles (2 fixed: skinned to `Head_2`; the free one to the two
  feather bones), 1 triangle, two links, a stretch link, a range keeping the free particle within 8 cm of where its
  skinning puts it, a transition set; both feather bones set from the one triangle; 3 colliders; no virtual
  collision points. The hood string: 5 particles, 3 triangles, bend links and bend stiffness too.
- **A bone set from a triangle** (`havok-cloth::deform`): the triangle's frame (its centroid; its first two corners
  less the centroid; their cross product) times the bone's stored matrix, made orthonormal. So the stored matrix of a
  new bone is the inverse of its triangle's frame at rest times the bone's rest place. The pair's offsets are bytes:
  64 a bone, 6 a triangle.
- **The lists**: each part is an object of the file's parameter archive (`Name`, its base bone, axes, a material
  preset: `burlap`, `silk`, `hair`, `old_leather`) and of the actor pack's `ClothParam` `MeshList` (`BaseBone`,
  `Preset`, `ParamCoefDict` of stiffness and radius factors, `KeepBoneLength`, twist settings).

**Plan for (5):**
- In Blender: new bones named `Ph_...` (the game's own prefix for bones physics moves) hanging in a chain from a bone
  that isn't one (the chain's anchor: a game bone, or a new one that isn't simulated). Each chain becomes a part.
- The part: a strip of particles two wide down the chain (at each joint, one on the chain and one beside it, across
  the bone), the top two fixed and carried by the anchor; each bone set from the strip's triangle at its head; links
  along and across the strip, a stretch link down the chain, a range around where its rest place would carry each
  particle; the settings (gravity, damping, mass, radius, solver) of the file's own smallest part. No colliders at
  first.
- Written as a new cloth data of the model's cloth file: a copy of its smallest part with every array made for the
  strip, its skeleton the chain and its parents up to `Root`; listed in the parameter archive (and in `ClothParam`
  if the game needs it).
- Checked: the file reads back and simulates (the new bones swing and settle in *Cloth physics*), then in game.

**Done (2026-11-14): new parts.** `modstudio-core::cloth_new`:
- `add_parts` copies the file's smallest part whose skin is in object space (its cloth data, sim cloth, buffers,
  transform set, operators, states, constraint sets and skeleton, through `TagfileImage::references_of`; colliders
  shared, not copied) and makes every array of the copy for the strip, as the plan says; the copy is listed in the
  cloth container, its skeleton in the animation container, its entry in the parameter archive (the template's, with
  its name and base bone).
- What the game's data needed that a first try didn't have: no link between two fixed particles (a link moves each
  end by the stored stiffness times that particle's inverse mass, `havok-cloth::constraints`, not divided by the
  pair's sum: the game's files store the share of the error a pass corrects divided by the pair's inverse masses, 0.05
  on the feather's links for a half; a link between two fixed particles weighs nothing); and only the layouts a file
  spells out (a type's members are in a file's type table only where the file stores one): two-bone skin blocks with
  all the weight on the anchor where the file has no one-bone ones (27 files), range entries with a stiffness of
  their own where it has no plain ones (11 files).
- `examples/cloth_new_check.rs`: on every cloth file, a chain of two new bones hanging from its model's highest bone
  gets a part: 162 of the 171 read back, settle within 5% of the chain's length of the skeleton's places (they sag a
  little), swing in the test motion and come back. Not done: 8 files whose parts all skin in bone space (no template;
  said so), 1 file no model has.
- `chains_of`: bones the build added whose names start with `Ph_` (the game's own prefix) or with *Simulate as
  cloth* ticked (`pipeline/scene_tools.py`, run when the app opens a scene: a button and tick box in the Bone tab; the
  tick is the custom property `modstudio_cloth`, as Blender 5 keeps properties a script registers apart from custom
  properties, out of reach of the build and of a scene opened without the script), each chain from one whose
  parent isn't one, on down the only such child (more than one: the first, and a note); joints the bones' heads and
  the last one's tip (`bone_tips.json`, which `prep_for_game.py` writes beside the export); the strip across the
  model's x axis, a quarter of the first bone wide. The build adds the parts to the model's cloth file (the one it
  just wrote, else the game's), simulates it, and lists them ("new part ...").
- Through Blender: the feather four times larger hung from adult Link's right hand on a new bone `Ph_Plume` (0.62 m):
  the part settles hanging straight down (the bone was modelled 22 degrees off), swings up to 30 degrees in the test
  motion (its range: half its length) and comes back (`examples/cloth_swing.rs`).
- Checked in game (2026-11-14): the plume swings from adult Link's hand. The game runs a part from the cloth file
  alone: the actor pack's `ClothParam` doesn't list it, and didn't need to.

**Done (2026-11-14): colliders for new parts, bone-space templates, cutscene skeletons.**
- A new part meets the file's colliders within reach of its chain (from its anchor: the chain's length and a
  particle's radius; capsules and spheres on a bone), the nearest first, at most 22 (the most a game's part has), but
  those a free particle of the strip starts inside of (its anchor's own, mostly: they would throw it off at the first
  step): its collider list, its map's bones and offsets (each collider's place relative to its bone), its free
  particles' masks, its per-collider pinch data and sidedness (an entry of another part's, or zeros), the colliders'
  bones in its skeleton and in what its simulation reads. The build lists them ("meets ..."). The plume from adult
  Link's hand: 35 within reach, the nearest 22 (his right arm, leg and waist, the gear on his back).
- A template whose skin is in bone space (8 files have no other: Big Octo's, ...): each particle's place relative to
  the anchor, its weight in the fourth value, blocks of 16, 8, 5 or 4 vertices for one to four bones (as the file
  spells out). `cloth_new_check`: 170 of the 171 files take a new part (the other has no model).
- **Cutscene skeletons.** Adult and child Link's shared event animations (`Player_*_Demo_Animation`: drinking from a
  bottle, chests, items got, the ocarina) have a skeleton file beside them (`.Skeleton.bfres`), the model's skeleton
  bone for bone; the per-cutscene files (`Dm_*_Player_Adult`, 90 of them) have none. Skeleton files are laid out as
  animation files are (only the first relocation section used, no GPU data): `bfres_write::write_skeleton` writes one
  with bones added (the skeleton half of `write_edited`, now `append_skeleton`); `bones_add`: all 58 such files of the
  game's take two bones, read back, find every name, write again unchanged. The build adds the bones to every
  skeleton file beside an animation file the model's actors give it that is its skeleton bone for bone (a warning
  for one that isn't: its animations won't move the new bones), so it stays so. Through Blender: the plume (a plain
  new bone) waving in 79 of adult Link's ocarina animations: tracks in all, the skeleton file with the bone.

Checked in game (2026-11-14): the plume on a plain new bone swings in adult Link's ocarina animations and hangs still
otherwise. Holding the ocarina plays 721-frame waits (`Ocarina_Adult_Wait`, `_OpenEye_Wait`, a wait per note: the
actor's animation sequence, `AS/Player_Adult.root.asb`, names them); a first test without them showed nothing. With
the cloth plume (its skeleton file changed too), taking out the ocarina plays as before. The colliders were in the
cloth test; whether the plume bumps off the leg wasn't looked at closely.

**Done (2026-11-14), after 0.4.0:**
- **Untouched meshes kept.** The round trip through Blender changed four of the player's meshes a little: Blender
  averages the custom normals of a vertex the game splits on a hard edge (adult Link's `Hood_B_low`, child Link's
  `Angin_Knot_Head_Band` and `Boots_Cover`), and positions on bones whose bind place isn't quite their rest place move
  by about 0.1 mm (`Shirt_Inner_Body_low2`). So `make_scene.py` stamps each mesh with a fingerprint of its geometry as
  Blender holds it (`scene_nodes.mesh_fingerprint`: vertices, faces, corner normals, UVs, attributes, weights,
  materials, shape keys, modifiers, its place), `prep_for_game.py` lists the meshes whose fingerprint is unchanged
  (after joining new meshes, before its own changes) in `unchanged_meshes.json`, and the build keeps those as the game
  has them. Unedited adult and child Link: all 25 meshes kept, the model byte for byte the game's; with the feather
  edited, it alone is replaced. Projects made before have no fingerprints: the comparison as before.
- **New parts' settings from the scene**: Swing, Stiffness, Gravity and Colliders on the chain's top bone
  (`scene_tools.py`'s *Cloth settings*; `bones.json`; `cloth_new::PartSettings`). Stiffness adds links across each
  joint down both sides; every segment now has both diagonals, so it can't twist about the chain (with one, a lower
  segment flipped over in the test motion). A two-bone plume, stiffness 0.6 against 0 (swing 1): its middle bends
  up to 108 degrees and ends 12 off, against 180 and 74. Colliders by name, gravity and swing as set, read back.

**Done (2026-11-14): animations of any length (your own in place of the game's).** The game picks animations by
name through each actor's animation sequence (`AS/<actor>.root.asb`; adult Link's is 2.2 MB), so a new name would
need new states there; instead an action replaces one of the game's animations, at its own length.
- `anim_scene.action_length`: an action's length is its frame range (manual where set, else its keys; Blender's
  `frame_range` doesn't widen a one-key range); any action given `game_anim_file` and `game_anim_name` stands for
  that animation.
- `AnimWriter::set_frame_count`: the animation's frame count, its new curves written that long, its baked size
  counted for it. `anim_edit::apply`: a scene action of another length gives the animation that length and writes
  every bone the scene has (base values, curves where they move).
- Each animation file has an event file (`AnimationEvent/Animation/<file>.anim.baev`, 459 of them: sounds, effects,
  foot IK at given frames); it isn't changed, and the build warns that its events stay at their frames.
- Through Blender: adult Link's `Wait` (601 frames) made a new animation of 121 (every key removed, the head turning
  40 degrees each way, everything else at rest): written 121 long, all 295 tracks, the head's turn and the arm's
  rest read back. Checked in game (2026-11-14): standing still, adult Link plays the new idle (rest pose, the head
  turning), looping at its own length.

**Done (2026-11-14): meshlet shapes read (export and preview).** 896 shapes in 779 model files (683 map pieces,
88 objects, 4 cubemaps, three NPC mask variants, one NPC equipment model) keep their geometry as meshlets: no meshes, an empty
vertex buffer, and one GPU buffer of clusters of up to 64 vertices and 124 triangles, each packed at the precision it
needs. No outside description of the format is known; it was worked out from the files (`meshlet.rs` has the
layout).
- The buffer: a header (the shape's box minimum, a position step of 0.0005, flags for tangents, two colour sets and
  two more UV sets, the meshlets per LOD), a record per meshlet (40 bytes, 8 more per extra UV or colour set), then
  each meshlet's data: its vertices bit-packed (least significant bit first), then 3 indices a triangle, each just
  wide enough for the vertex count.
- A vertex: x, y, z offsets from the meshlet's base (on the step grid, widths in the record); the normal, octahedral,
  6 to 12 bits a component; with tangents, the bitangent sign and the tangent (octahedral, 8+8); each colour set's
  varying channels (8-bit, or f32 where values leave 0..1; constant channels are in the record); each UV set's
  offsets from its base, in steps of 2^(e-16) (e from 4 to 7 per component, in the record).
- What the record also holds and the reader doesn't need: twice the meshlet's extent per axis (for culling), a normal
  cone (axis and cutoff; most meshlets have none), 2 when there are tangents.
- Checked on all 896 shapes (`examples/meshlet_decode.rs`, 535,636 meshlets, every LOD): every vertex bit accounted
  for, indices in range, each meshlet's data right after the last, offsets from 0 up to the extent the record gives,
  normals with the faces (mean cosine 0.85), the tangent sign with the UVs (99.3%; the rest degenerate), vertices two
  meshlets share with the same UVs whether or not their UV scales differ. Exported through the glTF writer (shared
  vertices made one): the Keaton mask, Deku Tree cave rocks and a Gerudo Valley cliff rendered in Blender with their
  textures where they belong.

**Next:** writing meshlet shapes (a build that replaces a map piece or object); new animation names (the sequence
files) later.

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
