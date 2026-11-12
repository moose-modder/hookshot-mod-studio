"""Blender (background): does the built model look like the scene? So that an edit the build lost doesn't go
unnoticed.

    blender --background <scene.blend> --python check_result.py -- --config <check_config.json> --side scene
    blender --background --factory-startup --python check_result.py -- --config <check_config.json> --side result

Both sides are rendered the same way: base colour and opacity only (each Principled or Diffuse BSDF becomes an
emission of its base colour), orthographic, from the front, back, left and right, in the rest pose and in a test
pose. The test pose turns every game bone a little around a world axis chosen from its name, at its head, so both
sides get the same pose whatever their bone axes; bones the game doesn't have follow their parent, as they would
in game. A second render with one flat colour per mesh tells which mesh each pixel shows.

The scene side (the meshes the build uses, as prep_for_game.py collects them) frames the views and writes its
renders; the result side imports the built model (modstudio export with the mod overlaid, materials made as
make_scene.py makes them), renders, compares and writes check_report.json, with an image of each view that
differs (scene | result | differences in red).
"""

import argparse
import hashlib
import json
import math
import os
import sys
import time

import bpy
import numpy as np
from mathutils import Matrix, Vector

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

# Views: name -> direction from the model to the camera. glTF models face -Y in Blender.
VIEWS = {"front": (0, -1, 0), "back": (0, 1, 0), "left": (1, 0, 0), "right": (-1, 0, 0)}
POSES = ("rest", "pose")


def log(msg):
    print(f"[check] {msg}", flush=True)


# ---------------------------------------------------------------- materials

def flatten(mat, mode):
    """A copy of `mat` that shows its base colour and opacity (mode "color"), or one flat colour from the object's
    pass index (mode "id", opaque where the opacity is over one half). Returns (copy, nodes not flattened)."""
    m = mat.copy()
    m.name = f"CHECK_{mode}_{mat.name}"
    if not m.use_nodes:
        m.use_nodes = True
    nt = m.node_tree
    left = []
    out = next((n for n in nt.nodes if n.type == 'OUTPUT_MATERIAL' and n.is_active_output), None) or \
        next((n for n in nt.nodes if n.type == 'OUTPUT_MATERIAL'), None)
    if out is not None:
        # Displacement would move the surface in one render only if it differs; volume isn't drawn by the game.
        for name in ("Displacement", "Volume"):
            for link in list(out.inputs[name].links):
                nt.links.remove(link)
    def id_emission():
        em = nt.nodes.new("ShaderNodeEmission")
        info = nt.nodes.new("ShaderNodeObjectInfo")
        scale = nt.nodes.new("ShaderNodeMath")
        scale.operation = 'DIVIDE'
        scale.inputs[1].default_value = 255.0
        nt.links.new(info.outputs["Object Index"], scale.inputs[0])
        nt.links.new(scale.outputs[0], em.inputs["Color"])
        return em

    bsdfs = [n for n in nt.nodes if n.type in ('BSDF_PRINCIPLED', 'BSDF_DIFFUSE')]
    if mode == "id" and not bsdfs and out is not None:
        # (e.g. an emission-only material) the whole surface gets the id
        for link in list(out.inputs["Surface"].links):
            nt.links.remove(link)
        nt.links.new(id_emission().outputs[0], out.inputs["Surface"])
    for b in bsdfs:
        color_in = b.inputs["Base Color"] if b.type == 'BSDF_PRINCIPLED' else b.inputs["Color"]
        alpha_in = b.inputs.get("Alpha") if b.type == 'BSDF_PRINCIPLED' else None
        tr = nt.nodes.new("ShaderNodeBsdfTransparent")
        mix = nt.nodes.new("ShaderNodeMixShader")
        if mode == "color":
            em = nt.nodes.new("ShaderNodeEmission")
            if color_in.is_linked:
                nt.links.new(color_in.links[0].from_socket, em.inputs["Color"])
            else:
                em.inputs["Color"].default_value = color_in.default_value
        else:
            em = id_emission()
        if alpha_in is not None and (alpha_in.is_linked or alpha_in.default_value < 1.0):
            if mode == "color":
                if alpha_in.is_linked:
                    nt.links.new(alpha_in.links[0].from_socket, mix.inputs["Fac"])
                else:
                    mix.inputs["Fac"].default_value = alpha_in.default_value
            else:
                step = nt.nodes.new("ShaderNodeMath")
                step.operation = 'GREATER_THAN'
                step.inputs[1].default_value = 0.5
                if alpha_in.is_linked:
                    nt.links.new(alpha_in.links[0].from_socket, step.inputs[0])
                else:
                    step.inputs[0].default_value = alpha_in.default_value
                nt.links.new(step.outputs[0], mix.inputs["Fac"])
        else:
            mix.inputs["Fac"].default_value = 1.0
        nt.links.new(tr.outputs[0], mix.inputs[1])
        nt.links.new(em.outputs[0], mix.inputs[2])
        for link in list(b.outputs[0].links):
            nt.links.new(mix.outputs[0], link.to_socket)
        nt.nodes.remove(b)
    for n in nt.nodes:
        if n.type.startswith("BSDF_") and n.type not in ('BSDF_TRANSPARENT',) or n.type in (
                'SUBSURFACE_SCATTERING', 'BSDF_HAIR', 'BSDF_HAIR_PRINCIPLED', 'EEVEE_SPECULAR'):
            left.append(f"{mat.name}: {n.name}")
    return m, left


class Materials:
    """Per object, its original materials and the flattened copies, to swap between renders."""

    def __init__(self, objects):
        self.objects = objects
        self.original = {o: [s.material for s in o.material_slots] for o in objects}
        self.copies = {"color": {}, "id": {}}
        self.unflattened = set()
        fallback = bpy.data.materials.new("CHECK_none")
        for o in objects:
            if not o.material_slots:
                o.data.materials.append(fallback)
                self.original[o] = [fallback]
            for m in self.original[o]:
                m = m or fallback
                for mode in ("color", "id"):
                    if m.name not in self.copies[mode]:
                        copy, left = flatten(m, mode)
                        self.copies[mode][m.name] = copy
                        self.unflattened.update(left)

    def use(self, mode):
        for o in self.objects:
            for i, m in enumerate(self.original[o]):
                o.material_slots[i].material = self.copies[mode][(m or bpy.data.materials["CHECK_none"]).name]


# ---------------------------------------------------------------- pose

def bone_rotation(name, degrees):
    """A rotation about a world axis, chosen from the bone's name (the same in every run)."""
    h = hashlib.sha256(name.encode("utf-8")).digest()
    u = [int.from_bytes(h[i:i + 4], "little") / 2 ** 32 for i in range(0, 12, 4)]
    z = 2 * u[0] - 1
    t = 2 * math.pi * u[1]
    r = math.sqrt(max(0.0, 1 - z * z))
    axis = Vector((r * math.cos(t), r * math.sin(t), z))
    angle = math.radians(degrees) * (2 * u[2] - 1)
    return Matrix.Rotation(angle, 4, axis)


def set_pose(arm, game_bones, degrees):
    """degrees 0: the rest pose. Else the test pose (see the module docstring)."""
    for pb in arm.pose.bones:
        pb.matrix_basis = Matrix.Identity(4)
    arm.data.pose_position = 'POSE'
    if degrees:
        a = arm.matrix_world
        a_inv = a.inverted()
        world = {}   # bone -> its deformation, in world space
        posed = {}   # bone -> its posed matrix, in armature space

        def visit(bone, parent_d):
            d = parent_d
            if bone.name in game_bones:
                head = a @ bone.head_local
                d = parent_d @ Matrix.Translation(head) @ bone_rotation(bone.name, degrees) @ Matrix.Translation(-head)
            world[bone.name] = d
            posed[bone.name] = a_inv @ d @ a @ bone.matrix_local
            parent = posed[bone.parent.name] @ (bone.parent.matrix_local.inverted() @ bone.matrix_local) \
                if bone.parent else bone.matrix_local
            arm.pose.bones[bone.name].matrix_basis = parent.inverted() @ posed[bone.name]
            for child in bone.children:
                visit(child, d)

        for root in [b for b in arm.data.bones if b.parent is None]:
            visit(root, Matrix.Identity(4))
    bpy.context.view_layer.update()


# ---------------------------------------------------------------- rendering

def setup_render(cfg):
    s = bpy.context.scene
    s.render.engine = 'CYCLES'
    s.cycles.device = 'CPU'
    s.cycles.use_denoising = False
    s.cycles.use_adaptive_sampling = False
    s.cycles.max_bounces = 0
    s.cycles.transparent_max_bounces = 32
    s.render.film_transparent = True
    s.render.use_compositing = False
    s.render.use_sequencer = False
    s.render.image_settings.file_format = 'PNG'
    s.render.image_settings.color_mode = 'RGBA'
    s.render.image_settings.color_depth = '8'
    s.render.resolution_percentage = 100
    s.render.dither_intensity = 0.0  # else the 8-bit mesh ids come out +-1
    if s.world is None:
        s.world = bpy.data.worlds.new("CHECK_world")
    s.world.use_nodes = True
    bg = next((n for n in s.world.node_tree.nodes if n.type == 'BACKGROUND'), None)
    if bg:
        bg.inputs["Strength"].default_value = 0.0
    cam_data = bpy.data.cameras.new("CHECK_camera")
    cam_data.type = 'ORTHO'
    cam = bpy.data.objects.new("CHECK_camera", cam_data)
    s.collection.objects.link(cam)
    s.camera = cam
    return cam


def frame_views(objects, cfg):
    """Camera per view around the meshes' rest-pose bounds."""
    deps = bpy.context.evaluated_depsgraph_get()
    pts = []
    for o in objects:
        ev = o.evaluated_get(deps)
        me = ev.to_mesh()
        co = np.zeros(len(me.vertices) * 3)
        me.vertices.foreach_get("co", co)
        co = co.reshape(-1, 3)
        if len(co):
            m = np.array(o.matrix_world)
            pts.append(co @ m[:3, :3].T + m[:3, 3])
        ev.to_mesh_clear()
    pts = np.concatenate(pts)
    lo, hi = pts.min(0), pts.max(0)
    center = (lo + hi) / 2
    size = hi - lo
    res = cfg.get("resolution", 1024)
    margin = 1.0 + cfg.get("margin", 0.12)
    views = {}
    for name, d in VIEWS.items():
        width = max(float(size[0] if d[0] == 0 else size[1]), 1e-4)
        height = max(float(size[2]), 1e-4)
        # ortho_scale spans the render's longer side; the shorter one gets the same margin
        if height >= width:
            resolution = [max(16, round(res * width / height)), res]
        else:
            resolution = [res, max(16, round(res * height / width))]
        views[name] = {"center": center.tolist(), "dir": list(d), "ortho_scale": max(width, height) * margin,
                       "distance": float(size.max() * 4 + 1), "resolution": resolution}
    return views, {"lo": lo.tolist(), "hi": hi.tolist()}


def render(cam, view, path, mode, cfg):
    s = bpy.context.scene
    d = Vector(view["dir"])
    cam.location = Vector(view["center"]) + d * view["distance"]
    cam.rotation_euler = (-d).to_track_quat('-Z', 'Y').to_euler()
    cam.data.ortho_scale = view["ortho_scale"]
    cam.data.clip_end = view["distance"] * 3
    s.render.resolution_x, s.render.resolution_y = view["resolution"]
    if mode == "color":
        s.cycles.samples = cfg.get("samples", 4)
        s.cycles.filter_width = 1.5
        s.view_settings.view_transform = 'Standard'
    else:
        s.cycles.samples = 1
        s.cycles.filter_width = 0.01
        s.view_settings.view_transform = 'Raw'
    s.view_settings.look = 'None'
    s.view_settings.exposure = 0.0
    s.view_settings.gamma = 1.0
    s.render.filepath = path
    bpy.ops.render.render(write_still=True)


def render_all(objects, arm, cfg, views, prefix, game_bones):
    cam = setup_render(cfg)
    mats = Materials(objects)
    for o in bpy.data.objects:
        if o.type in ('MESH', 'CURVE', 'SURFACE', 'META', 'FONT', 'VOLUME', 'POINTCLOUD', 'CURVES',
                      'GPENCIL', 'GREASEPENCIL', 'LIGHT'):
            o.hide_render = o not in objects
    for o in objects:
        o.hide_render = False
    index = {}
    for i, o in enumerate(objects):
        o.pass_index = i + 1
        index[i + 1] = o.name
    out = cfg["out"]
    t0 = time.time()
    for pose in POSES:
        set_pose(arm, game_bones, cfg.get("pose_degrees", 12) if pose == "pose" else 0)
        for name, view in views.items():
            for mode in ("color", "id"):
                mats.use(mode)
                render(cam, view, os.path.join(out, f"{prefix}_{name}_{pose}_{mode}.png"), mode, cfg)
    log(f"{prefix}: {len(POSES) * len(views) * 2} renders in {time.time() - t0:.0f}s")
    return index, sorted(mats.unflattened)


# ---------------------------------------------------------------- comparing

def load(path):
    img = bpy.data.images.load(path, check_existing=False)
    w, h = img.size
    px = np.zeros(w * h * 4, np.float32)
    img.pixels.foreach_get(px)
    bpy.data.images.remove(img)
    return np.flipud(px.reshape(h, w, 4))


def erode(mask):
    out = mask.copy()
    out[1:, :] &= mask[:-1, :]
    out[:-1, :] &= mask[1:, :]
    out[:, 1:] &= mask[:, :-1]
    out[:, :-1] &= mask[:, 1:]
    return out


def dilate(mask):
    out = mask.copy()
    out[1:, :] |= mask[:-1, :]
    out[:-1, :] |= mask[1:, :]
    out[:, 1:] |= mask[:, :-1]
    out[:, :-1] |= mask[:, 1:]
    return out


def regions(mask, cell, min_cell):
    """Groups of neighbouring cells (cell x cell pixels) with at least min_cell marked pixels."""
    h, w = mask.shape
    gh, gw = -(-h // cell), -(-w // cell)
    padded = np.zeros((gh * cell, gw * cell), bool)
    padded[:h, :w] = mask
    counts = padded.reshape(gh, cell, gw, cell).sum((1, 3))
    hot = counts >= min_cell
    seen = np.zeros_like(hot)
    out = []
    for y, x in zip(*np.nonzero(hot)):
        if seen[y, x]:
            continue
        stack, cells = [(y, x)], []
        seen[y, x] = True
        while stack:
            cy, cx = stack.pop()
            cells.append((cy, cx))
            for ny, nx in ((cy + 1, cx), (cy - 1, cx), (cy, cx + 1), (cy, cx - 1)):
                if 0 <= ny < gh and 0 <= nx < gw and hot[ny, nx] and not seen[ny, nx]:
                    seen[ny, nx] = True
                    stack.append((ny, nx))
        ys = [c[0] for c in cells]
        xs = [c[1] for c in cells]
        box = (int(min(ys)) * cell, int(min(xs)) * cell, min(h, (int(max(ys)) + 1) * cell),
               min(w, (int(max(xs)) + 1) * cell))
        out.append(box)
    return out


def compare(cfg, views, scene_index, result_index):
    out = cfg["out"]
    color_tol = cfg.get("color_tolerance", 24) / 255
    cell, min_cell, min_pixels = cfg.get("cell", 16), cfg.get("cell_min", 24), cfg.get("min_pixels", 80)
    joined = cfg.get("joined", {})
    found = []
    images = []
    for pose in POSES:
        for name in views:
            p = lambda side, mode: os.path.join(out, f"{side}_{name}_{pose}_{mode}.png")  # noqa: E731
            e, r = load(p("scene", "color")), load(p("result", "color"))
            ie, ir = load(p("scene", "id")), load(p("result", "id"))
            cov_e, cov_r = e[..., 3] > 0.5, r[..., 3] > 0.5
            colour = cov_e & cov_r & (np.abs(e[..., :3] - r[..., :3]).max(-1) > color_tol)
            shape = cov_e ^ cov_r
            mask = dilate(erode(colour | shape))
            id_e = np.rint(ie[..., 0] * 255).astype(int) * (ie[..., 3] > 0.5)
            id_r = np.rint(ir[..., 0] * 255).astype(int) * (ir[..., 3] > 0.5)
            view_found = []
            for box in regions(mask, cell, min_cell):
                y0, x0, y1, x1 = box
                sub = mask[y0:y1, x0:x1]
                n = int(sub.sum())
                if n < min_pixels:
                    continue
                meshes = {}
                for ids, names in ((id_e[y0:y1, x0:x1][sub], scene_index), (id_r[y0:y1, x0:x1][sub], result_index)):
                    for i, c in zip(*np.unique(ids[ids > 0], return_counts=True)):
                        nm = names.get(int(i), f"#{i}")
                        nm = f"{nm} (joined into {joined[nm]})" if nm in joined else nm
                        meshes[nm] = meshes.get(nm, 0) + int(c)
                kind = "shape" if shape[y0:y1, x0:x1][sub].sum() > colour[y0:y1, x0:x1][sub].sum() else "colour"
                view_found.append({"view": name, "pose": pose, "pixels": n, "kind": kind,
                                   "box": [x0, y0, x1, y1],
                                   "meshes": dict(sorted(meshes.items(), key=lambda kv: -kv[1])[:4])})
            if view_found:
                found += view_found
                images.append(write_diff_image(e, r, mask, view_found, os.path.join(out, f"differs_{name}_{pose}.png")))
    return found, images


def write_diff_image(e, r, mask, found, path):
    """scene | result | result greyed with the differing pixels red and the regions boxed."""
    def on_grey(x):
        a = x[..., 3:4]
        return x[..., :3] * a + 0.25 * (1 - a)
    left, mid = on_grey(e), on_grey(r)
    right = mid.mean(-1, keepdims=True).repeat(3, -1) * 0.6
    right[mask] = (1.0, 0.1, 0.1)
    for f in found:
        x0, y0, x1, y1 = f["box"]
        right[y0:y1, [x0, x1 - 1]] = (1.0, 0.9, 0.0)
        right[[y0, y1 - 1], x0:x1] = (1.0, 0.9, 0.0)
    img = np.concatenate([left, mid, right], 1)
    h, w = img.shape[:2]
    rgba = np.concatenate([img, np.ones((h, w, 1))], -1).astype(np.float32)
    im = bpy.data.images.new("CHECK_diff", w, h)
    im.pixels.foreach_set(np.flipud(rgba).ravel())
    im.filepath_raw = path
    im.file_format = 'PNG'
    im.save()
    bpy.data.images.remove(im)
    return path


# ---------------------------------------------------------------- the two sides

def scene_side(cfg):
    import prep_for_game as prep
    if bpy.context.object and bpy.context.object.mode != 'OBJECT':
        bpy.ops.object.mode_set(mode='OBJECT')
    rep = prep.Report()
    arm, objects = prep.collect_objects(cfg, rep)
    views, bounds = frame_views(objects, cfg)
    index, left = render_all(objects, arm, cfg, views, "scene", set(cfg["game_bones"]))
    with open(os.path.join(cfg["out"], "scene.json"), "w", encoding="utf-8") as f:
        json.dump({"views": views, "bounds": bounds, "index": index, "unflattened": left}, f, indent=2)


def result_side(cfg):
    import make_scene
    with open(os.path.join(cfg["out"], "scene.json"), encoding="utf-8") as f:
        scene = json.load(f)
    bpy.ops.wm.read_homefile(use_empty=True)
    bpy.ops.import_scene.gltf(filepath=cfg["result_glb"], merge_vertices=False)
    arm, _ = make_scene.tidy_glb_import()
    folder = os.path.dirname(os.path.abspath(cfg["result_glb"]))
    with open(os.path.join(folder, "model.json"), encoding="utf-8") as f:
        make_scene.build_materials(json.load(f), folder)
    objects = [o for o in bpy.context.scene.objects if o.type == 'MESH']
    index, left = render_all(objects, arm, cfg, scene["views"], "result", set(cfg["game_bones"]))
    scene_index = {int(k): v for k, v in scene["index"].items()}
    found, images = compare(cfg, scene["views"], scene_index, index)
    report = {"matches": not found, "differences": found, "images": images,
              "not_compared": sorted(set(scene["unflattened"]) | set(left)),
              "settings": {k: cfg.get(k) for k in ("color_tolerance", "pose_degrees", "resolution")}}
    with open(os.path.join(cfg["out"], "check_report.json"), "w", encoding="utf-8") as f:
        json.dump(report, f, indent=2)
    log(f"{len(found)} difference(s)")


def main():
    argv = sys.argv[sys.argv.index("--") + 1:]
    ap = argparse.ArgumentParser()
    ap.add_argument("--config", required=True)
    ap.add_argument("--side", choices=("scene", "result"), required=True)
    a = ap.parse_args(argv)
    with open(a.config, encoding="utf-8") as f:
        cfg = json.load(f)
    os.makedirs(cfg["out"], exist_ok=True)
    (scene_side if a.side == "scene" else result_side)(cfg)


if __name__ == "__main__":
    try:
        main()
    except SystemExit:
        raise
    except Exception:
        import traceback
        traceback.print_exc()
        sys.exit(1)
