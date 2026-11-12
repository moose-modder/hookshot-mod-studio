"""Builds a model replacement mod from a Blender scene in one go.

    python build_mod.py <project>.json [--skip-blender] [--install]

Steps (see README.md):
  1. Toolbox inspect-model: facts and original textures of the game's model
  2. Blender (background, the .blend is not changed): prep_for_game.py -> FBX + baked textures
  3. Toolbox replace-model: game model + FBX + textures -> <mod>/romfs
  4. Toolbox inspect-model: checks the result against the game's model
  5. restbl: resource size table for the mod
  6. With --install: copies the mod's romfs to install_dir (without its RESTBL) and makes one RESTBL for all
     installed mods (restbl_all.py)

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


def load_config(path):
    with open(path, encoding="utf-8") as f:
        cfg = json.load(f)
    for key in ("game_romfs", "model", "blend", "work_dir", "mod_dir", "tools"):
        if key not in cfg:
            raise BuildError(f"{path}: '{key}' is missing")
    for tool in ("blender", "toolbox", "restbl"):
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


def main():
    ap = argparse.ArgumentParser(description="Build a model replacement mod from a Blender scene.")
    ap.add_argument("project", help="project .json (extract_model.py writes one)")
    ap.add_argument("--skip-blender", action="store_true", help="reuse the FBX and textures of the last build")
    ap.add_argument("--install", action="store_true", help="copy the mod to install_dir afterwards")
    args = ap.parse_args()

    t_start = time.time()
    cfg = load_config(args.project)
    tools = cfg["tools"]
    romfs = os.path.abspath(cfg["game_romfs"])
    game_model = os.path.join(romfs, cfg["model"])
    work = os.path.abspath(cfg["work_dir"])
    mod_romfs = os.path.join(os.path.abspath(cfg["mod_dir"]), "romfs")
    os.makedirs(work, exist_ok=True)
    fbx = os.path.join(work, "model.fbx")
    textures = os.path.join(work, "textures")

    log(f"Model:  {game_model}")
    log(f"Scene:  {cfg['blend']} (saved {time.ctime(os.path.getmtime(cfg['blend']))})")
    log(f"Mod:    {mod_romfs}")

    # 1. The game's model
    vanilla_json = os.path.join(work, "vanilla.json")
    original_textures = os.path.join(work, "original_textures")
    run("Reading the game's model", [tools["toolbox"], "inspect-model", game_model, "--romfs", romfs,
        "--json", vanilla_json, "--export-textures", original_textures],
        os.path.join(work, "1_inspect_game.log"), lambda l: l.startswith(("Wrote", "Warning", "Error")))

    # 2. Blender
    if not args.skip_blender:
        prep = dict(cfg.get("prep", {}))
        prep.update({"vanilla_json": vanilla_json, "source_textures": original_textures,
                     "output": work, "fbx_name": "model.fbx"})
        prep_config = os.path.join(work, "prep_config.json")
        with open(prep_config, "w", encoding="utf-8") as f:
            json.dump(prep, f, indent=2)
        for stale in (fbx, os.path.join(work, "prep_report.json")):
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
    elif not os.path.exists(fbx):
        raise BuildError(f"--skip-blender, but there is no {fbx} from an earlier build")

    # 3. Toolbox
    replace = [tools["toolbox"], "replace-model", "--model", game_model, "--fbx", fbx,
               "--out", mod_romfs, "--romfs", romfs]
    if os.path.isdir(textures) and os.listdir(textures):
        replace += ["--textures", textures]
    replace += cfg.get("toolbox_options", [])
    run("Replacing the model in Toolbox", replace, os.path.join(work, "3_toolbox_replace.log"), toolbox_lines)

    # 4. Check the result
    result_json = os.path.join(work, "result.json")
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

    # 5. RESTBL
    version = game_version(cfg)
    run(f"Resource size table ({cfg.get('game', 'oot')} {version})",
        [tools["restbl"], os.path.abspath(cfg["mod_dir"]), "--game", cfg.get("game", "oot"), "--version", version],
        os.path.join(work, "5_restbl.log"))

    # 6. Install
    if args.install:
        target = cfg.get("install_dir")
        if not target:
            raise BuildError("--install needs install_dir in the project file")
        if os.path.basename(os.path.normpath(target)).lower() in ("mods", "load"):
            # (the mod's own folder is needed: Hookshot and emulators read <mods>\<name>\romfs, never <mods>\romfs)
            raise BuildError(f"install_dir is the mods folder itself ({target}); "
                             f"name the mod's folder in it, e.g. {os.path.join(target, cfg.get('name', 'MyMod'))}")
        log(f"\n== Installing to {target}")
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


if __name__ == "__main__":
    try:
        main()
    except BuildError as e:
        log(f"\nBUILD FAILED: {e}")
        sys.exit(1)
