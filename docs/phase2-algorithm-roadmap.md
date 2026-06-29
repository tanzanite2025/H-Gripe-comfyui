# Phase 2 Algorithm Roadmap — Super-Resolution, Detail Watchdog, Detail Repaint

> **Status:** Design proposal (Issue #4). This document does **not** implement
> any Phase 2 algorithm; it records the current Phase 1 baseline, the target
> production-grade behaviour, and a concrete integration plan so the work can be
> scheduled and reviewed.

## 0. Context

The PSD-first production chain is *functionally* complete end-to-end, but three
nodes ship a deliberate **Phase 1 / skeleton** core: the heavy ML/GPU algorithms
are stubbed by dependency-light CPU approximations so the pipeline, contracts,
and UI can be exercised without a GPU or large model downloads.

| Node | Tauri command(s) | Phase 1 core (today) | Phase 2 target |
| --- | --- | --- | --- |
| Image Enhance (super-res) | `enhance_image` | Pillow Lanczos upscale + Gaussian-blur denoise + unsharp mask (CPU) | SupIR / CCSR / Real-ESRGAN GPU restoration |
| Detail Watchdog | `detect_quality_issues` | Pillow+numpy rule heuristics (Laplacian variance, tile sharpness grid, alpha-rim halo, mean-colour drift) | ML/VLM semantic defect detection |
| Detail Repaint | `prepare_repaint_regions` + `composite_repaint` | crop/mask + feathered paste around a provider `image.edit` call | dedicated GPU inpainting backend |

The guiding principle for Phase 2 is **additive, opt-in backends**: the existing
CPU path stays as the always-available default and fallback; GPU/ML strength is
selected per run through a `profile_ref` (the mechanism already reserved in
`image_enhance_cli.py`), never by silently changing the default behaviour.

---

## 1. Super-Resolution — `enhance_image`

### 1.1 Phase 1 baseline
`python/bridge/image_enhance_cli.py` runs, on CPU only:
1. `_denoise` — blend the image with a Gaussian-blurred copy (`strength` 0..1).
2. Lanczos resample up to the PSD placeholder's pixel target.
3. `_sharpen` — unsharp mask, capped by `--max-sharpen` so logos / packaging
   text are not mangled.

Presets (`conservative` / `texture_rebuild` / `print_ready` / `custom`) resolve
to `(scale, denoise_strength, texture_strength)`. This restores *apparent*
sharpness but cannot synthesize detail that is not in the source pixels.

### 1.2 Phase 2 target
A GPU diffusion/restoration backend that **hallucinates plausible high-frequency
detail** while preserving identity and text:
- **SupIR** — best perceptual quality, prompt-guided restoration; heavy (SDXL +
  large adapters), needs a strong GPU.
- **CCSR** — more faithful / less hallucinated; good for product photography
  where over-invention is a risk.
- **Real-ESRGAN** — light, deterministic, fast; a strong mid-tier default and a
  good first integration target.

### 1.3 Integration plan
- Add a `--profile-ref <id>` argument (placeholder already documented in the
  CLI header). When present, the CLI dispatches to a backend module under
  `python/bridge/sr_backends/` (`realesrgan.py`, `ccsr.py`, `supir.py`) selected
  by the profile; when absent, the current CPU path runs unchanged.
- Backends declare their model weights + device requirements; weights are
  resolved from a cache dir (env `HGRIPE_MODEL_CACHE`), **not** bundled in the
  installer (keeps the Tauri bundle small — see Issue #2).
- Add a capability probe (`enhance_image` → `doctor`-style report) so the UI can
  grey out GPU presets when CUDA/weights are unavailable and fall back to CPU.
- Contract impact: none. Output stays `{fixed_image, scale, denoise_strength,
  texture_strength, ...}`; add optional `backend`/`model` fields for telemetry.

### 1.4 Dependencies & risks
`torch` + CUDA, `realesrgan`/`spandrel`, model weights (hundreds of MB–GB).
Risks: VRAM exhaustion (mitigate with tiled inference), non-determinism
(seedable), text/logo distortion (keep the `--max-sharpen` guard concept as a
post-pass identity check).

---

## 2. Detail Watchdog — `detect_quality_issues`

### 2.1 Phase 1 baseline
`python/bridge/detail_watchdog_cli.py` is **detect + report only**, `torch`-free,
using Pillow+numpy:
- `low_resolution` — global Laplacian-variance blur and/or smaller-than-placeholder.
- `face_blur` — per-tile sharpness grid merged into boxes.
- `edge_halo` — bright fringe on the semi-transparent alpha rim.
- `color_mismatch` — subject mean colour vs connected `visual_context`.

Semantic targets (hands, packaging text, logo deformation) are explicitly
**recorded as skipped**, not guessed.

### 2.2 Phase 2 target
Replace/augment heuristics with learned detectors that honestly cover the
skipped semantic targets:
- **Face/hand quality** — a face/hand landmark + quality model to flag
  malformed hands and blurred faces with real confidence.
- **Text/logo integrity** — OCR (e.g. PaddleOCR) + template/logo matching to
  detect garbled packaging text and deformed logos.
- **VLM defect pass** — a vision-language model prompt ("list visible artifacts,
  with bounding boxes") for open-ended defect discovery, reconciled with the
  rule layer.

### 2.3 Integration plan
- Keep the rule layer as the always-on baseline; ML detectors are additive
  passes gated by a `profile_ref` / watch-target list, each emitting into the
  **same `QualityReport` contract** (so `issue_masks` + `suggested_action`
  consumers — notably Detail Repaint — need no change).
- Newly-covered targets graduate from `skipped` to real findings; confidence and
  detector provenance are added as optional report fields.
- Detectors run behind a capability probe; missing weights ⇒ that pass is
  reported `skipped` exactly as today (no hard failure).

### 2.4 Dependencies & risks
`onnxruntime`/`torch`, OCR + detection weights. Risks: false positives causing
unnecessary repaint loops (tune thresholds, require agreement between rule + ML
for auto-action), latency (run ML passes only on flagged tiles).

---

## 3. Detail Repaint — `prepare_repaint_regions` / `composite_repaint`

### 3.1 Phase 1 baseline
`python/bridge/detail_repaint_cli.py` is the `torch`-free pixel backend:
- `prepare` — crop a padded window per issue region and write a same-size
  inpaint `mask` marking the issue core; emit a manifest.
- `composite` — paste provider-repainted crops back inside a *feathered* issue
  core (edge fusion at the seam), leaving padding context untouched.

The actual generative fix is the broker `image.edit` provider call, owned by the
Rust/TS orchestration layer — quality depends entirely on the configured
provider.

### 3.2 Phase 2 target
A first-class **local GPU inpainting backend** as an alternative to the remote
provider:
- Diffusion inpainting (SD/SDXL inpaint, or Flux Fill) driven by the same
  crop+mask+prompt manifest, for offline / privacy / cost-controlled runs.
- Optional ControlNet (edges/depth) conditioning to keep structure stable.
- Seam-aware blending beyond the current feather (e.g. Poisson / gradient-domain
  compositing) for harder seams.

### 3.3 Integration plan
- The `prepare`/`composite` split and manifest **already** isolate the generative
  step cleanly — Phase 2 adds a local backend that consumes the same manifest, so
  the orchestrator chooses "provider `image.edit`" vs "local inpaint" by
  `profile_ref` with **no contract change**.
- `composite` stays backend-agnostic; only an optional advanced-blend flag is
  added, defaulting to today's feather.

### 3.4 Dependencies & risks
`torch` + CUDA, inpaint model weights, optional ControlNet. Risks: identity
drift inside masked region (low denoise strength + tight masks), seam visibility
(advanced blend), VRAM (tiled per-region inference — already region-scoped by
`prepare`).

---

## 4. Cross-cutting concerns

- **Packaging (ties to Issue #2):** model weights are **not** bundled in the
  installer. Backends resolve weights from `HGRIPE_MODEL_CACHE` (downloaded /
  configured post-install). The bundled `python/bridge` + `custom_nodes` +
  `third_party` subtree stays the lightweight CPU baseline.
- **Capability probing:** extend `doctor` to report GPU/CUDA, installed backends,
  and cached weights so the UI can enable/disable GPU presets and always fall
  back to CPU.
- **Determinism & safety:** seedable backends; keep the text/logo guards; require
  rule+ML agreement before any *automatic* (non-user-confirmed) repaint.
- **Contracts are stable:** every Phase 2 backend emits the existing
  `QualityReport` / `RepaintReport` / enhance JSON shapes. Phase 2 is selected
  per run via `profile_ref`; the CPU path remains the default and fallback.

## 5. Suggested sequencing

1. **SR first (highest visible win, lowest risk):** Real-ESRGAN backend behind
   `profile_ref` + capability probe + CPU fallback.
2. **Repaint local backend:** reuse the existing manifest; add SD/SDXL inpaint.
3. **Watchdog ML passes:** OCR/logo + face/hand, then a VLM pass; graduate the
   currently-`skipped` semantic targets.
4. **SupIR/CCSR** as premium SR profiles once the backend dispatch + weight cache
   are proven by Real-ESRGAN.
