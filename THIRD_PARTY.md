# Third-party code and references

## Ported code

| Source | License | Used for |
|---|---|---|
| [Syroot NintenTools](https://gitlab.com/Syroot/NintenTools) (Syroot), as maintained in [KillzXGaming/LegacySwitchLibraries](https://github.com/KillzXGaming/LegacySwitchLibraries) | MIT, [LICENSES/Syroot-NintenTools-MIT.txt](LICENSES/Syroot-NintenTools-MIT.txt) | byte layouts of BFRES and BNTX (`modstudio-formats`). Baseline: commit `0e568c2` (2026-11-06). |

LegacySwitchLibraries states "(c) Syroot, licensed under MIT" in its project files but has no LICENSE file of its
own. The license text in `LICENSES/` ("Copyright (c) 2017 Syroot") is the one kept in
[KillzXGaming/NintenTools.Bfres](https://github.com/KillzXGaming/NintenTools.Bfres), the fork of Syroot's original
library.

## References

| Source | License | Used as |
|---|---|---|
| [Switch Toolbox](https://github.com/KillzXGaming/Switch-Toolbox) (KillzXGaming and contributors) | GPL-3.0 | reference for model import/export logic (attribute formats, LODs, skinning), and as a local test reference |
| [tegra_swizzle](https://github.com/ScanMountGoat/tegra_swizzle) (SMG), and [KillzXGaming's fork](https://github.com/KillzXGaming/tegra_swizzle) with the Orin layout | MIT | the block-linear layout and its two GOB patterns, from the Tegra X1 and Orin manuals (`modstudio-formats/src/tegra.rs`) |

## Not shipped

- **Blender** is installed by the user; the scripts in `pipeline/` run inside it.
- Rust crates are listed in `Cargo.lock` and checked by `cargo deny` (`deny.toml`). The larger ones: `ruzstd`
  (zstd, MIT), `texture2ddecoder` (ASTC and BC decoding, MIT/Apache-2.0), `png`, `serde_json`.
