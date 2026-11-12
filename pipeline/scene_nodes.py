"""Node helpers shared by the Blender scripts (make_scene.py, prep_for_game.py)."""
import bpy

RG_GROUP = "Reconstruct RG Normal"


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
