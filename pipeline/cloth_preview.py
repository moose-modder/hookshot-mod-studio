"""Blender, with a scene open: the cloth preview (`modstudio cloth`, build/cloth_preview.json) as the action
"Cloth preview" on the scene's armature. Play it to see the meshes swing as the game's cloth swings them.

    blender <scene.blend> --python cloth_preview.py -- --preview <cloth_preview.json> [--armature Armature]

The preview holds, frame by frame, where the test motion has the model and where each bone the cloth moves is,
in the game's space (y up, metres). They become the pose of the armature's root bones (the motion) and of those
bones, in the armature's space: the glTF importer's axes (the game's x, y, z is Blender's x, -z, y) at the scene's
size (fitted from the bones, so a scene at another size than the game's works too). Every other bone follows its
parent.

The action only poses the armature: a build reads the rest pose (prep_for_game.py drops any action first), so the
action can stay in the scene. Running this again replaces it. The scene is left at 30 frames a second, with its
frame range set to the preview's.
"""
import argparse
import json
import sys

import bpy
from bpy_extras import anim_utils
from mathutils import Matrix

ACTION = "Cloth preview"
# the game's (glTF's) axes to Blender's, as the glTF importer turns them
TO_BLENDER = Matrix(((1, 0, 0, 0), (0, 0, -1, 0), (0, 1, 0, 0), (0, 0, 0, 1)))


def log(msg):
    print("[cloth] " + msg, flush=True)


def matrix(cols):
    """A column-major 4x4 (16 numbers) as a Matrix."""
    return Matrix([cols[0::4], cols[1::4], cols[2::4], cols[3::4]])


def scene_size(arm, bones):
    """How many times the game's size the armature is: the bones' rest places fitted (least squares) to the
    preview's."""
    num = den = 0.0
    for b in bones:
        bone = arm.data.bones.get(b["name"])
        if bone is None:
            continue
        game = (TO_BLENDER @ matrix(b["rest"])).translation
        num += game.dot(bone.matrix_local.translation)
        den += game.dot(game)
    return num / den if den > 1e-12 else 1.0


def in_order(arm):
    """The armature's bones, every parent before its children."""
    out = []

    def visit(bone):
        out.append(bone)
        for c in bone.children:
            visit(c)

    for root in [b for b in arm.data.bones if b.parent is None]:
        visit(root)
    return out


def keyed_values(arm, preview):
    """For each bone the action keys (the roots, and the bones the cloth moves): its pose (location, rotation,
    scale) in every frame."""
    size = scene_size(arm, preview["bones"])
    to_arm = Matrix.Scale(size, 4) @ TO_BLENDER
    from_arm = to_arm.inverted()
    driven = {}
    for b in preview["bones"]:
        if b["name"] in arm.data.bones:
            driven[b["name"]] = (matrix(b["rest"]).inverted(), b["frames"])
    missing = [b["name"] for b in preview["bones"] if b["name"] not in arm.data.bones]
    bones = in_order(arm)
    roots = [b.name for b in bones if b.parent is None]
    rest = {b.name: b.matrix_local for b in bones}
    rest_inv = {name: m.inverted() for name, m in rest.items()}
    keyed = roots + [b.name for b in bones if b.name in driven and b.parent is not None]
    basis_now = {pb.name: pb.matrix_basis.copy() for pb in arm.pose.bones}
    values = {name: [] for name in keyed}
    for f in range(preview["frames"]):
        moved = to_arm @ matrix(preview["motion"][f]) @ from_arm
        posed = {}
        for bone in bones:
            name = bone.name
            # where it is with its parent's pose and its own rest place relative to the parent
            follows = posed[bone.parent.name] @ rest_inv[bone.parent.name] @ rest[name] if bone.parent else rest[name]
            if name in driven:
                rest_game_inv, frames = driven[name]
                posed[name] = to_arm @ matrix(frames[f]) @ rest_game_inv @ from_arm @ rest[name]
            elif bone.parent is None:
                posed[name] = moved @ rest[name]
            else:
                posed[name] = follows @ basis_now[name]
            if name in values:
                values[name].append(follows.inverted() @ posed[name])
    return values, size, missing


def make_action(arm, values, frames):
    old = bpy.data.actions.get(ACTION)
    if old is not None:
        bpy.data.actions.remove(old)
    action = bpy.data.actions.new(ACTION)
    slot = action.slots.new(id_type='OBJECT', name=arm.name)
    ad = arm.animation_data or arm.animation_data_create()
    ad.action = action
    ad.action_slot = slot
    channels = anim_utils.action_ensure_channelbag_for_slot(action, slot)
    linear = [1] * frames  # LINEAR in the keyframes' interpolation enum (CONSTANT, LINEAR, BEZIER, ...)
    scaled = 0
    for name, basis in values.items():
        pb = arm.pose.bones[name]
        if pb.rotation_mode != 'QUATERNION':
            pb.rotation_mode = 'QUATERNION'
        locs, quats, scales = [], [], []
        last = None
        for m in basis:
            loc, q, scale = m.decompose()
            if last is not None and last.dot(q) < 0:
                q.negate()
            last = q
            locs.append(loc)
            quats.append(q)
            scales.append(scale)
        channels_of = [("location", 3, locs), ("rotation_quaternion", 4, quats)]
        if any(abs(s[k] - 1) > 1e-3 for s in scales for k in range(3)):
            channels_of.append(("scale", 3, scales))
            scaled += 1
        for prop, n, series in channels_of:
            path = f'pose.bones["{name}"].{prop}'
            for i in range(n):
                fc = channels.fcurves.new(path, index=i, group_name=name)
                fc.keyframe_points.add(frames)
                co = []
                for f, v in enumerate(series):
                    co += [f + 1, v[i]]
                fc.keyframe_points.foreach_set("co", co)
                fc.keyframe_points.foreach_set("interpolation", linear)
                fc.update()
    return action, scaled


def main():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    ap = argparse.ArgumentParser()
    ap.add_argument("--preview", required=True)
    ap.add_argument("--armature", default="Armature")
    a = ap.parse_args(argv)
    with open(a.preview, encoding="utf-8") as f:
        preview = json.load(f)
    arm = bpy.data.objects.get(a.armature)
    if arm is None or arm.type != 'ARMATURE':
        arms = [o for o in bpy.data.objects if o.type == 'ARMATURE']
        if len(arms) != 1:
            raise RuntimeError(f"armature {a.armature!r} not found")
        arm = arms[0]
    if bpy.context.object and bpy.context.object.mode == 'EDIT':
        bpy.ops.object.mode_set(mode='OBJECT')
    values, size, missing = keyed_values(arm, preview)
    action, scaled = make_action(arm, values, preview["frames"])
    scene = bpy.context.scene
    scene.render.fps = preview["fps"]
    scene.render.fps_base = 1
    scene.frame_start = 1
    scene.frame_end = preview["frames"]
    scene.frame_set(1)
    log(f"{preview['source']}: action {action.name!r} on {arm.name}, {len(values)} bones keyed over "
        f"{preview['frames']} frames ({len(preview['parts'])} cloth parts)"
        + (f", the scene is {size:.4g} times the game's size" if abs(size - 1) > 1e-3 else "")
        + (f", {scaled} bone(s) scaled" if scaled else ""))
    if missing:
        log(f"not in the armature, left out: {missing[:8]}{' ...' if len(missing) > 8 else ''}")
    for w in preview.get("warnings", []):
        log("note: " + w)


if __name__ == "__main__":
    try:
        main()
    except Exception:
        import traceback
        traceback.print_exc()
        if bpy.app.background:
            sys.exit(1)
