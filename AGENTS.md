# Notes for AI coding agents

Read [README.md](README.md) and [docs/plan.md](docs/plan.md) first.

- **No game files, and nothing made from them, leave the PC.** That covers models, textures, archives, exported
  images and meshes, `.blend` files, JSON dumps of a model's contents, built mods, and screenshots. Never commit them,
  paste them into issues, or use them in CI. `.gitignore` covers the usual extensions; check `git status` anyway.
- **Tests without the game.** Unit tests build their input in code (synthetic models, textures, archives).
  Checks against a real dump (round trips, comparisons with Switch Toolbox's output, in-game runs) are done locally
  and reported in the PR, never automated in CI.
- **Writers edit in place.** When writing a game file, keep the original's bytes and replace only what changed.
  Parts we don't model (materials, shader data, unknown fields) pass through untouched. An unchanged model read
  and written again must give a byte-identical file.
- **Porting.** Byte layouts come from Syroot's NintenTools libraries (MIT) as maintained in
  KillzXGaming/LegacySwitchLibraries; the baseline commit is in `THIRD_PARTY.md`. Name the source file a ported part
  comes from in a comment, and update `THIRD_PARTY.md` when porting from somewhere new.
- **Dependencies** must pass `cargo deny check` (`deny.toml`). Prefer writing small, well-defined parts (BC1/4/5
  encoding, SARC) over adding crates; never add prebuilt binaries.
- `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` must pass.
- The Python in `pipeline/` runs inside Blender 5.x (`bpy`); keep it working with the pinned Blender version. The
  CLI and the app carry these scripts built in (`include_str!` in `modstudio-core/src/tools.rs`): rebuild after
  changing one, or set `MODSTUDIO_PIPELINE` to the `pipeline` folder to use it as it is.
- The app (`app/`): `npm ci`, then `npm run check` (svelte-check, no warnings) and `npm run build` must pass. Its
  Rust side only calls `modstudio-core`; put new behaviour there, where the CLI gets it too and tests can reach it.
- Checking the app's page: start it with `MODSTUDIO_WEBVIEW_ARGS=--remote-debugging-port=<port>` and drive it
  through DevTools; `MODSTUDIO_SETTINGS=<file>` points it at other folders so real projects and mods stay as
  they are. Never screenshot the desktop (other windows are the user's).
