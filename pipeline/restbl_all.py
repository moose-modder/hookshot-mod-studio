"""One resource size table (RESTBL) for all installed mods.

    python restbl_all.py [--config config.json]

The game reads one RESTBL. When several mods each ship one, Hookshot uses the mod whose folder sorts last, so
the other mods' grown files get their vanilla sizes and fail to load (a texture then shows as a default one).
This works out a table over the files of every enabled mod (as Hookshot overlays them) and installs it as its own
mod, TABLE_MOD, which sorts after the others. build_mod.py --install runs it; run it by hand after adding,
removing or turning off a mod.

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

HERE = os.path.dirname(os.path.abspath(__file__))
TABLE_MOD = "zzz-ResourceSizeTable"
TABLE_DIR = os.path.join("System", "Resource")


def is_table(rel):
    return os.path.dirname(rel).lower() == TABLE_DIR.lower() and ".rsizetable" in os.path.basename(rel).lower()


def game_version(game_romfs, configured=None):
    if configured:
        return str(configured).replace(".", "")
    tables = glob.glob(os.path.join(game_romfs, TABLE_DIR, "ResourceSizeTable.Product.*.rsizetable.zs"))
    versions = {m.group(1) for m in (re.search(r"Product\.(\d+)\.", os.path.basename(t)) for t in tables) if m}
    if len(versions) != 1:
        raise RuntimeError("Could not tell the game version from the game's RESTBL. Set game_version, e.g. \"111\".")
    return versions.pop()


def enabled_mods(mods_dir):
    """Mods as Hookshot reads them: folders with a romfs, not starting with '-' or '_', sorted ignoring case
    (a later mod's file wins)."""
    names = [n for n in os.listdir(mods_dir)
             if n != TABLE_MOD and not n.startswith(("-", "_")) and os.path.isdir(os.path.join(mods_dir, n, "romfs"))]
    return sorted(names, key=str.lower)


def rebuild(mods_dir, restbl, game, version, work_dir, log=print):
    """Writes <mods_dir>/TABLE_MOD/romfs/System/Resource/<table>. Returns the table's path, or None if no mod
    changes a file."""
    mods = enabled_mods(mods_dir)
    files = {}
    others = []
    for mod in mods:
        romfs = os.path.join(mods_dir, mod, "romfs")
        for root, _, names in os.walk(romfs):
            for name in names:
                rel = os.path.relpath(os.path.join(root, name), romfs)
                if is_table(rel):
                    others.append(mod)
                else:
                    files[rel.lower()] = (rel, os.path.join(root, name))
    table_mod = os.path.join(mods_dir, TABLE_MOD)
    if not files:
        if os.path.isdir(table_mod):
            shutil.rmtree(table_mod)
        log("No mod files: no resource size table needed")
        return None

    # All mods' files in one folder, as one mod
    combined = os.path.join(work_dir, "AllMods")
    if os.path.isdir(combined):
        shutil.rmtree(combined)
    for rel, src in files.values():
        dst = os.path.join(combined, "romfs", rel)
        os.makedirs(os.path.dirname(dst), exist_ok=True)
        shutil.copyfile(src, dst)
    cmd = [restbl, combined, "--game", game, "--version", version]
    proc = subprocess.run(cmd, capture_output=True, text=True, encoding="utf-8", errors="replace")
    with open(os.path.join(work_dir, "restbl_all.log"), "w", encoding="utf-8") as f:
        f.write(" ".join(cmd) + "\n\n" + proc.stdout + proc.stderr)
    if proc.returncode != 0:
        raise RuntimeError(f"restbl failed (exit {proc.returncode}): {(proc.stdout + proc.stderr).strip()[-2000:]}")
    made = glob.glob(os.path.join(combined, "romfs", TABLE_DIR, "*.rsizetable*"))
    if not made and "No RESTBL needed" in proc.stdout:
        # Every mod file fits the game's own table (e.g. a model that didn't grow): no table, and an old one goes.
        if os.path.isdir(table_mod):
            shutil.rmtree(table_mod)
        shutil.rmtree(combined)
        log(f"No resource size table needed for {len(mods)} mod(s) {mods}: every file fits the game's own table")
        return None
    if len(made) != 1:
        raise RuntimeError(f"restbl wrote {len(made)} tables in {combined}; expected one")

    if os.path.isdir(table_mod):
        shutil.rmtree(table_mod)
    out = os.path.join(table_mod, "romfs", TABLE_DIR, os.path.basename(made[0]))
    os.makedirs(os.path.dirname(out), exist_ok=True)
    shutil.copyfile(made[0], out)
    shutil.rmtree(combined)
    summary = [l.strip() for l in proc.stdout.splitlines() if "aren't vanilla" in l]
    log(f"Resource size table for {len(mods)} mod(s) {mods}: {out}")
    for line in summary:
        log("   " + line)
    if others:
        log(f"   (their own tables are not used: {sorted(set(others))})")
    return out


def main():
    ap = argparse.ArgumentParser(description="One resource size table for all installed mods.")
    ap.add_argument("--config", default=os.path.join(HERE, "config.json"))
    a = ap.parse_args()
    with open(a.config, encoding="utf-8") as f:
        cfg = json.load(f)
    version = game_version(cfg["game_romfs"], cfg.get("game_version"))
    work = os.path.join(cfg["workspace"], ".restbl_all")
    os.makedirs(work, exist_ok=True)
    rebuild(cfg["install_mods"], cfg["tools"]["restbl"], cfg.get("game", "oot"), version, work)


if __name__ == "__main__":
    try:
        main()
    except RuntimeError as e:
        print(f"FAILED: {e}")
        sys.exit(1)
