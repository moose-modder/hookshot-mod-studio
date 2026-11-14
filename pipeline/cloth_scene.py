"""A model's cloth in the Blender scene, to edit (shared by make_scene.py, prep_for_game.py, and run on its own to add
the cloth to an existing scene).

    blender --background <scene.blend> --python cloth_scene.py -- --cloth <cloth_scene.json> [--armature Armature]

The cloth (`modstudio-core`'s `cloth_edit::scene_of`, cloth_scene.json) becomes the collection "Cloth":
- "Cloth colliders": a wireframe object per collider (capsule, sphere or plane), named after it and following its
  bone. Move, turn and scale the object to move and resize the collider (scaling along a capsule's length lengthens
  it, across it widens it); parent it to another bone to have it follow that one. The mesh is only a picture: edits
  in edit mode are not read.
- "Cloth parts": an empty per simulated part, at the middle of the bones it moves, with its settings as custom
  properties: gravity (in Blender's axes), damping, substeps and solve iterations, scales of the game's values
  (particle radius, friction, stretch and bend stiffness, swing range: 1 keeps them), and a "meets <collider>"
  switch for each of its colliders.

`read_cloth` (the build, prep_for_game.py) reads it back in the game's space, the scene's size taken out, as the
JSON `cloth_edit::apply` takes. The armature has to be at rest (the build puts it there).
"""
import argparse
import json
import math
import re
import sys

import bmesh
import bpy
from mathutils import Matrix, Vector

COLLECTION = "Cloth"
COLLIDERS = "Cloth colliders"
PARTS = "Cloth parts"
# custom properties
ID = "cloth_collider_id"  # a collider object: its item in the game's file
SHAPE = "cloth_shape"  # its shape as made (JSON: game units, the collider's own frame)
PART = "cloth_part"  # a part's empty: the part's name
PART_COLLIDERS = "cloth_colliders"  # its colliders, in its order (JSON: [[id, name], ...])
ORIGINAL = "cloth_collider_name"  # a collider object: the collider's name in the file (a copy has another object name)
PARTS_PROP = "cloth_parts"  # a collider object: the parts that have it, by name (comma-separated; "all")
SOURCE = "cloth_source"  # the collection: the cloth file
SIZE = "cloth_scene_size"  # the collection: the scene's size, times the game's
# the game's (glTF's) axes to Blender's, as the glTF importer turns them
TO_BLENDER = Matrix(((1, 0, 0, 0), (0, 0, -1, 0), (0, 1, 0, 0), (0, 0, 0, 1)))
# a part's settings: property, description, minimum, maximum
SCALES = [
    ("particle_radius", "The particles' radius, times the game's (how far they keep from colliders)", 0.01, 10.0),
    ("friction", "The particles' friction on colliders, times the game's", 0.0, 10.0),
    ("stretch_stiffness", "How hard the cloth keeps its length, times the game's (stiffness stays at most 1)", 0.0, 10.0),
    ("bend_stiffness", "How hard the cloth keeps its shape, times the game's", 0.0, 10.0),
    ("swing_range", "How far the cloth may swing from the skinned mesh, times the game's", 0.01, 10.0),
]


def log(msg):
    print("[cloth] " + msg, flush=True)


def matrix(cols):
    """A column-major 4x4 (16 numbers) as a Matrix."""
    return Matrix([cols[0::4], cols[1::4], cols[2::4], cols[3::4]])


def columns(m):
    return [m[r][c] for c in range(4) for r in range(4)]


def scene_size(arm, bones):
    """How many times the game's size the armature is: the bones' rest places fitted (least squares) to the game's."""
    num = den = 0.0
    for b in bones:
        bone = arm.data.bones.get(b["name"])
        if bone is None:
            continue
        game = (TO_BLENDER @ matrix(b["place"])).translation
        num += game.dot(bone.matrix_local.translation)
        den += game.dot(game)
    return num / den if den > 1e-12 else 1.0


def to_armature(size):
    """The game's space to the armature's."""
    return Matrix.Scale(size, 4) @ TO_BLENDER


def _aim(direction):
    """A rotation taking +Z to `direction`."""
    d = Vector(direction)
    if d.length < 1e-9:
        return Matrix.Identity(4)
    return Vector((0, 0, 1)).rotation_difference(d.normalized()).to_matrix().to_4x4()


def shape_mesh(name, shape, size, extent):
    """A mesh picturing `shape` in its collider's frame, as the object's own coordinates: the game's, turned to
    Blender's axes and times the scene's size. `extent`: how large a plane is drawn (game units)."""
    to_local = to_armature(size)
    bm = bmesh.new()
    kind = shape["kind"]
    if kind == "capsule":
        start = to_local @ Vector(shape["start"])
        end = to_local @ Vector(shape["end"])
        radius = shape["radius"] * size
        length = (end - start).length
        bmesh.ops.create_uvsphere(bm, u_segments=16, v_segments=8, radius=max(radius, 1e-5))
        for v in bm.verts:
            v.co.z += length / 2 if v.co.z > 1e-9 else -length / 2 if v.co.z < -1e-9 else 0
        bmesh.ops.transform(bm, matrix=Matrix.Translation((start + end) / 2) @ _aim(end - start), verts=bm.verts)
    elif kind == "sphere":
        bmesh.ops.create_uvsphere(bm, u_segments=16, v_segments=8, radius=max(shape["radius"] * size, 1e-5))
        bmesh.ops.translate(bm, vec=to_local @ Vector(shape["centre"]), verts=bm.verts)
    elif kind == "plane":
        n = Vector(shape["normal"])
        centre = to_local @ (-shape["distance"] * n)
        normal = (to_local.to_3x3() @ n).normalized()
        half = extent * size / 2
        bmesh.ops.create_grid(bm, x_segments=4, y_segments=4, size=half)
        # and a line along the outward normal: the side the cloth stays on
        a = bm.verts.new((0, 0, 0))
        b = bm.verts.new((0, 0, half / 2))
        bm.edges.new((a, b))
        bmesh.ops.transform(bm, matrix=Matrix.Translation(centre) @ _aim(normal), verts=bm.verts)
    else:
        bmesh.ops.create_cube(bm, size=0.02 * size)
    mesh = bpy.data.meshes.new(name)
    bm.to_mesh(mesh)
    bm.free()
    return mesh


def _collection(name, parent):
    c = bpy.data.collections.get(name)
    if c is None:
        c = bpy.data.collections.new(name)
        parent.children.link(c)
    return c


def add_cloth(scene, arm):
    """The cloth `scene` (cloth_scene.json, loaded) shown in the Blender scene around armature `arm`: the collection
    "Cloth" (see the module's notes). Returns how many colliders and parts it made."""
    if bpy.data.collections.get(COLLECTION) is not None:
        raise RuntimeError(f"the scene already has a collection {COLLECTION!r}")
    size = scene_size(arm, scene.get("bones", []))
    to_arm = to_armature(size)
    from_arm = to_arm.inverted()
    root = _collection(COLLECTION, bpy.context.scene.collection)
    root[SOURCE] = scene["source"]
    root[SIZE] = size
    colliders = _collection(COLLIDERS, root)
    parts = _collection(PARTS, root)
    bpy.context.view_layer.update()
    # how large planes are drawn: the model's extent, from its bones
    if arm.data.bones:
        heads = [b.head_local for b in arm.data.bones]
        extent = max(max(h[k] for h in heads) - min(h[k] for h in heads) for k in range(3)) / max(size, 1e-9)
    else:
        extent = 1.0
    names = {}
    for c in scene["colliders"]:
        obj = bpy.data.objects.new(c["name"], shape_mesh(c["name"], c["shape"], size, max(extent * 0.3, 0.05)))
        colliders.objects.link(obj)
        names[c["id"]] = obj.name
        obj[ID] = c["id"]
        obj[ORIGINAL] = c["name"]
        obj[SHAPE] = json.dumps(c["shape"])
        obj[PARTS_PROP] = ", ".join(c.get("parts") or [])
        obj.id_properties_ui(PARTS_PROP).update(
            description="The simulated parts that collide with this collider, by name (comma-separated, or all)")
        obj.display_type = 'WIRE'
        obj.show_in_front = True
        obj.hide_render = True
        world = arm.matrix_world @ to_arm @ matrix(c["place"]) @ from_arm
        bone = c.get("bone")
        obj.parent = arm
        if bone and bone in arm.data.bones:
            obj.parent_type = 'BONE'
            obj.parent_bone = bone
        elif bone:
            log(f"{c['name']}: bone {bone} isn't in the armature; it follows the armature")
        bpy.context.view_layer.update()
        obj.matrix_world = world
    for p in scene["parts"]:
        emp = bpy.data.objects.new(p["name"], None)
        parts.objects.link(emp)
        emp.empty_display_type = 'SPHERE'
        emp.empty_display_size = 0.03 * size
        emp.hide_render = True
        emp.parent = arm
        emp.matrix_world = arm.matrix_world @ Matrix.Translation(to_arm @ Vector(p.get("centre", (0, 0, 0))))
        emp[PART] = p["name"]
        emp[PART_COLLIDERS] = json.dumps([[c["id"], names.get(c["id"], str(c["id"]))] for c in p["colliders"]])
        emp["gravity"] = list(TO_BLENDER.to_3x3() @ Vector(p["gravity"]))
        emp.id_properties_ui("gravity").update(
            subtype='ACCELERATION', description="Gravity on the part, in Blender's axes (m/s^2; down is -Z)")
        emp["damping"] = p["damping"]
        emp.id_properties_ui("damping").update(
            min=0.0, max=1.0, precision=4,
            description="The share of the cloth's speed lost each second (the game's files: mostly 0.001)")
        emp["substeps"] = p["substeps"]
        emp.id_properties_ui("substeps").update(min=1, max=16, description="Simulation steps a frame")
        emp["iterations"] = p["iterations"]
        emp.id_properties_ui("iterations").update(min=1, max=16, description="Constraint passes a step")
        for prop, description, lo, hi in SCALES:
            emp[prop] = p[prop]
            emp.id_properties_ui(prop).update(min=lo, max=hi, description=description)
        for c in p["colliders"]:
            key = f"meets {names.get(c['id'], c['id'])}"
            emp[key] = bool(c["meets"])
            emp.id_properties_ui(key).update(description="Whether the part's cloth collides with this collider")
    log(f"{scene['source']}: {len(scene['colliders'])} collider(s), {len(scene['parts'])} part(s)"
        + (f", the scene at {size:.4g} times the game's size" if abs(size - 1) > 1e-3 else ""))
    return len(scene["colliders"]), len(scene["parts"])


def is_cloth_object(obj):
    """A collider, a part's empty, or anything put in the colliders' collection (not part of the model)."""
    return ID in obj or PART in obj or any(c.name == COLLIDERS for c in obj.users_collection)


def _parts_of(obj, all_parts):
    """The parts an object's `cloth_parts` lists ("all": every part); None when it has no such property."""
    if PARTS_PROP not in obj:
        return None
    names = [n.strip() for n in re.split(r"[,;]", str(obj[PARTS_PROP])) if n.strip()]
    if any(n.lower() == "all" for n in names):
        return list(all_parts)
    return names


def _new_name(name):
    """A new collider's name in the file: the object's, Blender's ".001" and other signs as underscores."""
    return re.sub(r"[^A-Za-z0-9_]", "_", name)


def _fitted(obj, scale, size):
    """The shape of a mesh the scene adds to the colliders, from its bounding box in its own frame (times its scale),
    in the game's units and axes: flat, a plane (its normal along the flat side, +); about as wide as long, a sphere;
    else a capsule along its longest side."""
    to_game3 = TO_BLENDER.to_3x3().inverted()
    points = [Vector([v.co[k] * scale[k] for k in range(3)]) for v in obj.data.vertices]
    if not points:
        return None
    lo = Vector([min(p[k] for p in points) for k in range(3)])
    hi = Vector([max(p[k] for p in points) for k in range(3)])
    dims = hi - lo
    centre = (lo + hi) / 2
    order = sorted(range(3), key=lambda k: dims[k])

    def unit(k):
        return Vector([1.0 if i == k else 0.0 for i in range(3)])

    def game(v):
        return to_game3 @ (v / size)

    if dims[order[0]] < 1e-3 * max(dims[order[2]], 1e-9):
        n = (to_game3 @ unit(order[0])).normalized()
        return {"kind": "plane", "normal": list(n), "distance": -n.dot(game(centre))}
    if dims[order[2]] <= 1.25 * dims[order[0]]:
        return {"kind": "sphere", "centre": list(game(centre)), "radius": sum(dims) / 6 / size}
    k = order[2]
    radius = (dims[order[0]] + dims[order[1]]) / 4
    half = max(dims[k] / 2 - radius, 0.0)
    return {"kind": "capsule", "start": list(game(centre - unit(k) * half)),
            "end": list(game(centre + unit(k) * half)), "radius": radius / size}


def _scaled(shape, s_game):
    """`shape` (the collider's own frame) under the scale `s_game` (a 3x3 in the game's axes)."""
    kind = shape["kind"]
    out = dict(shape)

    def across(axis):
        # the mean scale of two directions square to `axis`
        a = Vector(axis).normalized() if Vector(axis).length > 1e-9 else Vector((1, 0, 0))
        p = a.orthogonal().normalized()
        q = a.cross(p).normalized()
        return ((s_game @ p).length + (s_game @ q).length) / 2

    if kind == "capsule":
        start, end = Vector(shape["start"]), Vector(shape["end"])
        out["start"] = list(s_game @ start)
        out["end"] = list(s_game @ end)
        out["radius"] = shape["radius"] * across(end - start)
    elif kind == "sphere":
        out["centre"] = list(s_game @ Vector(shape["centre"]))
        out["radius"] = shape["radius"] * sum(s_game[k][k] for k in range(3)) / 3
    elif kind == "plane":
        # a plane's normal turns by the inverse transpose; its distance shrinks by as much as the normal grows
        n = s_game.inverted().transposed() @ Vector(shape["normal"])
        length = n.length if n.length > 1e-12 else 1.0
        out["normal"] = list(n / length)
        out["distance"] = shape["distance"] / length
    return out


def read_cloth(arm):
    """The scene's cloth, as `cloth_edit::apply` takes it (None when the scene has none), and warnings."""
    root = bpy.data.collections.get(COLLECTION)
    if root is None or SOURCE not in root:
        return None, []
    size = float(root.get(SIZE, 1.0))
    to_arm = to_armature(size)
    from_arm = to_arm.inverted()
    to_game3 = TO_BLENDER.to_3x3().inverted()
    warnings = []
    bpy.context.view_layer.update()
    arm_inverse = arm.matrix_world.inverted()
    all_parts = [emp[PART] for emp in bpy.data.objects if PART in emp]
    # the file's colliders: an object each; a second object of one (a copy, Shift+D) is a new collider copied from it
    by_id = {}
    for obj in bpy.data.objects:
        if ID in obj:
            by_id.setdefault(int(obj[ID]), []).append(obj)
    originals = {}
    for cid, objs in by_id.items():
        named = [o for o in objs if o.name == o.get(ORIGINAL)]
        originals[cid] = objs[0] if len(objs) == 1 else (named or sorted(objs, key=lambda o: o.name))[0]
    colliders = []
    for obj in bpy.data.objects:
        added = ID not in obj
        if added and not (obj.type == 'MESH' and any(c.name == COLLIDERS for c in obj.users_collection)):
            continue
        loc, rot, scale = (arm_inverse @ obj.matrix_world).decompose()
        rigid = Matrix.Translation(loc) @ rot.to_matrix().to_4x4()
        place = from_arm @ rigid @ to_arm
        bone = obj.parent_bone if obj.parent == arm and obj.parent_type == 'BONE' and obj.parent_bone else None
        if obj.parent not in (arm, None):
            warnings.append(f"{obj.name}: its parent isn't the armature; it's read where it is, on no bone")
        parts = _parts_of(obj, all_parts)
        if added:
            # a mesh put in the colliders' collection: its shape from its size
            shape = _fitted(obj, scale, size)
            if shape is None:
                warnings.append(f"{obj.name}: an empty mesh in {COLLIDERS}, left out")
                continue
            colliders.append({"id": 0, "name": _new_name(obj.name), "bone": bone, "place": columns(place),
                              "shape": shape, "parts": parts or []})
            continue
        shape = json.loads(obj[SHAPE])
        s_game = to_game3 @ Matrix.Diagonal(scale) @ TO_BLENDER.to_3x3()
        shape = _scaled(shape, s_game) if shape["kind"] != "other" else shape
        cid = int(obj[ID])
        if originals[cid] is obj:
            entry = {"id": cid, "name": obj.name, "bone": bone, "place": columns(place), "shape": shape}
            if parts is not None:
                entry["parts"] = parts
        else:
            entry = {"id": 0, "copy_of": cid, "name": _new_name(obj.name), "bone": bone, "place": columns(place),
                     "shape": shape, "parts": parts or []}
        colliders.append(entry)
    parts = []
    for emp in bpy.data.objects:
        if PART not in emp:
            continue
        own = json.loads(emp[PART_COLLIDERS])
        # a collider object renamed since: its switch is still under the name it had
        current = {int(o[ID]): o.name for o in bpy.data.objects if ID in o}
        meets = []
        for cid, name in own:
            key = f"meets {name}"
            if key not in emp and f"meets {current.get(cid, name)}" in emp:
                key = f"meets {current.get(cid, name)}"
            meets.append({"id": int(cid), "meets": bool(emp.get(key, True))})
        g = to_game3 @ Vector(emp["gravity"])
        part = {
            "name": emp[PART],
            "colliders": meets,
            "gravity": list(g),
            "damping": float(emp["damping"]),
            "substeps": int(emp["substeps"]),
            "iterations": int(emp["iterations"]),
        }
        for prop, _, _, _ in SCALES:
            part[prop] = float(emp[prop])
        parts.append(part)
    # the file's colliders whose objects are gone: deleted (no part collides with them any more)
    deleted = []
    for emp in bpy.data.objects:
        if PART in emp:
            for cid, name in json.loads(emp[PART_COLLIDERS]):
                if int(cid) not in by_id and int(cid) not in deleted:
                    deleted.append(int(cid))
    return {"source": root[SOURCE], "colliders": colliders, "parts": parts, "deleted": deleted}, warnings


def main():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    ap = argparse.ArgumentParser()
    ap.add_argument("--cloth", required=True)
    ap.add_argument("--armature", default="Armature")
    a = ap.parse_args(argv)
    with open(a.cloth, encoding="utf-8") as f:
        scene = json.load(f)
    arm = bpy.data.objects.get(a.armature)
    if arm is None or arm.type != 'ARMATURE':
        raise RuntimeError(f"armature {a.armature!r} not found")
    # placed at rest; the scene's own action and pose are put back after (the colliders follow their bones)
    action = arm.animation_data.action if arm.animation_data is not None else None
    if action is not None:
        arm.animation_data.action = None
    pose = {pb.name: pb.matrix_basis.copy() for pb in arm.pose.bones}
    for pb in arm.pose.bones:
        pb.matrix_basis = Matrix.Identity(4)
    add_cloth(scene, arm)
    for pb in arm.pose.bones:
        pb.matrix_basis = pose[pb.name]
    if action is not None:
        arm.animation_data.action = action
    bpy.ops.wm.save_mainfile()
    log(f"saved {bpy.data.filepath}")


if __name__ == "__main__":
    try:
        main()
    except Exception:
        import traceback
        traceback.print_exc()
        sys.exit(1)
