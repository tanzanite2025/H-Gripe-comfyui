# Editor resource & threading model

The forward-looking contract for **how the editor allocates compute** as the
manual editor ("small-PS popup") and the manual **video clip editor** grow many
features. It does not redefine individual edit backends; it defines *where each
kind of work runs, on which thread, and under what concurrency limit*, so new
tools can be added without blocking the UI or fighting over the GPU.

This started as a **planning document**; the staged rollout below is now
**fully landed** (steps 1-5 + the native ffmpeg backend). The model itself is
still the forward-looking contract for adding new editor tools — see the
per-step ✅ notes for what shipped and the PRs that shipped it.

## Origin state (the constraints that shaped this plan)

> Snapshot of the codebase *before* this rollout, kept for context. Every
> numbered constraint here has since been addressed by the [staged
> rollout](#staged-rollout); the live status lives there and in
> [`../implementation-status.md`](../implementation-status.md).

1. **The webview has a single UI thread.** Editor canvases (mask brush, magic
   wand, crop box, the planned rotate / colour tools) run on it. Mask / crop
   edits are recorded as **vector ops in params** and rasterised by the backend
   on confirm, so the front-end does almost no heavy compute. *(Still true, by
   design.)*
2. **~~Exactly one run is allowed at a time.~~** `useStudioRunController` held an
   `inFlight` ref shared by `run()` and `runUpToNode()`, so a confirm-to-result
   and a full-graph Run blocked each other. **Fixed in step 1**: preview runs on
   its own single-slot, latest-wins lane, decoupled from the run lock.
3. **~~The Rust backend runs nodes strictly serially.~~** `studio/exec.rs` walked
   the topological order with a sequential `.await` per node, serialising the
   GPU by accident. **Fixed in step 2**: an explicit lane scheduler with a GPU
   `Semaphore(1)` + CPU pool makes the serialisation policy, not accident.
4. **~~Each subprocess call reloads its model.~~** ONNX already had a native-Rust
   `ort` path; several cards shelled out to a Python CLI per call and reloaded
   the model each time (the dominant latency cost). **Fixed in steps 3-4**: an
   in-process ONNX warm pool (`onnx_pool.rs`) and a long-lived torch worker
   (`torch_worker.rs`) keep models resident across calls.
5. **~~Video is poster-frame only.~~** `video_probe_cli.py` (PyAV) extracted a
   single frame; there was no playback / scrubbing / seek. **Fixed in step 5**:
   a media engine (decoder seam + LRU frame cache + dedicated playback thread)
   with two `FrameSource` backends — the long-lived PyAV worker and an
   in-process **native ffmpeg** decoder (vendored libav, `native-ffmpeg`
   feature). Export/encode is still future work.

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

## Staged rollout — ✅ complete

All five stages have landed; the native ffmpeg backend (a follow-up to step 5)
too. This section is now a changelog of the rollout.

1. ✅ **Front-end foundation** (PR #145) — preview decoupled from the global
   `inFlight` lock onto its own single-slot, latest-wins + cancel lane; a `lane`
   discriminator on the op model so every tool declares its cost up front. First
   consumer: live mask-morphology (grow/shrink/feather/smooth) proxy preview.
2. ✅ **Rust orchestration skeleton** (PR #146) — the purely-serial `.await`
   loop in `exec.rs` is replaced by a lane scheduler carrying *(category,
   concurrency limit)* + a GPU `Semaphore(1)`; results stay deterministic.
3. ✅ **ONNX warm pool** (PR #147) — `studio/onnx_pool.rs` caches `ort::Session`
   in process-global managed state; `subject_model` / `subject_sam2` /
   `subject_matte` reuse it, killing per-call model reload.
4. ✅ **torch long-lived Python worker** (PR #148) — `studio/torch_worker.rs`
   spawns and keeps a torch worker alive; `image_enhance` (realesrgan) and
   `detail_repaint` (sd_inpaint) reuse it, falling back to the one-shot
   subprocess on worker failure.
5. ✅ **Video media engine** (PR #149) — `studio/video_engine.rs`: decoder seam
   (`FrameSource`) + LRU `frame_cache.rs` + a dedicated latest-wins playback
   thread; `video_scrub` command for timeline dragging. Decode is off the UI
   thread and the GPU compute queue.
6. ✅ **Native ffmpeg backend** (PR #150) — a second `FrameSource`
   (`studio/ffmpeg_native.rs`) decoding in-process with **vendored** LGPL-shared
   libav (`third_party/ffmpeg`, git-lfs; `native-ffmpeg` cargo feature, off by
   default). `make_frame_source()` wraps it with a PyAV fallback so a per-clip
   decode failure never regresses. Still future: trim / **export / encode**.

## Non-goals (for now)

- Multi-GPU / distributed execution.
- Reimplementing torch models in pure Rust (candle / burn) — out of scope; the
  managed Python worker is the pragmatic path.
- Streaming network video; this targets local files.
