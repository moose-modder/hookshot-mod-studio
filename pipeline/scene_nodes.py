"""Node helpers shared by the Blender scripts (make_scene.py, prep_for_game.py)."""
import hashlib
import json

import bpy

RG_GROUP = "Reconstruct RG Normal"

# make_scene.py stamps each game material with its fingerprint; the build compares (prep_for_game.py).
FINGERPRINT = "modstudio_fingerprint"
# Node settings that change what a node does (the rest is layout: location, width, colour, label, ...).
_SETTINGS = ("operation", "blend_type", "data_type", "interpolation", "projection", "extension", "space",
             "uv_map", "use_clamp", "clamp_type", "interpolation_type", "attribute_name", "attribute_type",
             "layer_name", "factor_mode", "clamp_factor", "clamp_result", "distribution", "subsurface_method",
             "invert", "use_alpha")


def _value(v):
    if isinstance(v, float):
        return round(v, 5)
    if isinstance(v, (int, bool, str)) or v is None:
        return v
    if hasattr(v, "name") and hasattr(v, "bl_rna"):  # an ID (image, node group)
        return v.name
    try:
        return [_value(x) for x in v]
    except TypeError:
        return str(v)


def _tree(tree, seen, image_key):
    nodes = []
    for n in sorted(tree.nodes, key=lambda n: n.name):
        if n.type in ('FRAME', 'REROUTE'):
            continue
        entry = {"node": n.bl_idname, "name": n.name,
                 "settings": {k: _value(getattr(n, k)) for k in _SETTINGS if hasattr(n, k)},
                 "inputs": {s.identifier: _value(s.default_value) for s in n.inputs
                            if not s.is_linked and hasattr(s, "default_value")}}
        if n.type == 'TEX_IMAGE':
            entry["image"] = image_key(n) if n.image else None
        if n.type == 'GROUP' and n.node_tree is not None:
            entry["group"] = n.node_tree.name
            if n.node_tree.name not in seen:
                seen[n.node_tree.name] = None
                seen[n.node_tree.name] = _tree(n.node_tree, seen, image_key)
        nodes.append(entry)
    links = sorted((l.from_node.name, l.from_socket.identifier, l.to_node.name, l.to_socket.identifier)
                   for l in tree.links if l.is_valid and not l.is_muted)
    return {"nodes": nodes, "links": links}


def material_fingerprint(mat, image_key=None):
    """A hash of what a material's nodes do: node types, settings, unlinked input values, images, links and the
    node groups they use. Moving nodes around doesn't change it; changing a value, a link or an image does.
    image_key(node): the name an image node's image counts under (default: the image's name)."""
    if not mat.use_nodes or mat.node_tree is None:
        return None
    groups = {}
    data = {"tree": _tree(mat.node_tree, groups, image_key or (lambda n: n.image.name)), "groups": groups}
    return hashlib.sha256(json.dumps(data, sort_keys=True).encode("utf-8")).hexdigest()[:24]


def rg_normal_group():
    """RG texture -> RGB tangent normal: z = sqrt(max(0, 1 - x^2 - y^2)) from x, y in -1..1.
    The game's normal maps hold only red and green."""
    g = bpy.data.node_groups.get(RG_GROUP)
    if g:
        return g
    g = bpy.data.node_groups.new(RG_GROUP, "ShaderNodeTree")
    g.interface.new_socket("RG Texture", in_out="INPUT", socket_type="NodeSocketColor")
    g.interface.new_socket("RGB Normal", in_out="OUTPUT", socket_type="NodeSocketColor")
    n, link = g.nodes, g.links.new
    gin, gout = n.new("NodeGroupInput"), n.new("NodeGroupOutput")
    sep = n.new("ShaderNodeSeparateXYZ")
    signed = n.new("ShaderNodeVectorMath")
    signed.operation = "MULTIPLY_ADD"
    signed.inputs[1].default_value = (2.0, 2.0, 0.0)
    signed.inputs[2].default_value = (-1.0, -1.0, 0.0)
    dot = n.new("ShaderNodeVectorMath")
    dot.operation = "DOT_PRODUCT"
    one_minus = n.new("ShaderNodeMath")
    one_minus.operation = "SUBTRACT"
    one_minus.inputs[0].default_value = 1.0
    clamp = n.new("ShaderNodeMath")
    clamp.operation = "MAXIMUM"
    clamp.inputs[1].default_value = 0.0
    sqrt = n.new("ShaderNodeMath")
    sqrt.operation = "SQRT"
    encode = n.new("ShaderNodeMath")
    encode.operation = "MULTIPLY_ADD"
    encode.inputs[1].default_value = 0.5
    encode.inputs[2].default_value = 0.5
    comb = n.new("ShaderNodeCombineXYZ")
    link(gin.outputs[0], sep.inputs[0])
    link(gin.outputs[0], signed.inputs[0])
    link(signed.outputs[0], dot.inputs[0])
    link(signed.outputs[0], dot.inputs[1])
    link(dot.outputs["Value"], one_minus.inputs[1])
    link(one_minus.outputs[0], clamp.inputs[0])
    link(clamp.outputs[0], sqrt.inputs[0])
    link(sqrt.outputs[0], encode.inputs[0])
    link(sep.outputs["X"], comb.inputs["X"])
    link(sep.outputs["Y"], comb.inputs["Y"])
    link(encode.outputs[0], comb.inputs["Z"])
    link(comb.outputs[0], gout.inputs[0])
    for i, node in enumerate((gin, sep, signed, dot, one_minus, clamp, sqrt, encode, comb, gout)):
        node.location = (i * 180, 0)
    return g
