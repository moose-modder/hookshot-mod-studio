# Model mod pipeline

The current pipeline: Python scripts that drive Blender, `modstudio` (this repo's Rust tool) and, for building
the mod, a Switch Toolbox build with command-line commands. The Toolbox steps (`inspect-model`, `replace-model`)
are being replaced by `modstudio` too (see `docs/plan.md`); the Blender scripts stay.

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

1. **Reads the game's model** (`Toolbox.exe inspect-model`): meshes, materials, attributes and textures,
   and exports the original textures as PNG into `work_dir\original_textures`.
2. **Prepares the scene** (Blender in the background, `prep_for_game.py`):
   - meshes named `<name>_WORK` are used for `<name>` (an old `<name>` export copy is ignored)
   - **puts the scene's look into the game's textures** (see *Materials and textures* below)
   - one material per mesh, only the UV maps and colour layers the game mesh has, at most 4 bone
     weights per vertex (Toolbox's importer keeps 4 anyway), mesh data named like the object
   - exports `work_dir\model.fbx` (scale 0.01, as Toolbox needs) and `work_dir\textures\*.png`
3. **Replaces the model** (`Toolbox.exe replace-model`): copies the game's model and the textures being
   replaced into `mod_dir\romfs`, imports the FBX with *map original materials*, *keep original skin
   count*, *original attribute formats*, no bone import, and LOD counts like the original (dummy LODs).
   Textures keep their format; ASTC (which Toolbox can't encode) becomes BC7 (BC5 for `_Nrm`).
4. **Checks the result** against the game's model: every mesh keeps its material, skin count and LOD count.
5. **RESTBL** (`restbl.exe` from the RESTBL Calculator): writes
   `romfs\System\Resource\ResourceSizeTable.Product.<version>.Nin_Ounce_NVN2.rsizetable.zs`.
6. With `--install`: copies `mod_dir\romfs` into `install_dir`, the mod's own folder: for Hookshot
   `<game folder>\mods\ImpaMod`, for an emulator `...\load\0400486024DEC000\ImpaMod`. (Not the `mods` folder
   itself: a `romfs` directly in it is not read. The script refuses a folder named `mods` or `load`.)
   The mod's RESTBL is left out: the game reads **one** table, and with several mods Hookshot takes the one of
   the mod that sorts last, so the others' grown files would fail to load (textures show as a flat default).
   Instead `restbl_all.py` makes one table over every enabled mod in the mods folder and installs it as the mod
   `zzz-ResourceSizeTable`, which sorts last. **Run `python restbl_all.py` yourself after adding, removing or
   turning off a mod** (it reads `config.json`).

Logs of every step are in `work_dir` (`1_inspect_game.log` ... `5_restbl.log`, `prep_report.json`).

Options: `--skip-blender` reuses the last FBX and textures (e.g. to only change Toolbox options).

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
- Changes made inside a game material itself (e.g. new nodes in `Mt_Skin`) are not baked, only noted in the
  log: put them on a new material.
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
| `prep.grow` | rings of faces baked around the changed ones (default 1) |
| `prep.color_threshold`, `normal_threshold`, `opacity_threshold` | how different a face must look to be baked (defaults 3/255, 0.02, 3/255) |
| `prep.samples`, `bake_margin`, `probe_size`, `pack_grid`, `pack_margin` | bake samples (4), bake margin in pixels (3), scratch layout size (2048), packing grid (512), space between islands (0.006) |
| `toolbox_options` | extra `replace-model` options, e.g. `["--no-match-lods"]` |

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

- Meshes are matched to the game's by name (`<mesh>__<material>`, as Toolbox exports them). A new mesh
  name gets a warning: Toolbox can't know its material.
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
