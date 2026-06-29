import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// The single H-Gripe Desktop front end. Builds to the Tauri `frontendDist`
// (../dist); the output is gitignored and produced by the Tauri before* hooks.
export default defineConfig({
  plugins: [react()],
  // Relative base so assets resolve correctly when served via tauri://localhost.
  base: "./",
  build: {
    outDir: "../dist",
    emptyOutDir: true,
  },
  test: {
    globals: true,
    environment: "node",
  },
});
