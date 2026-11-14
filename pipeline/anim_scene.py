"""The game's animations in the Blender scene, to edit (shared by prep_for_game.py, and run on its own to add
animations to a scene).

    blender --background <scene.blend> --python anim_scene.py -- --add <anim.json> [<anim.json> ...] [--armature Armature]

Each animation (`modstudio-core`'s `anim_edit::write_for_blender`: a header, and every animated bone's place in the
model at every frame) becomes an action on the armature, named after the animation, with a key on every frame
(frame 1 is the game's frame 0) on each bone it animates. The action keeps where it came from as custom properties
(the game's file and animation, its length and the scene's size), and a fake user, so it stays in the .blend while
another action plays. The last added is the armature's action. An action of the same animation added again replaces
it.

Edit an action with Blender's tools (keys can be changed, added and removed; constraints and drivers count, as the
pose they give is what's read). The build (`read_anims`, from prep_for_game.py) plays every such action and records
each bone's place at every frame of the game's animation, as a change from its rest place in the game's space (the
scene's size taken out): `anim_edit::apply` writes the bones that changed into the game's file. Bones the game's
animation doesn't animate are only reported.
"""
import argparse
import json
import os
import sys
from array import array

import bpy
from bpy_extras import anim_utils
from mathutils import Matrix

# custom properties of an action made from a game animation
FILE = "game_anim_file"
NAME = "game_anim_name"
FRAMES = "game_anim_frames"  # the game's frame count (the action has one more frame: 0 to it)
LOOPING = "game_anim_looping"
SIZE = "game_scene_size"  # the scene's size, times the game's
# the game's (glTF's) axes to Blender's, as the glTF importer turns them
TO_BLENDER = Matrix(((1, 0, 0, 0), (0, 0, -1, 0), (0, 1, 0, 0), (0, 0, 0, 1)))


def log(msg):
    print("[anim] " + msg, flush=True)


def matrix(cols):
    """A column-major 4x4 (16 numbers) as a Matrix."""
    return Matrix([cols[0::4], cols[1::4], cols[2::4], cols[3::4]])


def columns(m):
    """A Matrix as 16 numbers, column-major."""
    return [m[r][c] for c in range(4) for r in range(4)]


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


def scene_size(arm, bones):
    """How many times the game's size the armature is: the bones' rest places fitted (least squares) to the
    game's."""
    num = den = 0.0
    for b in bones:
        bone = arm.data.bones.get(b["name"])
        if bone is None:
            continue
        game = (TO_BLENDER @ matrix(b["rest"])).translation
        num += game.dot(bone.matrix_local.translation)
        den += game.dot(game)
    return num / den if den > 1e-12 else 1.0


def find_armature(name):
    arm = bpy.data.objects.get(name)
    if arm is None or arm.type != 'ARMATURE':
        arms = [o for o in bpy.data.objects if o.type == 'ARMATURE']
        if len(arms) != 1:
            raise RuntimeError(f"armature {name!r} not found")
        arm = arms[0]
    return arm


def add_anim(arm, header_path):
    """One game animation as an action on `arm` (replacing an action of the same animation)."""
    with open(header_path, encoding="utf-8") as f:
        h = json.load(f)
    data = array("f")
    with open(os.path.join(os.path.dirname(header_path), h["data"]), "rb") as f:
        data.frombytes(f.read())
    if sys.byteorder != "little":
        data.byteswap()
    frames = h["frame_count"] + 1
    bones_in = h["bones"]
    nb = len(bones_in)
    if len(data) < frames * nb * 16:
        raise RuntimeError(f"{header_path}: {len(data)} floats for {frames} frames of {nb} bones")
    size = scene_size(arm, bones_in)
    to_arm = Matrix.Scale(size, 4) @ TO_BLENDER
    from_arm = to_arm.inverted()
    driven = {}
    for k, b in enumerate(bones_in):
        if b["name"] in arm.data.bones:
            driven[b["name"]] = (matrix(b["rest"]).inverted(), k)
    missing = [b["name"] for b in bones_in if b["name"] not in arm.data.bones]
    order = in_order(arm)
    rest = {b.name: b.matrix_local for b in order}
    rest_inv = {name: m.inverted() for name, m in rest.items()}
    values = {b.name: [] for b in order if b.name in driven}
    for f in range(frames):
        posed = {}
        base = f * nb * 16
        for bone in order:
            name = bone.name
            follows = posed[bone.parent.name] @ rest_inv[bone.parent.name] @ rest[name] if bone.parent else rest[name]
            if name in driven:
                rest_game_inv, k = driven[name]
                at = base + k * 16
                game = matrix(data[at:at + 16])
                posed[name] = to_arm @ game @ rest_game_inv @ from_arm @ rest[name]
                values[name].append(follows.inverted() @ posed[name])
            else:
                posed[name] = follows
    # an action of the same animation goes
    for old in [a for a in bpy.data.actions if a.get(FILE) == h["file"] and a.get(NAME) == h["name"]]:
        bpy.data.actions.remove(old)
    action = bpy.data.actions.new(h["name"])
    action[FILE] = h["file"]
    action[NAME] = h["name"]
    action[FRAMES] = h["frame_count"]
    action[LOOPING] = bool(h.get("looping", False))
    action[SIZE] = size
    action.use_fake_user = True
    action.frame_range = (1, frames)
    action.use_frame_range = True
    slot = action.slots.new(id_type='OBJECT', name=arm.name)
    ad = arm.animation_data or arm.animation_data_create()
    ad.action = action
    ad.action_slot = slot
    channels = anim_utils.action_ensure_channelbag_for_slot(action, slot)
    linear = [1] * frames  # LINEAR in the keyframes' interpolation enum
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
        for prop, n, series in (("location", 3, locs), ("rotation_quaternion", 4, quats), ("scale", 3, scales)):
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
    log(f"{h['file']} / {h['name']}: action {action.name!r}, {len(values)} bones keyed over {frames} frames"
        + (f", the scene is {size:.4g} times the game's size" if abs(size - 1) > 1e-3 else ""))
    if missing:
        log(f"not in the armature, left out: {missing[:8]}{' ...' if len(missing) > 8 else ''}")
    return action


def game_actions():
    """The scene's actions made from game animations."""
    return [a for a in bpy.data.actions if FILE in a and NAME in a and FRAMES in a]


def read_anims(arm, out_dir):
    """Every game animation's action played on `arm` and each bone's place recorded at every frame of the game's
    animation, as its change from rest in the game's space: `anim_edits.json` and `anim_edits.bin` in `out_dir`
    (removed when the scene has none). The armature's action and pose are put back after."""
    json_path = os.path.join(out_dir, "anim_edits.json")
    bin_path = os.path.join(out_dir, "anim_edits.bin")
    for p in (json_path, bin_path):
        if os.path.exists(p):
            os.remove(p)
    actions = game_actions()
    if not actions:
        return None
    scene = bpy.context.scene
    ad = arm.animation_data or arm.animation_data_create()
    kept = (ad.action, ad.action_slot if ad.action else None, scene.frame_current)
    # the meshes needn't follow while the bones are read
    muted = []
    for o in bpy.data.objects:
        for m in o.modifiers:
            if m.type == 'ARMATURE' and m.show_viewport:
                m.show_viewport = False
                muted.append(m)
    order = in_order(arm)
    names = [b.name for b in order]
    rest_inv = {b.name: b.matrix_local.inverted() for b in order}
    data = array("f")
    entries = []
    try:
        for action in actions:
            size = float(action.get(SIZE, 1.0))
            to_arm = Matrix.Scale(size, 4) @ TO_BLENDER
            from_arm = to_arm.inverted()
            ad.action = action
            if action.slots:
                ad.action_slot = action.slots[0]
            for pb in arm.pose.bones:
                pb.matrix_basis = Matrix.Identity(4)
            frames = int(action[FRAMES]) + 1
            offset = len(data)
            for f in range(frames):
                scene.frame_set(f + 1)
                for name in names:
                    posed = arm.pose.bones[name].matrix
                    data.extend(columns(from_arm @ posed @ rest_inv[name] @ to_arm))
            entries.append({
                "file": action[FILE],
                "name": action[NAME],
                "frame_count": frames - 1,
                "action": action.name,
                "bones": names,
                "offset": offset,
            })
            log(f"read {action.name!r} ({action[NAME]}): {frames} frames, {len(names)} bones")
    finally:
        for m in muted:
            m.show_viewport = True
        ad.action = kept[0]
        if kept[0] is not None and kept[1] is not None:
            ad.action_slot = kept[1]
        scene.frame_set(kept[2])
    if sys.byteorder != "little":
        data.byteswap()
    with open(bin_path, "wb") as f:
        data.tofile(f)
    with open(json_path, "w", encoding="utf-8") as f:
        json.dump({"data": "anim_edits.bin", "anims": entries}, f, indent=1)
    return entries


def main():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    ap = argparse.ArgumentParser()
    ap.add_argument("--add", nargs="+", required=True)
    ap.add_argument("--armature", default="Armature")
    a = ap.parse_args(argv)
    arm = find_armature(a.armature)
    if bpy.context.object and bpy.context.object.mode != 'OBJECT':
        bpy.ops.object.mode_set(mode='OBJECT')
    scene = bpy.context.scene
    scene.render.fps = 30
    scene.render.fps_base = 1
    last = None
    for path in a.add:
        last = add_anim(arm, path)
    if last is not None:
        scene.frame_start = 1
        scene.frame_end = int(last[FRAMES]) + 1
        scene.frame_set(1)
    bpy.ops.wm.save_mainfile()
    log(f"saved {bpy.data.filepath}")


if __name__ == "__main__":
    try:
        main()
    except Exception:
        import traceback
        traceback.print_exc()
        if bpy.app.background:
            sys.exit(1)
