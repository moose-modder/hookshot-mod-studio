"""Blender (run when the app opens a project's scene): Mod Studio's settings in Blender's own panels.

    blender <scene.blend> --python scene_tools.py

- **Bone tab, "Mod Studio" panel: "Simulate as cloth"**: a bone added to the game's skeleton with this ticked (or
  named `Ph_...`, the game's own prefix for bones its physics moves) is simulated: the build makes a part of the
  model's cloth file for it and the ticked or `Ph_` bones chained below it (`pipeline/README.md`, *Bones*).
- On a simulated bone, **the chain's settings** (the topmost bone of a chain that has them counts): Swing, Stiffness,
  Gravity and Colliders, custom properties too (`modstudio_cloth_swing` ...), made with their defaults when the box
  is ticked (or by the panel's "Cloth settings" button, on a `Ph_` bone or one ticked before).

The tick is a custom property of the bone (`modstudio_cloth`, true or false), not a property this script registers:
Blender 5 keeps those apart from the bone's own custom properties, out of reach of a scene opened without the
script, and of the build. So it stays with the scene, shows as a tick box under the bone's Custom Properties when the
scene is opened without this script, and the build reads it either way.
"""

import bpy

PROP = "modstudio_cloth"
PREFIX = "Ph_"
# a simulated chain's settings, on its top bone: (property, default, min, max, label, description)
SETTINGS = (
    ("modstudio_cloth_swing", 0.5, 0.01, 4.0, "Swing",
     "How far it may swing: a share of each point's distance down the chain, from where the bone above would carry "
     "it (0.5: half)"),
    ("modstudio_cloth_stiffness", 0.0, 0.0, 1.0, "Stiffness",
     "How firmly it keeps the shape it was made in: its first bone's angle to the bone it hangs from, and the angles "
     "at its joints (0: it hangs and swings freely; more: a bone that sticks out springs back to its place, with a "
     "bounce; small bones need more)"),
    ("modstudio_cloth_gravity", 1.0, 0.0, 10.0, "Gravity", "The game's gravity times this"),
)
COLLIDERS = "modstudio_cloth_colliders"


def _bone(context):
    # (a context has these where a bone is shown: the Bone tab, in edit mode and out of it)
    return getattr(context, "edit_bone", None) or getattr(context, "bone", None)


def _add_settings(bone):
    """The chain's settings on `bone`, the ones it lacks at their defaults, with their ranges and descriptions."""
    for name, default, low, high, _label, description in SETTINGS:
        if name not in bone.keys():
            bone[name] = default
        try:
            bone.id_properties_ui(name).update(min=low, max=high, soft_min=low, soft_max=high,
                                               description=description)
        except (TypeError, AttributeError):
            pass
    if COLLIDERS not in bone.keys():
        bone[COLLIDERS] = "auto"


class MODSTUDIO_OT_bone_cloth(bpy.types.Operator):
    """A bone you add, simulated by the game's cloth physics: it hangs from the bone above it and swings, with the
    simulated bones chained below it (bones named Ph_... are simulated too)"""
    bl_idname = "modstudio.bone_cloth"
    bl_label = "Simulate as cloth"
    bl_options = {'REGISTER', 'UNDO'}

    @classmethod
    def poll(cls, context):
        return _bone(context) is not None

    def execute(self, context):
        bone = _bone(context)
        bone[PROP] = True
        _add_settings(bone)
        return {'FINISHED'}


class MODSTUDIO_OT_cloth_settings(bpy.types.Operator):
    """The simulated chain's settings on this bone, at their defaults: how far it swings, how stiff it is, its gravity
    and which colliders it meets"""
    bl_idname = "modstudio.cloth_settings"
    bl_label = "Cloth settings"
    bl_options = {'REGISTER', 'UNDO'}

    @classmethod
    def poll(cls, context):
        return _bone(context) is not None

    def execute(self, context):
        _add_settings(_bone(context))
        return {'FINISHED'}


class MODSTUDIO_PT_bone(bpy.types.Panel):
    bl_label = "Mod Studio"
    bl_space_type = 'PROPERTIES'
    bl_region_type = 'WINDOW'
    bl_context = "bone"

    @classmethod
    def poll(cls, context):
        return _bone(context) is not None

    def draw(self, context):
        bone = _bone(context)
        layout = self.layout
        if PROP in bone.keys():
            layout.prop(bone, f'["{PROP}"]', text="Simulate as cloth")
        else:
            layout.operator(MODSTUDIO_OT_bone_cloth.bl_idname, icon='CHECKBOX_DEHLT')
        if bone.name.startswith(PREFIX):
            layout.label(text=f"Simulated: its name starts with {PREFIX}", icon='INFO')
        layout.label(text="Only for bones you add to the game's skeleton.")
        if bone.name.startswith(PREFIX) or bone.get(PROP):
            box = layout.box()
            box.label(text="The chain from here down:")
            if any(name in bone.keys() for name, *_ in SETTINGS) or COLLIDERS in bone.keys():
                for name, _default, _low, _high, label, _description in SETTINGS:
                    if name in bone.keys():
                        box.prop(bone, f'["{name}"]', text=label)
                if COLLIDERS in bone.keys():
                    box.prop(bone, f'["{COLLIDERS}"]', text="Colliders")
                    box.label(text="auto, none, or names (the Cloth collection's), commas between")
            else:
                box.operator(MODSTUDIO_OT_cloth_settings.bl_idname, icon='PREFERENCES')


CLASSES = (MODSTUDIO_OT_bone_cloth, MODSTUDIO_OT_cloth_settings, MODSTUDIO_PT_bone)


def register():
    for c in CLASSES:
        bpy.utils.register_class(c)


if __name__ == "__main__":
    try:
        register()
    except ValueError:
        # already registered (the script run twice in one session)
        pass
