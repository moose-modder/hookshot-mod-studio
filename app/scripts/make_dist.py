"""Makes a release to hand out as a zip: dist/HookshotModStudio (and the source beside it).

    python app/scripts/make_dist.py

1. The app's web page (npm run build in app/).
2. The app and the CLI built for release in target/dist, with this PC's paths in them mapped to neutral names
   (no user folders in the files) and the debug-info link naming only the .pdb.
3. dist/HookshotModStudio: hookshot-mod-studio.exe, modstudio.exe, a blank settings.json, README.txt, LICENSE,
   LICENSES/, THIRD_PARTY.md, and THIRD_PARTY_LICENSES.txt (the license of every Rust crate and web package built
   into the programs).
4. Checks that neither program holds the home folder's path.
5. dist/hookshot-mod-studio-source.zip: the source of the current commit (git archive), which the GPL asks to go
   with the program.

Needs cargo (rustup's, or ~/.cargo/bin), npm and git. Only the standard library.
"""

import hashlib
import json
import os
import shutil
import subprocess
import sys
import zipfile
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
APP = REPO / "app"
DIST = REPO / "dist"
OUT = DIST / "HookshotModStudio"
TARGET = REPO / "target" / "dist"
TRIPLE = "x86_64-pc-windows-msvc"


def run(cmd, cwd=REPO, env=None, capture=False):
    print(">", " ".join(str(c) for c in cmd), flush=True)
    shell = os.name == "nt" and cmd[0] in ("npm", "npx")
    if env is not None and not shell:
        # (Windows looks programs up on this process's PATH, not the one passed on)
        cmd = [shutil.which(cmd[0], path=env["PATH"]) or cmd[0]] + list(cmd[1:])
    r = subprocess.run(cmd, cwd=cwd, env=env, shell=shell, text=True,
                       capture_output=capture, encoding="utf-8", errors="replace")
    if r.returncode != 0:
        if capture:
            print(r.stdout, r.stderr)
        sys.exit(f"failed: {' '.join(str(c) for c in cmd)}")
    return r.stdout if capture else None


def cargo_env():
    env = dict(os.environ)
    home = Path.home()
    cargo_home = Path(env.get("CARGO_HOME", home / ".cargo"))
    env["PATH"] = str(cargo_home / "bin") + os.pathsep + env.get("PATH", "")
    env["CARGO_TARGET_DIR"] = str(TARGET)
    # The last matching prefix wins: the general one first.
    flags = [
        f"--remap-path-prefix={home}=~",
        f"--remap-path-prefix={cargo_home / 'registry' / 'src'}=cargo",
        f"--remap-path-prefix={cargo_home / 'git' / 'checkouts'}=cargo-git",
        f"--remap-path-prefix={REPO}=hookshot-mod-studio",
    ]
    if os.name == "nt":
        flags.append("-Clink-arg=/PDBALTPATH:%_PDB%")
    env["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(flags)
    env.pop("RUSTFLAGS", None)
    return env


def version():
    for line in (REPO / "Cargo.toml").read_text(encoding="utf-8").splitlines():
        if line.startswith("version"):
            return line.split('"')[1]
    return "0.0.0"


# ---------------------------------------------------------------- licenses

LICENSE_NAMES = ("license", "licence", "copying", "notice", "unlicense")


def license_files(folder):
    if not folder.is_dir():
        return []
    return sorted(p for p in folder.iterdir()
                  if p.is_file() and p.name.lower().startswith(LICENSE_NAMES))


def rust_packages(env):
    """(name, version, license, folder) of every crate built into the two programs (normal dependencies for
    Windows; build-time tools aren't in the programs)."""
    tree = run(["cargo", "tree", "-p", "modstudio-app", "-p", "modstudio-cli", "--target", TRIPLE, "-e", "normal",
                "--prefix", "none", "-f", "{p}"], env=env, capture=True)
    wanted = set()
    for line in tree.splitlines():
        parts = line.replace(" (*)", "").split()
        if len(parts) >= 2 and parts[1].startswith("v") and "(" not in line.replace(" (*)", ""):
            wanted.add((parts[0], parts[1][1:]))
    meta = json.loads(run(["cargo", "metadata", "--format-version", "1", "--filter-platform", TRIPLE],
                          env=env, capture=True))
    out = []
    for p in meta["packages"]:
        if (p["name"], p["version"]) in wanted:
            out.append((p["name"], p["version"], p.get("license") or p.get("license_file") or "?",
                        Path(p["manifest_path"]).parent))
    return sorted(out)


def web_packages():
    """(name, version, license, folder) of the packages bundled into the page: the app's dependencies and Svelte's
    runtime, with what they depend on."""
    nm = APP / "node_modules"
    seen = {}
    todo = list(json.loads((APP / "package.json").read_text(encoding="utf-8"))["dependencies"]) + ["svelte"]
    while todo:
        name = todo.pop()
        if name in seen:
            continue
        pkg = json.loads((nm / name / "package.json").read_text(encoding="utf-8"))
        seen[name] = (name, pkg["version"], pkg.get("license", "?"), nm / name)
        todo += list(pkg.get("dependencies", {}))
    return sorted(seen.values())


def write_third_party(path, rust, web):
    lines = ["Third-party software in Hookshot Mod Studio", "",
             "The programs are built from these Rust crates and web packages, under their own licenses (texts below,",
             "each followed by the packages it comes with). See THIRD_PARTY.md for code ported or used as a reference.",
             ""]
    texts = {}
    for title, pkgs in (("Rust crates (in hookshot-mod-studio.exe and modstudio.exe)", rust),
                        ("Web packages (in the app's page, inside hookshot-mod-studio.exe)", web)):
        lines += [f"== {title}", ""]
        for name, ver, lic, folder in pkgs:
            files = license_files(folder)
            lines.append(f"{name} {ver}: {lic}" + ("" if files else "  (the package carries no license file)"))
            for f in files:
                text = f.read_text(encoding="utf-8", errors="replace").strip()
                key = hashlib.sha256(text.encode()).hexdigest()
                texts.setdefault(key, (text, []))[1].append(f"{name} {ver} ({f.name})")
        lines.append("")
    lines += ["A package marked as carrying no license file is under the license it names; its source, with its",
              "copyright notices, is on crates.io (Rust crates) or npmjs.com (web packages), and the texts of those",
              "licenses are among the texts below.", ""]
    lines += ["== License texts", ""]
    for text, users in texts.values():
        lines += ["-" * 100, "For: " + ", ".join(users), "-" * 100, text, ""]
    path.write_text("\n".join(lines), encoding="utf-8")
    return len(rust), len(web), len(texts)


README = """Hookshot Mod Studio {version}

Model mods for Hookshot: pick a model from your own Hookshot install, edit it in Blender, then build the mod and
install it into Hookshot's mods folder.

You need
- Windows 10 or 11 with the Microsoft Edge WebView2 Runtime (Windows 11 has it; on Windows 10 it comes with
  Microsoft Edge, or get it from Microsoft)
- a Hookshot install (its romfs and mods folders)
- Blender 5 or newer

Start
1. Unzip this folder somewhere you can write to (not Program Files): the app keeps its settings in settings.json,
   next to the program.
2. Run hookshot-mod-studio.exe. The Settings page asks for your Hookshot folder (which fills in the game files and
   mods folders), a folder for your projects, and Blender.
3. Models: pick one and make a project. Projects: open it in Blender, edit, save, then Build and install. Restart
   Hookshot to see the mod. Installed mods: turn mods on and off.

modstudio.exe does the same from the command line ("modstudio help") and shares settings.json.

Nothing from the game is in these programs: models and textures are read from your own Hookshot install, and what
you make from them stays on your PC.

License: GPL-3.0-or-later (LICENSE). The source code of this version is in hookshot-mod-studio-source.zip, handed
out with this folder. Third-party software: THIRD_PARTY.md and THIRD_PARTY_LICENSES.txt.
"""

BLANK_SETTINGS = {"game": "oot", "game_romfs": "", "install_mods": "", "workspace": "", "tools": {}}


def main():
    env = cargo_env()
    ver = version()
    run(["npm", "run", "build"], cwd=APP)
    run(["cargo", "build", "--release", "-p", "modstudio-app", "--features", "tauri/custom-protocol"], env=env)
    run(["cargo", "build", "--release", "-p", "modstudio-cli"], env=env)

    if OUT.exists():
        shutil.rmtree(OUT)
    OUT.mkdir(parents=True)
    for exe in ("hookshot-mod-studio.exe", "modstudio.exe"):
        shutil.copy2(TARGET / "release" / exe, OUT / exe)
    (OUT / "settings.json").write_text(json.dumps(BLANK_SETTINGS, indent=2) + "\n", encoding="utf-8")
    (OUT / "README.txt").write_text(README.format(version=ver), encoding="utf-8")
    shutil.copy2(REPO / "LICENSE", OUT / "LICENSE")
    shutil.copytree(REPO / "LICENSES", OUT / "LICENSES")
    shutil.copy2(REPO / "THIRD_PARTY.md", OUT / "THIRD_PARTY.md")
    crates, packages, texts = write_third_party(OUT / "THIRD_PARTY_LICENSES.txt", rust_packages(env), web_packages())
    print(f"licenses: {crates} crates, {packages} web packages, {texts} distinct license texts")

    home = str(Path.home()).encode()
    for exe in ("hookshot-mod-studio.exe", "modstudio.exe"):
        data = (OUT / exe).read_bytes()
        for form in (home, home.replace(b"\\", b"/"), home.replace(b"\\", b"\\\\")):
            if form.lower() in data.lower():
                sys.exit(f"{exe} still holds {form.decode()}")
    print("checked: no home folder path in the programs")

    source = DIST / "hookshot-mod-studio-source.zip"
    run(["git", "archive", "--format=zip", "--prefix=hookshot-mod-studio/", "-o", str(source), "HEAD"])
    if run(["git", "status", "--porcelain"], capture=True).strip():
        print("note: the working tree has uncommitted changes; the source zip is the last commit")
    with zipfile.ZipFile(source) as z:
        print(f"source: {source.name}, {len(z.namelist())} files")
    print(f"done: {OUT}")


if __name__ == "__main__":
    main()
