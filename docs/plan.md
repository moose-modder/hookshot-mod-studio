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
  `Phive/Cloth/<model>.Nin_Ounce_NVN2.bphcl` (171 files). Each simulated part (child Link has 20, adult Link 22)
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
- Blender can't run Havok Cloth: how a change swings shows in game only. The build report lists what changed in
  each part.

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
- Work: the skeleton written with added bones (bone list, smooth and rigid matrix lists, inverse matrices, the
  meshes' skin indices). The writer edits the file's front part in place today; it would lay it out again. To find
  out first: where bone names live and how a new name is added (names in 6,406 models are IDs into the shared
  `ExternalBinaryString.bfres`, phase 1), and what the 55 `*_Animation.Skeleton.bfres` files are for.

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
