"""Builds a model replacement mod from a Blender scene in one go.

    python build_mod.py <project>.json [--skip-blender] [--install]

Steps (see README.md), with modstudio (tools.modstudio; without it, Switch Toolbox's commands do 1, 3 and 4 with
an FBX export):
  1. modstudio export: facts (model.json) and original textures of the game's model
  2. Blender (background, the .blend is not changed): prep_for_game.py -> glTF + baked textures
  3. modstudio replace: game model + glTF + textures -> <mod>/romfs
  4. modstudio inspect: checks the result against the game's model
  5. check_result.py (Blender): the built model rendered as the scene is, and compared with it (modstudio only;
     --no-check skips it)
  6. restbl: resource size table for the mod
  7. With --install: replaces install_dir's romfs with the mod's (without its RESTBL) and makes one RESTBL for
     all installed mods (restbl_all.py)

Only the Python standard library is needed.
"""

import argparse
import glob
import json
import os
import re
import shutil
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import restbl_all  # noqa: E402


class BuildError(Exception):
    pass


def log(msg=""):
    print(msg, flush=True)


def run(step, cmd, log_path, show=lambda line: True):
    """Runs a tool, shows the lines `show` accepts and writes everything to log_path."""
    log(f"\n== {step}")
    t0 = time.time()
    with open(log_path, "w", encoding="utf-8", errors="replace") as logf:
        logf.write(" ".join(f'"{c}"' if " " in c else c for c in cmd) + "\n\n")
        proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                text=True, encoding="utf-8", errors="replace")
        for line in proc.stdout:
            logf.write(line)
            line = line.rstrip()
            if line and show(line):
                log("   " + line)
        code = proc.wait()
    log(f"   ({time.time() - t0:.0f}s, exit {code}, log: {log_path})")
    if code != 0:
        raise BuildError(f"{step} failed (exit {code}). See {log_path}")


def toolbox_lines(line):
    return line.startswith(("Copied", "Opening", "Replacing", "Saving", "Done", "Wrote", "Warning", "Error", "   at "))


def read_json_output(log_path):
    """The JSON a tool printed, from its log (whose first lines are the command and a blank line)."""
    with open(log_path, encoding="utf-8") as f:
        text = f.read()
    return json.loads(text[text.index("\n\n") + 2:])


def load_config(path):
    with open(path, encoding="utf-8") as f:
        cfg = json.load(f)
    for key in ("game_romfs", "model", "blend", "work_dir", "mod_dir", "tools"):
        if key not in cfg:
            raise BuildError(f"{path}: '{key}' is missing")
    builder = "modstudio" if cfg["tools"].get("modstudio") else "toolbox"
    for tool in ("blender", builder, "restbl"):
        if not os.path.exists(cfg["tools"].get(tool, "")):
            raise BuildError(f"{path}: tools.{tool} not found: {cfg['tools'].get(tool)}")
    if not os.path.exists(os.path.join(cfg["game_romfs"], cfg["model"])):
        raise BuildError(f"Game model not found: {os.path.join(cfg['game_romfs'], cfg['model'])}")
    if not os.path.exists(cfg["blend"]):
        raise BuildError(f"Blender file not found: {cfg['blend']}")
    return cfg


def game_version(cfg):
    if cfg.get("game_version"):
        return str(cfg["game_version"]).replace(".", "")
    tables = glob.glob(os.path.join(cfg["game_romfs"], "System", "Resource", "ResourceSizeTable.Product.*.rsizetable.zs"))
    versions = [re.search(r"Product\.(\d+)\.", os.path.basename(t)) for t in tables]
    versions = [m.group(1) for m in versions if m]
    if len(versions) != 1:
        raise BuildError("Could not tell the game version from the game's RESTBL. Set game_version, e.g. \"111\".")
    return versions[0]


def compare_models(vanilla, result):
    """Checks every game mesh kept its material, skin count and LOD count. Returns problems."""
    problems, rows = [], []
    vm = {m["name"]: m for m in vanilla["models"][0]["meshes"]}
    rm = {m["name"]: m for m in result["models"][0]["meshes"]}
    for name, v in vm.items():
        r = rm.get(name)
        if r is None:
            problems.append(f"game mesh {name} is missing from the result")
            continue
        if r["material"] != v["material"]:
            problems.append(f"{name}: material {r['material']} instead of {v['material']}")
        if r["skinCount"] != v["skinCount"]:
            problems.append(f"{name}: skin count {r['skinCount']} instead of {v['skinCount']}")
        if r["lods"] != v["lods"]:
            problems.append(f"{name}: {r['lods']} LODs instead of {v['lods']}")
        rows.append(f"   {name:34} {r['material']:16} verts {v['vertices']:>6} -> {r['vertices']:<6} "
                    f"skin {r['skinCount']}  LODs {r['lods']}")
    for name in rm:
        if name not in vm:
            problems.append(f"{name} is a new mesh; its material may be wrong")
    vt = {t["name"]: t for t in vanilla["textures"]}
    for t in result["textures"]:
        o = vt.get(t["name"])
        if o and (o["format"], o["width"], o["height"]) != (t["format"], t["width"], t["height"]):
            rows.append(f"   texture {t['name']:40} {o['format']} {o['width']}x{o['height']} -> "
                        f"{t['format']} {t['width']}x{t['height']}")
    return problems, rows


def check_scene(cfg, tools, romfs, mod_romfs, work, vanilla_json):
    """Step 5: renders the scene and the built model the same way and compares them (check_result.py).
    Returns the report."""
    check = os.path.join(work, "check")
    if os.path.isdir(check):
        shutil.rmtree(check)
    os.makedirs(check)
    result = os.path.join(check, "result")
    run("Comparing the result with the scene", [tools["modstudio"], "export", cfg["model"], result,
        "--romfs", romfs, "--overlay", mod_romfs], os.path.join(work, "5_check_export.log"), lambda l: False)
    with open(vanilla_json, encoding="utf-8") as f:
        bones = json.load(f)["models"][0]["bones"]
    joined = {}
    report_path = os.path.join(work, "prep_report.json")
    if os.path.exists(report_path):
        with open(report_path, encoding="utf-8") as f:
            joined = {n: target for target, m in json.load(f)["meshes"].items() for n in m.get("joined", [])}
    prep = cfg.get("prep", {})
    glb = glob.glob(os.path.join(result, "*.glb"))
    if len(glb) != 1:
        raise BuildError(f"the check's export made {len(glb)} .glb files in {result}")
    settings = dict(cfg.get("check", {}))
    settings.update({"out": check, "result_glb": glb[0], "game_bones": bones, "joined": joined,
                     "armature": prep.get("armature", "Armature"),
                     "work_suffix": prep.get("work_suffix", "_WORK"), "exclude": prep.get("exclude", [])})
    config = os.path.join(check, "check_config.json")
    with open(config, "w", encoding="utf-8") as f:
        json.dump(settings, f, indent=2)
    script = os.path.join(HERE, "check_result.py")
    show = lambda l: l.startswith("[check]") or "Error" in l  # noqa: E731
    run("   the scene", [tools["blender"], "--background", cfg["blend"], "--python", script, "--",
        "--config", config, "--side", "scene"], os.path.join(work, "5_check_scene.log"), show)
    run("   the result", [tools["blender"], "--background", "--factory-startup", "--python", script, "--",
        "--config", config, "--side", "result"], os.path.join(work, "5_check_result.log"), show)
    with open(os.path.join(check, "check_report.json"), encoding="utf-8") as f:
        report = json.load(f)
    if report["matches"]:
        log("   The result looks like the scene (base colour, every view, rest and test pose).")
    else:
        by_mesh = {}
        for d in report["differences"]:
            for mesh in d["meshes"]:
                by_mesh.setdefault(mesh, []).append(f"{d['view']} {d['pose']} ({d['kind']}, {d['pixels']} px)")
        for mesh, where in sorted(by_mesh.items()):
            log(f"   WARNING: {mesh} looks different from the scene: {', '.join(where)}")
        log(f"   Images (scene | result | differences in red): {os.path.join(check, 'differs_*.png')}")
    for item in report.get("not_compared", []):
        log(f"   Note: not compared exactly (a shader the check can't flatten): {item}")
    return report


def main():
    ap = argparse.ArgumentParser(description="Build a model replacement mod from a Blender scene.")
    ap.add_argument("project", help="project .json (extract_model.py writes one)")
    ap.add_argument("--skip-blender", action="store_true", help="reuse the FBX and textures of the last build")
    ap.add_argument("--install", action="store_true", help="copy the mod to install_dir afterwards")
    ap.add_argument("--no-check", action="store_true", help="skip comparing the result with the scene")
    args = ap.parse_args()

    t_start = time.time()
    cfg = load_config(args.project)
    tools = cfg["tools"]
    romfs = os.path.abspath(cfg["game_romfs"])
    game_model = os.path.join(romfs, cfg["model"])
    work = os.path.abspath(cfg["work_dir"])
    mod_romfs = os.path.join(os.path.abspath(cfg["mod_dir"]), "romfs")
    os.makedirs(work, exist_ok=True)
    use_ms = bool(tools.get("modstudio"))
    fbx = os.path.join(work, "model.fbx")
    glb = os.path.join(work, "model.glb")
    scene_export = glb if use_ms else fbx
    textures = os.path.join(work, "textures")

    log(f"Model:  {game_model}")
    log(f"Scene:  {cfg['blend']} (saved {time.ctime(os.path.getmtime(cfg['blend']))})")
    log(f"Mod:    {mod_romfs}")

    # 1. The game's model
    if use_ms:
        original_textures = os.path.join(work, "original")
        vanilla_json = os.path.join(original_textures, "model.json")
        run("Reading the game's model", [tools["modstudio"], "export", game_model, original_textures,
            "--romfs", romfs], os.path.join(work, "1_inspect_game.log"), lambda l: False)
    else:
        vanilla_json = os.path.join(work, "vanilla.json")
        original_textures = os.path.join(work, "original_textures")
        run("Reading the game's model", [tools["toolbox"], "inspect-model", game_model, "--romfs", romfs,
            "--json", vanilla_json, "--export-textures", original_textures],
            os.path.join(work, "1_inspect_game.log"), lambda l: l.startswith(("Wrote", "Warning", "Error")))

    # 2. Blender
    if not args.skip_blender:
        prep = dict(cfg.get("prep", {}))
        prep.update({"vanilla_json": vanilla_json, "source_textures": original_textures, "output": work})
        if use_ms:
            # modstudio keeps each mesh's own skin count (up to 8); Toolbox's importer keeps 4.
            prep.update({"glb_name": "model.glb", "max_weights": max(8, prep.get("max_weights", 8))})
        else:
            prep["fbx_name"] = "model.fbx"
        prep_config = os.path.join(work, "prep_config.json")
        with open(prep_config, "w", encoding="utf-8") as f:
            json.dump(prep, f, indent=2)
        for stale in (scene_export, os.path.join(work, "prep_report.json")):
            if os.path.exists(stale):
                os.remove(stale)
        try:
            run("Preparing the scene in Blender", [tools["blender"], "--background", cfg["blend"],
                "--python", os.path.join(HERE, "prep_for_game.py"), "--", "--config", prep_config],
                os.path.join(work, "2_blender.log"), lambda l: l.startswith("[prep]") or "Error" in l)
        finally:
            report_path = os.path.join(work, "prep_report.json")
            if os.path.exists(report_path):
                with open(report_path, encoding="utf-8") as f:
                    report = json.load(f)
                if report["errors"]:
                    raise BuildError("Blender found problems:\n  " + "\n  ".join(report["errors"]))
    elif not os.path.exists(scene_export):
        raise BuildError(f"--skip-blender, but there is no {scene_export} from an earlier build")

    # 3. The mod's model and textures
    has_textures = os.path.isdir(textures) and any(f.lower().endswith(".png") for f in os.listdir(textures))
    result_json = os.path.join(work, "result.json")
    if use_ms:
        # Files of an earlier build go first: a texture no longer changed must not stay in the mod.
        model_out = os.path.join(mod_romfs, cfg["model"])
        for stale in [model_out] + glob.glob(os.path.join(mod_romfs, "Tex", "*.bntx.zs")):
            if os.path.exists(stale):
                os.remove(stale)
        replace = [tools["modstudio"], "replace", cfg["model"], glb, mod_romfs, "--romfs", romfs]
        if has_textures:
            replace += ["--textures", textures]
        log_path = os.path.join(work, "3_replace.log")
        run("Building the model and textures (modstudio)", replace, log_path, lambda l: False)
        summary = read_json_output(log_path)
        for shape in summary["shapes"]:
            log(f"   {shape['name']:44} {shape['status']:9} {shape['vertices']:>6} vertices "
                f"{shape['triangles']:>6} triangles")
        textures_done = summary.get("textures") or {}
        for t in textures_done.get("written", []):
            log(f"   texture {t['name']:40} {t['from']} -> {t['to']} {t['width']}x{t['height']}")
        for w in summary["warnings"] + textures_done.get("warnings", []):
            log(f"   Warning: {w}")

        # 4. Check the result
        log_path = os.path.join(work, "4_inspect_result.log")
        run("Reading the result", [tools["modstudio"], "inspect", model_out, "--romfs", romfs], log_path,
            lambda l: False)
        with open(result_json, "w", encoding="utf-8") as f:
            json.dump(read_json_output(log_path), f, indent=2)
    else:
        replace = [tools["toolbox"], "replace-model", "--model", game_model, "--fbx", fbx,
                   "--out", mod_romfs, "--romfs", romfs]
        if has_textures:
            replace += ["--textures", textures]
        replace += cfg.get("toolbox_options", [])
        run("Replacing the model in Toolbox", replace, os.path.join(work, "3_toolbox_replace.log"), toolbox_lines)

        # 4. Check the result
        run("Reading the result", [tools["toolbox"], "inspect-model", os.path.join(mod_romfs, cfg["model"]),
            "--romfs", romfs, "--json", result_json], os.path.join(work, "4_inspect_result.log"),
            lambda l: l.startswith(("Warning", "Error")))
    with open(vanilla_json, encoding="utf-8") as f:
        vanilla = json.load(f)
    with open(result_json, encoding="utf-8") as f:
        result = json.load(f)
    problems, rows = compare_models(vanilla, result)
    log("\n".join(rows))
    if problems:
        raise BuildError("The result doesn't match the game's model:\n  " + "\n  ".join(problems))

    # 5. Does it look like the scene?
    check = None
    if use_ms and not args.no_check:
        check = check_scene(cfg, tools, romfs, mod_romfs, work, vanilla_json)

    # 6. RESTBL (the last build's table goes first: restbl would count it as one of the mod's files)
    for stale in glob.glob(os.path.join(mod_romfs, "System", "Resource", "*.rsizetable*")):
        os.remove(stale)
    version = game_version(cfg)
    run(f"Resource size table ({cfg.get('game', 'oot')} {version})",
        [tools["restbl"], os.path.abspath(cfg["mod_dir"]), "--game", cfg.get("game", "oot"), "--version", version],
        os.path.join(work, "6_restbl.log"))

    # 7. Install
    if args.install:
        target = cfg.get("install_dir")
        if not target:
            raise BuildError("--install needs install_dir in the project file")
        if os.path.basename(os.path.normpath(target)).lower() in ("mods", "load"):
            # (the mod's own folder is needed: Hookshot and emulators read <mods>\<name>\romfs, never <mods>\romfs)
            raise BuildError(f"install_dir is the mods folder itself ({target}); "
                             f"name the mod's folder in it, e.g. {os.path.join(target, cfg.get('name', 'MyMod'))}")
        log(f"\n== Installing to {target}")
        # The mod's folder gets exactly this build: files of an earlier one (a texture no longer changed) go.
        if os.path.isdir(os.path.join(target, "romfs")):
            shutil.rmtree(os.path.join(target, "romfs"))
        # Without the mod's own RESTBL: the game reads one table, so one is made for all installed mods
        shutil.copytree(mod_romfs, os.path.join(target, "romfs"), dirs_exist_ok=True,
                        ignore=lambda d, names: [n for n in names if ".rsizetable" in n.lower()])
        for stale in glob.glob(os.path.join(target, "romfs", "System", "Resource", "*.rsizetable*")):
            os.remove(stale)
        restbl_all.rebuild(os.path.dirname(os.path.normpath(target)), tools["restbl"], cfg.get("game", "oot"),
                           version, work, log)

    files = sorted(os.path.relpath(p, mod_romfs) for p in glob.glob(os.path.join(mod_romfs, "**", "*"), recursive=True)
                   if os.path.isfile(p))
    log(f"\nBuilt {cfg.get('name', 'mod')} in {time.time() - t_start:.0f}s:")
    for f in files:
        log(f"   romfs\\{f}")
    if check is not None and not check["matches"]:
        meshes = sorted({m for d in check["differences"] for m in d["meshes"]})
        log(f"\nWARNING: the result doesn't look like the scene in {len(check['differences'])} place(s), on "
            f"{', '.join(meshes)}. See {os.path.join(work, 'check', 'differs_*.png')}")


if __name__ == "__main__":
    try:
        main()
    except BuildError as e:
        log(f"\nBUILD FAILED: {e}")
        sys.exit(1)
