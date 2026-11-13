import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// The app's web side, served to Tauri's window (src-tauri/tauri.conf.json): a dev server on a fixed port
// for `npm run tauri dev`, built into dist/ for `npm run tauri build`.
export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  build: {
    target: "es2022",
    outDir: "dist",
    chunkSizeWarningLimit: 2000,
  },
});
