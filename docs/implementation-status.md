# Implementation Status — what's landed vs. still planned

> **Purpose:** a single, long-lived cross-reference of which documented
> capabilities are actually implemented today versus still in the design/roadmap
> stage. The per-card docs under [`docs/cards/`](cards/) remain the frozen
> contracts; the [Phase 2 roadmap](design/phase2-algorithm-roadmap.md) and the
> [executor-split design](design/executor-split-and-psd-chain-hardening.md) hold
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
| Match Light & Color (`color_match_cli.py`) | 🟡 Partial | Rule-based light/colour match (CPU baseline), plus an opt-in **`engine` seam** (`python/bridge/color_backends/`) with an **`onnx_harmonize`** learned matcher + capability probe + CPU fallback. Real ONNX inference is opt-in (deps/weight not bundled). See §2. |
| PSD Export (`compose_psd_cli.py`) | ✅ Landed | Smart-object replacement + `.psd`/preview/metadata triplet. |
| Refine Mask Edge (`edge_refine_cli.py`) | 🟡 Partial | CPU clean/feather + trimap-aware hand-off (protects the matte unknown band) landed, plus an opt-in **`engine` seam** (`python/bridge/matting_backends/`) with an **`onnx_matting`** learned matter (solves the trimap unknown band) + capability probe + CPU fallback. Real ONNX inference is opt-in (deps/weight not bundled). See §2. |
| Image Enhance (`image_enhance_cli.py`) | 🟡 Partial | CPU Lanczos upscale + denoise + unsharp default, plus an opt-in **`engine` seam** (`python/bridge/sr_backends/`) with a **Real-ESRGAN** backend + capability probe + CPU fallback, and an opt-in **`--device`** (`auto`/`cpu`/`cuda`) selector that the report echoes truthfully (`device_requested` vs the `device` actually used). Real GPU inference is opt-in (deps/weight not bundled). See §2. |
| Detail Watchdog (`detail_watchdog_cli.py`) | 🟡 Partial | Always-on CPU rule layer, plus an opt-in **`engine` seam** (`python/bridge/detector_backends/`) with an **`onnx_defect`** detector + capability probe + rule-only fallback. The trained models behind it are not bundled; semantic targets stay `skipped` until a detector covers them. See §2. |
| Detail Repaint (`detail_repaint_cli.py`) | 🟡 Partial | `prepare`/`composite` around a provider `image.edit` call, plus an opt-in **`engine` seam** (`python/bridge/inpaint_backends/`) with a **`sd_inpaint`** local backend (`repaint` subcommand) + capability probe + provider fallback. Real GPU inference is opt-in (deps/weight not bundled). See §2. |

## 2. Phase 2 algorithm backends — [`design/phase2-algorithm-roadmap.md`](design/phase2-algorithm-roadmap.md)

The roadmap is **partly landed**: the per-card `engine` seams now ship across
all five PSD cards (only their trained weights are pending). The
guiding principle is additive, opt-in backends selected per run via the local
card's `engine` param (the API-card `profile_ref` is a separate credentials
concept), with the CPU path remaining the default and fallback.

| Item | Status | What's missing |
| --- | --- | --- |
| **Super-resolution** GPU backend | 🟡 Partial | `engine` seam + `python/bridge/sr_backends/` registry + **Real-ESRGAN** backend (lazy torch, weight from `HGRIPE_MODEL_CACHE`) + `--probe-engines` capability probe + graceful CPU fallback **landed**. Still ⛔: CCSR / SupIR backends, real-inference CI (opt-in like ViTMatte), installer weight story. |
| **Detail Watchdog** ML/VLM passes | 🟡 Partial | `engine` seam + `python/bridge/detector_backends/` registry + **`onnx_defect`** detector (lazy `onnxruntime`, weight from `HGRIPE_WATCHDOG_MODEL` / `HGRIPE_MODEL_CACHE`, hands/text/logo) + `--probe-engines` probe + graceful rule-only fallback **landed**. The synthesised-model ONNX session path (incl. the new `onnx_providers` provider selection) now runs in CI via the **`python bridge (onnx inference)`** lane. Still ⛔: the actual trained face/hand-quality, OCR + logo/template, and VLM defect models behind it, plus real *trained-weight* inference CI (opt-in like ViTMatte). Currently-`skipped` targets graduate to real findings only once a real weight lands. |
| **Detail Repaint** local inpaint backend | 🟡 Partial | `engine` seam + `python/bridge/inpaint_backends/` registry + **`sd_inpaint`** backend (lazy `torch`/`diffusers`, weight from `HGRIPE_INPAINT_MODEL` / `HGRIPE_MODEL_CACHE`) consuming the existing crop+mask+prompt manifest via the `repaint` subcommand + `--probe-engines` probe + graceful provider fallback **landed**. Still ⛔: SDXL / Flux Fill backends, optional ControlNet, Poisson/gradient-domain seam blending, real-inference CI (opt-in like ViTMatte). |
| **Match Light & Color** learned matcher | 🟡 Partial | `engine` seam + `python/bridge/color_backends/` registry + **`onnx_harmonize`** backend (lazy `onnxruntime`, weight from `HGRIPE_COLOR_MODEL` / `HGRIPE_MODEL_CACHE`) consuming the same subject/alpha/background inputs and emitting into the existing `match_report` contract + `--probe-engines` probe + graceful CPU-heuristic fallback **landed**. The learned correction is applied inside the subject alpha, scaled by `strength`. The synthesised-model ONNX session path (incl. the new `onnx_providers` provider selection) now runs in CI via the **`python bridge (onnx inference)`** lane. Still ⛔: the actual trained harmonisation weight, real *trained-weight* inference CI (opt-in like ViTMatte), installer weight story. |
| **Refine Mask Edge** learned matter | 🟡 Partial | `engine` seam + `python/bridge/matting_backends/` registry + **`onnx_matting`** backend (lazy `onnxruntime`, weight from `HGRIPE_MATTING_MODEL` / `HGRIPE_MODEL_CACHE`) that solves a high-quality alpha for the trimap **unknown band** (hair / fur / glass) and replaces the heuristic source matte there, while the definite FG/BG regions still get the morphology/guided/feather clean-up + `--probe-engines` probe + graceful CPU-heuristic fallback (and a skip when no trimap is connected) **landed**. The synthesised-model ONNX session path (incl. the new `onnx_providers` provider selection) now runs in CI via the **`python bridge (onnx inference)`** lane. Still ⛔: the actual trained matting weight (ViTMatte / IndexNet / MODNet export), real *trained-weight* inference CI (opt-in like ViTMatte), installer weight story. |
| **Capability probe / weight cache** | 🟡 Partial | Per-engine `--probe-engines` + `HGRIPE_MODEL_CACHE` resolution **landed for Image Enhance, Detail Watchdog, Detail Repaint, Match Light & Color and Refine Mask Edge**; the `probe_engines` Tauri command aggregates all five into a **cross-card capability report** that the Dashboard surfaces and the inspector uses to **grey out unavailable engines** (the CPU/`rules`/`provider` baseline stays enabled) **landed**. **GPU/CUDA device detail landed** too: a one-shot `device_probe_cli.py` (shared `sr_backends.device_probe`) reports `cuda_available` + CUDA device names/VRAM (via `torch`) and the available **ONNX Runtime execution providers**, aggregated once into the report's `runtime` field and shown in the Dashboard **Compute** section so the UI can warn that a GPU engine falls back to CPU on a box with no CUDA device. Each engine entry now also carries an **`accelerated`** flag (the GPU-capable ML backends are `true`; the CPU/`rules`/`provider` baseline is `false`), which the **Inspector** pairs with the device probe to badge the selected engine "runs on GPU" vs "no CUDA — runs on CPU (slower)". That badge is now **truthful for the ONNX engines** (`onnx_matting` / `onnx_harmonize` / `onnx_defect`): they used to hard-code the CPU execution provider, so a shared `sr_backends.onnx_providers()` now selects `CUDAExecutionProvider` first when ONNX Runtime exposes it (CPU always last as the fallback), mirroring the torch backends' existing "cuda if available else cpu" auto behaviour. **Cached-weight inventory landed** too: each ML engine reports its non-bundled `weight` (`path` / `present` / `size_mb`), so the Dashboard shows what is downloaded vs still missing rather than only "engine unavailable" (the CPU/`rules`/`provider` baseline carries no weight). An explicit per-node **`device`** selection has **landed for Image Enhance, Refine Mask Edge, Match Light & Color and Detail Watchdog**: a shared `sr_backends.resolve_device()` (torch) backs the CLI's `--device` (`auto`/`cpu`/`cuda`) param for Real-ESRGAN, and the ONNX analogue `sr_backends.onnx_providers(available, device=…)` + `provider_device()` backs it for the `onnx_matting` learned matter, the `onnx_harmonize` learned matcher and the `onnx_defect` defect detector (an explicit `cpu` pins the CPU provider; `cuda` degrades to CPU when ORT exposes no accelerator). Each backend returns the device it actually ran on, and the enhance / edge / match / watchdog reports record `device_requested` + `device` (reported truthfully). With `onnx_defect` wired, **every ONNX engine now honours `--device`** at the CLI. **All four ONNX cards are now wired end-to-end through the desktop app** (Detail Watchdog, Image Enhance, Refine Mask Edge, Match Light & Color): each card's Tauri command (`detect_quality_issues` / `enhance_image` / `refine_mask_edge` / `match_light_color`) + Graph executor thread the node's `device` param into the CLI, the matching report (`WatchdogReport` / `EnhanceReport` / `EdgeReport` / `MatchReport`) carries `device` + `device_requested`, and the Inspector exposes a `device` selector (`auto`/`cpu`/`cuda`, shown only for that card's ONNX engine — `onnx_defect` / `realesrgan` / `onnx_matting` / `onnx_harmonize`) alongside the existing GPU/CPU-fallback note. A per-node **`precision`** (`auto`/`fp32`/`fp16`) selection has now **landed for the torch backends** (Image Enhance's `realesrgan` + Detail Repaint's `sd_inpaint`): a shared `sr_backends.resolve_precision()` resolves `auto`→`fp16` on CUDA / `fp32` on CPU, an explicit `fp16` degrades truthfully to `fp32` on a CPU run, and `fp32` always stays full. The torch backends bind `torch.half()` accordingly and return the precision they actually ran; the CLIs (`--precision`), Tauri commands (`enhance_image` / `local_repaint_regions`) and reports (`EnhanceReport` / `RepaintReport`) carry `precision_requested` + `precision`, and the Inspector exposes a `precision` selector shown only for those two GPU-capable torch engines. The ONNX engines keep no `precision` knob (export-fixed precision). |

## 3. Subject Mask / Matte — [`subject-mask-matte.md`](cards/subject-mask-matte.md)

| Item | Status | Notes |
| --- | --- | --- |
| Manual brush / eraser / wand / marquee / morphology | ✅ Landed | Phase 1 Mask-Edit tool set. |
| Auto modes via in-process model cascade | ✅ Landed | BiRefNet lite / U²-Netp salient cascade + point-prompt **SAM 2**, `builtin-cpu` fallback. |
| SAM 2 point prompts (positive **and** negative) | ✅ Landed | Left-click include (green), right-click exclude (red) → `point_labels`; builtin fallback excludes connected components. |
| Alpha matting (continuous alpha) | ✅ Landed | `alpha_matting` → trimap → **ViTMatte** (`ort`) when the weight resolves, else deterministic image-guided **guided-filter** `builtin-cpu-matte`. |
| Matting paint tool (hand-painted unknown band) | ✅ Landed | `matte_strokes` stamped onto the trimap before matting. |
| Trimap hand-off to Refine Mask Edge | ✅ Landed | `trimap` output → Refine `trimap` input protects the soft-alpha band. |
| **`auto_person` portrait-matting net** | 🟡 Partial | The **`u2net_human_seg`** human-segmentation net (Apache-2.0, ~168 MB, env `HGRIPE_PERSON_MODEL` / `scripts/fetch-person-model.*`) slots into `segmenter_for_mode` behind the same trait: `auto_person` leads with it (so the matte tracks people, not generic saliency), then falls through to BiRefNet → U²-Netp → `builtin-cpu`; other modes keep the generic priority. Still ⛔: bundling the weight in the installer (downloadable big tier today). |
| **Pen / Lasso (bezier paths)** | ⛔ Planned (Phase 3) | UI greyed; `paths` are stored but **not rasterised** (field versioned for forward-compat). |
| **SAM 2 multi-variant XY compare (T/S/B/L)** | ⛔ Planned | Only `sam2 tiny` is fetched today; multi-weight comparison not wired. |

## 4. Executor-split / management surfaces — [`design/executor-split-and-psd-chain-hardening.md`](design/executor-split-and-psd-chain-hardening.md)

| Item | Status | Notes |
| --- | --- | --- |
| Executor lanes (Graph / Local / Compute / Api / Hybrid) | ✅ Landed | `StudioExecutor` + `studio_executor_for_kind` + `executor` field on node specs. |
| Input hardening (CMYK/ICC normalise, EXIF, `--max-decode-pixels`) | ✅ Landed | Across the PSD cards. |
| Colour pipeline: wide-gamut 16-bit working space + manual/model split — [`design/colour-pipeline.md`](design/colour-pipeline.md) | ✅ Landed | **P1–P5 landed.** CMYK decode coverage ✅ (#180–#186). Canonical surface is now **16-bit ProPhoto** for profiled CMYK (#188–#190); the card/model/output boundary colour-manages **ProPhoto → sRGB**, while plain images / naive CMYK egress as an exact bit-narrow (byte-exact contract held). **P4 (manual-path 16-bit chain) complete**: `image_buffer` carries the 16-bit `WorkingImage` natively (P4a #191), crop walks it end-to-end with 16-bit PNG-with-ICC output (P4b #192), 16-bit TIFF-with-ICC output + crop `format` param (P4c #193), subject-mask 16-bit cutout/RGBA products (P4d #194), close-out P4e: the remaining manual cards (`matchLightColor`, `detailWatchdog`, `refineMaskEdge`, `imageEnhance`) are python-bridge cards whose pixel work is reconciled in P5 — the native `imageEnhance` cpu fast path is pinned byte-identical to the Python cpu engine, so it moves with P5 too. **P5 (Python-bridge parity) complete**: every bridge CLI colour-manages ProPhoto-tagged manual products to sRGB at ingress via the shared `wide_gamut.py` (#202), and the enhance cpu fast path no longer re-embeds the stale ProPhoto profile on its sRGB output (#203); cross-engine parity is pinned by a Rust-written fixture asserted to the same goldens on both sides. **Open decisions closed**: TRC — working space stays gamma-encoded with per-operation linear-light where the maths need it (first landing: enhance colour resample on both engines, #205); local-model bit depth — 8-bit sRGB for all eight current integrations (all trained on 8-bit sRGB; revisit per future integration). Initiative complete. |
| **Local model management surface** | 🟡 Partial | The per-node params have landed (`engine`, `device` on the ONNX + torch engines, `precision` on the torch engines) along with the Dashboard capability/weight-inventory/Compute reporting. Still ⛔: `weights_path` management and a dedicated "local model manager" panel over that surface. |
| **"API manager" UI** | ⛔ Planned | Enumerate `api` cards + profiles/credentials. |
| Per-card `engine` seams (matcher) | 🟡 Partial | Image Enhance, Detail Watchdog, Detail Repaint, Match Light & Color and Refine Mask Edge expose real opt-in `engine` seams (`realesrgan` / `onnx_defect` / `sd_inpaint` / `onnx_harmonize` / `onnx_matting`); every PSD production card with an ML upside now has a seam. Only the trained weights remain ⛔. |

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

## 7. Editor resource & threading model — [`design/editor-resource-model.md`](design/editor-resource-model.md)

The full staged rollout of the editor compute/threading model has **landed**.

| Item | Status | Notes |
| --- | --- | --- |
| Preview lane (single-slot, latest-wins, decoupled from run lock) | ✅ Landed | PR #145; first consumer is live mask-morphology proxy preview. |
| Explicit exec-lane scheduler + GPU `Semaphore(1)` | ✅ Landed | PR #146; replaces the accidental serial `.await` loop in `exec.rs`. |
| ONNX warm pool (`onnx_pool.rs`) | ✅ Landed | PR #147; process-global `ort::Session` reuse (see §1/§3). |
| Long-lived torch worker (`torch_worker.rs`) | ✅ Landed | PR #148; realesrgan / sd_inpaint stay warm, one-shot fallback on failure. |
| Video media engine (decoder seam + frame cache + playback thread) | ✅ Landed | PR #149; `video_engine.rs` + `frame_cache.rs`, `video_scrub` command. |
| Native in-process ffmpeg `FrameSource` | ✅ Landed | PR #150; `ffmpeg_native.rs` links **vendored** libav (`third_party/ffmpeg`, git-lfs) behind the off-by-default `native-ffmpeg` feature, with PyAV fallback. |
| Video trim / **export / encode** | ⛔ Planned | The media engine is decode/scrub only; there is no encode path yet. |

## 8. Out of scope (explicit product-direction decisions)

These were floated in early vision/research notes but are **not** committed
work. The product today is PSD-first, single-image, CPU bridge.

| Item | Status | Notes |
| --- | --- | --- |
| Video **subject** axis (temporal mask tracking / flicker smoothing) | ⛔ Not planned | Would need a video predictor (SAM 2 memory bank); the bundled SAM 2 ONNX is the **image** variant. Distinct from the decode/scrub **media engine**, which *has* landed (§7) — this row is about propagating a *mask* across frames, not playback. Needs a separate product decision. |
| Private local SD video content-aware fill | ⛔ Not planned | Video axis is out of scope; the *still-image* local SD inpaint `engine` (`sd_inpaint`) has landed as an opt-in alternative to provider `image.edit` (see §2). |
