"""Blender (background): a new scene from a model exported by extract_model.py, saved as a .blend to edit.

    blender --background --factory-startup --python make_scene.py -- --glb <model.glb> --blend <out.blend>
    blender --background --factory-startup --python make_scene.py -- --fbx <model.fbx> --blend <out.blend>

From `modstudio export` (--glb; model.json and the textures beside it):
- the armature is named "Armature" (the project files' default); bone display shapes the importer adds are removed
- every material is made from model.json: the _Alb texture to Base Color and Alpha, the _Nrm texture through the
  "Reconstruct RG Normal" group (the game's normal maps hold only red and green; the group works out blue), the
  other textures as unconnected image nodes to paint on. (The importer's own materials would multiply the colour
  by the vertex colours, as glTF means it, which the game doesn't.)

From Switch Toolbox's export (--fbx):
- the FBX is imported with Blender's defaults (the armature turned 90 degrees on X, scale 1)
- vertices no face uses (the lower LODs' vertices, which that export includes as loose points) are removed
- the RG group is put between each _Nrm texture and its Normal Map node

Either way build_mod.py exports the scene at scale 0.01 as Toolbox needs, and an existing .blend is never
overwritten.
"""
import argparse
import json
import os
import re
import sys

import bpy

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from scene_nodes import RG_GROUP, rg_normal_group  # noqa: E402

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
        linked = set()
        for i, tex in enumerate(m["textures"]):
            img = image(tex)
            if img is None:
                continue
            node = nt.nodes.new("ShaderNodeTexImage")
            node.image = img
            node.label = f"{samplers[i]}: {tex}" if i < len(samplers) else tex
            node.location = (-700, 300 - 280 * i)
            role = texture_role(tex)
            if role in linked or role == "aux":
                continue
            linked.add(role)
            if role == "color":
                nt.links.new(node.outputs["Color"], bsdf.inputs["Base Color"])
                nt.links.new(node.outputs["Alpha"], bsdf.inputs["Alpha"])
            else:
                grp = nt.nodes.new("ShaderNodeGroup")
                grp.node_tree = rg_normal_group()
                grp.label = RG_GROUP
                grp.location = (-400, node.location.y)
                nm = nt.nodes.new("ShaderNodeNormalMap")
                nm.location = (-200, node.location.y)
                nt.links.new(node.outputs["Color"], grp.inputs[0])
                nt.links.new(grp.outputs[0], nm.inputs["Color"])
                nt.links.new(nm.outputs["Normal"], bsdf.inputs["Normal"])
    return len(materials), sorted(set(missing))


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
    # images are referenced from the export folder, beside the .blend
    bpy.ops.wm.save_as_mainfile(filepath=a.blend, relative_remap=True)
    log(f"saved {a.blend}")


main()
