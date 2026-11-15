"""Blender (run when the app opens a project's scene): Mod Studio's settings in Blender's own panels.

    blender <scene.blend> --python scene_tools.py

- **Bone tab, "Mod Studio" panel: "Simulate as cloth"**: a bone added to the game's skeleton with this ticked (or
  named `Ph_...`, the game's own prefix for bones its physics moves) is simulated: the build makes a part of the
  model's cloth file for it and the ticked or `Ph_` bones chained below it (`pipeline/README.md`, *Bones*).

The tick is a custom property of the bone (`modstudio_cloth`, true or false), not a property this script registers:
Blender 5 keeps those apart from the bone's own custom properties, out of reach of a scene opened without the
script, and of the build. So it stays with the scene, shows as a tick box under the bone's Custom Properties when the
scene is opened without this script, and the build reads it either way.
"""

import bpy

PROP = "modstudio_cloth"
PREFIX = "Ph_"


def _bone(context):
    # (a context has these where a bone is shown: the Bone tab, in edit mode and out of it)
    return getattr(context, "edit_bone", None) or getattr(context, "bone", None)


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
        _bone(context)[PROP] = True
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


CLASSES = (MODSTUDIO_OT_bone_cloth, MODSTUDIO_PT_bone)


def register():
    for c in CLASSES:
        bpy.utils.register_class(c)


if __name__ == "__main__":
    try:
        register()
    except ValueError:
        # already registered (the script run twice in one session)
        pass
