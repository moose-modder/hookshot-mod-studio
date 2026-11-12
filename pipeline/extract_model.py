"""Gets a game model ready to edit: the reverse of build_mod.py.

    python extract_model.py <model> [--name <name>] [--no-blend] [--force]

<model> is a .bfres.zs file, a path inside the game's romfs (Model/Npc_Hylia_WU_Impa.Npc_Hylia_WU_Impa.bfres.zs),
or part of a name to look for in its Model folder (Impa). Several matches are listed, nothing is done.

Steps (paths from config.json):
  1. modstudio export: <workspace>/<name>/<model>.glb, the textures its materials use as .png beside it
     (shared CmnTex_* ones too), model.json (meshes, materials, skin counts, textures and their formats).
     Without tools.modstudio in config.json, Switch Toolbox's export-model writes an .fbx instead.
  2. Blender (background): <workspace>/<name>/<name>.blend, the model imported, materials made from its textures
     (make_scene.py). Never overwritten: delete it yourself to start the scene over.
  3. A project file for build_mod.py, pipeline/<name>.json (not overwritten either)

The game's files are only read. Only the Python standard library is needed.
"""

import argparse
import glob
import json
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))


class ExtractError(Exception):
    pass


def log(msg=""):
    print(msg, flush=True)


def run(step, cmd, log_path):
    log(f"\n== {step}")
    with open(log_path, "w", encoding="utf-8", errors="replace") as logf:
        logf.write(" ".join(f'"{c}"' if " " in c else c for c in cmd) + "\n\n")
        proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, encoding="utf-8",
                                errors="replace")
        for line in proc.stdout:
            logf.write(line)
            line = line.rstrip()
            if line.startswith(("Wrote", "Warning", "Error", "Textures:", "   at ", "[scene]")) or "Error:" in line:
                log("   " + line)
        code = proc.wait()
    if code != 0:
        raise ExtractError(f"{step} failed (exit {code}). See {log_path}")


def find_model(romfs, what):
    """The model file for a path or a part of a name"""
    for path in (what, os.path.join(romfs, what)):
        if os.path.isfile(path):
            return os.path.abspath(path)
    term = what.lower().replace("\\", "/").split("/")[-1]
    files = [f for f in glob.glob(os.path.join(romfs, "Model", "*.bfres.zs")) if ".anim." not in os.path.basename(f).lower()]
    hits = [f for f in files if term in os.path.basename(f).lower()]
    exact = [f for f in hits if os.path.basename(f).lower().split(".")[0] == term.split(".")[0]]
    if len(exact) == 1:
        return exact[0]
    if len(hits) == 1:
        return hits[0]
    if not hits:
        raise ExtractError(f"No model in {os.path.join(romfs, 'Model')} matches '{what}'")
    names = sorted(os.path.basename(f) for f in hits)
    shown = "\n  ".join(names[:60]) + (f"\n  ... and {len(names) - 60} more" if len(names) > 60 else "")
    raise ExtractError(f"{len(names)} models match '{what}', name one:\n  {shown}")


def main():
    ap = argparse.ArgumentParser(description="Export a game model (.glb or .fbx, .png) and set up a Blender scene for it.")
    ap.add_argument("model", help="model file, path inside the romfs, or part of its name")
    ap.add_argument("--name", help="short name for the folder, .blend and project file (default: the model's name)")
    ap.add_argument("--config", default=os.path.join(HERE, "config.json"))
    ap.add_argument("--no-blend", action="store_true", help="only the export, no Blender scene")
    ap.add_argument("--force", action="store_true", help="export again into a folder that has an export (the .blend and "
                                                          "project file are still kept)")
    args = ap.parse_args()

    with open(args.config, encoding="utf-8") as f:
        cfg = json.load(f)
    romfs = os.path.abspath(cfg["game_romfs"])
    tools = cfg["tools"]
    exporter = "modstudio" if tools.get("modstudio") else "toolbox"
    for tool in (exporter,) + (() if args.no_blend else ("blender",)):
        if not os.path.exists(tools.get(tool, "")):
            raise ExtractError(f"{args.config}: tools.{tool} not found: {tools.get(tool)}")

    model = find_model(romfs, args.model)
    rel = os.path.relpath(model, romfs).replace("\\", "/")
    if rel.startswith("../"):
        raise ExtractError(f"{model} is not inside the game's romfs {romfs}")
    stem = os.path.basename(model).split(".")[0]
    name = args.name or stem
    mod_name = name[:1].upper() + name[1:] + "Mod"
    out = os.path.join(os.path.abspath(cfg["workspace"]), name)
    blend = os.path.join(out, f"{name}.blend")
    project = os.path.join(HERE, f"{name}.json")

    log(f"Model:  {model}")
    log(f"Folder: {out}")
    exports = lambda: glob.glob(os.path.join(out, "*.glb")) + glob.glob(os.path.join(out, "*.fbx"))  # noqa: E731
    if exports() and not args.force:
        raise ExtractError(f"{out} already has an export (edited textures there would be replaced). "
                           f"Use --force to export again, or --name for another folder.")
    os.makedirs(out, exist_ok=True)

    # 1. The model (.glb or .fbx), textures, model.json
    if exporter == "modstudio":
        run("Exporting the model (modstudio)", [tools["modstudio"], "export", model, out, "--romfs", romfs],
            os.path.join(out, "1_export.log"))
        scene_input = ["--glb", sorted(glob.glob(os.path.join(out, "*.glb")), key=os.path.getmtime)[-1]]
    else:
        run("Exporting the model (Toolbox)", [tools["toolbox"], "export-model", model, "--romfs", romfs, "--out", out],
            os.path.join(out, "1_export.log"))
        scene_input = ["--fbx", sorted(glob.glob(os.path.join(out, "*.fbx")), key=os.path.getmtime)[-1]]
    exported = scene_input[1]
    with open(os.path.join(out, "model.json"), encoding="utf-8") as f:
        info = json.load(f)
    meshes = info["models"][0]["meshes"]

    # 2. Blender scene
    if args.no_blend:
        pass
    elif os.path.exists(blend):
        log(f"\n== Blender scene: {blend} exists, kept")
    else:
        run("Making the Blender scene", [tools["blender"], "--background", "--factory-startup", "--python-exit-code", "1",
            "--python", os.path.join(HERE, "make_scene.py"), "--"] + scene_input + ["--blend", blend],
            os.path.join(out, "2_scene.log"))

    # 3. Project file for build_mod.py
    if os.path.exists(project):
        log(f"\n== Project file: {project} exists, kept")
    else:
        work = out.replace("\\", "/")
        proj = {
            "name": mod_name,
            "game": cfg.get("game", "oot"),
            "game_romfs": cfg["game_romfs"],
            "model": rel,
            "blend": f"{work}/{name}.blend",
            "work_dir": f"{work}/build",
            "mod_dir": f"{os.path.abspath(cfg['workspace'])}/{mod_name}".replace("\\", "/"),
            "install_dir": f"{cfg['install_mods']}/{mod_name}",
            "tools": tools,
            "prep": {"armature": "Armature", "max_weights": 4, "merge": []},
            "toolbox_options": [],
        }
        with open(project, "w", encoding="utf-8") as f:
            json.dump(proj, f, indent=2)
        log(f"\n== Wrote the project file {project}")

    pngs = glob.glob(os.path.join(out, "*.png"))
    log(f"\nDone: {os.path.basename(exported)}, {len(pngs)} textures, {len(meshes)} meshes:")
    for m in meshes:
        log(f"   {m['name']:40} {m['material']:20} skin {m['skinCount']}  LODs {m['lods']}")
    log("\nNext:")
    if not args.no_blend:
        log(f"   edit {blend} (keep the mesh names; one material per mesh, or a merge rule in the project file)")
    log(f"   python build_mod.py {os.path.basename(project)} --install")


if __name__ == "__main__":
    try:
        main()
    except ExtractError as e:
        log(f"\nFAILED: {e}")
        sys.exit(1)
