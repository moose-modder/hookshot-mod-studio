"""Prepares a Blender scene for building the mod (modstudio replace, or Switch Toolbox's replace-model).

Run by the build (crates/modstudio-core/src/build.rs: the app, `modstudio build`):
    blender --background scene.blend --python prep_for_game.py -- --config prep_config.json

Exports glTF for modstudio replace (config glb_name) or FBX for Switch Toolbox's replace-model (fbx_name).

The .blend file is never saved. Everything happens on the copy Blender loads.

For every mesh parented to the armature:
  * If "<name>_WORK" exists, it is the source for "<name>" (a stale "<name>" export copy is ignored).
  * A mesh the game model doesn't have is joined into the game mesh that uses its material (or the one
    prep.join names), with that mesh's UV maps and colour layers (missing colours from its nearest vertices).
  * Bone weights are limited to max_weights per vertex and normalized.
  * The mesh data gets the object's name (Assimp, and so Toolbox, matches meshes by mesh data name).

The game draws every mesh with its own material and that material's textures; Blender materials don't
survive. So what the scene shows is put into the model's own textures (the ones named after the model,
never shared CmnTex_* ones):

  * Painted textures: a scene image of one of those textures that differs from the game's is used.
  * Custom materials: faces on any material other than the mesh's game material are baked (a game material
    changed in place, i.e. with nodes the import doesn't make, counts as a custom one):
      1. Which faces look different: they are baked with their material and with the game material,
         side by side in a scratch layout (diffuse colour and normals). Faces that differ, and `grow`
         rings of faces around them, are moved. (A merge rule moves all faces of its extra materials.)
      2. The moved faces get a new UV layout packed into the free space of the texture, around the
         islands of every mesh whose material uses the same textures (those stay exactly in place).
      3. Every texture of the model's own that the game material uses is baked for the moved faces:
           _Alb  -> diffuse colour of the faces' materials (alpha: the original's, sampled)
           _Nrm  -> tangent space normals
           other -> the original texture sampled where the face was (or through a merge rule
                    material's source UV map, blended by its weight attribute)
         The materials read their textures through the original UVs, as in the viewport.
      4. Pixels of the other islands are copied back unchanged. Channels that are constant or copies of
         red in the original stay that way.
  * All faces then get the game material, and UV maps and colour layers the game mesh doesn't have are
    removed.

Writes <output>/<fbx_name>, <output>/textures/<texture>.png and <output>/prep_report.json.
"""

import bpy
import bmesh
import contextlib
import fnmatch
import json
import math
import os
import re
import sys
import time

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from scene_nodes import FINGERPRINT, RG_GROUP, material_fingerprint, rg_normal_group  # noqa: E402

LAYOUT = "PREP_layout"
PROBE = "PREP_probe"


def log(msg):
    print(f"[prep] {msg}", flush=True)


class Report:
    def __init__(self):
        self.warnings = []
        self.errors = []
        self.meshes = {}
        self.textures = []
        self.painted = []

    def warn(self, msg):
        self.warnings.append(msg)
        log("WARNING: " + msg)

    def error(self, msg):
        self.errors.append(msg)
        log("ERROR: " + msg)


# ---------------------------------------------------------------- UV rasterizing

def _triangle_pixels(pts, width, height):
    """(y0, x0, mask) of the pixel centres inside a triangle given in pixel units, or None."""
    (ax, ay), (bx, by), (cx, cy) = pts
    x0, x1 = int(max(0, np.floor(pts[:, 0].min()))), int(min(width - 1, np.ceil(pts[:, 0].max())))
    y0, y1 = int(max(0, np.floor(pts[:, 1].min()))), int(min(height - 1, np.ceil(pts[:, 1].max())))
    d = (by - cy) * (ax - cx) + (cx - bx) * (ay - cy)
    if x1 < x0 or y1 < y0 or abs(d) < 1e-12:
        return None
    X, Y = np.meshgrid(np.arange(x0, x1 + 1) + 0.5, np.arange(y0, y1 + 1) + 0.5)
    l1 = ((by - cy) * (X - cx) + (cx - bx) * (Y - cy)) / d
    l2 = ((cy - ay) * (X - cx) + (ax - cx) * (Y - cy)) / d
    return y0, x0, (l1 >= 0) & (l2 >= 0) & (l1 + l2 <= 1)


def raster_triangles(uv, tris, width, height):
    """Counts per pixel how many of the triangles (loop index triples) cover its centre."""
    grid = np.zeros((height, width), np.int32)
    for loops in tris:
        r = _triangle_pixels(uv[list(loops)] * (width, height), width, height)
        if r:
            y0, x0, inside = r
            grid[y0:y0 + inside.shape[0], x0:x0 + inside.shape[1]] += inside
    return grid


def raster_ids(uv, tris, width, height):
    """Per pixel the index of the triangle covering its centre (-1: none), and per triangle the pixel
    (row, column) its centre falls in, so that even a triangle smaller than a pixel has one."""
    grid = np.full((height, width), -1, np.int32)
    centres = np.zeros((len(tris), 2), np.int64)
    for k, loops in enumerate(tris):
        pts = uv[list(loops)] * (width, height)
        c = pts.mean(0)
        centres[k] = (min(height - 1, max(0, int(c[1]))), min(width - 1, max(0, int(c[0]))))
        r = _triangle_pixels(pts, width, height)
        if r:
            y0, x0, inside = r
            sub = grid[y0:y0 + inside.shape[0], x0:x0 + inside.shape[1]]
            sub[inside] = k
    return grid, centres


def dilate(mask, pixels):
    out = mask.copy()
    for _ in range(pixels):
        grown = out.copy()
        grown[1:, :] |= out[:-1, :]
        grown[:-1, :] |= out[1:, :]
        grown[:, 1:] |= out[:, :-1]
        grown[:, :-1] |= out[:, 1:]
        out = grown
    return out


# ---------------------------------------------------------------- mesh data

def mesh_uv(me, uv_name):
    uv = np.zeros(len(me.loops) * 2)
    me.uv_layers[uv_name].data.foreach_get("uv", uv)
    return uv.reshape(-1, 2)


def material_indices(me):
    idx = np.zeros(len(me.polygons), np.int32)
    me.polygons.foreach_get("material_index", idx)
    return idx


def triangles(me):
    """Loop index triples of the mesh's triangles and the polygon each belongs to."""
    me.calc_loop_triangles()
    loops = np.zeros(len(me.loop_triangles) * 3, np.int32)
    me.loop_triangles.foreach_get("loops", loops)
    polys = np.zeros(len(me.loop_triangles), np.int32)
    me.loop_triangles.foreach_get("polygon_index", polys)
    return loops.reshape(-1, 3), polys


def triangle_areas(points):
    """Areas of triangles given as (n, 3, 2) or (n, 3, 3) arrays."""
    a, b, c = points[:, 0], points[:, 1], points[:, 2]
    cross = np.cross(b - a, c - a)
    return 0.5 * (np.abs(cross) if cross.ndim == 1 else np.linalg.norm(cross, axis=1))


def grow_faces(me, faces, rings):
    """Adds `rings` rings of faces that share a vertex with the selected ones."""
    if rings <= 0 or not faces.any():
        return faces
    totals = np.zeros(len(me.polygons), np.int32)
    me.polygons.foreach_get("loop_total", totals)
    loop_poly = np.repeat(np.arange(len(me.polygons)), totals)
    loop_vert = np.zeros(len(me.loops), np.int32)
    me.loops.foreach_get("vertex_index", loop_vert)
    faces = faces.copy()
    for _ in range(rings):
        marked = np.zeros(len(me.vertices), bool)
        marked[loop_vert[faces[loop_poly]]] = True
        touched = np.zeros(len(me.polygons), bool)
        np.logical_or.at(touched, loop_poly, marked[loop_vert])
        faces |= touched
    return faces


def select_only(objs, active):
    if bpy.context.object and bpy.context.object.mode != 'OBJECT':
        bpy.ops.object.mode_set(mode='OBJECT')
    for ob in bpy.context.view_layer.objects:
        ob.select_set(False)
    for ob in objs:
        ob.select_set(True)
    bpy.context.view_layer.objects.active = active


# ---------------------------------------------------------------- scene setup

def collect_objects(cfg, rep):
    arm = bpy.data.objects.get(cfg["armature"])
    if arm is None or arm.type != 'ARMATURE':
        raise RuntimeError(f"Armature {cfg['armature']!r} not found")

    suffix = cfg.get("work_suffix", "_WORK")
    exclude = cfg.get("exclude", [])
    meshes = [o for o in bpy.data.objects
              if o.type == 'MESH' and o.parent == arm and not any(fnmatch.fnmatch(o.name, p) for p in exclude)]
    names = {o.name for o in meshes}

    #A "<name>_WORK" object replaces "<name>"
    sources, stale = [], []
    for o in meshes:
        if o.name.endswith(suffix):
            sources.append((o, o.name[:-len(suffix)]))
        elif o.name + suffix in names:
            stale.append(o)
        else:
            sources.append((o, o.name))
    for o in stale:
        log(f"Ignoring {o.name}, its source is {o.name}{suffix}")
        bpy.data.objects.remove(o, do_unlink=True)

    result = []
    for o, game_name in sources:
        o.name = game_name
        o.data = o.data.copy()  #Never change data other objects share
        #Free the name, else the copy gets "<name>.001" and Toolbox can't match the mesh
        existing = bpy.data.meshes.get(game_name)
        if existing is not None and existing != o.data:
            existing.name = game_name + "_prep_original"
        o.data.name = game_name
        if o.name != game_name or o.data.name != game_name:
            raise RuntimeError(f"Could not name {o.name} / {o.data.name} {game_name}")
        o.hide_set(False)
        o.hide_viewport = False
        o.hide_render = False  #Baking needs it
        result.append(o)
    arm.hide_set(False)
    arm.hide_viewport = False
    return arm, result


def _corner_points(o):
    """World positions of a mesh's vertices and, per corner, its vertex."""
    me = o.data
    co = np.zeros(len(me.vertices) * 3)
    me.vertices.foreach_get("co", co)
    co = co.reshape(-1, 3) @ np.array(o.matrix_world.to_3x3()).T + np.array(o.matrix_world.translation)
    loop_vert = np.zeros(len(me.loops), np.int32)
    me.loops.foreach_get("vertex_index", loop_vert)
    return co, loop_vert


def _nearest(points, targets):
    """Index of the nearest target for each point."""
    out = np.zeros(len(points), np.int64)
    for i in range(0, len(points), 512):
        d = ((points[i:i + 512, None, :] - targets[None, :, :]) ** 2).sum(-1)
        out[i:i + 512] = d.argmin(1)
    return out


def match_layers(extra, target, rep):
    """Gives a mesh that is joined into a game mesh the target's UV maps and colour layers, so the join keeps
    them: UV maps by position (renamed), a missing colour layer filled from the nearest vertex of the target
    (game meshes keep data in vertex colours, e.g. hair shading), layers the target doesn't have removed."""
    me, tme = extra.data, target.data
    for i, uv in enumerate(list(tme.uv_layers)):
        if i < len(me.uv_layers):
            me.uv_layers[i].name = uv.name
        else:
            me.uv_layers.new(name=uv.name)
            rep.warn(f"{extra.name}: has no UV map {uv.name}; its faces get UV 0,0 there")
    for uv in list(me.uv_layers)[len(tme.uv_layers):]:
        me.uv_layers.remove(uv)

    co, loop_vert = _corner_points(extra)
    tco, tloop_vert = _corner_points(target)
    for ca in list(tme.color_attributes):
        if me.color_attributes.get(ca.name) is not None:
            continue
        prop = "color_srgb" if ca.data_type == 'BYTE_COLOR' else "color"
        src = np.zeros(len(ca.data) * 4, np.float32)
        ca.data.foreach_get(prop, src)
        src = src.reshape(-1, 4)
        new = me.color_attributes.new(ca.name, ca.data_type, ca.domain)
        if ca.domain == 'CORNER':
            pick = _nearest(co[loop_vert], tco[tloop_vert])
        else:
            pick = _nearest(co, tco)
        new.data.foreach_set(prop, src[pick].ravel())
        log(f"{extra.name}: colour layer {ca.name} filled from the nearest vertices of {target.name}")
    names = {ca.name for ca in tme.color_attributes}
    for ca in [ca for ca in me.color_attributes if ca.name not in names]:
        log(f"{extra.name}: removed colour layer {ca.name} ({target.name} has none of that name)")
        me.color_attributes.remove(ca)


def _world_points(o, limit=None):
    co, _ = _corner_points(o)
    if limit and len(co) > limit:
        co = co[np.linspace(0, len(co) - 1, limit).astype(int)]
    return co


def _mean_distance(points, target):
    """Mean distance from points to the nearest vertices of target."""
    from mathutils import kdtree
    co = _world_points(target)
    tree = kdtree.KDTree(len(co))
    for i, p in enumerate(co):
        tree.insert(p, i)
    tree.balance()
    return float(np.mean([tree.find(p)[2] for p in points])) if len(points) else math.inf


def _bones_used(o, arm):
    bones = {b.name for b in arm.data.bones}
    names = {g.index: g.name for g in o.vertex_groups}
    return {names[g.group] for v in o.data.vertices for g in v.groups if g.weight > 0 and names[g.group] in bones}


def _surface(o):
    """(3D area in world units, UV area of its first UV map) of a mesh."""
    me = o.data
    co, _ = _corner_points(o)
    tris, _ = triangles(me)
    loop_vert = np.zeros(len(me.loops), np.int32)
    me.loops.foreach_get("vertex_index", loop_vert)
    area3d = float(triangle_areas(co[loop_vert[tris]]).sum()) if len(tris) else 0.0
    if not me.uv_layers:
        return area3d, 0.0
    uv = mesh_uv(me, me.uv_layers[0].name)
    return area3d, float(triangle_areas(uv[tris]).sum()) if len(tris) else 0.0


def _resolution_estimate(new, target, games, vanilla_meshes, textures, size=256, block=8):
    """Roughly the share of the target's texel density the new mesh would get if baked into the free space of the
    target's textures (as bake_group's packing reports it): the texture's wholly free blocks (scattered gaps
    between islands don't take new ones) against the new mesh's area at that density, at half packing
    efficiency."""
    group_of = texture_groups(textures)
    key = group_of(vanilla_meshes[target.name]["material"])
    covered = np.zeros((size, size), bool)
    for g in games:
        if group_of(vanilla_meshes[g.name]["material"]) == key and g.data.uv_layers:
            tris, _ = triangles(g.data)
            covered |= raster_triangles(mesh_uv(g.data, g.data.uv_layers[0].name), tris, size, size) > 0
    covered = dilate(covered, 1)
    free = 1.0 - covered.reshape(size // block, block, size // block, block).any((1, 3)).mean()
    area3d, area_uv = _surface(target)
    needed = _surface(new)[0] * (area_uv / area3d if area3d else 0.0)
    return min(1.0, math.sqrt(0.5 * free / needed)) if needed > 0 else 1.0


def join_new_meshes(objects, vanilla_meshes, textures, arm, cfg, rep):
    """The model is edited in place, so a mesh the game model doesn't have can't be added: it is joined into a
    game mesh, chosen as
      * the one prep.join {"<new mesh>": "<game mesh>"} names, else
      * the one that uses its material (the nearest of several), else (a new material)
      * among the game meshes with textures of this model's own to bake into, within three times the distance of
        the nearest (or 25 cm more; the new mesh keeps its own bone weights either way): the nearest whose
        textures have room for it at half their texel density or better (else the one with the most), skinned
        when the new mesh follows more than one bone.
    Its faces on other materials than the target's are then baked into the target's textures, all of them (their
    UVs mean nothing there). Returns (the remaining objects, {target: index of its first joined face})."""
    rules = cfg.get("join", {})
    games = [o for o in objects if o.name in vanilla_meshes]
    by_material = {}
    for o in games:
        by_material.setdefault(vanilla_meshes[o.name]["material"], []).append(o)
    by_name = {o.name: o for o in objects}
    joins = {}
    for o in objects:
        if o.name in vanilla_meshes:
            continue
        target = rules.get(o.name)
        if target is None:
            used = {o.material_slots[i].material.name for i in np.unique(material_indices(o.data))
                    if i < len(o.material_slots) and o.material_slots[i].material}
            candidates = sorted({t for m in used for t in by_material.get(plain_name(m), [])}, key=lambda t: t.name)
            points = _world_points(o, 2000)
            how = "uses its material"
            if not candidates:
                candidates = [g for g in games if textures.owned(vanilla_meshes[g.name]["material"])
                              and g.data.uv_layers]
                how = "is the nearest with textures to bake into"
            if not candidates:
                rep.error(f"{o.name} is not a mesh of the game model and no game mesh can take it. Join it into a "
                          f"game mesh in Blender, or name one: \"prep\": {{\"join\": {{\"{o.name}\": \"<game mesh>\"}}}}")
                continue
            dist = {g.name: _mean_distance(points, g) for g in candidates} if len(candidates) > 1 else {}
            candidates.sort(key=lambda g: dist.get(g.name, 0.0))
            if how.startswith("is the nearest"):
                bends = len(_bones_used(o, arm)) > 1
                d0 = dist.get(candidates[0].name, 0.0)
                near = [g for g in candidates if dist.get(g.name, 0.0) <= max(3 * d0, d0 + 0.25)]
                if bends and any(vanilla_meshes[g.name]["skinCount"] >= 2 for g in near):
                    near = [g for g in near if vanilla_meshes[g.name]["skinCount"] >= 2]
                room = {g.name: _resolution_estimate(o, g, games, vanilla_meshes, textures) for g in near}
                roomy = [g for g in near if room[g.name] >= 0.5]
                best = roomy[0] if roomy else max(near, key=lambda g: room[g.name])
                how = (f"is the nearest with room in its textures to bake into (about {room[best.name] * 100:.0f}% "
                       f"of their resolution; others near: "
                       + (", ".join(f"{g.name} {dist.get(g.name, 0.0):.3f} away, {room[g.name] * 100:.0f}%"
                                    for g in near if g is not best) or "none") + ")")
                if bends and vanilla_meshes[best.name]["skinCount"] < 2:
                    rep.warn(f"{o.name} follows several bones, but joins {best.name}, which follows one: it won't bend")
            else:
                best = candidates[0]
            target = best.name
            log(f"{o.name} is not a mesh of the game model: it joins {target}"
                + (f" ({dist[target]:.3f} away)" if target in dist else "") + f", which {how}")
        if target not in by_name or target not in vanilla_meshes:
            rep.error(f"{o.name}: join target {target} is not a game mesh in the scene")
            continue
        joins.setdefault(target, []).append(o)

    # The join deletes the joined objects: decide what stays before it.
    gone = {id(e) for extras in joins.values() for e in extras}
    kept = [o for o in objects if id(o) not in gone]
    first_joined = {}
    for target, extras in joins.items():
        t = by_name[target]
        names = [e.name for e in extras]
        for e in extras:
            match_layers(e, t, rep)
        first_joined[target] = len(t.data.polygons)
        select_only([t] + extras, t)
        bpy.ops.object.join()
        log(f"Joined {names} into {target}")
        rep.meshes.setdefault(target, {})["joined"] = names
    return kept, first_joined


LOD_SAME = 0.95  # a game LOD with at least this share of LOD 0's triangles gets the more detailed version


def make_lods(objects, vanilla_meshes, cfg, rep):
    """Lower LODs of each mesh, in the proportions of the game mesh's own LODs: "<mesh>.lod<n>" copies, decimated
    (collapse, symmetric in X), with smooth normals. modstudio replace gives each LOD its version; a mesh it keeps
    as the game's ignores them. Returns the copies.
    Compared on the townsman's face and hair (LODs 3 and 4, a few hundred triangles): welding the copy first
    (prep.lod_weld, a distance) closes eyelids and lips into one surface that decimation then opens; without it
    and with symmetry the face stays closed and no seams opened."""
    made, report = [], {}
    for o in objects:
        vm = vanilla_meshes.get(o.name)
        counts = (vm or {}).get("lodTriangles") or []
        if len(counts) < 2 or not counts[0]:
            continue
        triangles = sum(len(p.vertices) - 2 for p in o.data.polygons)
        versions = []
        for n in range(1, len(counts)):
            ratio = counts[n] / counts[0]
            if ratio >= LOD_SAME:
                continue
            dup = o.copy()
            dup.data = o.data.copy()
            dup.name = dup.data.name = f"{o.name}.lod{n}"
            for c in o.users_collection:
                c.objects.link(dup)
            me = dup.data
            if getattr(me, "has_custom_normals", False):
                with bpy.context.temp_override(object=dup, active_object=dup):
                    bpy.ops.mesh.customdata_custom_splitnormals_clear()
            me.polygons.foreach_set("use_smooth", np.ones(len(me.polygons), bool))
            if "sharp_face" in me.attributes:
                me.attributes.remove(me.attributes["sharp_face"])
            dec = dup.modifiers.new("PREP_decimate", 'DECIMATE')
            dec.decimate_type = 'COLLAPSE'
            dec.ratio = ratio
            dec.use_collapse_triangulate = True
            dec.use_symmetry = cfg.get("lod_symmetry", True)
            dec.symmetry_axis = 'X'
            with bpy.context.temp_override(object=dup):
                bpy.ops.object.modifier_move_to_index(modifier=dec.name, index=0)
            weld_distance = cfg.get("lod_weld", 0)
            if weld_distance:
                weld = dup.modifiers.new("PREP_weld", 'WELD')
                weld.merge_threshold = weld_distance
                with bpy.context.temp_override(object=dup):
                    bpy.ops.object.modifier_move_to_index(modifier=weld.name, index=0)
            made.append(dup)
            versions.append(f"{n}: {round(triangles * ratio)}")
        if versions:
            report[o.name] = versions
            rep.meshes.setdefault(o.name, {})["lods"] = versions
    if report:
        log(f"Lower LODs (decimated to the game's proportions, triangles): {report}")
    return made


def limit_weights(o, arm, max_weights):
    bones = {b.name for b in arm.data.bones}
    names = {g.index: g.name for g in o.vertex_groups}
    groups = {g.index: g for g in o.vertex_groups}
    trimmed, largest = 0, 0.0
    for v in o.data.vertices:
        ws = sorted(((g.weight, g.group) for g in v.groups if names[g.group] in bones and g.weight > 0), reverse=True)
        if len(ws) > max_weights:
            trimmed += 1
            largest = max(largest, ws[max_weights][0])
            for _, gi in ws[max_weights:]:
                groups[gi].remove([v.index])
        keep = ws[:max_weights]
        total = sum(w for w, _ in keep)
        if total > 0 and abs(total - 1.0) > 1e-4:
            for w, gi in keep:
                groups[gi].add([v.index], w / total, 'REPLACE')
    unweighted = sum(1 for v in o.data.vertices if not any(names[g.group] in bones and g.weight > 0 for g in v.groups))
    return trimmed, largest, unweighted


# ---------------------------------------------------------------- textures

KIND = re.compile(r"_([A-Za-z]+\d?)(?:\.\d+)?$")


def texture_kind(name):
    m = KIND.search(name)
    return m.group(1) if m else ""


# Roles model.json's material looks give, over the names: a texture named "..._Alb" on a mask sampler (eye
# shadows bind theirs to _gn3) is no colour. Filled by learn_roles().
LOOK_ROLES = {}
# Colour textures whose alpha isn't opacity (only opaque materials use them: the Zoras' skin keeps other data
# there), and colour textures the game tints (grey ones: a baked colour would be tinted again).
DATA_ALPHA = set()
TINTED = set()


def learn_roles(model):
    """Textures named like colour textures that no material's look draws as colour count as "aux"; colour
    textures of opaque materials keep their alpha as data. From model.json's material looks."""
    colours = set()
    named = set()
    opaque, see_through = set(), set()
    for m in model["materials"]:
        look = m.get("look")
        if look is None:
            return  # an older model.json: the names decide
        colours.update(look.get(k) for k in ("texture", "iris", "sclera") if look.get(k))
        named.update(t for t in m["textures"] if texture_kind(t) == "Alb")
        if look.get("texture"):
            mode = (m.get("alpha") or {}).get("mode")
            (opaque if mode == "opaque" else see_through).add(look["texture"])
            if m.get("tint"):
                TINTED.add(look["texture"])
    LOOK_ROLES.update({t: "aux" for t in named - colours})
    DATA_ALPHA.update(opaque - see_through)


def texture_role(name, roles):
    if name in roles:
        return roles[name]
    if name in LOOK_ROLES:
        return LOOK_ROLES[name]
    return {"Alb": "color", "Nrm": "normal"}.get(texture_kind(name), "aux")


def plain_name(name):
    """Blender's duplicate suffix removed: "Mt_Skin.001" -> "Mt_Skin"."""
    return re.sub(r"\.\d{3}$", "", name)


def load_image(path, colorspace):
    img = bpy.data.images.load(path, check_existing=False)
    img.colorspace_settings.name = colorspace
    # colour and alpha apart: alpha may be data (the Zoras' skin), and premultiplying blackens the colour there
    img.alpha_mode = 'CHANNEL_PACKED'
    return img


def image_pixels(img):
    w, h = img.size
    px = np.zeros(w * h * 4, np.float32)
    img.pixels.foreach_get(px)
    return px.reshape(h, w, 4)


def save_png(px, path, colorspace):
    h, w = px.shape[:2]
    img = bpy.data.images.new("PREP_save", w, h, alpha=True, float_buffer=False)
    img.colorspace_settings.name = colorspace
    img.pixels.foreach_set(px.ravel())
    img.filepath_raw = path
    img.file_format = 'PNG'
    img.save()
    bpy.data.images.remove(img)


def match_channel_layout(src, out):
    """Channels that are constant or a copy of red in the original stay that way."""
    s = src.reshape(-1, 4)
    o = out.reshape(-1, 4)
    for c in (1, 2, 3):
        if np.abs(s[:, c] - s[:, 0]).max() <= 1.5 / 255:
            o[:, c] = o[:, 0]
        elif np.ptp(s[:, c]) <= 1.5 / 255:
            o[:, c] = s[0, c]


class Textures:
    """The model's own textures (named after the model or the material: shared CmnTex_* and detail textures
    are never changed). Each starts from the game's PNG, or from the scene's image if it was painted on."""

    def __init__(self, cfg, model, rep):
        self.src = cfg["source_textures"]
        self.out = os.path.join(cfg["output"], "textures")
        self.model_name = model["name"]
        self.materials = {m["name"]: m for m in model["materials"]}
        self.rep = rep
        self.painted = {}

    def owned(self, material):
        m = self.materials.get(material)
        if not m:
            return []
        return [t for t in m["textures"] if self.model_name in t or t.startswith(material + "_")]

    def all_owned(self):
        return sorted({t for name in self.materials for t in self.owned(name)})

    @staticmethod
    def colorspace(name, roles=None):
        return 'sRGB' if texture_role(name, roles or {}) == "color" else 'Non-Color'

    def original(self, name):
        return os.path.join(self.src, name + ".png")

    def path(self, name):
        return self.painted.get(name) or self.original(name)

    def exists(self, name):
        return os.path.exists(self.path(name))

    def load(self, name, roles=None):
        return load_image(self.path(name), self.colorspace(name, roles))

    def find_painted(self):
        """Scene images of the model's textures that differ from the game's are used for the mod."""
        def stems(img):
            yield plain_name(img.name)
            yield os.path.splitext(img.name)[0]
            if img.filepath:
                yield os.path.splitext(os.path.basename(bpy.path.abspath(img.filepath)))[0]

        images = [i for i in bpy.data.images if i.source in ('FILE', 'GENERATED') and not i.name.startswith("PREP_")]
        for name in self.all_owned():
            orig_path = self.original(name)
            candidates = [i for i in images if name in set(stems(i))]
            if not candidates or not os.path.exists(orig_path):
                continue
            ref_img = load_image(orig_path, self.colorspace(name))
            ref = image_pixels(ref_img)
            bpy.data.images.remove(ref_img)
            for img in candidates:
                if img.size[0] == 0:
                    continue  #Its file is missing
                px = image_pixels(img)
                if px.shape == ref.shape:
                    changed = float((np.abs(px - ref).max(-1) > 0.5 / 255).mean())
                    # Under 0.01% is two decoders disagreeing on a block or two, not painting.
                    if changed < 1e-4:
                        continue
                    note = f"{changed * 100:.2f}% of pixels"
                else:
                    note = f"resized {ref.shape[1]}x{ref.shape[0]} -> {px.shape[1]}x{px.shape[0]}"
                    self.rep.warn(f"texture {name} was resized in the scene ({note})")
                path = os.path.join(self.out, name + ".png")
                save_png(px, path, self.colorspace(name))
                self.painted[name] = path
                self.rep.painted.append({"name": name, "image": img.name, "change": note})
                self.rep.textures.append({"name": name, "role": "painted", "path": path})
                log(f"Texture {name} was painted on in the scene (image {img.name}, {note}): using it")
                break


def texture_groups(textures):
    """Materials that share any of the model's own textures share their UV space: one group."""
    parent = {}

    def find(x):
        parent.setdefault(x, x)
        while parent[x] != x:
            parent[x] = parent[parent[x]]
            x = parent[x]
        return x

    for name in textures.materials:
        find(name)
        for t in textures.owned(name):
            parent[find(name)] = find("tex:" + t)
    return find


# ---------------------------------------------------------------- bake helpers

BAKE_PASS = {"color": dict(type='DIFFUSE', pass_filter={'COLOR'}),
             "normal": dict(type='NORMAL', normal_space='TANGENT', normal_r='POS_X', normal_g='POS_Y', normal_b='POS_Z'),
             "aux": dict(type='EMIT')}


class Temp:
    """Materials and images made for baking, removed afterwards."""

    def __init__(self):
        self.mats, self.imgs = [], []

    def material(self, name):
        m = bpy.data.materials.new(name)
        m.use_nodes = True
        self.mats.append(m)
        return m

    def image(self, name, w, h, colorspace, float_buffer=False):
        img = bpy.data.images.new(name, w, h, alpha=True, float_buffer=float_buffer)
        img.colorspace_settings.name = colorspace
        self.imgs.append(img)
        return img

    def keep_image(self, img):
        self.imgs.append(img)
        return img

    def clear(self):
        for m in self.mats:
            bpy.data.materials.remove(m)
        for img in self.imgs:
            bpy.data.images.remove(img)
        self.mats, self.imgs = [], []


def set_target(m, img):
    nt = m.node_tree
    n = nt.nodes.get("PREP_target") or nt.nodes.new("ShaderNodeTexImage")
    n.name = "PREP_target"
    n.image = img
    for other in nt.nodes:
        other.select = False
    n.select = True
    nt.nodes.active = n


#Principled inputs that make Cycles' diffuse colour pass differ from the base colour (metallic 1, as the FBX
#import sets, bakes black; the default specular level darkens by 4%) or that the game's textures don't hold
BAKE_NEUTRAL = {"Alpha": 1.0, "Metallic": 0.0, "Transmission Weight": 0.0, "Subsurface Weight": 0.0,
                "Specular IOR Level": 0.0, "Coat Weight": 0.0, "Sheen Weight": 0.0, "Emission Strength": 0.0}


def bake_copy(m, temp):
    """A copy of a material to bake from, whose diffuse colour pass is exactly its base colour."""
    c = m.copy() if m else temp.material("PREP_blank")
    if m:
        temp.mats.append(c)
    c.use_nodes = True
    for n in c.node_tree.nodes:
        if n.type != 'BSDF_PRINCIPLED':
            continue
        for name, value in BAKE_NEUTRAL.items():
            if name in n.inputs:
                for link in list(n.inputs[name].links):
                    c.node_tree.links.remove(link)
                n.inputs[name].default_value = value
    return c


def opacity_copy(m, temp):
    """A copy of a material whose emission is its opacity, mixed the way its shaders are: a Principled BSDF
    gives its Alpha, a Transparent BSDF 0, any other shader 1."""
    c = m.copy() if m else temp.material("PREP_blank")
    if m:
        temp.mats.append(c)
    c.use_nodes = True
    nt = c.node_tree
    for n in list(nt.nodes):
        if n.type in ('MIX_SHADER', 'ADD_SHADER', 'OUTPUT_MATERIAL') or not n.outputs:
            continue
        if n.outputs[0].type != 'SHADER':
            continue
        em = nt.nodes.new("ShaderNodeEmission")
        if n.type == 'BSDF_PRINCIPLED' and n.inputs["Alpha"].is_linked:
            nt.links.new(n.inputs["Alpha"].links[0].from_socket, em.inputs["Color"])
        else:
            if n.type == 'BSDF_PRINCIPLED':
                value = n.inputs["Alpha"].default_value
            else:
                value = 0.0 if n.type == 'BSDF_TRANSPARENT' else 1.0
            em.inputs["Color"].default_value = (value, value, value, 1.0)
        for link in list(n.outputs[0].links):
            nt.links.new(em.outputs[0], link.to_socket)
        nt.nodes.remove(n)
    return c


def source_material(temp, me, src_img, settings, default_uv, constant, channel, rep, mesh_name):
    """Emission of the original texture sampled through a UV map (the face's original UVs unless a merge rule
    names a source UV map), blended with `constant` by a weight attribute if one is given."""
    m = temp.material("PREP_source")
    nt = m.node_tree
    for n in list(nt.nodes):
        if n.type != 'OUTPUT_MATERIAL':
            nt.nodes.remove(n)
    out = next(n for n in nt.nodes if n.type == 'OUTPUT_MATERIAL')
    em = nt.nodes.new("ShaderNodeEmission")
    nt.links.new(em.outputs[0], out.inputs["Surface"])
    source_uv = settings.get("source_uv") or (default_uv if settings.get("source") != "median" else None)
    if source_uv and source_uv not in me.uv_layers:
        rep.warn(f"{mesh_name}: source UV {source_uv} not found, using the median")
        source_uv = None
    if not source_uv:
        em.inputs["Color"].default_value = (*constant[:3], 1.0)
        return m
    uvn = nt.nodes.new("ShaderNodeUVMap")
    uvn.uv_map = source_uv
    tex = nt.nodes.new("ShaderNodeTexImage")
    tex.image = src_img
    tex.interpolation = 'Closest' if settings.get("closest") else 'Linear'
    nt.links.new(uvn.outputs["UV"], tex.inputs["Vector"])
    sampled = tex.outputs["Alpha" if channel == "alpha" else "Color"]
    weight = settings.get("source_weight")
    if weight:
        mix = nt.nodes.new("ShaderNodeMix")
        mix.data_type = 'RGBA'
        mix.clamp_factor = True
        mix.inputs[6].default_value = (*constant[:3], 1.0)
        nt.links.new(sampled, mix.inputs[7])
        att = nt.nodes.new("ShaderNodeAttribute")
        att.attribute_name = weight
        nt.links.new(att.outputs["Fac"], mix.inputs[0])
        nt.links.new(mix.outputs[2], em.inputs["Color"])
    else:
        nt.links.new(sampled, em.inputs["Color"])
    return m


@contextlib.contextmanager
def face_materials(o, faces, make, dummy):
    """While active: the faces in `faces` bake with make(slot index, slot material); all others with `dummy`."""
    me = o.data
    idx = material_indices(me)
    orig = list(me.materials)
    used = set(np.unique(idx[faces]).tolist())
    try:
        for i, m in enumerate(orig):
            me.materials[i] = make(i, m) if i in used else dummy
        me.materials.append(dummy)
        moved = idx.copy()
        moved[~faces] = len(orig)
        me.polygons.foreach_set("material_index", moved)
        me.update()
        yield
    finally:
        me.materials.pop(index=len(me.materials) - 1)
        for i, m in enumerate(orig):
            me.materials[i] = m
        me.polygons.foreach_set("material_index", idx)
        me.update()


def use_cycles(samples):
    scene = bpy.context.scene
    scene.render.engine = 'CYCLES'
    scene.cycles.samples = samples
    scene.cycles.device = 'CPU'
    scene.render.bake.use_selected_to_active = False


def run_bake(objs, role, uv_layer, margin):
    select_only(objs, objs[0])
    bpy.ops.object.bake(margin=margin, margin_type='EXTEND', use_clear=False, target='IMAGE_TEXTURES',
                        uv_layer=uv_layer, **BAKE_PASS[role])


def reference_material(game_material, textures, temp):
    """How the game material looks in the scene: the scene's own material of that name, else one built from
    the textures the way make_scene.py imports it (colour and RG normal map)."""
    m = bpy.data.materials.get(game_material)
    if m is not None and m.use_nodes:
        return m
    m = temp.material("PREP_reference_" + game_material)
    nt = m.node_tree
    bsdf = next(n for n in nt.nodes if n.type == 'BSDF_PRINCIPLED')
    owned = textures.owned(game_material)
    alb = next((t for t in owned if texture_role(t, {}) == "color" and textures.exists(t)), None)
    nrm = next((t for t in owned if texture_role(t, {}) == "normal" and textures.exists(t)), None)
    if alb:
        tex = nt.nodes.new("ShaderNodeTexImage")
        tex.image = temp.keep_image(textures.load(alb))
        nt.links.new(tex.outputs["Color"], bsdf.inputs["Base Color"])
        nt.links.new(tex.outputs["Alpha"], bsdf.inputs["Alpha"])
    if nrm:
        tex = nt.nodes.new("ShaderNodeTexImage")
        tex.image = temp.keep_image(textures.load(nrm))
        grp = nt.nodes.new("ShaderNodeGroup")
        grp.node_tree = rg_normal_group()
        nm = nt.nodes.new("ShaderNodeNormalMap")
        nt.links.new(tex.outputs["Color"], grp.inputs[0])
        nt.links.new(grp.outputs[0], nm.inputs["Color"])
        nt.links.new(nm.outputs["Normal"], bsdf.inputs["Normal"])
    return m


def srgb(x):
    x = np.clip(x, 0.0, 1.0)
    return np.where(x <= 0.0031308, 12.92 * x, 1.055 * np.power(x, 1 / 2.4) - 0.055)


# ---------------------------------------------------------------- 1. which faces change

def detect_changes(o, info, textures, cfg, rep):
    """Bakes the custom-material faces with their material and with the game material into a scratch layout.
    Returns the faces whose diffuse colour or normal differs."""
    me = o.data
    faces = info["custom"]
    size = cfg.get("probe_size", 2048)
    temp = Temp()
    t0 = time.time()
    use_cycles(1)
    me.uv_layers.active = me.uv_layers[info["uv"]]
    probe = me.uv_layers.new(name=PROBE)
    try:
        me.uv_layers.active = probe
        select_only([o], o)
        bpy.ops.object.mode_set(mode='EDIT')
        bpy.context.tool_settings.mesh_select_mode = (False, False, True)
        bpy.ops.mesh.select_all(action='DESELECT')
        bm = bmesh.from_edit_mesh(me)
        bm.faces.ensure_lookup_table()
        for i in np.flatnonzero(faces):
            bm.faces[i].select_set(True)
        bmesh.update_edit_mesh(me)
        bpy.ops.uv.smart_project(angle_limit=math.radians(66), island_margin=0.002, area_weight=0.0,
                                 correct_aspect=True, scale_to_bounds=True)
        bpy.ops.object.mode_set(mode='OBJECT')

        dummy = temp.material("PREP_dummy")
        set_target(dummy, temp.image("PREP_dummy", 16, 16, 'Non-Color'))
        reference = reference_material(info["game_material"], textures, temp)
        result = {}
        for role, copy, kind in (("color", bake_copy, "color"), ("normal", bake_copy, "normal"),
                                 ("opacity", opacity_copy, "aux")):
            own, ref = {}, copy(reference, temp)
            for side in ("own", "game"):
                img = temp.image(f"PREP_probe_{role}_{side}", size, size, 'Non-Color', float_buffer=True)

                def make(i, m, side=side, img=img, own=own, ref=ref, copy=copy):
                    if side == "game":
                        set_target(ref, img)
                        return ref
                    if i not in own:
                        own[i] = copy(m, temp)
                    set_target(own[i], img)
                    return own[i]

                with face_materials(o, faces, make, dummy):
                    run_bake([o], kind, PROBE, 0)
                result[role, side] = image_pixels(img)

        def diff(role):
            return np.abs(result[role, "own"][..., :3] - result[role, "game"][..., :3]).max(-1)

        diff_color = np.abs(srgb(result["color", "own"][..., :3]) - srgb(result["color", "game"][..., :3])).max(-1)
        score = np.maximum.reduce([diff_color / cfg.get("color_threshold", 3 / 255),
                                   diff("normal") / cfg.get("normal_threshold", 0.02),
                                   diff("opacity") / cfg.get("opacity_threshold", 3 / 255)])

        tris, tri_poly = triangles(me)
        keep = faces[tri_poly]
        tris, tri_poly = tris[keep], tri_poly[keep]
        grid, centres = raster_ids(mesh_uv(me, PROBE), tris, size, size)
        tri_score = score[centres[:, 0], centres[:, 1]].copy()
        covered = grid >= 0
        np.maximum.at(tri_score, grid[covered], score[covered])
        poly_score = np.zeros(len(me.polygons))
        np.maximum.at(poly_score, tri_poly, tri_score)
        changed = poly_score > 1.0
    finally:
        if bpy.context.object and bpy.context.object.mode != 'OBJECT':
            bpy.ops.object.mode_set(mode='OBJECT')
        me.uv_layers.remove(me.uv_layers[PROBE])
        me.uv_layers.active = me.uv_layers[info["uv"]]
        temp.clear()
    log(f"{o.name}: {int(changed.sum())} of {int(faces.sum())} faces on custom materials look different from "
        f"{info['game_material']} ({time.time() - t0:.1f}s)")
    return changed


# ---------------------------------------------------------------- 2. new layout

def _raster_pixels(pts, w, h):
    """Pixels of a (w, h) grid covered by triangles given in pixel units, plus the pixel of each triangle's
    centre (so that slivers count)."""
    grid = np.zeros((h, w), bool)
    for tri in pts:
        r = _triangle_pixels(tri, w, h)
        if r:
            y0, x0, inside = r
            grid[y0:y0 + inside.shape[0], x0:x0 + inside.shape[1]] |= inside
    c = pts.mean(1).astype(int)
    grid[np.clip(c[:, 1], 0, h - 1), np.clip(c[:, 0], 0, w - 1)] = True
    return grid


def place_islands(islands, occupied, margin_px, scale):
    """Places islands (UV triangles, (n, 3, 2) each) scaled by `scale` into the free pixels of `occupied`
    (a square grid, True = taken), largest first, each at the lowest free position, turned 0 or 90 degrees.
    Returns per island (turned, offset in UV units), or None if they don't all fit."""
    size = occupied.shape[0]
    occ = occupied.copy()
    occ_fft = np.fft.rfft2(occ.astype(np.float32))
    areas = [triangle_areas(t).sum() for t in islands]
    placed = [None] * len(islands)
    for k in sorted(range(len(islands)), key=lambda k: -areas[k]):
        best = None
        for turned in (False, True):
            pts = islands[k] * (scale * size)
            if turned:
                pts = np.stack([pts[..., 1], -pts[..., 0]], -1)
            low = pts.reshape(-1, 2).min(0)
            pts = pts - low
            w, h = (int(np.ceil(v)) + 1 for v in pts.reshape(-1, 2).max(0))
            if w > size or h > size:
                continue
            mask = _raster_pixels(pts, w, h)
            pad = np.zeros((size, size), np.float32)
            pad[:h, :w] = mask
            #hits[y, x]: taken pixels under the island placed at (x, y)
            hits = np.fft.irfft2(occ_fft * np.conj(np.fft.rfft2(pad)), s=(size, size))[:size - h + 1, :size - w + 1]
            ys, xs = np.nonzero(hits < 0.5)
            if len(ys) and (best is None or ys[0] * size + xs[0] < best[0]):
                best = (ys[0] * size + xs[0], turned, xs[0], ys[0], mask, low)
        if best is None:
            return None
        _, turned, x, y, mask, low = best
        grown = dilate(np.pad(mask, margin_px), margin_px)
        y0, x0 = y - margin_px, x - margin_px
        ys0, xs0 = max(0, -y0), max(0, -x0)
        ys1, xs1 = min(grown.shape[0], size - y0), min(grown.shape[1], size - x0)
        occ[y0 + ys0:y0 + ys1, x0 + xs0:x0 + xs1] |= grown[ys0:ys1, xs0:xs1]
        occ_fft = np.fft.rfft2(occ.astype(np.float32))
        placed[k] = (turned, (np.array([x, y]) - low) / size)
    return placed


def pack_layout(objs, infos, cfg, rep):
    """Gives every object a LAYOUT UV map (a copy of its game UVs) and unwraps the moved faces into the
    space the other faces of all the objects leave free. Returns layout statistics.

    Blender unwraps the moved faces (one temporary mesh holding all of them). Its packer is not used: it
    ignores locked islands of other objects and overlapping (stacked or mirrored) ones, which most game
    models have. place_islands packs into a grid of the pixels the kept faces use instead."""
    t0 = time.time()
    size = cfg.get("pack_grid", 512)
    margin_px = max(1, int(np.ceil(cfg.get("pack_margin", 0.006) * size)))
    occupied = np.zeros((size * 4, size * 4), bool)
    verts, faces, uvs, back = [], [], [], []
    offset = 0
    for o in objs:
        me = o.data
        game = mesh_uv(me, infos[o]["uv"])
        moved = infos[o]["moved"] if infos[o]["moved"] is not None else np.zeros(len(me.polygons), bool)
        tris, tri_poly = triangles(me)
        occupied |= raster_triangles(game, tris[~moved[tri_poly]], size * 4, size * 4) > 0
        me.uv_layers.active = me.uv_layers[infos[o]["uv"]]
        me.uv_layers.active = me.uv_layers.new(name=LAYOUT)
        if not moved.any():
            continue
        co = np.zeros(len(me.vertices) * 3)
        me.vertices.foreach_get("co", co)
        mw = np.array(o.matrix_world)
        verts.append(co.reshape(-1, 3) @ mw[:3, :3].T + mw[:3, 3])
        loop_vert = np.zeros(len(me.loops), np.int32)
        me.loops.foreach_get("vertex_index", loop_vert)
        starts = np.zeros(len(me.polygons), np.int32)
        totals = np.zeros(len(me.polygons), np.int32)
        me.polygons.foreach_get("loop_start", starts)
        me.polygons.foreach_get("loop_total", totals)
        loops = np.concatenate([np.arange(s, s + t) for s, t in zip(starts[moved], totals[moved])])
        faces.extend((loop_vert[s:s + t] + offset).tolist() for s, t in zip(starts[moved], totals[moved]))
        uvs.append(game[loops])
        back.append((o, loops))
        offset += len(me.vertices)

    # The grid: pixels any kept face touches (found at 4x), grown by the margin, and the border
    taken = dilate(occupied.reshape(size, 4, size, 4).any(axis=(1, 3)), margin_px)
    taken[:margin_px] = taken[-margin_px:] = True
    taken[:, :margin_px] = taken[:, -margin_px:] = True

    # Unwrap
    from bpy_extras.mesh_utils import mesh_linked_uv_islands
    tmp_me = bpy.data.meshes.new("PREP_pack")
    tmp_me.from_pydata(np.vstack(verts).tolist(), [], faces)
    tmp_me.uv_layers.new(name=LAYOUT).data.foreach_set("uv", np.vstack(uvs).ravel())
    tmp = bpy.data.objects.new("PREP_pack", tmp_me)
    bpy.context.scene.collection.objects.link(tmp)
    try:
        select_only([tmp], tmp)
        bpy.ops.object.mode_set(mode='EDIT')
        bpy.context.tool_settings.mesh_select_mode = (False, False, True)
        bpy.ops.mesh.select_all(action='SELECT')
        bpy.ops.uv.smart_project(angle_limit=math.radians(66), island_margin=0.0, area_weight=0.0,
                                 correct_aspect=True, scale_to_bounds=False)
        bpy.ops.uv.average_islands_scale()
        bpy.ops.object.mode_set(mode='OBJECT')
        unwrapped = mesh_uv(tmp_me, LAYOUT)
        island_polys = mesh_linked_uv_islands(tmp_me)
        tris, tri_poly = triangles(tmp_me)
    finally:
        if bpy.context.object and bpy.context.object.mode != 'OBJECT':
            bpy.ops.object.mode_set(mode='OBJECT')
        bpy.data.objects.remove(tmp, do_unlink=True)
        bpy.data.meshes.remove(tmp_me)

    # Pack: about the largest scale at which all islands fit
    island_of = np.zeros(len(faces), np.int32)
    for k, polys in enumerate(island_polys):
        island_of[polys] = k
    island_tris = [tris[island_of[tri_poly] == k] for k in range(len(island_polys))]
    shapes = [unwrapped[t] for t in island_tris]
    area = sum(triangle_areas(s).sum() for s in shapes)
    scale = math.sqrt(cfg.get("pack_fill", 0.6) * (1.0 - taken.mean()) / area)
    tries = 0

    def attempt(s):
        nonlocal tries
        tries += 1
        return place_islands(shapes, taken, margin_px, s)

    # A scale that fits and one that doesn't: shrink from the estimate until they fit, or grow until they don't
    fits, fails = None, None
    result = attempt(scale)
    while result is None and tries < 25:
        fails, scale = scale, scale * 0.85
        result = attempt(scale)
    if result is None:
        raise RuntimeError("the moved faces don't fit into the texture's free space")
    fits = (scale, result)
    while fails is None and tries < 6:
        result = attempt(scale * 1.15)
        if result is None:
            fails = scale * 1.15
        else:
            scale *= 1.15
            fits = (scale, result)
    for _ in range(cfg.get("pack_refine", 3) if fails else 0):  #Then bisect between the two
        mid = math.sqrt(fits[0] * fails)
        result = attempt(mid)
        if result is None:
            fails = mid
        else:
            fits = (mid, result)
    scale, placed = fits
    packed = unwrapped.copy()
    for t, (turned, shift) in zip(island_tris, placed):
        sel = np.unique(t.ravel())
        p = unwrapped[sel] * scale
        if turned:
            p = np.stack([p[:, 1], -p[:, 0]], -1)
        packed[sel] = p + shift

    start = 0
    for o, loops in back:
        lay = mesh_uv(o.data, LAYOUT)
        lay[loops] = packed[start:start + len(loops)]
        o.data.uv_layers[LAYOUT].data.foreach_set("uv", lay.ravel())
        start += len(loops)

    log(f"   packed {len(island_polys)} islands at scale {scale:.3f} ({tries} tries) in {time.time() - t0:.1f}s")

    # Checks: nothing else moved, no overlaps, how much resolution the moved faces get
    moved_max, kept_tris, moved_tris = 0.0, [], []
    kept_density, moved_density = [0.0, 0.0], [0.0, 0.0]
    for o in objs:
        me = o.data
        game, lay = mesh_uv(me, infos[o]["uv"]), mesh_uv(me, LAYOUT)
        tris, tri_poly = triangles(me)
        moved = infos[o]["moved"] if infos[o]["moved"] is not None else np.zeros(len(me.polygons), bool)
        m = moved[tri_poly]
        if (~m).any():
            moved_max = max(moved_max, float(np.abs(lay[tris[~m]] - game[tris[~m]]).max()))
        kept_tris.append((lay, tris[~m]))
        moved_tris.append((lay, tris[m]))
        co = np.zeros(len(me.vertices) * 3)
        me.vertices.foreach_get("co", co)
        loop_vert = np.zeros(len(me.loops), np.int32)
        me.loops.foreach_get("vertex_index", loop_vert)
        world = triangle_areas(co.reshape(-1, 3)[loop_vert[tris]])
        uv_area = triangle_areas(lay[tris])
        kept_density[0] += uv_area[~m].sum(); kept_density[1] += world[~m].sum()
        moved_density[0] += uv_area[m].sum(); moved_density[1] += world[m].sum()

    def raster(pairs, n):
        grid = np.zeros((n, n), np.int32)
        for uv, tris in pairs:
            grid += raster_triangles(uv, tris, n, n)
        return grid

    g_kept, g_moved = raster(kept_tris, 1024), raster(moved_tris, 1024)
    resolution = None
    if kept_density[1] > 0 and moved_density[1] > 0 and kept_density[0] > 0:
        resolution = math.sqrt((moved_density[0] / moved_density[1]) / (kept_density[0] / kept_density[1]))
    stats = {"seconds": round(time.time() - t0, 1), "original_islands_moved": moved_max,
             "overlap_with_original_px": int(((g_kept > 0) & (g_moved > 0)).sum()),
             "new_self_overlap_px": int((g_moved > 1).sum()),
             "new_coverage": round(float((g_moved > 0).mean()), 3),
             "original_coverage": round(float((g_kept > 0).mean()), 3),
             "resolution_vs_original": round(resolution, 2) if resolution else None}
    return stats


# ---------------------------------------------------------------- 3. bake

def colour_guided(px, src_px, guide, cfg, name):
    """AO, roughness, thickness, ... of moved faces where their colour changed: the values the original texture
    has where its colour is similar (bare skin gets skin's values, not those of the leaves that were there).
    Where the colour didn't change, the values sampled where the face was stay; in between they blend."""
    h, w = px.shape[:2]

    def at_size(a):  #The guide is at the colour texture's size
        gh, gw = a.shape[:2]
        if (gh, gw) == (h, w):
            return a
        return a[(np.arange(h) * gh // h)[:, None], (np.arange(w) * gw // w)[None, :]]

    new, orig, src, keep, free = (at_size(guide[k]) for k in ("new", "orig", "src", "keep", "free"))
    levels = cfg.get("guide_levels", 16)

    def bins(colour):
        q = np.clip((colour[..., :3] * levels).astype(np.int32), 0, levels - 1)
        return (q[..., 0] * levels + q[..., 1]) * levels + q[..., 2]

    # Mean value per colour bin, over the kept texels (opaque ones, where the colour texture has alpha)
    sample = keep & (src[..., 3] > 0.5) if np.ptp(src[..., 3]) > 1.5 / 255 else keep
    if not sample.any() or not free.any():
        return px
    keys = bins(src[sample])
    count = np.bincount(keys, minlength=levels ** 3)
    table = np.stack([np.bincount(keys, src_px[sample][:, c], minlength=levels ** 3) for c in range(4)], -1)
    filled = np.flatnonzero(count)
    table[filled] /= count[filled, None]
    # Empty bins take the nearest filled one
    grid = np.stack(np.unravel_index(np.arange(levels ** 3), (levels,) * 3), -1).astype(np.float32)
    nearest = np.concatenate([filled[np.argmin(((g[:, None, :] - grid[filled][None, :, :]) ** 2).sum(-1), axis=1)]
                              for g in np.array_split(grid, max(1, len(grid) // 256))])
    table = table[nearest]

    change = np.abs(new[..., :3] - orig[..., :3]).max(-1)
    weight = np.clip((change - cfg.get("guide_from", 2 / 255)) / cfg.get("guide_range", 24 / 255), 0, 1) * free
    out = px.copy()
    sel = weight > 0
    out[sel] = px[sel] * (1 - weight[sel, None]) + table[bins(new[sel])] * weight[sel, None]
    log(f"   {name}: {sel.sum()} texels follow the colour change (from {int(sample.sum())} reference texels)")
    return out


def colour_versions(folder, base):
    """The other colour versions of "<base>.0" in folder: ["<base>.1", "<base>.2", ...]."""
    pattern = re.compile(re.escape(base) + r"\.(\d+)\.png", re.IGNORECASE)
    found = sorted(int(m.group(1)) for m in map(pattern.fullmatch, os.listdir(folder)) if m and m.group(1) != "0")
    return [f"{base}.{k}" for k in found]


@contextlib.contextmanager
def shown_as(texture, px):
    """Every scene image of `texture` (its PNG, also as ".001" copies) shows the pixels px for a while."""
    pattern = re.compile(re.escape(texture) + r"(\.png)?(\.\d{3})?", re.IGNORECASE)
    images = [img for img in bpy.data.images if not img.name.startswith("PREP_") and pattern.fullmatch(img.name)
              and tuple(img.size) == (px.shape[1], px.shape[0])]
    saved = [(img, image_pixels(img)) for img in images]
    try:
        for img in images:
            img.pixels.foreach_set(px.ravel())
            img.update()
        yield images
    finally:
        for img, old in saved:
            img.pixels.foreach_set(old.ravel())
            img.update()


def bake_group(objs, infos, textures, cfg, rep):
    """Moves the changed faces of a group of meshes that share textures into free texture space and bakes
    them into each texture."""
    names = [o.name for o in objs if infos[o]["moved"] is not None]
    log(f"Baking {names} into the textures shared by {sorted({infos[o]['game_material'] for o in objs})}")
    stats = pack_layout(objs, infos, cfg, rep)
    log(f"   new UV layout {stats}")
    if stats["original_islands_moved"] > 1e-6:
        rep.error(f"{names}: the original UV islands moved by {stats['original_islands_moved']}")
    if stats["overlap_with_original_px"] > 64:
        rep.warn(f"{names}: new UV islands overlap the original ones ({stats['overlap_with_original_px']} px at 1024x1024)")
    if stats["new_self_overlap_px"] > 0.01 * max(1, stats["new_coverage"] * 1024 * 1024):
        rep.warn(f"{names}: new UV islands overlap each other ({stats['new_self_overlap_px']} px at 1024x1024)")
    if stats["resolution_vs_original"] and stats["resolution_vs_original"] < cfg.get("min_resolution", 0.5):
        rep.warn(f"{names}: the texture has little free space; the baked faces get "
                 f"{stats['resolution_vs_original'] * 100:.0f}% of the original texture resolution")
    for n in names:
        rep.meshes.setdefault(n, {})["uv"] = stats

    movers = [o for o in objs if infos[o]["moved"] is not None]
    textures_of = {}
    for o in movers:
        rule = infos[o]["rule"] or {}
        textures_of[o] = rule.get("textures") or textures.owned(infos[o]["game_material"])
    roles = {}
    for o in movers:
        roles.update((infos[o]["rule"] or {}).get("roles", {}))
    #Colour first: the other textures follow its changes (see colour_guided)
    all_textures = sorted({t for ts in textures_of.values() for t in ts},
                          key=lambda t: (texture_role(t, roles) != "color", t))
    # "<texture>.0" is one of several colour versions (Tex/<texture>.1, .2, ...; the game picks one per
    # character, the scene shows 0). Every version gets the moved faces: after version 0, each is baked with the
    # scene's images of version 0 showing that version, so a material that mixes in the original colour gets
    # each version's. (A painted version 0 changes version 0 only.)
    tasks = []
    for t in all_textures:
        tasks.append((t, None))
        m = re.fullmatch(r"(.*)\.0", t)
        if m:
            found = colour_versions(textures.src, m.group(1))
            if not found:
                rep.error(f"{t} has colour versions, but they aren't in {textures.src} (the build decodes them); "
                          f"the others wouldn't get the baked faces")
            tasks += [(v, t) for v in found]
    guide = None

    # Faces whose texels stay: every face of the group that isn't moved, in the layout (= game UVs)
    def tris_of(o, moved_side):
        me = o.data
        tris, tri_poly = triangles(me)
        moved = infos[o]["moved"] if infos[o]["moved"] is not None else np.zeros(len(me.polygons), bool)
        sel = moved[tri_poly] if moved_side else ~moved[tri_poly]
        return mesh_uv(me, LAYOUT), tris[sel]

    kept = [tris_of(o, False) for o in objs]
    moved_tris = [tris_of(o, True) for o in movers]
    use_cycles(cfg.get("samples", 4))
    temp = Temp()
    masks = {}
    try:
        dummy = temp.material("PREP_dummy")
        set_target(dummy, temp.image("PREP_dummy", 16, 16, 'Non-Color'))
        for name, version_of in tasks:
            if not textures.exists(name):
                # (the export writes every texture the game has: this one the material names, the game doesn't)
                log(f"   {name}: the game has no such texture (the material names it); nothing to bake")
                continue
            users = [o for o in movers if (version_of or name) in textures_of[o]]
            role = texture_role(version_of or name, roles)
            colorspace = 'sRGB' if role == "color" else 'Non-Color'
            src = temp.keep_image(textures.load(name, roles))
            w, h = src.size
            src_px = image_pixels(src)
            out = temp.image("PREP_" + name, w, h, colorspace)
            out.pixels.foreach_set(src_px.ravel())
            if (w, h) not in masks:
                keep, cover = np.zeros((h, w), bool), np.zeros((h, w), bool)
                for uv, tris in kept:
                    keep |= raster_triangles(uv, tris, w, h) > 0
                for uv, tris in moved_tris:
                    cover |= raster_triangles(uv, tris, w, h) > 0
                masks[w, h] = keep, cover
            keep, cover = masks[w, h]
            median = np.median(src_px[keep], axis=0) if keep.any() else src_px.reshape(-1, 4).mean(0)

            def own_material(target):
                copies = {}

                def make(i, m):
                    if i not in copies:
                        copies[i] = bake_copy(m, temp)
                        set_target(copies[i], target)
                    return copies[i]
                return make

            def sampled(o, channel, target):
                rule = infos[o]["rule"] or {}
                value = (median[3],) * 3 if channel == "alpha" else median

                def make(i, m):
                    settings = rule.get("materials", {}).get(m.name if m else "", {})
                    s = source_material(temp, o.data, src, settings, infos[o]["uv"], value, channel, rep, o.name)
                    set_target(s, target)
                    return s
                return make

            t0 = time.time()
            swap = contextlib.ExitStack()
            if version_of:
                swap.enter_context(shown_as(version_of, src_px))
            with swap, contextlib.ExitStack() as stack:
                for o in users:
                    make = own_material(out) if role in ("color", "normal") else sampled(o, "color", out)
                    stack.enter_context(face_materials(o, infos[o]["moved"], make, dummy))
                run_bake(users, role, LAYOUT, cfg.get("bake_margin", 3))
            px = image_pixels(out)

            if role == "color" and (version_of or name) in TINTED:
                rep.warn(f"{name}: the game tints this texture with its material's colour, so the baked "
                         f"colours come out tinted again in the game")
            # Alpha of the colour texture (when it isn't constant): the faces' materials' opacity; for an opaque
            # material, whose alpha is data, the original's alpha where each moved face was
            if role == "color" and np.ptp(src_px[..., 3]) > 1.5 / 255:
                data_alpha = (version_of or name) in DATA_ALPHA
                alpha = temp.image("PREP_alpha_" + name, w, h, 'Non-Color')
                alpha_px = src_px.copy()
                alpha_px[..., 0] = src_px[..., 3]
                alpha.pixels.foreach_set(alpha_px.ravel())
                swap = contextlib.ExitStack()
                if version_of:
                    swap.enter_context(shown_as(version_of, src_px))
                with swap, contextlib.ExitStack() as stack:
                    for o in users:
                        copies = {}

                        def make(i, m, copies=copies):
                            if i not in copies:
                                copies[i] = opacity_copy(m, temp)
                                set_target(copies[i], alpha)
                            return copies[i]
                        if data_alpha:
                            make = sampled(o, "alpha", alpha)
                        stack.enter_context(face_materials(o, infos[o]["moved"], make, dummy))
                    run_bake(users, "aux", LAYOUT, cfg.get("bake_margin", 3))
                px[..., 3] = image_pixels(alpha)[..., 0]

            if role == "color" and guide is None:
                # The original colour where each moved face was, to tell how much the colour changed
                orig = temp.image("PREP_orig_" + name, w, h, colorspace)
                orig.pixels.foreach_set(src_px.ravel())
                with contextlib.ExitStack() as stack:
                    for o in users:
                        stack.enter_context(face_materials(o, infos[o]["moved"], sampled(o, "color", orig), dummy))
                    run_bake(users, "aux", LAYOUT, cfg.get("bake_margin", 3))
                free = np.zeros((h, w), bool)  #Faces without merge-rule source settings
                for o, (uv, tris) in zip(movers, moved_tris):
                    if not ((infos[o]["rule"] or {}).get("materials")):
                        free |= raster_triangles(uv, tris, w, h) > 0
                guide = {"new": px.copy(), "orig": image_pixels(orig), "src": src_px, "keep": keep,
                         "free": dilate(free, cfg.get("bake_margin", 3))}
            elif role == "aux" and guide is not None:
                px = colour_guided(px, src_px, guide, cfg, name)

            match_channel_layout(src_px, px)
            protect = dilate(keep, cfg.get("protect_margin", 2)) & ~cover
            px[protect] = src_px[protect]  #The other islands stay exactly as they were
            path_out = os.path.join(textures.out, name + ".png")
            save_png(px, path_out, colorspace)
            textures.painted[name] = path_out  #A later group starts from this result
            changed = float((np.abs(px - src_px)[..., :3].max(-1) > 1.5 / 255).mean())
            log(f"   baked {name} ({role}, {w}x{h}) for {[o.name for o in users]} in {time.time() - t0:.1f}s, "
                f"{changed * 100:.1f}% of pixels changed")
            rep.textures = [t for t in rep.textures if t["name"] != name]
            rep.textures.append({"name": name, "role": role, "size": [w, h], "meshes": [o.name for o in users],
                                 "changed_fraction": round(changed, 4), "path": path_out})
    finally:
        temp.clear()

    # The layout becomes the game UV map
    for o in objs:
        me = o.data
        if infos[o]["moved"] is not None:
            lay = mesh_uv(me, LAYOUT)
            me.uv_layers[infos[o]["uv"]].data.foreach_set("uv", lay.ravel())
        me.uv_layers.remove(me.uv_layers[LAYOUT])
        me.uv_layers.active = me.uv_layers[infos[o]["uv"]]


# ---------------------------------------------------------------- materials

IMPORT_NODES = {'BSDF_PRINCIPLED', 'OUTPUT_MATERIAL', 'TEX_IMAGE', 'NORMAL_MAP', 'GROUP', 'UVMAP',
                'TEX_COORD', 'MAPPING', 'VERTEX_COLOR', 'SEPARATE_COLOR', 'SEPRGB', 'FRAME', 'REROUTE'}


def custom_faces(o, game_material):
    """Faces on a material other than the game material (".001" copies count as the game material)."""
    names = [plain_name(s.material.name) if s.material else None for s in o.material_slots]
    custom_slots = [i for i, n in enumerate(names) if n is not None and n != game_material]
    return np.isin(material_indices(o.data), custom_slots)


def use_game_material(o, game_material):
    me = o.data
    mat = bpy.data.materials.get(game_material) or bpy.data.materials.new(game_material)
    me.materials.clear()
    me.materials.append(mat)
    me.polygons.foreach_set("material_index", np.zeros(len(me.polygons), np.int32))
    me.update()


EDITED_SUFFIX = "_scene_edit"


def edited_nodes(m):
    """How a game material was changed in the scene, as a list of what (empty: not changed): against the
    fingerprint make_scene.py stamped it with (any value, link, image or node), else (scenes made before the
    stamp) nodes the import doesn't make (node groups other than the RG normal group count)."""
    stamp = m.get(FINGERPRINT)
    if stamp:
        if material_fingerprint(m) == stamp:
            return []
        added = [n.name for n in m.node_tree.nodes if n.type not in IMPORT_NODES or (
            n.type == 'GROUP' and (n.node_tree is None or plain_name(n.node_tree.name) != RG_GROUP))]
        return added or ["settings or links of its nodes"]
    out = []
    for n in m.node_tree.nodes:
        if n.type not in IMPORT_NODES:
            out.append(n.name)
        elif n.type == 'GROUP' and (n.node_tree is None or plain_name(n.node_tree.name) != RG_GROUP):
            out.append(n.name)
    return out


def edit_game_materials(objects, vanilla_meshes, textures, rep):
    """A game material changed in place in the scene (new nodes, e.g. a mask that recolours part of the
    texture) is treated like a custom material: renamed on this copy, so its faces are detected and baked, and
    the game material is rebuilt from the textures for the comparison."""
    game = {vanilla_meshes[o.name]["material"] for o in objects if o.name in vanilla_meshes}
    seen = set()
    for o in objects:
        for s in o.material_slots:
            m = s.material
            if not m or m.name in seen or not m.use_nodes or plain_name(m.name) not in game:
                continue
            seen.add(m.name)
            nodes = edited_nodes(m)
            if not nodes:
                continue
            base = plain_name(m.name)
            if not textures.owned(base):
                rep.warn(f"material {m.name} was changed in the scene ({', '.join(nodes)}), but has no textures of "
                         f"this model's own to bake it into; the game uses it unchanged")
                continue
            m.name = base + EDITED_SUFFIX
            log(f"Material {base} was changed in the scene ({', '.join(nodes)}): its faces are baked like a new "
                f"material's")
            rep.meshes.setdefault(o.name, {}).setdefault("edited_game_materials", []).append(base)


# ---------------------------------------------------------------- main

def attribute_counts(vanilla_mesh):
    names = [a["name"] for a in vanilla_mesh["attributes"]]
    uvs = sum(1 for n in names if n.startswith("_u")) + sum(2 for n in names if n.startswith("_g3d_02_u"))
    colors = sum(1 for n in names if n.startswith("_c"))
    return uvs, colors


def main():
    argv = sys.argv[sys.argv.index("--") + 1:]
    cfg = json.load(open(argv[argv.index("--config") + 1], encoding="utf-8"))
    rep = Report()
    t_start = time.time()

    vanilla = json.load(open(cfg["vanilla_json"], encoding="utf-8"))
    model = vanilla["models"][cfg.get("model_index", 0)]
    vanilla_meshes = {m["name"]: m for m in model["meshes"]}
    learn_roles(model)

    os.makedirs(os.path.join(cfg["output"], "textures"), exist_ok=True)
    for f in os.listdir(os.path.join(cfg["output"], "textures")):
        if f.lower().endswith(".png"):
            os.remove(os.path.join(cfg["output"], "textures", f))

    if bpy.context.object and bpy.context.object.mode != 'OBJECT':
        bpy.ops.object.mode_set(mode='OBJECT')
    arm, objects = collect_objects(cfg, rep)
    log(f"Meshes: {sorted(o.name for o in objects)}")
    textures = Textures(cfg, model, rep)
    objects, first_joined = join_new_meshes(objects, vanilla_meshes, textures, arm, cfg, rep)

    rules = {r["mesh"]: r for r in cfg.get("merge", [])}
    for name in rules:
        if name not in {o.name for o in objects}:
            rep.error(f"merge rule for {name}, but there is no such mesh")

    bones = {b.name for b in arm.data.bones}
    missing_bones = [b for b in model["bones"] if b not in bones]
    if missing_bones:
        rep.warn(f"{len(missing_bones)} game bones are not in the armature, e.g. {missing_bones[:5]}")

    # Textures: painted ones, then custom materials baked into them
    textures.find_painted()
    edit_game_materials(objects, vanilla_meshes, textures, rep)
    infos = {}
    for o in objects:
        vm = vanilla_meshes.get(o.name)
        if vm is None:
            continue  # reported by join_new_meshes
        rule = rules.get(o.name)
        game_material = (rule or {}).get("into_material") or vm["material"]
        if game_material not in textures.materials:
            rep.error(f"{o.name}: material {game_material} is not in the game model")
            continue
        uv = (rule or {}).get("uv") or (o.data.uv_layers[0].name if o.data.uv_layers else None)
        if uv is not None and uv not in o.data.uv_layers:
            rep.error(f"{o.name}: UV map {uv} not found")
            continue
        infos[o] = {"game_material": game_material, "uv": uv, "rule": rule, "moved": None,
                    "custom": custom_faces(o, game_material)}

    for o, info in infos.items():
        custom = info["custom"]
        if not custom.any():
            continue
        materials = sorted({o.material_slots[i].material.name for i in np.unique(material_indices(o.data)[custom])})
        if info["uv"] is None:
            rep.error(f"{o.name}: has no UV map")
            continue
        if not textures.owned(info["game_material"]):
            rep.warn(f"{o.name}: {info['game_material']} has no textures of this model's own to bake "
                     f"{materials} into; the game material is used as it is")
            continue
        rule = info["rule"]
        if rule is not None and not rule.get("detect", False):
            moved = custom.copy()
        else:
            moved = detect_changes(o, info, textures, cfg, rep)
            moved = grow_faces(o.data, moved, (rule or {}).get("grow", cfg.get("grow", 1)))
        if o.name in first_joined:
            # A joined mesh's faces on other materials: their UVs mean nothing in these textures, so always baked
            joined = np.zeros(len(o.data.polygons), bool)
            joined[first_joined[o.name]:] = True
            moved = moved | (joined & custom)
        rep.meshes.setdefault(o.name, {}).update({"custom_materials": materials, "custom_faces": int(custom.sum()),
                                                  "baked_faces": int(moved.sum())})
        if moved.any():
            info["moved"] = moved
            log(f"{o.name}: baking {int(moved.sum())} faces of {materials} into {info['game_material']}")
        else:
            log(f"{o.name}: {materials} look like {info['game_material']}; nothing to bake")
            if any(m.endswith(EDITED_SUFFIX) for m in materials):
                rep.warn(f"{o.name}: the changes to {info['game_material']} don't show in what the build bakes "
                         f"(colour, normals, opacity), so they don't reach the game")

    group_of = texture_groups(textures)
    groups = {}
    for o, info in infos.items():
        groups.setdefault(group_of(info["game_material"]), []).append(o)
    for key, objs in groups.items():
        if any(infos[o]["moved"] is not None for o in objs):
            bake_group([o for o in objs if infos[o]["uv"] is not None], infos, textures, cfg, rep)

    for o, info in infos.items():
        if info["custom"].any():
            use_game_material(o, info["game_material"])

    for o in objects:
        info = rep.meshes.setdefault(o.name, {})
        vm = vanilla_meshes.get(o.name)
        used = {p.material_index for p in o.data.polygons}
        if len(used) > 1:
            rep.error(f"{o.name} uses {len(used)} materials {[o.material_slots[i].material.name for i in used]}. "
                      "Split the mesh.")

        if vm is not None:
            n_uv, n_color = attribute_counts(vm)
            me = o.data
            extra_uv = [u.name for u in me.uv_layers][max(n_uv, 1):]
            extra_color = [a.name for a in me.color_attributes][n_color:]
            handled = o in infos and infos[o]["custom"].any()
            if handled:
                for n in extra_uv:
                    me.uv_layers.remove(me.uv_layers[n])
                for n in extra_color:
                    me.color_attributes.remove(me.color_attributes[n])
                if extra_uv or extra_color:
                    log(f"{o.name}: removed UV maps {extra_uv} and colour layers {extra_color} the game mesh doesn't have")
            elif extra_uv or extra_color:
                rep.warn(f"{o.name}: has UV maps {extra_uv} / colour layers {extra_color} the game mesh doesn't have")
            if n_color and not me.color_attributes:
                rep.warn(f"{o.name}: the game mesh has vertex colours, this mesh has none (fine if unpainted: "
                         f"all-white colours are left out of the export, and Toolbox writes white)")
            info["game_vertices"] = vm["vertices"]
            info["game_skin_count"] = vm["skinCount"]

        trimmed, largest, unweighted = limit_weights(o, arm, cfg.get("max_weights", 4))
        if unweighted:
            rep.warn(f"{o.name}: {unweighted} vertices have no bone weights")
        info.update({"vertices": len(o.data.vertices), "triangles": sum(len(p.vertices) - 2 for p in o.data.polygons),
                     "weights_trimmed": trimmed, "largest_weight_removed": round(largest, 3),
                     "material": o.material_slots[0].material.name if o.material_slots else None})
        if trimmed:
            log(f"{o.name}: {trimmed} vertices limited to {cfg.get('max_weights', 4)} weights (largest removed {largest:.3f})")

    for name in vanilla_meshes:
        if name not in {o.name for o in objects}:
            rep.warn(f"game mesh {name} is not in the scene. The replace removes it.")

    if not rep.errors:
        if cfg.get("glb_name") and cfg.get("lods", "decimate") == "decimate":
            objects = objects + make_lods(objects, vanilla_meshes, cfg, rep)
        select_only([arm] + objects, arm)
        if cfg.get("glb_name"):
            # For modstudio replace: the scene as glTF, with up to 8 influences, tangents and every colour layer.
            # Blender's exporter only makes tangents for triangles: meshes with larger faces get a Triangulate
            # modifier (custom normals kept), which the export applies (on this copy; the .blend isn't saved).
            split = []
            for o in objects:
                if any(len(p.vertices) > 3 for p in o.data.polygons):
                    mod = o.modifiers.new("PREP_triangulate", 'TRIANGULATE')
                    if hasattr(mod, "keep_custom_normals"):
                        mod.keep_custom_normals = True
                    with bpy.context.temp_override(object=o):
                        bpy.ops.object.modifier_move_to_index(modifier=mod.name, index=0)
                    split.append(o.name)
            if split:
                log(f"Triangulated for the export (Blender makes tangents for triangles only): {split}")
            glb = os.path.join(cfg["output"], cfg["glb_name"])
            bpy.ops.export_scene.gltf(filepath=glb, export_format='GLB', use_selection=True, export_yup=True,
                                      export_apply=True, export_skins=True, export_influence_nb=8,
                                      export_all_influences=False, export_tangents=True, export_normals=True,
                                      export_texcoords=True, export_vertex_color='ACTIVE',
                                      export_all_vertex_colors=True, export_materials='NONE',
                                      export_animations=False, export_def_bones=False,
                                      export_rest_position_armature=True, export_extras=False,
                                      export_cameras=False, export_lights=False)
            log(f"Exported {glb}")
        else:
            fbx = os.path.join(cfg["output"], cfg["fbx_name"])
            #Scale 0.01: Toolbox (Assimp) applies the FBX unit scale that Blender writes
            bpy.ops.export_scene.fbx(filepath=fbx, use_selection=True, object_types={'ARMATURE', 'MESH'},
                                     add_leaf_bones=False, bake_anim=False, use_mesh_modifiers=True,
                                     mesh_smooth_type='OFF', use_tspace=False, global_scale=0.01,
                                     path_mode='STRIP', embed_textures=False)
            log(f"Exported {fbx}")

    report = {"warnings": rep.warnings, "errors": rep.errors, "meshes": rep.meshes, "textures": rep.textures,
              "painted": rep.painted, "seconds": round(time.time() - t_start, 1)}
    with open(os.path.join(cfg["output"], "prep_report.json"), "w", encoding="utf-8") as f:
        json.dump(report, f, indent=2)
    log(f"Done in {report['seconds']}s with {len(rep.errors)} error(s) and {len(rep.warnings)} warning(s)")
    sys.exit(1 if rep.errors else 0)


if __name__ == "__main__":
    try:
        main()
    except SystemExit:
        raise
    except Exception:
        import traceback
        traceback.print_exc()
        sys.exit(1)
