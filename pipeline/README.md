# Model mod pipeline

Python scripts that drive Blender and `modstudio` (this repo's Rust tool) to turn a game model into a Blender
scene and the edited scene into a mod. Switch Toolbox is no longer needed; without `tools.modstudio` the scripts
still use a Toolbox build with command-line commands, as they did first. The resource size table still comes
from `restbl.exe` (see `docs/plan.md`).

Builds a model replacement mod (Ocarina of Time, Switch 2) from a Blender scene:

```
cd pipeline
copy config.example.json config.json      (then fill in the paths)
python extract_model.py Npc_U_GreatFairy --name greatfairy
python build_mod.py greatfairy.json --install
```

**Save the .blend in Blender first.** The pipeline reads the file from disk and never changes it.

## Starting a new model

```
python extract_model.py Zelda_Adult --name zelda
```

The model can be a `.bfres.zs` file, a path inside the game's romfs, or part of its name (several matches are
listed). Paths come from `config.json`. It makes:

1. `<workspace>\zelda\` (`modstudio export`): the model as `.glb` (skeleton, the most detailed LOD of each mesh,
   skin weights, UVs, vertex colours), every texture its materials use as `.png` (shared `CmnTex_*` ones too) and
   `model.json` (meshes, materials, skin counts, vertex attributes, texture formats). A texture the game doesn't
   ship (some materials name ones that don't exist) is listed in `1_export.log`.
2. `<workspace>\zelda\zelda.blend`: the model imported, the armature named `Armature`, each material made from
   its textures (`_Alb` to colour and alpha, `_Nrm` through the RG normal group so the viewport shows it right,
   the others as image nodes to paint on).
3. `pipeline\zelda.json`: the project file for `build_mod.py` (mod `ZeldaMod`, installed to the game's
   `mods\ZeldaMod`). Add merge rules there if you give a mesh extra materials.

The .blend and the project file are never overwritten; an existing export needs `--force` (edited PNGs in that
folder would be replaced). `--no-blend` only exports. Without `tools.modstudio` in `config.json`, Toolbox's
`export-model` makes an `.fbx` instead, as before (scenes made either way build the same).

What it does:

1. **Reads the game's model** (`modstudio export`): meshes, materials, attributes and textures (`model.json`),
   and the original textures as PNG, into `work_dir\original`.
2. **Prepares the scene** (Blender in the background, `prep_for_game.py`):
   - meshes named `<name>_WORK` are used for `<name>` (an old `<name>` export copy is ignored)
   - **puts the scene's look into the game's textures** (see *Materials and textures* below)
   - one material per mesh, only the UV maps and colour layers the game mesh has, at most 8 bone weights per
     vertex, faces with more than 3 corners triangulated (Blender exports tangents for triangles only)
   - exports `work_dir\model.glb` and `work_dir\textures\*.png`
3. **Builds the mod** (`modstudio replace`): the game's model with each scene mesh in place of the game mesh of
   its name, in that mesh's vertex formats, skin count (the strongest influences) and LOD count (the lower LODs
   are the mesh decimated in the proportions of the game's own, see `prep.lods`); a game mesh missing from the
   scene is removed. A scene mesh that is still the game's own (the
   same triangles with the same positions, UVs, colours and skinning, normals within 1 degree) is **kept** as the
   game has it, its LODs and tangents included; the log says `kept`. Each changed texture is written in its own
   format, or for ASTC (not encoded here) as BC5 (normal maps), BC4 (one channel) or BC7. Files of an earlier
   build are removed first.
4. **Checks the result** against the game's model: every mesh keeps its material, skin count and LOD count.
5. **Compares the result with the scene** (`check_result.py`, Blender): the built model is read back (with the
   mod's textures) and rendered the way the scene is: base colour and opacity, from the front, back and both
   sides, in the rest pose and in a test pose (every game bone turned a little, so skinning shows). Where the
   two differ, the build says which meshes and in which views, and leaves images in `work_dir\check`
   (`differs_<view>_<pose>.png`: scene | result | differences in red). An edit the build lost, a mesh that
   didn't come along or a vertex skinned to the wrong bone shows up here. Lighting, normal maps, roughness and
   other maps are not compared, nor are distance versions (LODs).
6. **RESTBL** (`restbl.exe` from the RESTBL Calculator): writes
   `romfs\System\Resource\ResourceSizeTable.Product.<version>.Nin_Ounce_NVN2.rsizetable.zs`.
7. With `--install`: replaces the `romfs` in `install_dir` (the mod's own folder) with `mod_dir\romfs`: for Hookshot
   `<game folder>\mods\ImpaMod`, for an emulator `...\load\0400486024DEC000\ImpaMod`. (Not the `mods` folder
   itself: a `romfs` directly in it is not read. The script refuses a folder named `mods` or `load`.)
   The mod's RESTBL is left out: the game reads **one** table, and with several mods Hookshot takes the one of
   the mod that sorts last, so the others' grown files would fail to load (textures show as a flat default).
   Instead `restbl_all.py` makes one table over every enabled mod in the mods folder and installs it as the mod
   `zzz-ResourceSizeTable`, which sorts last. **Run `python restbl_all.py` yourself after adding, removing or
   turning off a mod** (it reads `config.json`).

Logs of every step are in `work_dir` (`1_inspect_game.log` ... `6_restbl.log`, `prep_report.json`,
`check\check_report.json`).

Options: `--skip-blender` reuses the last export and textures; `--no-check` skips step 5.

Without `tools.modstudio`, steps 1, 3 and 4 use Switch Toolbox (`inspect-model`, `replace-model`) and step 2 exports
`model.fbx` at scale 0.01 with 4 weights per vertex, as before.

## Materials and textures

The game draws each mesh with its own material and that material's textures; Blender materials don't come
along. So the build bakes what the scene shows into the model's own textures (the ones named after the model,
e.g. `Mt_Skin_Npc_U_GreatFairy_Alb`; shared `CmnTex_*` and detail textures are never changed). Nothing needs
configuring for this:

- **Custom materials.** Give the faces you change their own material (anything not named like the mesh's game
  material; a `.001` copy counts as the game material). Shader nodes, extra UV maps, attributes and masks all
  work: it's baked as Blender renders it.
  1. *Which faces change*: the faces are baked twice into a scratch layout, with their material and with the
     game material: base colour, normals and opacity. Faces that differ, plus one ring of faces around them
     (`grow`), are baked; the rest keep their original UVs. (A material that blends a change in with a mask only
     costs texture space where the mask is.)
  2. *New UV layout*: those faces get a new unwrap packed into the free space of the texture, around the islands
     of **every** mesh whose material uses the same textures (e.g. body, finger nails and leaves on the Great
     Fairy's skin textures). Stacked and mirrored UVs are handled. The original islands stay byte-identical.
  3. *Bake*, for each texture of the game material: `_Alb` gets the base colour (alpha: the material's opacity,
     where the texture has alpha), `_Nrm` the tangent-space normals, the rest (AO, roughness, thickness, ...)
     the original texture sampled where the face was. Where the colour changed (e.g. leaves turned into skin),
     those maps instead take the values the original texture has where its colour is similar (skin's AO and
     roughness, not the leaves' that were there), blending with the sampled ones as the change fades. Merge-rule
     materials with `source_uv` settings keep their own sampling. Metallic and specular settings don't darken
     the colour.
- **Painted textures.** If a scene image of one of those textures was painted on (it differs from the game's),
  the painted version is used. Pack or save the image so the `.blend` has it.
- **Changes inside a game material** (new nodes in `Mt_Face`, e.g. a mask that recolours part of the texture)
  are baked the same way: the material counts as a custom one, and the faces that look different are moved and
  baked. (Changed settings of the import's own nodes, without new nodes, aren't noticed.)
- **Colour versions.** Some shared townsfolk models have their clothes texture in up to 14 colours
  (`..._Alb.0`, `.1`, ...; the game picks one per character). The scene shows version 0. Baking into such a
  texture is not supported yet (only version 0 would get the moved faces): the build stops with an error. A
  painted texture changes version 0 only.
- Only this model's meshes are kept clear of. A texture file another model also uses (unusual: they're named
  after the model) isn't checked.

The log lists per mesh how many faces were baked, the new layout (`resolution_vs_original`: texel size of the
baked faces relative to the rest; a warning below 0.5) and per texture the share of pixels changed.
`prep_report.json` has the details.

## Project file

`extract_model.py` writes one (`<name>.json`). Project files hold personal paths and are not committed.
Paths can use `/`.

| Key | |
|---|---|
| `game_romfs` | romfs folder of the game dump. Never written to. |
| `model` | model to replace, relative to `game_romfs` |
| `blend` | Blender scene |
| `work_dir` | intermediate files and logs |
| `mod_dir` | the mod; files go to `mod_dir\romfs\...` |
| `install_dir` | optional, for `--install`: the mod's folder, e.g. `<game folder>/mods/ImpaMod` |
| `game_version` | optional, e.g. `"111"`; else read from the game's RESTBL file name |
| `tools` | paths to Blender, `modstudio` (`target/release/modstudio.exe` after `cargo build --release`), Toolbox (a build with the command line commands) and `restbl.exe` |
| `prep.armature` | armature object name |
| `prep.max_weights` | bone weights per vertex (default 4) |
| `prep.merge` | optional list of rules, see below |
| `prep.lods` | `"decimate"` (default): an edited mesh's lower LODs are made from it with Blender's Decimate, in the proportions of the game's own LODs (`prep.lod_symmetry`, default true; `prep.lod_weld`, a weld distance first, default off); `"full"`: every LOD gets the full mesh. Meshes kept as the game's keep the game's LODs either way |
| `prep.join` | optional `{"<new mesh>": "<game mesh>"}`: where a mesh the game model doesn't have goes (default: the game mesh with its material) |
| `prep.grow` | rings of faces baked around the changed ones (default 1) |
| `prep.color_threshold`, `normal_threshold`, `opacity_threshold` | how different a face must look to be baked (defaults 3/255, 0.02, 3/255) |
| `prep.samples`, `bake_margin`, `probe_size`, `pack_grid`, `pack_margin` | bake samples (4), bake margin in pixels (3), scratch layout size (2048), packing grid (512), space between islands (0.006) |
| `toolbox_options` | extra `replace-model` options, e.g. `["--no-match-lods"]` |
| `check` | optional settings of step 5: `color_tolerance` (out of 255, default 24), `min_pixels` (smallest area reported, default 80), `pose_degrees` (test pose, default 12), `resolution` (default 1024) |

A merge rule is only needed to override the automatic choice for one mesh: all faces of its extra materials are
baked (no detection, unless `"detect": true`), with per-material settings:

```json
{
  "mesh": "Skin_Body_W__Mt_Skin",
  "into_material": "Mt_Skin",
  "materials": {
    "Mt_Skin_BareArms_Test": { "source_uv": "BareArmSourceUV", "source_weight": "BareArmSourceWeight" }
  }
}
```

- `into_material`: game material the faces end up on (default: the game mesh's material).
- `materials`: optional per extra material. Textures other than `_Alb`/`_Nrm` (AO, Gn2, Thc, ...) can't be
  baked from Blender materials; they are filled by sampling the original texture, by default through the
  faces' own UVs (where they were in the texture). `source_uv` names another UV map to sample through (for new
  geometry whose UVs don't point anywhere useful), blended by the `source_weight` attribute (0 = the original's
  typical value, 1 = sampled); `"source": "median"` uses the typical value.
- Optional `textures` (list of texture names to bake; default: the model's own textures of the material),
  `roles` (`{"<texture>": "color" | "normal" | "aux"}`), `uv` (game UV map; default: the first) and `grow`.

## Notes

- Meshes are matched to the game's by name (`<mesh>__<material>`, as Toolbox exports them). The model is
  edited in place, so a **new mesh** (e.g. a beard on the hair material) is joined into the game mesh that uses
  its material, with that mesh's UV maps and colour layers (vertex colours it lacks are copied from the nearest
  vertices: game meshes keep shading data there). If its material is on several game meshes or is a new one,
  name the mesh in the project file: `"prep": {"join": {"<new mesh>": "<game mesh>"}}`.
- A mesh may use any number of materials: the game gets one, its own, with the others baked into its textures.
- Normal maps are baked in Blender's tangent space (+Y), the same convention the scene's
  "Reconstruct RG Normal" node group reads them with.
- "the game mesh has vertex colours, this mesh has none" is fine for a mesh you haven't painted: the export leaves
  out colour sets that are all white, and Toolbox writes white back.
- Until `modstudio` replaces it, `tools.toolbox` must be a Switch Toolbox build with the headless commands and two
  fixes for this game: bone indices stored as 16-bit integers (format 0x030a, which Toolbox's library calls
  16_SNorm), and colour sets that start at `_c1`.
- Toolbox commands can also be run by hand, see `Toolbox.exe replace-model` without arguments.
  Toolbox is a Windows app: from PowerShell use `Toolbox.exe ... | Out-Host` so it waits for the output.
