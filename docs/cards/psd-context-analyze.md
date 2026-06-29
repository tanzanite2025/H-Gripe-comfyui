# PSD Context Analyze card

Executor: **local** (always `python/bridge/analyze_psd_cli.py`, never networks).
Backend: `analyze_psd_context` Tauri command → `analyze_psd_cli.py`
(`psd_tools` + Pillow + numpy, CPU-only in Phase 1).

Reads a PSD *template* and distils it into a machine-usable `VisualContext`:
background colour / lighting heuristics, the target placeholder's geometry (plus
an inset "safe area"), a written placeholder mask, a background preview PNG, a
luminance histogram PNG, and a ready-to-append `prompt_suffix` describing the
template's light & colour. This document is the card's contract. The analysis is
deliberately heuristic (median-cut palette, a 3×3 light-direction grid, a
red/blue colour-temperature estimate); a learned VLM lighting backend is a
future `profile_ref` mode behind this same contract.

## Inputs (ports)

| Port | Type | Required | Notes |
| --- | --- | --- | --- |
| `template` | PSD path | yes* | The `.psd` template to analyse. *If unconnected, the `psd_path` param is used so the node also works as a standalone source. |

## Parameters

| Param | Type | Default | Range / values | Notes |
| --- | --- | --- | --- | --- |
| `psd_path` | path | `""` | | Fallback template when no `template` port is connected. |
| `background_layer` | string | `""` (whole PSD) | layer name | Layer to composite for the background statistics; empty composites the whole PSD. |
| `target_placeholder` | string | `""` (whole canvas) | layer name | Placeholder layer whose bounds define `placeholder.bounds` / `safe_area`; empty uses the whole canvas. |
| `reference_layers` | string[] | `[]` | newline / JSON list | **Advisory only in Phase 1** — parsed but not consumed by the heuristics. |
| `max_decode_pixels` | int | `96_000_000` | `>= 0` (0 disables) | Refuses a PSD whose `canvas_w * canvas_h` exceeds this **before** compositing (decompression-bomb / OOM guard). |
| `output_dir` | path | run output dir | | Where the mask / background / histogram PNGs are written. Validated server-side. |

## Background statistics

The background is composited as **RGBA** and every statistic is weighted by the
background's own alpha, so a cut-out background plate's transparent (black)
regions never describe the target colour:

- `mean_color`, `brightness` and `contrast` are alpha-weighted means / weighted
  standard deviation of the Rec.601 luminance.
- `dominant_palette` (median-cut) only feeds pixels whose alpha is `>= 0.5`
  (`_OPAQUE_FRACTION`), so transparent regions do not invent a phantom dark
  swatch.
- `color_temperature` is derived from the alpha-weighted mean RGB.
- the light direction grid (below) weights each cell by alpha and **excludes
  fully-transparent cells** from the brightest-cell vote.

If the background is fully transparent, the whole frame is used as a fallback
(uniform weight).

## Lighting heuristics

- **Direction** — the background luminance is split into a 3×3 grid; the
  brightest (alpha-weighted) cell names the key-light direction
  (`top-left` … `bottom-right`). A near-uniform grid (`spread < 0.08`) reads as
  `center` (flat / ambient).
- **Quality** — `hard` when the cell luminance `spread >= 0.35` **or** global
  `contrast >= 0.45`; otherwise `soft`.
- **Colour temperature** — `2000 + (blue/red) * 4500`, clamped to `2000..12000`K
  and rounded to the nearest 100K. `<= 5000`K reads `warm`, `>= 7000`K `cool`,
  else `neutral`. A heuristic, not a calibrated measurement.

## Placeholder geometry

Placeholder bounds reuse the compose node's `_resolve_placeholder`
(`custom_nodes/hgripe_psd_nodes.py`) so geometry stays a single source of truth
with the PSD Compose card. The `safe_area` is the bounds inset by 5% on each
axis (clamped to non-negative width/height). A binary placeholder mask
(`255` inside the bounds) is written for downstream nodes.

## Histogram artifact

A 256-bin, alpha-weighted luminance histogram is rendered to a small
`<stem>_histogram.png` (256×100, light bars on a dark ground) and its path is
returned as `background.histogram_path`. The bins use the same alpha weight as
the other statistics, so transparent regions do not spike bin 0.

## Boundary behaviour

| Condition | Behaviour |
| --- | --- |
| Missing `template` port **and** blank `psd_path` | Rust handler errors `PSD Context Analyze needs a PSD template ...` before shelling out. |
| Template file not on disk | `FileNotFoundError: PSD template not found: <path>`. |
| Canvas larger than `max_decode_pixels` | `ValueError: PSD canvas too large to composite safely: WxH ...` (before compositing). |
| `max_decode_pixels = 0` | Guard disabled. |
| `background_layer` not found | Falls back to compositing the whole PSD. |
| `target_placeholder` empty / not found | Whole canvas is used as the placeholder bounds. |
| Fully transparent background | Statistics fall back to uniform weight over the whole frame. |
| `reference_layers` set | Parsed but not consumed (advisory in Phase 1). |

## `VisualContext` (emitted JSON)

Matches the contract defined once in
`apps/desktop-tauri/src-tauri/src/contracts.rs` and mirrored in
`apps/desktop-tauri/studio-ui/src/types/production.ts`.

| Field | Type | Notes |
| --- | --- | --- |
| `background.mean_color` | `[r, g, b]` | Alpha-weighted mean, 0–255. |
| `background.dominant_palette` | `string[]` | Up to 5 `#rrggbb` swatches, most frequent first. |
| `background.brightness` | float `0..1` | Alpha-weighted mean luminance / 255. |
| `background.contrast` | float `0..1` | Alpha-weighted luminance std / 128, clamped. |
| `background.histogram_path` | path | Written luminance histogram PNG. |
| `background.image_path` | path | Written background preview PNG (RGBA). |
| `lighting.direction` | enum | `center` or one of the eight compass cells. |
| `lighting.quality` | enum | `hard` \| `soft`. |
| `lighting.color_temperature` | int (K) | `2000..12000`, nearest 100K. |
| `lighting.description` | string | Human-readable summary. |
| `placeholder.layer_name` | string | Requested placeholder name (`""` = whole canvas). |
| `placeholder.bounds` | `{x, y, width, height}` | Placeholder rectangle in canvas pixels. |
| `placeholder.mask_path` | path | Binary placeholder mask PNG. |
| `placeholder.safe_area` | `{x, y, width, height}` | Bounds inset 5% per axis. |
| `prompt_suffix` | string | Lighting/colour hint to append downstream. |

## Outputs (ports)

| Port | Type | Notes |
| --- | --- | --- |
| `visual_context` | JSON | The full `VisualContext` above. |
| `prompt_suffix` | string | Convenience copy of `visual_context.prompt_suffix`. |
| `background_image` | image path | The background preview PNG. |
| `placeholder_mask` | image path | The placeholder mask PNG. |
| `placeholder_bounds` | JSON | `{x, y, width, height}`. |

## Tests

- `python/bridge/tests/test_analyze_psd_cli.py` — palette ignores transparent
  pixels, light-direction grid (corner / uniform / transparent-cell
  suppression), warm vs cool colour temperature, report shape + histogram
  artifact, safe-area inset, transparent region does not darken the mean,
  oversized-canvas guard (+ `0` disables), missing template, default
  `max_decode_pixels` (run: `pytest python/bridge/tests`).
- `src-tauri/src/studio/psd_analyze.rs` — the missing-template guard, the
  `reference_layers` line parsing, and the optional-param helper.
