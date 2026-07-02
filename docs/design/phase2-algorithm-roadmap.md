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
| Match Light & Color | `match_light_color` | Reinhard Lab transfer / per-channel histogram match weighted to shadows/highlights, brand-colour protected (CPU) | learned image-harmonisation backend |
| Refine Mask Edge | `refine_mask_edge` | erode/dilate morphology + numpy guided-filter edge snapping + feather + colour decontamination, trimap unknown band protected (CPU) | learned alpha-matting backend |

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
**Status: the seam + Real-ESRGAN have landed** (the rest of this section is the
design it was built to; ⛔ items are CCSR/SupIR + real-inference CI).
The selector is the local card's **`engine` param** (`cpu` | `realesrgan` | …),
not `--profile-ref` — `profile_ref` is the API-card credentials concept, and
Image Enhance is a `local` card (see `executor-split-and-psd-chain-hardening.md`).

- ✅ Add an `--engine <id>` argument. When non-`cpu`, the CLI dispatches to a
  backend module under `python/bridge/sr_backends/` (`realesrgan.py` landed;
  `ccsr.py`, `supir.py` ⛔) selected by the registry; when `cpu`/absent the
  current CPU path runs unchanged.
- ✅ Backends declare weights + device requirements; weights resolve from a
  cache dir (env `HGRIPE_MODEL_CACHE`, or `HGRIPE_REALESRGAN_MODEL`), **not**
  bundled in the installer (keeps the Tauri bundle small — see Issue #2).
- ✅ Capability probe (`image_enhance_cli.py --probe-engines`) reports which
  engines are usable so the UI can grey out unavailable ones; any miss falls
  back to CPU and records `engine_fallback_reason`. ✅ The UI greying itself:
  the inspector's `engine` select greys unavailable options from the
  cross-card `probe_engines` report (see §6 Capability probing).
- ✅ Contract impact: none. Output adds optional `engine` / `engine_requested` /
  `engine_fallback_reason` / `backend_model` telemetry fields.
- ⛔ A real-inference CI job (opt-in like the ViTMatte e2e), since CI does not
  install `torch` + the weight.

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
**Status: the seam has landed** (the rest of this section is the design it was
built to; ⛔ items are the concrete face/hand/OCR/VLM models + real-inference CI).
As with Image Enhance, the selector is the local card's **`engine` param**
(`rules` | `onnx_defect` | …), not `--profile-ref` (Detail Watchdog is a `local`
card).

- ✅ Keep the rule layer as the always-on baseline; ML detectors are additive
  passes selected by `engine`, each emitting into the **same `QualityReport`
  contract** (so `issue_masks` + `suggested_action` consumers — notably Detail
  Repaint — need no change). The detectors register under
  `python/bridge/detector_backends/` (mirroring `sr_backends`).
- ✅ Newly-covered targets graduate from `skipped` to real findings; detector
  provenance is added as optional report fields (`engine` / `engine_requested` /
  `engine_fallback_reason` / `detectors` / `backend_model`).
- ✅ Detectors run behind a capability probe (`detail_watchdog_cli.py
  --probe-engines`); missing deps/weights ⇒ the rule-only report runs and the
  uncovered targets stay `skipped` exactly as today (no hard failure), with the
  reason recorded.
- 🟡 `onnx_defect` is the first concrete detector: a generic ONNX object
  detector seam covering hands/text/logo (`malformed_hands` / `garbled_text` /
  `deformed_logo`). Its weight is not bundled; ⛔ the actual trained
  face/hand-quality, OCR/logo and VLM models behind it.
- 🟡 The gated unit test that synthesises a tiny ONNX detector to exercise the
  session path (incl. the `onnx_providers` execution-provider selection) now
  runs in CI: the **`python bridge (onnx inference)`** lane installs `onnx` +
  `onnxruntime` per PR (no weight download needed since the model is
  synthesised). ⛔ remaining: real *trained-weight* inference (opt-in like the
  ViTMatte e2e), since CI does not fetch the trained detector weight.

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
**Status: the seam + `sd_inpaint` + the advanced-blend flag (`blend=poisson`,
gradient-domain seam compositing in `composite`, defaulting to the feather)
have landed** (the rest of this section is the design it was built to; ⛔ items
are SDXL / Flux Fill backends, ControlNet and real-inference CI). The selector is the local card's
**`engine` param** (`provider` | `sd_inpaint` | …); `provider` stays the default
and the fallback.
- The `prepare`/`composite` split and manifest **already** isolate the generative
  step cleanly — the `repaint` subcommand (`python/bridge/inpaint_backends/`) adds
  a local backend that consumes the same manifest, so the orchestrator chooses
  "provider `image.edit`" vs "local inpaint" by the `engine` param with **no
  contract change**: a `repaint` run emits the same `{index, path}` list that
  `composite` already consumes, and an unavailable/`provider` engine emits an
  empty list + `engine_fallback_reason` so the remote path runs unchanged.
- `composite` stays backend-agnostic; only an optional advanced-blend flag is
  added, defaulting to today's feather. ✅ Landed as `--blend feather|poisson`
  (a DST-based exact Poisson solve over the rectangular issue core, falling
  back to the feather on a too-small region).

### 3.4 Dependencies & risks
`torch` + CUDA, inpaint model weights, optional ControlNet. Risks: identity
drift inside masked region (low denoise strength + tight masks), seam visibility
(advanced blend), VRAM (tiled per-region inference — already region-scoped by
`prepare`).

---

## 4. Match Light & Color — `match_light_color`

### 4.1 Phase 1 baseline
`python/bridge/color_match_cli.py` runs, on CPU only: a Reinhard Lab statistics
transfer / per-channel histogram match (`color_transfer` | `histogram_match` |
`hybrid`), weighted toward shadows/highlights, sparing high-chroma (brand)
pixels, and acting only inside the subject's alpha. `prompt_only` emits just the
prompt suffix. Emits `{matched_image, prompt_suffix, match_report}`.

### 4.2 Phase 2 target
A **learned image-harmonisation** network (foreground harmonisation, e.g.
PCT-Net / Harmonizer-style) that predicts a per-pixel light/colour correction
consistent with the background while preserving brand colours and material cues
better than the global Lab/histogram statistics.

### 4.3 Integration plan
**Status: the seam + `onnx_harmonize` have landed** (the trained weight is the
remaining piece). Mirroring the SR / Watchdog / Repaint seams:

- A new **`engine` param** (`cpu` | `onnx_harmonize` | …); `cpu` stays the
  default and always-available heuristic baseline.
- `python/bridge/color_backends/` is the registry (`known_engines` / `resolve` /
  `probe`); 🟡 `onnx_harmonize` is the first concrete backend: it composites the
  subject over the resized background for context, runs an ONNX harmoniser
  (lazy `onnxruntime`, weight from `HGRIPE_COLOR_MODEL` / `HGRIPE_MODEL_CACHE`),
  and returns the harmonised RGB at the source geometry.
- The learned correction is applied **inside the subject alpha, scaled by
  `strength`**, so it honours the same region/strength contract as the
  heuristic, and emits into the **same** `match_report` (plus `engine` /
  `engine_requested` / `engine_fallback_reason` / `backend_model` telemetry).
- `--probe-engines` reports availability; `match_light_color` joins the
  cross-card `probe_engines` aggregation so the inspector greys it out when its
  dep/weight is missing. Missing dep/weight → graceful fallback to the heuristic.

### 4.4 Dependencies & risks
`onnxruntime` + a harmonisation weight (not bundled). Risks: identity / brand-
colour drift (mitigated by the alpha-masked, strength-scaled blend), and the
weight is opt-in so real-inference CI is gated like ViTMatte.

---

## 5. Refine Mask Edge — `refine_mask_edge`

### 5.1 Phase 1 baseline
`python/bridge/edge_refine_cli.py` runs, on CPU only (Pillow + numpy, no OpenCV):
erode/dilate morphology to bite off the white fringe, a numpy guided filter that
snaps the matte to the subject's own luminance edges, a Gaussian feather, and
edge colour decontamination. When a matting **trimap** is connected, the unknown
band (hair / fur / glass) is *protected* from the erode/feather clean-up and
restored from the source matte. Emits `{refined_image, refined_mask,
edge_report}`.

### 5.2 Phase 2 target
A **learned alpha-matting** network (ViTMatte / IndexNet / MODNet-style) that
solves true continuous alpha in the trimap's unknown band — recovering fine hair
and semi-transparent edges the global guided filter flattens — while leaving the
definite FG/BG regions to the deterministic heuristic clean-up.

### 5.3 Integration plan
**Status: the seam + `onnx_matting` have landed** (the trained weight is the
remaining piece). Mirroring the SR / Watchdog / Repaint / Match Light & Color
seams:

- A new **`engine` param** (`cpu` | `onnx_matting` | …); `cpu` stays the default
  and always-available heuristic baseline.
- `python/bridge/matting_backends/` is the registry (`known_engines` / `resolve`
  / `probe`); 🟡 `onnx_matting` is the first concrete backend: it runs an ONNX
  matting network (lazy `onnxruntime`, weight from `HGRIPE_MATTING_MODEL` /
  `HGRIPE_MODEL_CACHE`) over the subject + trimap and returns a refined alpha at
  the source geometry.
- The learned alpha **replaces the source matte only inside the protected
  (unknown) band**, so the definite regions still get the morphology/guided/
  feather clean-up and the geometry / report contract is unchanged (plus
  `engine` / `engine_requested` / `engine_fallback_reason` / `backend_model`
  telemetry). A learned matter is meaningful only with a trimap, so without one
  the node records a skip reason and keeps the heuristic.
- `--probe-engines` reports availability; `refine_mask_edge` joins the cross-card
  `probe_engines` aggregation so the inspector greys it out when its dep/weight
  is missing. Missing dep/weight → graceful fallback to the heuristic.

### 5.4 Dependencies & risks
`onnxruntime` + a matting weight (not bundled; the same family as the native
ViTMatte path in `subject_matte.rs`). Risks: trimap quality dominates matting
quality (the seam is gated on a connected trimap), and the weight is opt-in so
real-inference CI is gated like ViTMatte.

---

## 6. Cross-cutting concerns

- **Packaging (ties to Issue #2):** model weights are **not** bundled in the
  installer. Backends resolve weights from `HGRIPE_MODEL_CACHE` (downloaded /
  configured post-install). The bundled `python/bridge` + `custom_nodes` +
  `third_party` subtree stays the lightweight CPU baseline.
- **Capability probing:** ✅ each local card's CLI exposes `--probe-engines`, and
  the `probe_engines` Tauri command aggregates them into a **cross-card capability
  report** (the `doctor`-style probe). The Dashboard surfaces it and the inspector
  uses it to **grey out engines** whose deps/weights are missing on this box (the
  CPU/`rules` baseline stays enabled, so the node always falls back to CPU). ✅
  the report also carries **GPU/CUDA device detail** (the machine `runtime`
  probe — Dashboard **Compute** section + the inspector's per-engine "runs on
  GPU / falls back to CPU" badge) and a **cached-weight inventory** per engine
  (each ML engine's non-bundled `weight` path / `present` / `size_mb`, surfaced
  in the Dashboard so it is clear what is downloaded vs still missing). The ONNX
  engines honour that badge: a shared `sr_backends.onnx_providers()` selects
  `CUDAExecutionProvider` first when ONNX Runtime exposes it (CPU always last),
  mirroring the torch backends' "cuda if available else cpu" auto behaviour
  instead of the old hard-coded CPU provider. ✅ explicit per-node `device`
  selection has landed for the Image Enhance engine: the `--device`
  (`auto`/`cpu`/`cuda`) param threads into `RealEsrganBackend.upscale` via the
  shared `sr_backends.resolve_device()` helper, and the enhance report records
  both `device_requested` and the `device` the run *actually* used (an explicit
  `cuda` degrades to `cpu` on a box with no CUDA device, reported truthfully).
  The seam now also covers the **ONNX** engines: the shared
  `sr_backends.onnx_providers(available, device=…)` honours the same
  `auto`/`cpu`/`cuda` selection (an explicit `cpu` pins the CPU provider; `cuda`
  degrades to CPU when ORT exposes no accelerator) and `provider_device()` maps
  the bound provider back to a `cpu`/`cuda` label for the report. **Refine Mask
  Edge** (`onnx_matting`), **Match Light & Color** (`onnx_harmonize`) and
  **Detail Watchdog** (`onnx_defect`) are wired end-to-end (their `--device`
  threads into the session and the edge / match / watchdog report records
  `device_requested` + `device`) — every ONNX engine now honours `--device`,
  wired end-to-end through the Tauri commands / Graph executor and the inspector
  UI. A per-node **`precision`** (`auto`/`fp32`/`fp16`) selection has now landed
  for the **torch** backends (Image Enhance `realesrgan` + Detail Repaint
  `sd_inpaint`): a shared `sr_backends.resolve_precision()` resolves `auto`→
  `fp16` on CUDA / `fp32` on CPU, an explicit `fp16` degrades truthfully to
  `fp32` on a CPU run, the backends bind `torch.half()` accordingly, and the
  enhance / repaint reports record `precision_requested` + the `precision` the
  run *actually* used. The ONNX engines keep no `precision` knob (their
  precision is fixed at export). The "local model manager" surface has landed:
  the Dashboard **Local models** panel persists per-engine `weights_path`
  overrides + the shared cache dir (`model_paths.json`), applied as env vars on
  every bridge subprocess with real env vars still winning.
- **Determinism & safety:** seedable backends; keep the text/logo guards; require
  rule+ML agreement before any *automatic* (non-user-confirmed) repaint.
- **Contracts are stable:** every Phase 2 backend emits the existing
  `QualityReport` / `RepaintReport` / enhance JSON shapes. Phase 2 is selected
  per run via `profile_ref`; the CPU path remains the default and fallback.

## 7. Suggested sequencing

1. **SR first (highest visible win, lowest risk):** Real-ESRGAN backend behind
   `profile_ref` + capability probe + CPU fallback.
2. **Repaint local backend:** reuse the existing manifest; add SD/SDXL inpaint.
3. **Watchdog ML passes:** OCR/logo + face/hand, then a VLM pass; graduate the
   currently-`skipped` semantic targets.
4. **SupIR/CCSR** as premium SR profiles once the backend dispatch + weight cache
   are proven by Real-ESRGAN.

The per-card `engine` seams (1–3 above plus Match Light & Color and Refine Mask
Edge) have all landed; what remains across the board is the trained weights, the
premium backends, and the opt-in real-inference CI.
