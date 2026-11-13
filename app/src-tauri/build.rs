fn main() {
    // The web side is built into ../dist by `npm run build` (tauri build runs it). Tauri needs the folder to
    // exist to compile, so `cargo build` / `cargo clippy --workspace` work without Node (with an empty page).
    let _ = std::fs::create_dir_all("../dist");
    tauri_build::build();
}
