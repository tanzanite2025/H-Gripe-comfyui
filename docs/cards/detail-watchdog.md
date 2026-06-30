# Detail Watchdog card

Executor: **local** (always `python/bridge/detail_watchdog_cli.py`, never networks).
Backend: `detect_quality_issues` Tauri command → `detail_watchdog_cli.py` (Pillow + numpy, CPU-only in Phase 1).

Scans a candidate image for local quality breakdowns and emits a structured
`QualityReport` so the workflow can decide whether to re-run, hand-fix, or
repaint a region before composing into the PSD. This document is the card's
contract: what it accepts, what it guarantees, and how it behaves at the edges.
Phase 1 is **detect + report only** (it never repaints — `fixed_image` is the
input unchanged) and dependency-light (no OpenCV, no ML). Semantic detection of
hands, packaging text and logo deformation needs the later GPU/VLM backend and
is intentionally **not** attempted here; those watch targets are recorded as
skipped rather than guessed at.

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
| `watch_targets` | csv | all | `face,hands,text,logo,product_edges` | Empty = all. `hands`/`text`/`logo` are recorded in `skipped_targets`. |
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
`exif_transposed`, `max_decode_pixels`, `mask_consumed`.

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
  and `WatchdogReport` deserialization of the v1 hardening fields (plus legacy
  JSON defaults).
