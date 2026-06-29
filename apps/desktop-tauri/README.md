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

The shell frontend is a dependency-free Vite + TypeScript app in
[`shell-ui/`](shell-ui/) (no runtime dependencies, no React) that builds to the
Tauri `frontendDist` at `dist/` (gitignored); the Rust backend lives in
`src-tauri/`. All Tauri IPC goes through a single typed wrapper
([`shell-ui/src/tauri.ts`](shell-ui/src/tauri.ts)) whose command argument /
return types mirror the Rust `#[tauri::command]` signatures, so a rename or
shape change surfaces as a `tsc` error instead of a silent runtime drift. The
**Node Editor** is a separate Vite + React + TypeScript sub-app in
[`studio-ui/`](studio-ui/) whose build output is served at `dist/studio/` (also
gitignored). Its Tauri bridge reaches IPC via the parent window, so the embedded
editor can call backend commands (e.g. `run_task_json`, `generate_thumbnail`).

> **`dist/` is a build artifact.** Both the shell (`shell-ui/` → `dist/`) and
> the Node Editor (`studio-ui/` → `dist/studio/`) are generated and gitignored.
> The Tauri before* hooks build the shell **first** (which empties `dist/`) and
> studio-ui **second** (which writes `dist/studio/`), so the two coexist. A bare
> `cargo run` does **not** run these hooks and will show a blank window; build
> the frontends first (see below) or use the Tauri CLI.

> **Security (CSP):** `tauri.conf.json` now sets an explicit
> `app.security.csp` instead of `null`. It keeps `default-src 'self'` while
> allowing what the shell actually needs: `style-src 'unsafe-inline'` (React /
> React Flow inject styles), `img-src data: blob:` (thumbnails are read back as
> data URLs), `connect-src ipc: http://ipc.localhost` (Tauri IPC), and
> `frame-src` for the embedded **Node Editor** (same origin) and **Advanced
> Canvas** ComfyUI iframe on loopback (`http://127.0.0.1:*`/`http://localhost:*`,
> so a user-chosen port still embeds). All dynamic data spliced into the shell's
> `innerHTML` is HTML-escaped (`esc()` in `shell-ui/src/dom.ts`). The bundle is
> loaded as an external module script (`script-src 'self'`, no inline scripts).
>
> **Still to validate before release:** confirm with a **release build** that
> both iframes load and IPC works under this CSP (the dev box used to author it
> has no Rust toolchain, so this was not run locally). If a future feature needs
> a remote origin (web fonts, a hosted asset), widen the matching directive
> rather than reverting to `null`.

## Prerequisites (Windows)

- Rust toolchain `stable-x86_64-pc-windows-msvc`.
- Visual Studio Build Tools 2022 with the C++ workload + Windows SDK.
- WebView2 runtime (preinstalled on current Windows; otherwise install the
  Evergreen runtime).

## Build & run

```sh
# Build the frontends. A plain `cargo run` does NOT build them (the window
# would be blank / the Node Editor tab shows a hint). The Tauri CLI runs these
# automatically via the before* hooks. Build the shell FIRST (it empties
# dist/), then studio-ui (it writes dist/studio/), so the two coexist.
npm --prefix apps/desktop-tauri/shell-ui ci
npm --prefix apps/desktop-tauri/shell-ui run build    # -> apps/desktop-tauri/dist
npm --prefix apps/desktop-tauri/studio-ui ci
npm --prefix apps/desktop-tauri/studio-ui run build   # -> apps/desktop-tauri/dist/studio

# from the repository root (only after the frontends are built)
cargo run -p hgripe-desktop          # debug run
cargo build -p hgripe-desktop --release

# or, with the Tauri CLI (npm i -g @tauri-apps/cli) — builds both frontends
# for you in the right order (before* hooks)
cd apps/desktop-tauri
tauri dev                            # debug, with the frontend build hooks
tauri build
```

For shell development you can also run the Vite dev server
(`npm --prefix apps/desktop-tauri/shell-ui run dev`) or type-check in isolation
(`npm --prefix apps/desktop-tauri/shell-ui run typecheck`).

App icons are generated from `app-icon.png` via `tauri icon app-icon.png
--output src-tauri/icons`.
