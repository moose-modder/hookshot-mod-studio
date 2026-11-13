"""Blender (background): a new scene from a model exported by `modstudio export`, saved as a .blend to edit.
Run for a new project (the app, or `modstudio new`).

    blender --background --factory-startup --python make_scene.py -- --glb <model.glb> --blend <out.blend>
    blender --background --factory-startup --python make_scene.py -- --fbx <model.fbx> --blend <out.blend>

From `modstudio export` (--glb; model.json and the textures beside it):
- the armature is named "Armature" (the project files' default); bone display shapes the importer adds are removed
- every material is made from model.json: its "look" gives the base colour (the texture on the colour sampler,
  `_a0`, to Base Color, and to Alpha unless the material is opaque in the game ("alpha"); times its "tint" for
  grey textures, or the tint alone where the game has no colour texture; for the game's eye shader the iris
  over the sclera, scaled by the material's iris scale; eye overlays such as the cornea and eye shadows
  see-through), the _Nrm texture through the
  "Reconstruct RG Normal" group (the game's normal maps hold only red and green; the group works out blue), the
  other textures as unconnected image nodes to paint on. (The importer's own materials would multiply the colour
  by the vertex colours, as glTF means it, which the game doesn't.)

From Switch Toolbox's export (--fbx):
- the FBX is imported with Blender's defaults (the armature turned 90 degrees on X, scale 1)
- vertices no face uses (the lower LODs' vertices, which that export includes as loose points) are removed
- the RG group is put between each _Nrm texture and its Normal Map node

An existing .blend is never overwritten. (The FBX route is the older Switch Toolbox pipeline's; the build no longer
uses it.)
"""
import argparse
import json
import os
import re
import sys

import bpy

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from scene_nodes import FINGERPRINT, RG_GROUP, material_fingerprint, rg_normal_group  # noqa: E402

ARMATURE = "Armature"


def log(msg):
    print("[scene] " + msg, flush=True)


# "..._Alb", and "..._Alb.0" for one of a texture's colour versions (the clothes of shared townsfolk models have
# up to 14; the model uses version 0 and the game picks another per character). Same rule as prep_for_game.py.
KIND = re.compile(r"_([A-Za-z]+\d?)(?:\.\d+)?$")


def texture_role(name):
    m = KIND.search(name)
    return {"Alb": "color", "Nrm": "normal"}.get(m.group(1) if m else "", "aux")


# ---------------------------------------------------------------- from modstudio export (.glb)

def tidy_glb_import():
    """Names the armature "Armature" and removes the bone display shapes the glTF importer adds."""
    arms = [o for o in bpy.context.scene.objects if o.type == "ARMATURE"]
    if len(arms) != 1:
        raise RuntimeError(f"expected one armature, found {[o.name for o in arms]}")
    arm = arms[0]
    shapes = set()
    for pb in arm.pose.bones:
        if pb.custom_shape:
            shapes.add(pb.custom_shape)
            pb.custom_shape = None
    for o in shapes:
        bpy.data.objects.remove(o, do_unlink=True)
    arm.name = ARMATURE
    arm.data.name = ARMATURE
    return arm, len(shapes)


def build_materials(info, tex_dir):
    """Each of the model's materials, made from its textures. Returns (materials made, textures missing)."""
    materials = info["models"][0]["materials"]
    names = {m["name"] for m in materials}
    # The importer may make variants (".001") of a material: every slot gets the material of the plain name.
    for o in bpy.context.scene.objects:
        if o.type != "MESH":
            continue
        for slot in o.material_slots:
            if slot.material and slot.material.name not in names:
                plain = re.sub(r"\.\d{3}$", "", slot.material.name)
                if plain in names and bpy.data.materials.get(plain):
                    slot.material = bpy.data.materials[plain]
    missing = []

    def image(name):
        path = os.path.join(tex_dir, name + ".png")
        if not os.path.exists(path):
            missing.append(name)
            return None
        img = bpy.data.images.get(name + ".png") or bpy.data.images.load(path)
        img.colorspace_settings.name = "sRGB" if texture_role(name) == "color" else "Non-Color"
        # The game's textures keep data in alpha (the Zoras' skin): colour and alpha stay apart, or Blender's
        # premultiplied alpha draws the colour black where alpha is 0.
        img.alpha_mode = "CHANNEL_PACKED"
        return img

    for m in materials:
        mat = bpy.data.materials.get(m["name"]) or bpy.data.materials.new(m["name"])
        mat.use_nodes = True
        mat.surface_render_method = "DITHERED"
        nt = mat.node_tree
        nt.nodes.clear()
        out = nt.nodes.new("ShaderNodeOutputMaterial")
        out.location = (400, 0)
        bsdf = nt.nodes.new("ShaderNodeBsdfPrincipled")
        nt.links.new(bsdf.outputs["BSDF"], out.inputs["Surface"])
        samplers = m.get("samplers", [])
        # How the material looks with one base colour (model.json's "look", worked out from the material's
        # samplers and shader parameters by modstudio). Without it (an older export): the first _Alb texture.
        look = m.get("look") or {}
        kind = look.get("kind")
        nodes = {}
        linked = set()
        for i, tex in enumerate(m["textures"]):
            img = image(tex)
            if img is None:
                continue
            node = nt.nodes.new("ShaderNodeTexImage")
            node.image = img
            node.label = f"{samplers[i]}: {tex}" if i < len(samplers) else tex
            node.location = (-700, 300 - 280 * i)
            nodes.setdefault(tex, node)
            role = texture_role(tex)
            if role in linked or role == "aux":
                continue
            if role == "color":
                if kind is None:
                    linked.add(role)
                    nt.links.new(node.outputs["Color"], bsdf.inputs["Base Color"])
                    nt.links.new(node.outputs["Alpha"], bsdf.inputs["Alpha"])
            else:
                linked.add(role)
                grp = nt.nodes.new("ShaderNodeGroup")
                grp.node_tree = rg_normal_group()
                grp.label = RG_GROUP
                grp.location = (-400, node.location.y)
                nm = nt.nodes.new("ShaderNodeNormalMap")
                nm.location = (-200, node.location.y)
                nt.links.new(node.outputs["Color"], grp.inputs[0])
                nt.links.new(grp.outputs[0], nm.inputs["Color"])
                nt.links.new(nm.outputs["Normal"], bsdf.inputs["Normal"])
        tint = m.get("tint")
        # Opaque materials keep other data in the colour texture's alpha (the Zoras' skin): not opacity.
        alpha_is_opacity = (m.get("alpha") or {}).get("mode") != "opaque"
        if kind == "color" and look.get("texture") in nodes:
            node = nodes[look["texture"]]
            color = node.outputs["Color"]
            if tint:
                color = tinted(nt, color, tint)
            nt.links.new(color, bsdf.inputs["Base Color"])
            if alpha_is_opacity:
                nt.links.new(node.outputs["Alpha"], bsdf.inputs["Alpha"])
        elif kind in ("color", "plain") and tint:
            # no colour texture in the game (child Link's hair): the tint is the colour
            bsdf.inputs["Base Color"].default_value = (*tint, 1.0)
        elif kind == "eye":
            eye_nodes(nt, bsdf, nodes, look)
        elif kind == "overlay":
            # drawn over the eye in the game (cornea highlights, eye shadow): see-through here
            bsdf.inputs["Alpha"].default_value = 0.0
    return len(materials), sorted(set(missing))


def tinted(nt, color, tint):
    """`color` multiplied by the material's tint (linear RGB): the game colours grey textures this way (shared
    eyebrow and eyelash textures get each character's colour)."""
    mix = nt.nodes.new("ShaderNodeMix")
    mix.data_type = "RGBA"
    mix.blend_type = "MULTIPLY"
    mix.label = "Tint"
    mix.location = (-350, 350)
    mix.inputs["Factor"].default_value = 1.0
    a = next(i for i in mix.inputs if i.name == "A" and i.type == "RGBA")
    b = next(i for i in mix.inputs if i.name == "B" and i.type == "RGBA")
    b.default_value = (*tint, 1.0)
    nt.links.new(color, a)
    return next(o for o in mix.outputs if o.type == "RGBA")


def eye_nodes(nt, bsdf, nodes, look):
    """The game's eye shader: the sclera at the eye's UVs, the iris and its mask at the UVs scaled about the
    centre by the iris scale, the iris over the sclera where the mask is white (modstudio-core eyes.rs)."""
    iris, sclera, mask = (nodes.get(look[k]) for k in ("iris", "sclera", "mask"))
    if not (iris and sclera and mask):
        if iris:
            nt.links.new(iris.outputs["Color"], bsdf.inputs["Base Color"])
        return
    s = float(look["irisScale"])
    uv = nt.nodes.new("ShaderNodeTexCoord")
    uv.location = (-1300, 0)
    mapping = nt.nodes.new("ShaderNodeMapping")
    mapping.label = f"Iris scale {s:g}"
    mapping.location = (-1050, 0)
    mapping.inputs["Location"].default_value = (0.5 * (1 - s), 0.5 * (1 - s), 0.0)
    mapping.inputs["Scale"].default_value = (s, s, 1.0)
    nt.links.new(uv.outputs["UV"], mapping.inputs["Vector"])
    for node in (iris, mask):
        node.extension = "EXTEND"
        nt.links.new(mapping.outputs["Vector"], node.inputs["Vector"])
    mix = nt.nodes.new("ShaderNodeMix")
    mix.data_type = "RGBA"
    mix.label = "Iris over sclera"
    mix.location = (-350, 200)
    a = next(i for i in mix.inputs if i.name == "A" and i.type == "RGBA")
    b = next(i for i in mix.inputs if i.name == "B" and i.type == "RGBA")
    result = next(o for o in mix.outputs if o.type == "RGBA")
    nt.links.new(mask.outputs["Color"], mix.inputs["Factor"])
    nt.links.new(sclera.outputs["Color"], a)
    nt.links.new(iris.outputs["Color"], b)
    nt.links.new(result, bsdf.inputs["Base Color"])


# ---------------------------------------------------------------- from Switch Toolbox's export (.fbx)

def fix_normal_maps():
    """Puts the RG group between each _Nrm image and its Normal Map node. Returns the materials changed."""
    changed = []
    for mat in bpy.data.materials:
        if not mat.use_nodes:
            continue
        tree = mat.node_tree
        for nm in [x for x in tree.nodes if x.bl_idname == "ShaderNodeNormalMap"]:
            color = nm.inputs["Color"]
            if not color.is_linked:
                continue
            src = color.links[0].from_node
            if src.bl_idname != "ShaderNodeTexImage" or not src.image or "_nrm" not in src.image.name.lower():
                continue
            src.image.colorspace_settings.name = "Non-Color"
            grp = tree.nodes.new("ShaderNodeGroup")
            grp.node_tree = rg_normal_group()
            grp.label = RG_GROUP
            grp.location = ((src.location.x + nm.location.x) / 2, nm.location.y - 150)
            tree.links.remove(color.links[0])
            tree.links.new(src.outputs["Color"], grp.inputs[0])
            tree.links.new(grp.outputs[0], color)
            changed.append(mat.name)
    return changed


def remove_loose_vertices():
    """Toolbox's export writes each mesh's whole vertex buffer, but faces only for the first LOD: the other LODs'
    vertices come in as loose points on top of the mesh. Toolbox drops them on import (the LODs are made from the
    first), so they only get in the way. Returns how many were removed."""
    import bmesh
    removed = 0
    for o in bpy.context.scene.objects:
        if o.type != "MESH":
            continue
        bm = bmesh.new()
        bm.from_mesh(o.data)
        loose = [v for v in bm.verts if not v.link_faces]
        if loose:
            removed += len(loose)
            bmesh.ops.delete(bm, geom=loose, context="VERTS")
            bm.to_mesh(o.data)
            o.data.update()
        bm.free()
    return removed


def main():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    ap = argparse.ArgumentParser()
    src = ap.add_mutually_exclusive_group(required=True)
    src.add_argument("--glb", help="from modstudio export (model.json and the textures beside it)")
    src.add_argument("--fbx", help="from Switch Toolbox's export-model")
    ap.add_argument("--blend", required=True)
    a = ap.parse_args(argv)
    if os.path.exists(a.blend):
        raise RuntimeError(f"{a.blend} exists: not overwritten")

    bpy.ops.wm.read_homefile(use_empty=True)
    if a.glb:
        bpy.ops.import_scene.gltf(filepath=a.glb, merge_vertices=False)
        arm, shapes = tidy_glb_import()
        folder = os.path.dirname(os.path.abspath(a.glb))
        with open(os.path.join(folder, "model.json"), encoding="utf-8") as f:
            info = json.load(f)
        count, missing = build_materials(info, folder)
        meshes = sorted(o.name for o in bpy.context.scene.objects if o.type == "MESH")
        log(f"imported {len(meshes)} meshes, armature {arm.name} ({len(arm.data.bones)} bones)"
            + (f", removed {shapes} bone display shape(s)" if shapes else ""))
        log(f"made {count} materials" + (f"; textures not found: {missing}" if missing else ""))
    else:
        bpy.ops.import_scene.fbx(filepath=a.fbx)
        meshes = sorted(o.name for o in bpy.context.scene.objects if o.type == "MESH")
        arms = [o.name for o in bpy.context.scene.objects if o.type == "ARMATURE"]
        log(f"imported {len(meshes)} meshes, armature(s) {arms}")
        log(f"removed {remove_loose_vertices()} loose vertices (other LODs' vertices, which the build drops anyway)")
        changed = fix_normal_maps()
        log(f"RG normal maps reconstructed in {len(changed)} material(s)")
    # Each material as made: the build tells a material changed in the scene from this stamp.
    for m in bpy.data.materials:
        fp = material_fingerprint(m)
        if fp:
            m[FINGERPRINT] = fp
    # images are referenced from the export folder, beside the .blend
    bpy.ops.wm.save_as_mainfile(filepath=a.blend, relative_remap=True)
    log(f"saved {a.blend}")


if __name__ == "__main__":
    main()
