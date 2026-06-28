import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Built as a self-contained sub-app for now. It is NOT wired into the Tauri
// window yet (tauri.conf.json still serves ../dist), so the existing static
// shell and the embedded ComfyUI canvas are unaffected. A later PR will mount
// this build inside the desktop app.
export default defineConfig({
  plugins: [react()],
  // Relative base so the build can be served from a sub-path when embedded.
  base: "./",
  build: {
    outDir: "dist",
  },
  test: {
    globals: true,
    environment: "node",
  },
});
