# Refine Mask Edge card

Executor: **local** (always `python/bridge/edge_refine_cli.py`, never networks).
Backend: `refine_mask_edge` Tauri command → `edge_refine_cli.py` (Pillow + numpy, CPU-only in Phase 1).

Cleans up a cut-out subject's matte for PSD compositing: bites the fringe in,
snaps the matte to the subject's own luminance edges, feathers the transition,
and optionally decontaminates / re-colours the edge band so the seam matches the
target background. This document is the card's contract: what it accepts, what
it guarantees, and how it behaves at the edges. The pipeline is a heuristic
morphology + numpy guided-filter (He et al.) + Gaussian feather; an OpenCV
`guidedFilter` / learned matting backend is a future `profile_ref` mode behind
this same contract.

## Inputs (ports)

| Port | Type | Required | Notes |
| --- | --- | --- | --- |
| `image` | image path | yes | The cut-out subject. Its alpha is the matte unless `mask` is connected. |
| `mask` | image path | no | An explicit matte; takes precedence over the subject alpha. |
| `background` | image path | no | Target background; its colour is blended into the edge band when `background_blend_strength > 0`. |
| `placeholder_mask` | image path | no | PSD placeholder mask (advisory in Phase 1; not consumed by the pipeline). |
| `trimap` | image path | no | Matting trimap (FG=255 / unknown=128 / BG=0) from `Subject Mask`. Its *unknown* band is protected from the erode/feather clean-up so hair / fur / glass continuous alpha survives instead of being treated as binary fringe. |

## Parameters

| Param | Type | Default | Range / values | Notes |
| --- | --- | --- | --- | --- |
| `preset` | enum | `natural` | `clean` \| `natural` \| `soft` \| `custom` | A named preset overrides the sliders below; `custom` uses them verbatim. |
| `erode_px` | int | `1` | `>= 0` | Bite the fringe N px inward (Min filter). `custom` only. |
| `dilate_px` | int | `0` | `>= 0` | Grow the matte N px outward (Max filter). `custom` only. |
| `feather_px` | float | `4.0` | `>= 0` | Gaussian feather radius of the transition. `custom` only. |
| `guided_radius` | int | `8` | `>= 0` (0 disables) | Guided-filter radius that snaps the matte to luminance edges. `custom` only. |
| `edge_decontaminate` | bool | `true` | | Pull opaque subject colour into the band to kill white/coloured fringing. `custom` only. |
| `background_blend_strength` | float | `0.4` | `0..1` | Blend the band toward `background`'s colour (only when a background is connected). `custom` only. |
| `max_decode_pixels` | int | `96_000_000` | `>= 0` (0 disables) | Rejects an **input** (image, mask or background) larger than this before decoding (decompression-bomb guard). |
| `output_dir` | path | run output dir | | Validated server-side. |
| `output_name` | basename | `<image>_refined` | plain basename | Rejected if it contains `..` or a path separator (`reject_unsafe_output_name`). |

## Presets

| Preset | erode | dilate | feather | guided | decontaminate | blend |
| --- | --- | --- | --- | --- | --- | --- |
| `clean` | 1 | 0 | 2 | 4 | yes | 0.5 |
| `natural` | 1 | 0 | 6 | 8 | yes | 0.4 |
| `soft` | 0 | 0 | 12 | 12 | no | 0.3 |

## Matte source

An explicit `mask` always wins; otherwise the subject's own alpha is the matte.
The chosen source is recorded as `source_mask` (`explicit` / `alpha`). The mask
is resampled to the image size with **bilinear** interpolation (not the default
bicubic), so a matte cannot overshoot past `0..1` and ring at the very edge being
refined; the connected `background` is resampled the same way.

## Pipeline

1. **Morphology** — `erode_px` Min filter then `dilate_px` Max filter.
2. **Guided filter** — when `guided_radius > 0`, snaps the matte to the
   subject's luminance edges (numpy box-filter guided filter, `eps = 1e-3`).
3. **Feather** — Gaussian blur of radius `feather_px` for a stair-free
   transition.
4. **Edge band** — `min(α, 1-α) * 2`: pixels that are neither solidly in nor
   out. Decontamination and background blend act only here.
5. **Decontaminate** — when enabled and there is an opaque core (`α > 0.9`),
   pull that core's colour into the band to remove fringing.
6. **Background blend** — when a background is connected, blend the band toward
   the target colour with weight `band * background_blend_strength`.

When a `trimap` is connected, an extra **unknown-band protection** step runs
right after the feather (before the edge band is measured): inside the trimap's
unknown level (mid-grey, `0.25 < t < 0.75`, loaded nearest-neighbour so the
three levels survive resize) the refined matte is replaced with the upstream
soft alpha, blended via a lightly-feathered weight so the protected region joins
the cleaned-up definite areas without a step. This keeps genuine continuous
alpha (hair / fur / glass) intact rather than eroding/feathering it as fringe.

## Colour space & bit depth

> Working space / bit depth / ICC handling is defined once in
> [`docs/design/colour-pipeline.md`](../design/colour-pipeline.md) (the source
> of truth). Below is the **current** 8-bit sRGB behaviour; the decided target
> (16-bit wide-gamut canonical + sRGB model egress) is not yet implemented.

Inputs are *currently* normalised to an 8-bit RGB working space so
decontamination and background blend sample honest colour; the subject's
original mode is recorded as `source_mode`:

| Source mode | Handling |
| --- | --- |
| `RGB` / `RGBA` / `L` / `LA` | Used directly; alpha (when present) is the matte. |
| `P` (palette) | Expanded to RGB(A); transparency in `info` is treated as alpha. |
| `CMYK` | Converted to sRGB via the embedded ICC profile when present, else a naive convert. |
| `I` / `I;16*` / `F` (high bit) | Data range normalised down to 8-bit via numpy before RGB conversion. |

## Boundary behaviour

| Condition | Behaviour |
| --- | --- |
| Missing / blank `image` input | Rust handler errors `Mask Edge Refine needs a connected image input` before shelling out. |
| Missing image file on disk | `FileNotFoundError: subject image not found: <path>`. |
| Missing background file on disk | `FileNotFoundError: background image not found: <path>`. |
| No transitional edge (fully opaque / empty matte) | Refinement is a no-op; `edge_band_px: 0` and a `note` records why. |
| Input larger than `max_decode_pixels` | `ValueError: input image too large to decode safely: WxH ...` (before decode). |
| Unknown `preset` | `ValueError: unknown preset ...; expected one of [...]`. |
| EXIF-rotated input | Orientation normalised; `exif_transposed: true`. |
| Unsafe `output_name` (`..`, separators) | Rejected server-side. |

## `edge_report` fields

`preset`, `source_mask`, `source_mode`, `exif_transposed`, `max_decode_pixels`,
`erode_px`, `dilate_px`, `feather_px`, `guided_radius`, `edge_decontaminate`,
`background_blend_strength`, `background_applied`, `trimap_applied`,
`protected_band_px`, `edge_band_px`,
`coverage_before`, `coverage_after`, `output_size`, and an optional `note` when
there was no transitional edge to refine.

## Outputs (ports)

| Port | Type | Notes |
| --- | --- | --- |
| `refined_image` | image path | The refined RGBA PNG (decontaminated / blended colour + refined alpha). |
| `refined_mask` | image path | The refined matte as an 8-bit L PNG. |
| `edge_report` | JSON | The report above. |

## Tests

- `python/bridge/tests/test_edge_refine_cli.py` — erosion reduces coverage,
  feather widens the band, guided filter snaps to the luminance edge,
  decontamination pulls subject colour into the band, background blend, explicit
  mask precedence, the no-edge note, preset parsing, decode guard, CMYK source
  mode, invalid preset, missing image / background, output naming, and trimap
  unknown-band protection (the protected matte tracks the original soft alpha
  where erosion would otherwise bite it away) (run: `pytest python/bridge/tests`).
- `src-tauri/src/studio/edge_refine.rs` — the connected-image-input guard and
  param defaults matching the Python bridge.
