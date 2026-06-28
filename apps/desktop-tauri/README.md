# H-Gripe Desktop (Tauri shell)

Phase 2 desktop shell for H-Gripe. It wraps the `hgripe-api` crate in a
[Tauri](https://tauri.app) window and exposes the API-first workflow:

- **Dashboard** – runtime paths + `doctor` diagnostics.
- **Credentials / Profiles** – view summaries, validate, and edit
  `credentials.json` / `provider_profiles.json` in place.
- **Run Task** – submit an `ApiTask` JSON payload to the broker and inspect the
  `ApiResult`.
- **History** – list / view / rerun / clean up recorded tasks (SQLite history).
- **ComfyUI** – open a running ComfyUI web UI in the system browser.

The frontend is a dependency-free static page (`dist/`) using Tauri's global
API (`window.__TAURI__`); the Rust backend lives in `src-tauri/`.

## Prerequisites (Windows)

- Rust toolchain `stable-x86_64-pc-windows-msvc`.
- Visual Studio Build Tools 2022 with the C++ workload + Windows SDK.
- WebView2 runtime (preinstalled on current Windows; otherwise install the
  Evergreen runtime).

## Build & run

```sh
# from the repository root
cargo run -p hgripe-desktop          # debug run
cargo build -p hgripe-desktop --release

# or, with the Tauri CLI (npm i -g @tauri-apps/cli)
cd apps/desktop-tauri
tauri build
```

App icons are generated from `app-icon.png` via `tauri icon app-icon.png
--output src-tauri/icons`.
