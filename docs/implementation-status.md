# Implementation Status — what's landed vs. still planned

> **Purpose:** a single, long-lived cross-reference of which documented
> capabilities are actually implemented today versus still in the design/roadmap
> stage. The per-card docs under [`docs/cards/`](cards/) remain the frozen
> contracts; the [Phase 2 roadmap](phase2-algorithm-roadmap.md) and the
> [executor-split design](card-executor-split-and-psd-chain-hardening.md) hold
> the forward-looking plans. This file just consolidates the *gaps* so they
> don't get lost across documents.
>
> **How to read the status column:**
> - ✅ **Landed** — implemented and covered by tests / CI.
> - 🟡 **Partial** — a deliberate Phase 1 / CPU baseline is in place; the
>   production-grade (usually GPU/ML) path is not.
> - ⛔ **Planned** — design only, no implementation.
>
> Keep this table honest: when a feature lands, flip its row and link the PR.
> When a new card/feature is documented, add a row so the gap is tracked.

---

## 1. PSD production chain (the eight cards)

| Capability | Status | Notes |
| --- | --- | --- |
| PSD Context Analyze (`analyze_psd_cli.py`) | ✅ Landed | `VisualContext` (lighting / bounds / masks) extraction. |
| Match Light & Color (`color_match_cli.py`) | ✅ Landed (CPU) | Rule-based light/colour match. Learned matcher `engine` is ⛔ planned. |
| PSD Export (`compose_psd_cli.py`) | ✅ Landed | Smart-object replacement + `.psd`/preview/metadata triplet. |
| Refine Mask Edge (`edge_refine_cli.py`) | 🟡 Partial | CPU clean/feather + trimap-aware hand-off (protects the matte unknown band) landed. A learned-matting / `guidedFilter` `profile_ref` **engine mode** is ⛔ planned. |
| Image Enhance (`image_enhance_cli.py`) | 🟡 Partial | CPU Lanczos upscale + denoise + unsharp default, plus an opt-in **`engine` seam** (`python/bridge/sr_backends/`) with a **Real-ESRGAN** backend + capability probe + CPU fallback. Real GPU inference is opt-in (deps/weight not bundled). See §2. |
| Detail Watchdog (`detail_watchdog_cli.py`) | 🟡 Partial | Rule heuristics only; semantic targets reported `skipped`. See §2. |
| Detail Repaint (`detail_repaint_cli.py`) | 🟡 Partial | `prepare`/`composite` around a provider `image.edit` call; no local backend. See §2. |

## 2. Phase 2 algorithm backends — [`phase2-algorithm-roadmap.md`](phase2-algorithm-roadmap.md)

The roadmap is **mostly design-only today**, the first backend has landed. The
guiding principle is additive, opt-in backends selected per run via the local
card's `engine` param (the API-card `profile_ref` is a separate credentials
concept), with the CPU path remaining the default and fallback.

| Item | Status | What's missing |
| --- | --- | --- |
| **Super-resolution** GPU backend | 🟡 Partial | `engine` seam + `python/bridge/sr_backends/` registry + **Real-ESRGAN** backend (lazy torch, weight from `HGRIPE_MODEL_CACHE`) + `--probe-engines` capability probe + graceful CPU fallback **landed**. Still ⛔: CCSR / SupIR backends, real-inference CI (opt-in like ViTMatte), installer weight story, UI greying via the probe. |
| **Detail Watchdog** ML/VLM passes | ⛔ Planned | Face/hand quality model, OCR + logo/template matching, VLM defect pass. Currently-`skipped` semantic targets graduate to real findings only once these land. |
| **Detail Repaint** local inpaint backend | ⛔ Planned | Local GPU diffusion inpaint (SD/SDXL/Flux Fill) consuming the existing crop+mask+prompt manifest, optional ControlNet, Poisson/gradient-domain seam blending. |
| **Capability probe / weight cache** | 🟡 Partial | Per-engine `--probe-engines` + `HGRIPE_MODEL_CACHE` resolution **landed for Image Enhance**. Still ⛔: a `doctor`-style cross-card GPU/CUDA/backend report and the UI wiring that greys unavailable engines. |

## 3. Subject Mask / Matte — [`subject-mask-matte.md`](cards/subject-mask-matte.md)

| Item | Status | Notes |
| --- | --- | --- |
| Manual brush / eraser / wand / marquee / morphology | ✅ Landed | Phase 1 Mask-Edit tool set. |
| Auto modes via in-process model cascade | ✅ Landed | BiRefNet lite / U²-Netp salient cascade + point-prompt **SAM 2**, `builtin-cpu` fallback. |
| SAM 2 point prompts (positive **and** negative) | ✅ Landed | Left-click include (green), right-click exclude (red) → `point_labels`; builtin fallback excludes connected components. |
| Alpha matting (continuous alpha) | ✅ Landed | `alpha_matting` → trimap → **ViTMatte** (`ort`) when the weight resolves, else deterministic image-guided **guided-filter** `builtin-cpu-matte`. |
| Matting paint tool (hand-painted unknown band) | ✅ Landed | `matte_strokes` stamped onto the trimap before matting. |
| Trimap hand-off to Refine Mask Edge | ✅ Landed | `trimap` output → Refine `trimap` input protects the soft-alpha band. |
| **`auto_person` portrait-matting net** | ⛔ Planned | Slots into `segmenter_for_mode` behind the same trait. |
| **Pen / Lasso (bezier paths)** | ⛔ Planned (Phase 3) | UI greyed; `paths` are stored but **not rasterised** (field versioned for forward-compat). |
| **SAM 2 multi-variant XY compare (T/S/B/L)** | ⛔ Planned | Only `sam2 tiny` is fetched today; multi-weight comparison not wired. |

## 4. Executor-split / management surfaces — [`card-executor-split-and-psd-chain-hardening.md`](card-executor-split-and-psd-chain-hardening.md)

| Item | Status | Notes |
| --- | --- | --- |
| Executor lanes (Graph / Local / Compute / Api / Hybrid) | ✅ Landed | `StudioExecutor` + `studio_executor_for_kind` + `executor` field on node specs. |
| Input hardening (CMYK/ICC normalise, EXIF, `--max-decode-pixels`) | ✅ Landed | Across the PSD cards. |
| **Local model management surface** | ⛔ Planned | `engine` + future `weights_path` / `device` / `precision` as the only surface a "local model manager" UI would need. |
| **"API manager" UI** | ⛔ Planned | Enumerate `api` cards + profiles/credentials. |
| Per-card `engine` seams (matcher, watchdog detector) | ⛔ Planned | Placeholders only (`cpu` today). |

## 5. Packaging & verification gaps

| Item | Status | Notes |
| --- | --- | --- |
| Bundled CPU baseline (u²-netp ~4.6 MB) | ✅ Landed | Fetched at package time, shipped via `tauri.conf.json` `bundle.resources`. |
| **Big-tier weights bundling** (Issue #2) | ⛔ Planned | BiRefNet lite / SAM 2 / ViTMatte downloaded post-install; not in the installer. Installer packaging story undecided. |
| **ViTMatte real inference in CI** | 🟡 Partial | Weight-gated unit test + opt-in `tauri (vitmatte e2e)` job exists, but it's `workflow_dispatch` and skipped on normal PRs — real inference is only verified on manual trigger. |

## 6. Internationalisation (cards)

| Item | Status | Notes |
| --- | --- | --- |
| Node-card / Inspector / Palette / search / Mask-Edit i18n (中/英) | ✅ Landed | English `NODE_SPECS` source + `nodeSpecsI18n` / `maskToolsI18n` zh overlays + `localizeSpec` resolver. A coverage test fails CI if any node/param/port/tool ships without a zh entry. |

## 7. Out of scope (explicit product-direction decisions)

These appear in early vision/research notes
([`API_FIRST_DESKTOP_PLAN.md`](../API_FIRST_DESKTOP_PLAN.md),
[`PSD_AI_PRODUCTION_WORKFLOW_RESEARCH.md`](../PSD_AI_PRODUCTION_WORKFLOW_RESEARCH.md))
but are **not** committed work. The product today is PSD-first, single-image,
CPU bridge.

| Item | Status | Notes |
| --- | --- | --- |
| Video axis (temporal tracking / flicker smoothing / timeline scrubbing) | ⛔ Not planned | Would need a video predictor (SAM 2 memory bank) + a video timeline; the bundled SAM 2 ONNX is the **image** variant. Needs a separate product decision. |
| Private local SD video content-aware fill | ⛔ Not planned | Local SD = GPU; current `detailRepaint` uses provider `image.edit`. |
