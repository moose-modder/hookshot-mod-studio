# Third-party code and references

## Ported code

| Source | License | Used for |
|---|---|---|
| [Syroot NintenTools](https://gitlab.com/Syroot/NintenTools) (Syroot), as maintained in [KillzXGaming/LegacySwitchLibraries](https://github.com/KillzXGaming/LegacySwitchLibraries) | MIT, [LICENSES/Syroot-NintenTools-MIT.txt](LICENSES/Syroot-NintenTools-MIT.txt) | byte layouts of BFRES and BNTX (`modstudio-formats`). Baseline: commit `0e568c2` (2026-11-06). |

LegacySwitchLibraries states "(c) Syroot, licensed under MIT" in its project files but has no LICENSE file of its
own. The license text in `LICENSES/` ("Copyright (c) 2017 Syroot") is the one kept in
[KillzXGaming/NintenTools.Bfres](https://github.com/KillzXGaming/NintenTools.Bfres), the fork of Syroot's original
library.

## Vendored crates

Copied into this repository as workspace crates, with their license files, and changed here; each crate's README
lists the changes ("Changes in this copy").

| Source | License | Used for |
|---|---|---|
| [havok-formats](https://github.com/Kalmarv/havok-formats) (Kalmarv), commit `fa26978` (0.10.0, 2026-11-06), in `crates/havok-formats` | GPL-3.0-or-later, [crates/havok-formats/LICENSE](crates/havok-formats/LICENSE) | reading Havok tagfiles: the game's cloth files (`.bphcl`) |
| [havok-cloth](https://github.com/Kalmarv/havok-cloth) (Kalmarv), commit `d424ff4` (0.3.1, 2026-11-06), in `crates/havok-cloth` | GPL-3.0-or-later, [crates/havok-cloth/LICENSE](crates/havok-cloth/LICENSE) | simulating the game's cloth, for previews |

Both contain no Havok SDK code (their READMEs). havok-formats depends on `primase-foundations` (Kalmarv,
GPL-3.0-or-later; byte readers, from crates.io, its optional codecs off).

## References

| Source | License | Used as |
|---|---|---|
| [Switch Toolbox](https://github.com/KillzXGaming/Switch-Toolbox) (KillzXGaming and contributors) | GPL-3.0 | reference for model import/export logic (attribute formats, LODs, skinning), and as a local test reference |
| [tegra_swizzle](https://github.com/ScanMountGoat/tegra_swizzle) (SMG), and [KillzXGaming's fork](https://github.com/KillzXGaming/tegra_swizzle) with the Orin layout | MIT | the block-linear layout and its two GOB patterns, from the Tegra X1 and Orin manuals (`modstudio-formats/src/tegra.rs`) |

## Not shipped

- **Blender** is installed by the user; the scripts in `pipeline/` run inside it.
- The resource size table is worked out here from the game's own table (`docs/plan.md`, phase 3); no code of
  RESTBL Calculator (AGPL-3.0), which the pipeline ran as a separate program before, is used. Its output was
  only compared with.

## Dependencies

- Rust crates are listed in `Cargo.lock` and checked by `cargo deny` (`deny.toml`). The larger ones: `ruzstd`
  (zstd, MIT), `texture2ddecoder` (ASTC and BC decoding, MIT/Apache-2.0), `png`, `serde_json`; for the app,
  `tauri` and `tauri-plugin-dialog` (MIT/Apache-2.0), whose HTML/CSS crates are MPL-2.0.
- The app's web side (`app/package.json`, locked in `app/package-lock.json`): `three` (MIT, the 3D view),
  `svelte` (MIT), `@tauri-apps/api` and `@tauri-apps/plugin-dialog` (MIT/Apache-2.0); to build it, `vite` (MIT),
  `typescript` (Apache-2.0) and `@tauri-apps/cli` (MIT/Apache-2.0).
