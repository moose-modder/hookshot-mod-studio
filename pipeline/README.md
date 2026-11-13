# The Blender side of the build

The scripts here run inside the user's Blender (5.x). The app and `modstudio` (the CLI) run them; both carry them
built in and write them to `<workspace>/.modstudio/pipeline` before a run. To try a change to a script without
rebuilding, set `MODSTUDIO_PIPELINE` to this folder.

| Script | |
|---|---|
| `make_scene.py` | a new project's Blender scene, from the model's `.glb` export and its textures |
| `prep_for_game.py` | a build's Blender step: bakes the scene's look into the game's textures, exports glTF |
| `check_result.py` | the build check: renders the scene and the built model the same way and compares them |
| `scene_nodes.py` | shared by the others: the RG normal node group, material fingerprints |

**Save the .blend in Blender before building.** The build reads the file from disk and never changes it.

## Starting a new model

In the app: **Models**, pick one, **Make a project**. Or:

```
modstudio new Zelda_Adult --name zelda
```

The model can be a path in the romfs (`Model/Zelda_Adult.Zelda_Adult.bfres.zs`) or part of its name (several
matches are listed). It makes `<workspace>\zelda\`:

1. The model as `.glb` (`modstudio export`: skeleton, the most detailed LOD of each mesh, skin weights, UVs,
   vertex colours), every texture its materials use as `.png` (shared `CmnTex_*` ones too) and `model.json`
   (meshes, materials, skin counts, vertex attributes, texture formats). Textures the game doesn't ship (some
   materials name ones that don't exist) are listed.
2. `zelda.blend` (`make_scene.py`): the model imported, the armature named `Armature`, each material made from
   its textures as `model.json`'s `look` says: the texture on the colour sampler (`_a0`) to colour, and to
   alpha unless the game draws the material opaque (`alpha`; the Zoras' skin keeps other data in alpha); a grey
   texture times the material's colour (`tint`, a "Tint" node: shared eyebrow and eyelash textures get each
   character's colour), or the tint alone where the game has no colour texture (child Link's hair); for eyes the
   game draws with its eye shader, the iris over the sclera at the material's iris scale (a Mapping node
   labelled "Iris scale"); the eye's overlays (cornea highlights, eye shadows) see-through. Textures are loaded
   "Channel Packed" so colour stays as it is where alpha is 0. `_Nrm` goes through the RG normal group so the
   viewport shows it right, the others are image nodes to paint on.
3. `project.json`: the project (mod `ZeldaMod`, installed to `<mods>\ZeldaMod`). Merge rules and other settings
   go here (*Project file* below).

An existing project is never overwritten, nor an existing `.blend`.

## Building

In the app: the project's **Build** or **Build and install**. Or:

```
modstudio build <workspace>\zelda --install
```

1. **Reads the game's model** (`modstudio export`): `model.json` and the original textures as PNG, into
   `build\original` (with every colour version of a clothes texture, see below).
2. **Prepares the scene** (Blender in the background, `prep_for_game.py`):
   - meshes named `<name>_WORK` are used for `<name>` (an old `<name>` export copy is ignored)
   - **puts the scene's look into the game's textures** (see *Materials and textures* below)
   - one material per mesh, only the UV maps and colour layers the game mesh has, at most 8 bone weights per
     vertex, faces with more than 3 corners triangulated (Blender exports tangents for triangles only)
   - exports `build\model.glb` and `build\textures\*.png`
3. **Builds the mod** (`modstudio-core`'s replace): the game's model with each scene mesh in place of the game
   mesh of its name, in that mesh's vertex formats, skin count (the strongest influences) and LOD count (the lower
   LODs are the mesh decimated in the proportions of the game's own, see `prep.lods`); a game mesh missing from
   the scene is removed. A scene mesh that is still the game's own (the same triangles with the same positions,
   UVs, colours and skinning, normals within 1 degree) is **kept** as the game has it, its LODs and tangents
   included. Each changed texture is written in its own format, or for ASTC (not encoded here) as BC5 (normal
   maps), BC4 (one channel) or BC7. Files of an earlier build are removed first. The mod is `mod\romfs`.
4. **Checks the result** against the game's model: every mesh keeps its material, skin count and LOD count.
5. **Compares the result with the scene** (`check_result.py`, Blender): the built model is read back (with the
   mod's textures) and rendered the way the scene is: base colour and opacity, from the front, back and both
   sides, in the rest pose and in a test pose (every game bone turned a little, so skinning shows). Where the
   two differ, the build says which meshes and in which views, and leaves images in `build\check`
   (`differs_<view>_<pose>.png`: scene | result | differences in red; the app shows them). An edit the build
   lost, a mesh that didn't come along or a vertex skinned to the wrong bone shows up here. Lighting, normal
   maps, roughness and other maps are not compared, nor are distance versions (LODs). `--no-check` skips it.
6. **Resource size table** (`restbl` from RESTBL Calculator) for the mod on its own:
   `romfs\System\Resource\ResourceSizeTable.Product.<version>.Nin_Ounce_NVN2.rsizetable.zs`.
7. **Install** (`--install`, the app's *Build and install*): replaces the `romfs` of `<mods>\<name>` with the
   mod's, without its size table. The game reads **one** table, and with several mods Hookshot takes the one of
   the mod that sorts last, so the others' grown files would fail to load (textures show as a flat default).
   Instead one table is made over every enabled mod and installed as the mod `zzz-ResourceSizeTable`, which
   sorts last. Turning a mod on or off in the app's **Installed mods** (or `modstudio mods on|off <folder>`)
   makes it again; `modstudio mods table` does it by hand. A mod that was turned off is turned on by installing.

Logs of every step are in `build` (`2_blender.log`, `5_check_scene.log`, `6_restbl.log`, ...), with
`prep_report.json`, `check\check_report.json` and the build's report, `build_report.json`.

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
     where the texture has alpha; for a material the game draws opaque, whose alpha is data, the original's
     alpha sampled where the face was; a texture the game tints gets a warning, as the baked colour would be
     tinted again), `_Nrm` the tangent-space normals, the rest (AO, roughness, thickness, ...)
     the original texture sampled where the face was. Where the colour changed (e.g. leaves turned into skin),
     those maps instead take the values the original texture has where its colour is similar (skin's AO and
     roughness, not the leaves' that were there), blending with the sampled ones as the change fades. Merge-rule
     materials with `source_uv` settings keep their own sampling. Metallic and specular settings don't darken
     the colour.
- **Painted textures.** If a scene image of one of those textures was painted on (it differs from the game's),
  the painted version is used. Pack or save the image so the `.blend` has it.
- **Changes inside a game material** (new nodes in `Mt_Face`, e.g. a mask that recolours part of the texture,
  or a changed value, link or image) are baked the same way: the material counts as a custom one, and the faces
  that look different are moved and baked. `make_scene.py` stamps each material with a fingerprint of its nodes
  so the build sees any change; in scenes made before that, only nodes the import doesn't make are noticed. A
  change that doesn't show in colour, normals or opacity (e.g. roughness) is reported: it doesn't reach the game.
- **Colour versions.** Some shared townsfolk models have their clothes texture in up to 14 colours
  (`..._Alb.0`, `.1`, ...; the game picks one per character). The scene shows version 0; the build decodes the
  others and bakes the moved faces into every one: for each version the scene's images of version 0 show that
  version while baking, so a material that mixes in the original colour (stripes over the shirt) comes out in
  each character's colour. A painted texture changes version 0 only.
- Only this model's meshes are kept clear of. A texture file another model also uses (unusual: they're named
  after the model) isn't checked.

The log lists per mesh how many faces were baked, the new layout (`resolution_vs_original`: texel size of the
baked faces relative to the rest; a warning below 0.5) and per texture the share of pixels changed.
`prep_report.json` has the details.

## Project file

`project.json`, in the project's folder. Paths are relative to that folder (absolute ones work too, so project
files of the older Python pipeline load as they are; their `tools`, `game_romfs` and `toolbox_options` are
ignored: those are settings now).

| Key | |
|---|---|
| `name` | the mod's name: its folder in Hookshot's mods folder |
| `model` | model to replace, relative to the game's romfs |
| `blend` | Blender scene |
| `work_dir` | optional: intermediate files and logs (default `build`) |
| `mod_dir` | optional: the mod; files go to `mod_dir\romfs\...` (default `mod`) |
| `install_dir` | optional: where it's installed (default `<mods>\<name>`) |
| `prep.armature` | armature object name |
| `prep.max_weights` | bone weights per vertex (at least 8 are kept) |
| `prep.merge` | optional list of rules, see below |
| `prep.lods` | `"decimate"` (default): an edited mesh's lower LODs are made from it with Blender's Decimate, in the proportions of the game's own LODs (`prep.lod_symmetry`, default true; `prep.lod_weld`, a weld distance first, default off); `"full"`: every LOD gets the full mesh. Meshes kept as the game's keep the game's LODs either way |
| `prep.join` | optional `{"<new mesh>": "<game mesh>"}`: where a mesh the game model doesn't have goes (default: the game mesh with its material) |
| `prep.grow` | rings of faces baked around the changed ones (default 1) |
| `prep.color_threshold`, `normal_threshold`, `opacity_threshold` | how different a face must look to be baked (defaults 3/255, 0.02, 3/255) |
| `prep.samples`, `bake_margin`, `probe_size`, `pack_grid`, `pack_margin` | bake samples (4), bake margin in pixels (3), scratch layout size (2048), packing grid (512), space between islands (0.006) |
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

## Settings

One file shared by the app and the CLI: `%APPDATA%\HookshotModStudio\settings.json` on Windows,
`~/.config/hookshot-mod-studio/settings.json` elsewhere (`MODSTUDIO_SETTINGS` or the CLI's `--settings` name
another). The app's **Settings** page writes it; `modstudio settings --hookshot <Hookshot folder> --restbl
<restbl>` too.

| Key | |
|---|---|
| `game_romfs` | the game's files, `<Hookshot>\romfs`. Only read. |
| `install_mods` | Hookshot's mods folder, `<Hookshot>\mods` |
| `workspace` | where projects are kept, one folder each |
| `tools.blender`, `tools.restbl` | Blender 5 and RESTBL Calculator's `restbl` |
| `game_version` | optional, e.g. `"111"`; else read from the name of the game's own size table |

## Notes

- Meshes are matched to the game's by name (`<mesh>__<material>`, as the export names them). The model is
  edited in place, so a **new mesh** is joined into a game mesh, with that mesh's UV maps and colour layers
  (vertex colours it lacks are copied from the nearest vertices: game meshes keep shading data there):
  - on a game material (a beard on the hair material): the game mesh with that material (the nearest of
    several);
  - on a new material (a hat, a bracelet): among the game meshes near it with textures of their own, the
    nearest whose textures have room to bake it at half their resolution or better (else the one with the most
    room), a skinned one if the new mesh bends with several bones. Its faces are always baked into that mesh's
    textures; the log names the choice and the others it weighed.
  To choose yourself: `"prep": {"join": {"<new mesh>": "<game mesh>"}}` in the project file. The new mesh
  keeps its bone weights (up to the target's skin count), and is drawn with the target's material.
- A mesh may use any number of materials: the game gets one, its own, with the others baked into its textures.
- **Bones.** The game's skeleton is kept as it is:
  - A **new bone** (e.g. for an antenna): the game has no animation for it, so in game it would only follow its
    parent. Weights on it go to its nearest parent the game has, and the log says so; it looks the same.
  - A **moved bone** (in edit mode): the game's animations set each animated bone's own position (the
    townsman's animations hold the rest positions of most of his bones, `examples/anim_probe.rs`), so it would
    be put back. The mesh stays as modelled and bends around the bone's place in the game; the log lists moved
    bones. Reshape the mesh instead.
- Normal maps are baked in Blender's tangent space (+Y), the same convention the scene's
  "Reconstruct RG Normal" node group reads them with.
- Vertex colours: each of a mesh's colour sets is a float colour layer named after it, `_C0`, `_C1`, ... with the
  game's values as they are (game meshes keep data there: hair shading, values above 1 or below 0). Paint those
  layers; a new mesh joined into a game mesh gets them from the nearest vertices. Scenes made before these layers
  have 8-bit layers ("Color"), which still build.
- "the game mesh has vertex colours, this mesh has none" is fine for a mesh you haven't painted: white is written.
- `make_scene.py --fbx` and `prep_for_game.py`'s `fbx_name` are the older Switch Toolbox route; the build no
  longer uses them.
