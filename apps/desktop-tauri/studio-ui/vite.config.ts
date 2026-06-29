import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Built into the desktop shell's frontendDist under `/studio` so the shell can
// embed it as the "Node Editor" tab (iframe -> studio/index.html). The build
// output (../dist/studio) is gitignored and produced by tauri before* hooks.
export default defineConfig({
  plugins: [react()],
  // Relative base so assets resolve correctly when served from /studio/.
  base: "./",
  build: {
    outDir: "../dist/studio",
    emptyOutDir: true,
  },
  test: {
    globals: true,
    environment: "node",
  },
});
