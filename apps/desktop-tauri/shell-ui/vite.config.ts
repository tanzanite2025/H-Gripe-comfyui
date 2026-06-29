import { defineConfig } from "vite";

// The dependency-free desktop shell (Dashboard / Credentials / Profiles / Run /
// History / PSD / Advanced Canvas). Built into the Tauri frontendDist (../dist)
// that the webview loads directly. The React Node Editor sub-app (studio-ui)
// builds separately into ../dist/studio; the Tauri before* hooks build this
// shell FIRST (emptying ../dist) and studio-ui SECOND, so dist/studio survives.
export default defineConfig({
  // Relative base so hashed assets resolve under Tauri's custom protocol.
  base: "./",
  build: {
    outDir: "../dist",
    // outDir is outside the Vite root, so emptying must be opt-in. Safe because
    // the shell build runs before studio-ui writes dist/studio.
    emptyOutDir: true,
    rollupOptions: {
      // Keep the embedded Node Editor build (served at /studio) out of the
      // shell graph; it is fetched/iframed at runtime, not imported.
      external: [/^studio\//],
    },
  },
});
