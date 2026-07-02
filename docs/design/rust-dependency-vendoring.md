# Rust dependency vendoring

**Status:** Active policy.

H-Gripe Studio keeps Rust dependencies reproducible by cutting Cargo's default
network path to crates.io. Python packages are intentionally out of scope here.

## Layout

| Area | Path | Purpose |
| --- | --- | --- |
| Cargo source replacement | `.cargo/config.toml` | Replaces `crates-io` with the local vendor directory. |
| Registry crate snapshot | `third_party/cargo-vendor/` | Output of `cargo vendor --versioned-dirs`; contains every crates.io package resolved by `Cargo.lock`. |
| Owned colour-management fork | `third_party/moxcms/` | Editable local fork of `moxcms`; wired through workspace `[patch.crates-io]`. |
| Native FFmpeg binaries | `third_party/ffmpeg/` | Windows libav DLLs/headers/import libs for the optional native video backend. |

`third_party/cargo-vendor` is a snapshot. Do not hand-edit vendored registry
crates there. If a crate needs project-specific changes, move that crate to its
own explicit directory under `third_party/<crate>/`, add a `VENDOR.md`, and
wire it with `[patch.crates-io]` like `moxcms`.

## What is cut off

- Cargo will not resolve Rust packages from crates.io during normal workspace
  builds because `.cargo/config.toml` replaces crates.io with
  `third_party/cargo-vendor`.
- `moxcms` is not consumed from the registry snapshot either; it is an owned
  fork at `third_party/moxcms`.

## What is not covered

- Python bridge dependencies.
- Node/npm dependencies.
- Runtime/model downloads.
- Build scripts that download non-crate artifacts. In particular, the ORT
  crate's `download-binaries` feature may still need a pre-fetched/cached ONNX
  Runtime binary when building from a clean machine. Treat that as a separate
  runtime artifact policy, not a Cargo crate source policy.

## Update procedure

Use this only when deliberately upgrading Rust dependencies:

1. Temporarily allow Cargo to resolve from upstream by editing/removing the
   source replacement locally, or run the update in a disposable checkout.
2. Run the intended `cargo update ...` command.
3. Re-run:

   ```powershell
   cargo vendor --versioned-dirs third_party/cargo-vendor
   ```

4. Restore `.cargo/config.toml` if needed.
5. Run at least:

   ```powershell
   cargo check --workspace --offline
   cargo test -p hgripe-desktop studio::color --offline
   ```

6. For broader dependency changes, also run package tests that cover the touched
   area.

The lockfile, vendor snapshot, and any local fork changes must land in the same
commit so cloud-side work cannot accidentally build against different Rust
source code.

