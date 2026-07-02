# Detail Watchdog card

Executor: **local** (always `python/bridge/detail_watchdog_cli.py`, never networks).
Backend: `detect_quality_issues` Tauri command → `detail_watchdog_cli.py` (Pillow + numpy, CPU-only in Phase 1).

Scans a candidate image for local quality breakdowns and emits a structured
`QualityReport` so the workflow can decide whether to re-run, hand-fix, or
repaint a region before composing into the PSD. This document is the card's
contract: what it accepts, what it guarantees, and how it behaves at the edges.
Phase 1 is **detect + report only** (it never repaints — `fixed_image` is the
input unchanged). The CPU **rule layer** (Pillow + numpy, no ML) is the
always-available baseline and always runs. Semantic detection of hands,
packaging text and logo deformation needs a learned detector: the rule layer
never guesses at them, recording them as `skipped` instead. They graduate to
real findings through an **opt-in** ML detector selected by the `engine` param
(see *Engine seam* below); when the chosen detector's optional dependency or
weight is missing, the node falls back to the rule-only report and records why
(`engine_fallback_reason`) — it never hard-fails for lack of a model.

## Inputs (ports)

| Port | Type | Required | Notes |
| --- | --- | --- | --- |
| `image` | image path | yes | The candidate image to inspect. Its alpha rim is used for halo detection. |
| `visual_context` | JSON | no | Connected `VisualContext` (background `mean_color` + placeholder `bounds`) from PSD Context Analyze. |
| `target_bounds` | JSON | no | Standalone placeholder rectangle `{x,y,width,height}`; overrides `visual_context`'s placeholder for the size check. |
| `mask` | image path | no | **Advisory only** in Phase 1; not consumed (`mask_consumed: false`). Detection runs on the image's own alpha. |

## Parameters

| Param | Type | Default | Range / values | Notes |
| --- | --- | --- | --- | --- |
| `mode` | enum | `balanced` | `strict` \| `balanced` \| `lenient` | Detection aggressiveness (thresholds below). |
| `watch_targets` | csv | all | `face,hands,text,logo,product_edges` | Empty = all. `hands`/`text`/`logo` stay in `skipped_targets` unless an `engine` covers them. |
| `engine` | enum | `rules` | `rules` \| `onnx_defect` | Detection engine. `rules` = built-in CPU rule layer (always on). `onnx_defect` = opt-in ML detector for hands/text/logo, falls back to `rules` when its dep/weight is missing. |
| `max_decode_pixels` | int | `96_000_000` | `>= 0` (0 disables) | Rejects an **input** image larger than this before decoding (decompression-bomb guard). |
| `no_overlay` | bool | `false` | | Skip writing the issue-overlay PNG. |
| `output_dir` | path | run output dir | | Validated server-side. |
| `output_name` | basename | `<image>_issues` | plain basename | Rejected if it contains `..` or a path separator (`reject_unsafe_output_name`). |

## Detectors

| Issue type | What it catches | `suggested_action` |
| --- | --- | --- |
| `low_resolution` | Global Laplacian-variance blur, and/or the image being smaller than the connected placeholder bounds. | `image_enhance` |
| `face_blur` / `low_resolution` | Locally soft tiles from an 8-column sharpness grid, merged into boxes (`face_blur` when `face` is watched, else `low_resolution`). | `detail_redraw` |
| `edge_halo` | A bright fringe on the semi-transparent alpha rim of a cut-out (only when `product_edges` is watched). | `edge_refine` |
| `color_mismatch` | The subject's mean colour drifting from the connected background `mean_color`. | `color_match` |

The per-mode thresholds (`blur_floor`, `region_ratio`, `region_floor`,
`halo_delta`, `color_delta`) widen from `strict` → `balanced` → `lenient`. The
soft-region grid is fixed at **8 columns**, with rows scaled to the aspect
ratio (capped at 8).

## Colour space & bit depth

The decode is normalised to an 8-bit RGB working space (plus a separate alpha
plane) so the luminance / sharpness / colour heuristics sample honest data; the
source's original mode is recorded as `source_mode`:

| Source mode | Handling |
| --- | --- |
| `RGB` / `RGBA` / `L` / `LA` | Used directly; alpha (when present) feeds halo detection. |
| `P` (palette) | Expanded to RGB(A); transparency in `info` is treated as alpha. |
| `CMYK` | Converted to sRGB via the embedded ICC profile when present, else a naive convert. |
| `I` / `I;16*` / `F` (high bit) | Data range normalised down to 8-bit via numpy before RGB conversion. |

## Boundary behaviour

| Condition | Behaviour |
| --- | --- |
| Missing / blank `image` input | Rust handler errors `Detail Watchdog needs a connected image input` before shelling out. |
| Missing image file on disk | `FileNotFoundError: candidate image not found: <path>`. |
| Input larger than `max_decode_pixels` | `ValueError: input image too large to decode safely: WxH ...` (before decode). |
| Unknown `mode` | `ValueError: unknown mode ...; expected one of [...]`. |
| Unknown `watch_targets` entry | `ValueError: unknown watch target(s): [...]; expected [...]`. |
| Unknown `engine` | Rule-only report; `engine: rules`, `engine_fallback_reason: "unknown engine '...'"` (no error). |
| ML `engine` requested, dep/weight missing | Rule-only report; `engine: rules`, `engine_fallback_reason` explains (missing `onnxruntime` or weight); covered targets stay `skipped`. |
| EXIF-rotated input | Orientation normalised via the orientation tag; `exif_transposed: true`. |
| No issues found | `status: passed`, `issues: []`, no overlay PNG written. |
| `mask` connected | Ignored by detection (`mask_consumed: false`). |
| Unsafe `output_name` (`..`, separators) | Rejected server-side. |

## `quality_report` / `watchdog_report` fields

`quality_report` follows the shared contract: `status`
(`passed` \| `warning` \| `failed`) and `issues` (each `type`, `confidence`,
`bbox` `[x1,y1,x2,y2]`, `suggested_action`).

`watchdog_report` (diagnostics): `mode`, `watch_targets`, `skipped_targets`,
`image_size`, `target_size`, `global_sharpness`, `source_mode`,
`exif_transposed`, `max_decode_pixels`, `mask_consumed`, and the engine-seam
telemetry: `engine` (what actually ran — `rules` or a detector id),
`engine_requested` (what was asked for), `engine_fallback_reason` (why the
rule-only path was used, else `null`), `detectors` (learned passes that ran on
top of the rule layer), `backend_model` (the loaded weight file name, else
`null`).

## Engine seam (opt-in ML detectors)

The rule layer is the always-on baseline; learned detectors are **additive**
passes selected by `engine` and registered in `python/bridge/detector_backends/`
(mirroring the Image Enhance `sr_backends` super-resolution seam). A detector
declares the watch targets it covers, an `available()` probe (lazy — it never
imports `onnxruntime`/`torch` just to report availability), and a `detect()`
that emits issues into the **same** `QualityReport` contract, so the downstream
Detail Repaint consumer needs no change. Findings merge on top of the rule
findings; the targets a detector covers graduate out of `skipped_targets`.

| Engine | Deps | Weight | Covers | Emits |
| --- | --- | --- | --- | --- |
| `rules` | none | none | `face`, `product_edges` (+ global blur / colour) | `low_resolution`, `face_blur`, `edge_halo`, `color_mismatch` |
| `onnx_defect` | `onnxruntime` | `watchdog_defect.onnx` | `hands`, `text`, `logo` | `malformed_hands`, `garbled_text`, `deformed_logo` (all `suggested_action: detail_redraw`) |

`onnx_defect` resolves its weight from `HGRIPE_WATCHDOG_MODEL` (explicit path)
or `<model cache>/watchdog_defect.onnx` (`HGRIPE_MODEL_CACHE`, else the bundled
`resources/models`); the weight is **not** shipped in the installer. Model
contract: input `[1,3,H,W]` float32 RGB `0..1` (letterboxed), outputs either
`boxes` `[N,4]` xyxy / `scores` `[N]` / `labels` `[N]`, or a DB-style
segmentation **probability map** `[1,1,H,W]` (thresholded and split into
connected components, one detection per component). An optional sidecar
`<weight>.labels.json` describes the weight: either the bare class-id → target
map, or `{"labels": {...}, "normalize": "imagenet"}` to also request ImageNet
input normalisation. A weight that covers only some targets keeps the others
truthfully `skipped`. `scripts/fetch-watchdog-text.{sh,ps1}` fetches a real
trained text detector — the PP-OCRv3 det ONNX export (PaddleOCR, Apache-2.0,
~2.4 MB) — plus its sidecar, graduating the `text` target. Run
`detail_watchdog_cli.py --probe-engines` for the UI capability probe (which
engines are usable right now).

## Outputs (ports)

| Port | Type | Notes |
| --- | --- | --- |
| `fixed_image` | image path | The candidate, **unchanged** in Phase 1 (detect-only). |
| `quality_report` | JSON | The report above. |
| `issue_masks` | image path \| null | Overlay PNG with a red box per flagged region (null when no issues or `no_overlay`). |
| `watchdog_report` | JSON | The diagnostics above. |

## Tests

- `python/bridge/tests/test_detail_watchdog_cli.py` — sharp image passes and
  reports the hardening fields, low-resolution below target, edge halo on a rim,
  unsupported targets recorded as skipped, overlay written / suppressed, decode
  guard, CMYK and palette source mode, plain image not over-reported as
  transposed (Pillow 12), the advisory mask, invalid mode / watch target,
  missing image (run: `pytest python/bridge/tests`).
- `src-tauri/src/studio/detail_watchdog.rs` — the connected-image-input guard
  and `WatchdogReport` deserialization of the v1 hardening fields, the engine-
  seam telemetry fields, and legacy JSON defaults.
- `python/bridge/tests/test_detector_backends.py` — the detector registry /
  probe, unknown-engine and missing-weight fallback, the sidecar label map
  (both forms, incl. `normalize`), a gated end-to-end pass that synthesises a
  tiny ONNX detector (skipped unless `onnx` + `onnxruntime` import, mirroring
  the ViTMatte opt-in gate), and the gated
  `test_onnx_defect_real_inference_when_weight_present` e2e: the real trained
  PP-OCRv3 text-detection weight through the CLI — `text` graduates to real
  `garbled_text` findings on rendered text while hands/logo stay skipped. It
  skips without `onnxruntime` + the weight; the manual-dispatch
  **`python bridge (watchdog text e2e)`** CI lane fetches the sha256-checked
  weight (`scripts/fetch-watchdog-text.sh`) and runs it for real.
- `python/bridge/tests/test_detail_watchdog_cli.py` — also the `--engine`
  dispatch: default `rules`, unknown-engine fallback, unavailable-ML fallback.

## Verifying `onnx_defect` end-to-end

Real inference needs `onnxruntime` + a detector weight, which the per-PR CI
matrix does not install. Two verifiable paths exist:

- **CI (opt-in):** manually dispatch the CI workflow — the
  `python bridge (watchdog text e2e)` job installs `onnxruntime`, fetches the
  sha256-checked PP-OCRv3 weight via `scripts/fetch-watchdog-text.sh`, and runs
  the gated real-inference test (which skips on every normal run).
- **Manually:**

```
pip install onnxruntime
bash scripts/fetch-watchdog-text.sh
HGRIPE_WATCHDOG_MODEL=apps/desktop-tauri/src-tauri/resources/models/watchdog_defect.onnx \
python python/bridge/detail_watchdog_cli.py --image candidate.png \
  --engine onnx_defect --watch-targets text --output-dir out
# watchdog_report.engine == "onnx_defect"; garbled_text findings on text regions
```
