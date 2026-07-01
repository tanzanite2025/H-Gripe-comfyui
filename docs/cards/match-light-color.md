# Match Light & Color card

Executor: **local** (always `python/bridge/color_match_cli.py`, never networks).
Backend: `match_light_color` Tauri command → `color_match_cli.py` (Pillow + numpy, CPU-only in Phase 1).

Nudges a generated/cut-out subject toward a PSD background's light & colour so a
composite stops looking pasted-on. This document is the card's contract: what it
accepts, what it guarantees, and how it behaves at the edges. The matching is a
heuristic Reinhard / histogram transfer in Lab; a learned relight backend is a
future `profile_ref` mode behind this same contract.

## Inputs (ports)

| Port | Type | Required | Notes |
| --- | --- | --- | --- |
| `image` | image path | yes | The subject to correct. Its alpha defines the corrected region. |
| `background` | image path | no | The reference whose light/colour the subject is matched to. Without it the pixels pass through unchanged (`prompt_only`-like). |
| `mask` | image path | no | Narrows the corrected region further (multiplied into the subject alpha). |
| `visual_context` | JSON | no | Upstream PSD context; its `prompt_suffix` / `lighting` drives the emitted prompt suffix. |

## Parameters

| Param | Type | Default | Range / values | Notes |
| --- | --- | --- | --- | --- |
| `mode` | enum | `color_transfer` | `prompt_only` \| `color_transfer` \| `histogram_match` \| `hybrid` | `prompt_only` never touches pixels; `hybrid` runs transfer then a gentler histogram pass. |
| `strength` | float | `0.6` | `0..1` | Overall correction weight (base per-pixel blend). |
| `shadow_strength` | float | `0.0` | `0..1` | Extra correction weight in shadows (low L). |
| `highlight_strength` | float | `0.0` | `0..1` | Extra correction weight in highlights (high L). |
| `protect_saturation` | bool | `false` | | Match **luminance only**; the subject keeps its own a/b chroma. |
| `protect_brand_color` | bool | `true` | | Damps the shift on high-chroma (brand) pixels so a logo colour is not pulled toward the background. |
| `max_decode_pixels` | int | `96_000_000` | `>= 0` (0 disables) | Rejects an **input** (subject or background) larger than this before decoding (decompression-bomb guard). |
| `output_dir` | path | run output dir | | Validated server-side. |
| `output_name` | basename | `<image>_matched` | plain basename | Rejected if it contains `..` or a path separator (`reject_unsafe_output_name`). |

## Corrected region

The correction is applied inside the **subject alpha**, optionally narrowed by a
connected `mask`. If that leaves no coverage (fully transparent or empty mask),
the whole frame is used as a fallback. Transparent subject pixels keep their
original RGB (their correction weight is zero); the alpha channel is recombined
unchanged.

## Background statistics

Only **opaque** background pixels describe the target light/colour. The
background's own alpha is used as a weight, so a cut-out background plate's
transparent regions do not skew the target mean/std (in either the Reinhard
transfer or the histogram reference). If the background is fully transparent the
whole frame is used as a fallback.

## Modes

- **`prompt_only`** — writes a copy of the subject unchanged and emits only the
  prompt suffix. `applied: false`.
- **`color_transfer`** — Reinhard mean/std transfer in Lab toward the background
  stats. The per-channel std ratio is clamped to `0.5..2.0` so a near-flat
  subject channel cannot blow up.
- **`histogram_match`** — per-channel CDF match of the subject onto the
  (opaque) background.
- **`hybrid`** — transfer first, then a gentler (0.5×) histogram pass so the
  transfer stays dominant.

`protect_saturation` restricts every mode to the L channel. `protect_brand_color`
multiplies the correction weight by `1 - clamp(chroma/110, 0, 1)`, sparing
saturated pixels.

## Colour space & bit depth

> Working space / bit depth / ICC handling is defined once in
> [`docs/design/colour-pipeline.md`](../design/colour-pipeline.md) (the source
> of truth). Below is the **current** 8-bit sRGB behaviour; the decided target
> (16-bit wide-gamut canonical + sRGB model egress) is not yet implemented.

Both inputs are *currently* normalised to an 8-bit RGB working space (the
matching Lab conversion uses Pillow's `LAB`); the subject's original mode is
recorded as `source_mode`, the background's as `background_mode`:

| Source mode | Handling |
| --- | --- |
| `RGB` / `RGBA` / `L` / `LA` | Used directly; alpha (when present) defines the region. |
| `P` (palette) | Expanded to RGB(A); transparency in `info` is treated as alpha. |
| `CMYK` | Converted to sRGB via the embedded ICC profile when present, else a naive convert. |
| `I` / `I;16*` / `F` (high bit) | Data range normalised down to 8-bit via numpy before RGB conversion. |

## Boundary behaviour

| Condition | Behaviour |
| --- | --- |
| Missing / blank `image` input | Rust handler errors `Light & Color Match needs a connected image input` before shelling out. |
| Missing file on disk | `FileNotFoundError: subject image not found: <path>` (or `background image not found`). |
| No `background` connected (pixel mode) | Subject passed through unchanged; `applied: false`, `note` records why. |
| `strength = 0` | Pass-through; `applied: false`. |
| Unknown `mode` | `ValueError: unknown mode ...`. |
| Input larger than `max_decode_pixels` | `ValueError: input image too large to decode safely: WxH ...` (before decode). |
| Transparent background regions | Excluded from the target statistics (alpha-weighted). |
| Transparent subject regions | Left unchanged (zero correction weight). |
| Invalid `visual_context` JSON | Ignored; the suffix is synthesised from the background. |
| EXIF-rotated input | Orientation normalised; `exif_transposed: true`. |
| Unsafe `output_name` (`..`, separators) | Rejected server-side. |

## `match_report` fields

`mode`, `strength`, `shadow_strength`, `highlight_strength`,
`protect_saturation`, `protect_brand_color`, `source_mode`, `background_mode`,
`exif_transposed`, `max_decode_pixels`, `applied`, `before`, `after`,
`output_size`, optional `note`, and (when a transfer runs) `src_mean_lab`,
`dst_mean_lab`, `src_std_lab`, `dst_std_lab`. `before` / `after` each carry
`mean_color`, `color_temperature`, `contrast`.

## Outputs (ports)

| Port | Type | Notes |
| --- | --- | --- |
| `matched_image` | image path | The corrected RGBA PNG (alpha unchanged). |
| `match_report` | JSON | The report above. |
| `prompt_suffix` | string | Lighting hint reused from `visual_context` or synthesised from the background colour temperature. |

## Tests

- `python/bridge/tests/test_color_match_cli.py` — transfer moves the mean toward
  the background, hybrid stats, `protect_saturation` keeps chroma, background-
  alpha weighting, subject transparent region untouched, decode guard, CMYK /
  high-bit source modes, invalid context, output naming (run:
  `pytest python/bridge/tests`).
- `src-tauri/src/studio/color_match.rs` — the connected-image-input guard and
  param defaults.
