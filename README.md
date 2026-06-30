<div align="center">

# H-Gripe Studio
**An API-first, Rust-backed desktop workflow editor for AI image generation and professional PSD production.**

</div>

H-Gripe Studio is a local-first [Tauri](https://tauri.app) desktop app: a single
React front end (shell panels + an in-house React Flow node editor) over a Rust
backend (the `hgripe-api` broker + Tauri commands) and a CPU-only Python bridge
for image / PSD processing. You orchestrate remote provider calls (image / text /
audio generation and editing) and local PSD production steps as a node-based DAG,
with credentials, provider profiles, task history, and outputs all stored in one
local workspace.

> **ComfyUI has been removed.** H-Gripe began as a ComfyUI source branch, but the
> ComfyUI engine, frontend, and "Advanced Canvas" escape hatch are no longer part
> of the product — H-Gripe's own Rust/Tauri desktop app and node graph are the
> only surface. The `python/bridge` runtime is decoupled from ComfyUI's `main.py`
> (see `h-gripe.project.json`).

## Architecture

```
H-Gripe Studio (Tauri desktop)
  apps/desktop-tauri/
    studio-ui/     React + TS front end: shell panels + React Flow node editor
    src-tauri/     Rust backend: Tauri commands, Studio graph runner, PSD chain
  crates/hgripe-api/  API broker: provider adapters, retry/cache, task state, history
  python/bridge/      CPU-only Pillow/numpy CLIs for image + PSD processing
  docs/cards/         per-card contracts (inputs, params, outputs, boundaries)
```

- **API execution** runs through the `hgripe-api` broker (`run_task_json` /
  `run_studio_graph`): `openai_compatible`, `custom_http`, `replicate`, and a
  `mock` provider, with retry / caching / cancellation and local history.
- **PSD production** runs through Tauri commands that shell out to the
  `python/bridge` CLIs (Pillow + numpy, no GPU, no ML).
- The node editor is renderer-agnostic (a typed `WorkflowGraph` model + DAG
  runtime); see [`apps/desktop-tauri/studio-ui/README.md`](apps/desktop-tauri/studio-ui/README.md)
  and [`apps/desktop-tauri/README.md`](apps/desktop-tauri/README.md).

## PSD production cards

The PSD chain is a set of small, CPU-only cards. Each shells out to a
`python/bridge/*_cli.py` helper and has a contract doc under
[`docs/cards/`](docs/cards/):

| Card | Bridge CLI | What it does |
| --- | --- | --- |
| [PSD Context Analyze](docs/cards/psd-context-analyze.md) | `analyze_psd_cli.py` | Extract a `VisualContext` (lighting, bounds, masks) from a PSD. |
| [Match Light & Color](docs/cards/match-light-color.md) | `color_match_cli.py` | Match a generated image's light / colour to the scene. |
| [Refine Mask Edge](docs/cards/refine-mask-edge.md) | `edge_refine_cli.py` | Clean / feather a subject matte. |
| [Image Enhance](docs/cards/image-enhance.md) | `image_enhance_cli.py` | Global sharpen / tone enhancement. |
| [Detail Watchdog](docs/cards/detail-watchdog.md) | `detail_watchdog_cli.py` | Detect-only quality analysis (blur / halo / colour mismatch) → `QualityReport`. |
| [Detail Repaint](docs/cards/detail-repaint.md) | `detail_repaint_cli.py` | Two-stage localized repaint of flagged regions (prepare → provider `image.edit` → composite). |

These cards are **input-hardened**: candidate decodes normalise CMYK (via
embedded ICC), 16-bit / float, palette and grayscale sources to an 8-bit working
space, apply EXIF orientation, and refuse oversized inputs before decoding
(`--max-decode-pixels`). See the per-card docs and
[`docs/card-executor-split-and-psd-chain-hardening.md`](docs/card-executor-split-and-psd-chain-hardening.md).

## Local Workspace Mode

H-Gripe is local-first and personal-use oriented: there are no cloud accounts or multi-user profiles. Workflows, credentials, provider profiles, history, and generated outputs are all stored under a single local workspace rooted at `user/hgripe`.

Credential refs keep API keys out of workflow files. `openai_compatible` and `custom_http` tasks/nodes can use them. The default local credential file is ignored by git:

```text
user/hgripe/credentials.json
```

You can also point to another file with `HGRIPE_CREDENTIALS_FILE`.

Provider profiles keep non-secret provider defaults out of workflow files. `openai_compatible` and `custom_http` tasks/nodes can use them. The default local profile file is ignored by git:

```text
user/hgripe/provider_profiles.json
```

Profiles can define defaults such as `base_url`, `model`, `credentials_ref`, `no_auth`, headers, `params`, and `extra_body`. Use `profile_ref` on OpenAI-compatible and Custom HTTP tasks/nodes to load one. You can also point to another file with `HGRIPE_PROVIDER_PROFILES_FILE` or task param `profiles_file`.

Task history is recorded locally as JSONL and indexed into SQLite for UI/query use:

```text
user/hgripe/history/tasks.jsonl
user/hgripe/history/tasks.sqlite3
```

New history records also store a sanitized `task_snapshot` so a task can be rerun later without keeping inline API keys, tokens, passwords, or Authorization headers in history. Older records created before this field exists are still readable, but they are not rerunnable.

Generated/downloaded API outputs should use the local output root:

```text
user/hgripe/outputs
```

`openai_compatible image.generate` can save `b64_json` and downloaded `url` image outputs there and return those paths through `output_files`.
`openai_compatible audio.speech` saves generated audio bytes there by default and returns the local audio file through `output_files`.
`openai_compatible audio.transcriptions` and `audio.translations` upload local audio files with multipart requests and return extracted text through `output_json.text`.
`custom_http` can also save raw successful response bytes when `save_response=true`, which is useful for API endpoints that directly return images, audio, video, PDFs, or other files.
`custom_http` supports multipart form fields and local file uploads for APIs that accept images, audio, video, PDFs, or dataset files.
`custom_http async_job` can submit an async API job, poll a status endpoint, and download a final result URL into `output_files`.
`custom_http` can use `credentials_ref` for `base_url`, bearer API keys, env-based API keys, and secret/non-secret headers, keeping them out of workflow JSON.
`replicate run` creates a Replicate prediction (by `model` owner/name or `version`), polls until it succeeds or fails, and downloads each output URL into `output_files`, returning the raw prediction body through `output_json`. It accepts `credentials_ref`/`profile_ref` (provider `replicate`), `HGRIPE_REPLICATE_API_KEY`/`REPLICATE_API_TOKEN`, and `HGRIPE_REPLICATE_BASE_URL`.

Useful environment overrides:

```powershell
$env:HGRIPE_HISTORY_FILE="C:\path\to\tasks.jsonl"
$env:HGRIPE_HISTORY_DB="C:\path\to\tasks.sqlite3"
$env:HGRIPE_OUTPUT_DIR="C:\path\to\outputs"
$env:HGRIPE_HISTORY_DISABLED="1"
$env:HGRIPE_PROVIDER_PROFILES_FILE="C:\path\to\provider_profiles.json"
$env:HGRIPE_CUSTOM_HTTP_BASE_URL="https://api.example.com"
$env:HGRIPE_CUSTOM_HTTP_API_KEY="..."
$env:HGRIPE_REPLICATE_BASE_URL="https://api.replicate.com"
$env:HGRIPE_REPLICATE_API_KEY="..."
```

Local verification:

```powershell
cargo test -p hgripe-api
cargo build -p hgripe-api --bins
.\target\debug\hgripe-api-config.exe init --dry-run
.\target\debug\hgripe-api-config.exe init
.\target\debug\hgripe-api-config.exe doctor
.\target\debug\hgripe-api-config.exe profiles list
.\target\debug\hgripe-api-config.exe profiles show <profile_ref>
.\target\debug\hgripe-api-config.exe profiles resolve <profile_ref>
.\target\debug\hgripe-api-config.exe profiles validate
.\target\debug\hgripe-api-config.exe credentials list
.\target\debug\hgripe-api-config.exe credentials show <credential_ref>
.\target\debug\hgripe-api-config.exe credentials validate
.\target\debug\hgripe-api-history.exe list --limit 10
.\target\debug\hgripe-api-history.exe show <task_id>
.\target\debug\hgripe-api-history.exe rerun-task <task_id>
.\target\debug\hgripe-api-history.exe rerun <task_id>
.\target\debug\hgripe-api-history.exe cleanup --keep-latest 100
.\target\debug\hgripe-api-history.exe cleanup --keep-latest 100 --apply
```

`hgripe-api-history cleanup` defaults to dry-run. It only changes SQLite/JSONL history when `--apply` is provided. Output files are preserved unless `--delete-output-files` is also provided.

`hgripe-api-config credentials show` redacts inline API keys and secret-like headers before printing JSON.
`hgripe-api-config profiles resolve` previews a profile's effective provider settings without printing API keys or header values.
`hgripe-api-config doctor` summarizes config validation, profile-to-credential references, runtime paths, broker location, and H-Gripe env overrides without printing secret values.
`hgripe-api-config init` creates local config/history/output directories and starter credentials/profile templates. Existing files are preserved unless `--force` is provided.

## Desktop app: build & run

Prerequisites (Windows): Rust `stable-x86_64-pc-windows-msvc`, Visual Studio
Build Tools 2022 (C++ workload + Windows SDK), and the WebView2 runtime.

```sh
# Build the React front end first (a plain `cargo run` does NOT build it, so the
# window would be blank). The Tauri CLI runs this for you via the before* hooks.
npm --prefix apps/desktop-tauri/studio-ui ci
npm --prefix apps/desktop-tauri/studio-ui run build   # -> apps/desktop-tauri/dist

# run the desktop app from the repo root (after the front end is built)
cargo run -p hgripe-desktop
cargo build -p hgripe-desktop --release

# or, with the Tauri CLI (npm i -g @tauri-apps/cli) — builds the front end for you
cd apps/desktop-tauri
tauri dev
tauri build
```

See [`apps/desktop-tauri/README.md`](apps/desktop-tauri/README.md) and
[`apps/desktop-tauri/studio-ui/README.md`](apps/desktop-tauri/studio-ui/README.md)
for the front-end / backend boundary and editor features.

## Tests

```sh
# Rust: broker + desktop backend (Studio runner, PSD chain)
cargo test

# Python bridge: the CPU-only image/PSD CLIs
ruff check python/bridge
python -m pytest python/bridge/tests

# Front end: DAG runtime unit tests + typecheck
npm --prefix apps/desktop-tauri/studio-ui test
npm --prefix apps/desktop-tauri/studio-ui run typecheck
```

## License

[GPL-3.0](LICENSE).
