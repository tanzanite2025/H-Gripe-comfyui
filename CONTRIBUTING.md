# Contributing to H-Gripe Studio

Thanks for your interest in contributing to H-Gripe Studio — the Rust/Tauri
desktop workflow editor for AI image generation and PSD production. This
document is a high-level overview of how to get involved.

> H-Gripe began as a ComfyUI source branch, but ComfyUI has been removed from the
> product. Contributions target H-Gripe's own desktop app (`apps/desktop-tauri`),
> the `hgripe-api` broker (`crates/hgripe-api`), and the Python bridge
> (`python/bridge`).

## Reporting issues and feature requests

Before opening a new issue, search [open issues](https://github.com/tanzanite2025/H-Gripe-Studio/issues)
to see whether it has already been filed — add a 👍 reaction or a relevant
comment instead of a duplicate. When filing a bug, include repro steps, what you
expected vs. what happened, your OS, and relevant logs.

## Development setup

See [`README.md`](README.md) for the architecture and the full build/run/test
commands. In short:

```sh
# Desktop front end (build before running the Rust app, or use the Tauri CLI)
npm --prefix apps/desktop-tauri/studio-ui ci
npm --prefix apps/desktop-tauri/studio-ui run build

# Desktop app
cargo run -p hgripe-desktop
# or, with the Tauri CLI: cd apps/desktop-tauri && tauri dev
```

Prerequisites (Windows): Rust `stable-x86_64-pc-windows-msvc`, Visual Studio
Build Tools 2022 (C++ workload + Windows SDK), and the WebView2 runtime.

## Before you open a pull request

Run the checks for the area you touched and keep changes focused:

```sh
# Rust (broker + desktop backend)
cargo test
cargo clippy --all-targets

# Python bridge (CPU-only image/PSD CLIs)
ruff check python/bridge
python -m pytest python/bridge/tests

# Front end
npm --prefix apps/desktop-tauri/studio-ui test
npm --prefix apps/desktop-tauri/studio-ui run typecheck
```

Guidelines:

- Match the surrounding style and conventions; prefer minimal, scoped edits.
- Add or update tests for behaviour changes. New PSD cards should ship a
  `python/bridge/*_cli.py` helper, tests under `python/bridge/tests/`, the
  matching Rust report/struct, and a contract doc under [`docs/cards/`](docs/cards/).
- Don't commit secrets (`credentials.json`, API keys) or build artifacts
  (`apps/desktop-tauri/dist/`, `target/`).
- Write a clear PR description: what changed and why. CI must be green before review.

## License

By contributing, you agree that your contributions are licensed under the
project's [GPL-3.0](LICENSE) license.

## Thank you

Your contributions, large or small, make the project better. Thank you for taking
the time to contribute.
