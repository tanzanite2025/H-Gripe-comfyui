# Editor resource & threading model

The forward-looking contract for **how the editor allocates compute** as the
manual editor ("small-PS popup") and the manual **video clip editor** grow many
features. It does not redefine individual edit backends; it defines *where each
kind of work runs, on which thread, and under what concurrency limit*, so new
tools can be added without blocking the UI or fighting over the GPU.

This is a **planning document** — it records the agreed model and a staged
rollout. Nothing here is implemented yet beyond what is noted under "Current
state".

## Current state (the constraints that shape this plan)

1. **The webview has a single UI thread.** Editor canvases (mask brush, magic
   wand, crop box, the planned rotate / colour tools) run on it. Today mask /
   crop edits are recorded as **vector ops in params** and rasterised by the
   backend on confirm, so the front-end does almost no heavy compute yet.
2. **Exactly one run is allowed at a time.** `useStudioRunController` holds an
   `inFlight` ref shared by `run()` and `runUpToNode()`. A confirm-to-result and
   a full-graph Run therefore **block each other**.
3. **The Rust backend runs nodes strictly serially.** `studio/exec.rs` walks the
   topological order with a sequential `.await` per node — no parallelism, no
   thread pool, no semaphore. The GPU is thus serialised by accident, not by
   policy.
4. **Compute is split between native Rust and Python subprocesses.**
   - ONNX already has a **native-Rust path**: `Cargo.toml` depends on `ort`
     (ONNX Runtime, 2.0 rc) and `subject_matte.rs` / `subject_model.rs` /
     `subject_sam2.rs` run inference through it.
   - Several cards still **shell out to a Python CLI per call** (`subject_mask`,
     `color_match`, `image_enhance`, `edge_refine`, `detail_watchdog`), and the
     **torch** engines (realesrgan, sd_inpaint) are Python-only.
   - **Each subprocess call reloads its model** — the dominant latency cost, and
     the thing that gets worse as features and edit chains grow.
5. **Video is poster-frame only.** `python/bridge/video_probe_cli.py` (PyAV)
   extracts a single frame. There is **no playback / scrubbing / seek / export**
   — PyAV poster extraction is a stop-gap, not a clip-editor engine.

## The host is Rust (not Python)

The orchestrator — task queue, GPU semaphore, CPU thread pool, run-event
streaming to the webview — **is and should remain the Rust (Tauri) process**. It
has real threads (`tokio` / `rayon`), no GIL, and already owns the run lifecycle.
A separate process is *not* needed for orchestration.

The "keep models warm" concern is about the **worker that holds a model**, which
splits by engine:

| Engine | Warm pool lives in | Mechanism |
| --- | --- | --- |
| ONNX (matting, harmonize, defect, …) | **Rust, in-process** | cache `ort::Session` in Tauri managed state — **no subprocess, no IPC** |
| torch (realesrgan, sd_inpaint) | **a long-lived Python worker** | spawned & kept alive **by Rust**; talks over stdin / a local socket |
| video decode / encode | **Rust, native** | ffmpeg bindings + dedicated threads |

Python is therefore only ever a **Rust-managed worker** for the torch-only
engines — never the host. ONNX and video belong natively in Rust.

## Four lanes

Work is classified into four lanes by cost and latency budget. Every editor
feature (image *and* video) declares which lane it uses.

### 1. Interactive (< ~16-100 ms) — UI thread / webview canvas
Brush strokes, dragging the crop box, rotate handle, slider drag, **video
timeline scrubbing UI**. Runs entirely in the webview (Canvas2D / WebGL).
**Never touches the backend.** Large-image live filters that are too heavy for
the main thread go to an **OffscreenCanvas + Web Worker**, still front-end.

### 2. Preview (debounced, ~100 ms-1 s, latest-wins + cancellable)
A best-effort preview of an op on a **downscaled proxy image** (or a single
**video frame** for scrub). One in-flight job; a newer request **cancels** the
previous (debounce + abort). Must be **decoupled from the `inFlight` run lock**
so previews never block — or get blocked by — a full Run.

### 3. Render / Compute (heavy, full-resolution) — serial GPU queue + warm pool
Committed edits (the run-up-to-node that produces the bound result node), model
inference, and **video export / encode**. GPU work is gated by a
**`Semaphore(1)`** (one GPU job at a time); CPU-only geometry (crop / rotate /
flip) may run on a `rayon` pool in parallel.

### 4. Media playback — Rust decode threads + frame cache
Real-time video playback for the clip editor: dedicated decode thread(s), a
frame ring-buffer / cache, frame-accurate seek. **Independent of the GPU compute
queue** so playback never stalls on an inference job (and vice-versa).

## Concurrency policy

- **One GPU job at a time** (`Semaphore(1)`), shared across all compute. This is
  made *explicit policy* rather than the current accidental serial behaviour.
- **CPU geometry + decode may parallelise** on a bounded thread pool.
- **Preview is single-slot, latest-wins**; **render is a FIFO queue**;
  **playback owns its own threads**; **interactive never queues**.
- **Cancellation is first-class per lane**: preview cancels on a newer request;
  render keeps the existing per-run `CancellationToken`; playback stops on seek.

## Where future tools land

| Tool | Lanes | Weight |
| --- | --- | --- |
| crop / rotate / flip | interactive preview (canvas transform) + render (fast raster) | light, CPU |
| colour / curves / levels | interactive/preview (WebGL on proxy) + render (full-res) | medium, per-pixel |
| mask / matting / inpaint / enhance | preview (proxy, optional) + render (GPU queue + warm pool) | heavy, model |
| video trim / cut | interactive (timeline) + playback (decode) + render (encode on export) | heavy, media |
| video frame scrub | preview (single-frame, latest-wins) | medium, decode |

## Staged rollout

1. **Front-end foundation (cheap, no cargo needed, verifiable now):** decouple
   preview from the global `inFlight` lock (own single-slot, latest-wins +
   cancel lane); add a `lane` discriminator to the op model so every tool
   declares its cost up front.
2. **Rust orchestration skeleton:** replace the purely-serial `.await` loop in
   `exec.rs` with a job queue carrying *(category, concurrency limit)* and a GPU
   `Semaphore(1)`; keep deterministic results.
3. **ONNX warm pool (Rust / `ort`):** cache `ort::Session` in managed state;
   migrate the still-Python ONNX cards onto it, killing per-call model reload.
4. **torch long-lived Python worker:** Rust spawns and keeps it alive; replaces
   per-call subprocess + model reload for realesrgan / sd_inpaint.
5. **Video media engine (Rust + ffmpeg):** decode / playback threads + frame
   cache; foundation for the manual clip editor (playback, scrub, trim, export).

## Non-goals (for now)

- Multi-GPU / distributed execution.
- Reimplementing torch models in pure Rust (candle / burn) — out of scope; the
  managed Python worker is the pragmatic path.
- Streaming network video; this targets local files.
