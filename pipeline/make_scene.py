"""Blender (background): a new scene from a model exported by extract_model.py, saved as a .blend to edit.

    blender --background --factory-startup --python make_scene.py -- --fbx <model.fbx> --blend <out.blend>

- the FBX is imported with Blender's defaults (the transforms the pipeline expects: the armature turned 90 degrees
  on X, scale 1; build_mod.py exports it back at scale 0.01 as Toolbox needs)
- vertices no face uses (the lower LODs' vertices, which the export includes as loose points) are removed
- normal maps (_Nrm) hold only red and green, as the game stores them: each material gets a
  "Reconstruct RG Normal" node group between the texture and the Normal Map node, which works out blue,
  so the scene shows the normals right. (prep_for_game.py bakes normals in the same convention.)
- an existing .blend is never overwritten
"""
import argparse
import os
import sys

import bpy

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from scene_nodes import RG_GROUP, rg_normal_group  # noqa: E402


def log(msg):
    print("[scene] " + msg, flush=True)


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
    """The export writes each mesh's whole vertex buffer, but faces only for the first LOD: the other LODs' vertices
    come in as loose points on top of the mesh. Toolbox drops them on import (the LODs are made from the first),
    so they only get in the way. Returns how many were removed."""
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
    ap.add_argument("--fbx", required=True)
    ap.add_argument("--blend", required=True)
    a = ap.parse_args(argv)
    if os.path.exists(a.blend):
        raise RuntimeError(f"{a.blend} exists: not overwritten")

    bpy.ops.wm.read_homefile(use_empty=True)
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
