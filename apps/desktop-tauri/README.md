# H-Gripe Desktop (Tauri shell)

Phase 2 desktop shell for H-Gripe. It wraps the `hgripe-api` crate in a
[Tauri](https://tauri.app) window and exposes the API-first workflow.

## Positioning: shell + production panels + Advanced Canvas

The desktop app is intentionally **three layers**, and ComfyUI is *not* the
product's main surface — it is embedded as an advanced/escape-hatch canvas:

```
H-Gripe Desktop
  ├─ Shell        Dashboard · Credentials · Profiles · History · Outputs
  ├─ Production   PSD Studio · API Image · Batch Job (H-Gripe's own panels)
  └─ Advanced Canvas  embedded ComfyUI node editor (advanced workflows only)
```

Day-to-day production should go through H-Gripe's own panels. The embedded
ComfyUI (the **Advanced Canvas** tab, opened last in the nav and not by
default) is for complex node debugging, legacy workflows, and mature plugins —
it is a high-level canvas, not the final main UI. The app opens on
**Dashboard**, not on ComfyUI.

## Tabs

- **Dashboard** (default) – runtime paths + `doctor` diagnostics.
- **PSD Studio** – H-Gripe's production entry point. Compose a job from a
  provider profile + prompt + reference image + PSD template, preview the
  resulting `ApiTask`, run it through the broker, and open the outputs. The PSD
  template path is carried on the task (as `inputs.template_path`) so a future
  export step can write the generated image back into the template.
- **Credentials / Profiles** – view summaries, validate, and edit
  `credentials.json` / `provider_profiles.json` in place.
- **Run Task** – submit an `ApiTask` JSON payload to the broker and inspect the
  `ApiResult`.
- **History** – list / view / rerun / clean up recorded tasks (SQLite history).
- **PSD** – browse PSD exports (preview / metadata / smart-object markers).
- **Node Editor** – H-Gripe's own visual workflow canvas (the `studio-ui`
  React Flow sub-app), embedded as an iframe. This is the in-house production
  node graph (renderer-agnostic graph model + typed ports + DAG runtime); the
  **Advanced Canvas** below stays as the ComfyUI escape hatch. See
  [`studio-ui/`](studio-ui/) for the sub-app. The build is served at
  `dist/studio/` and loaded lazily on first open.
- **Advanced Canvas** – start/stop a local ComfyUI server and embed its full
  web UI in an `<iframe>` inside the app (it also offers an "Open in browser"
  escape hatch). This replaces the earlier "open in the system browser" flow.

The shell frontend is a dependency-free static page (`dist/`) using Tauri's
global API (`window.__TAURI__`); the Rust backend lives in `src-tauri/`. The
**Node Editor** is the one exception: it is a Vite + React + TypeScript sub-app
in [`studio-ui/`](studio-ui/) whose build output is served at `dist/studio/`
(gitignored). Its Tauri bridge reaches IPC via the parent window, so the
embedded editor can call backend commands (e.g. `run_task_json`,
`generate_thumbnail`).

> **Security TODO (before release):** `tauri.conf.json` currently sets
> `app.security.csp` to `null`, which disables CSP for development. Before
> shipping, tighten it to an explicit policy that still allows the embedded
> ComfyUI iframe (e.g. `frame-src` for the local ComfyUI origin) and Tauri's
> IPC, then validate the embed still loads with a release build.

## Prerequisites (Windows)

- Rust toolchain `stable-x86_64-pc-windows-msvc`.
- Visual Studio Build Tools 2022 with the C++ workload + Windows SDK.
- WebView2 runtime (preinstalled on current Windows; otherwise install the
  Evergreen runtime).

## Build & run

```sh
# Build the Node Editor sub-app once (and after changing studio-ui).
# A plain `cargo run` does NOT build it; the Node Editor tab shows a hint
# until this has run. The Tauri CLI runs this automatically (before* hooks).
npm --prefix apps/desktop-tauri/studio-ui ci
npm --prefix apps/desktop-tauri/studio-ui run build   # -> apps/desktop-tauri/dist/studio

# from the repository root
cargo run -p hgripe-desktop          # debug run
cargo build -p hgripe-desktop --release

# or, with the Tauri CLI (npm i -g @tauri-apps/cli) — builds studio-ui for you
cd apps/desktop-tauri
tauri build
```

App icons are generated from `app-icon.png` via `tauri icon app-icon.png
--output src-tauri/icons`.
