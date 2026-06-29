# Image Enhance / Super Resolution card

Executor: **local** (always `python/bridge/image_enhance_cli.py`, never networks).
Backend: `enhance_image` Tauri command → `image_enhance_cli.py` (Pillow + numpy, CPU-only in Phase 1).

Upscales and restores a low-resolution subject so it fills a PSD placeholder at
print DPI without going soft. This document is the card's contract: what it
accepts, what it guarantees, and how it behaves at the edges. The deep GPU
super-resolution backends (SupIR / CCSR / RealESRGAN) are a future `profile_ref`
mode behind this same contract.

## Inputs (ports)

| Port | Type | Required | Notes |
| --- | --- | --- | --- |
| `image` | image path | yes | The subject to enhance. |
| `target_bounds` | `{x, y, width, height}` | no | Connected PSD placeholder rect; used to derive the target size when no explicit target is set. |

## Parameters

| Param | Type | Default | Range / values | Notes |
| --- | --- | --- | --- | --- |
| `mode` | enum | `conservative` | `conservative` \| `texture_rebuild` \| `print_ready` \| `custom` | Presets set denoise/texture; `custom` uses the sliders below. |
| `target_width` | int px | `0` | `>= 0` (0 = auto) | Explicit target wins over `target_bounds`. |
| `target_height` | int px | `0` | `>= 0` (0 = auto) | |
| `target_dpi` | int | `300` | `>= 1` | Written into the output PNG metadata only. |
| `scale` | float | `2.0` | `> 0` | Fallback factor when no target size is resolved (`custom`). |
| `denoise_strength` | float | `0.3` | `0..1` | Edge-preserving median blend (`custom`). |
| `texture_strength` | float | `0.25` | `0..1` | Unsharp-mask detail (`custom`). |
| `preserve_text_logo` | bool | `true` | | Caps `texture_strength` at `0.4` so logos/packaging text are not mangled. |
| `max_pixels` | int | `48_000_000` | `>= 0` (0 disables) | Caps **output** pixels; the scale is reduced to fit and `clamped` is reported. |
| `max_decode_pixels` | int | `96_000_000` | `>= 0` (0 disables) | Rejects an **input** larger than this before decoding (decompression-bomb guard). |
| `output_dir` | path | run output dir | | Validated server-side. |
| `output_name` | basename | `<image>_enhanced` | plain basename | Rejected if it contains `..` or a path separator (`reject_unsafe_output_name`). |

### Presets

| Preset | scale | denoise | texture |
| --- | --- | --- | --- |
| `conservative` | 2.0 | 0.30 | 0.25 |
| `texture_rebuild` | 2.0 | 0.15 | 0.70 |
| `print_ready` | 2.0 | 0.20 | 0.50 |

## Target-size resolution

1. Explicit `target_width` / `target_height` (if either > 0).
2. Else `target_bounds.{width,height}` from a connected placeholder.
3. Else the preset/`custom` `scale`.

The factor is **uniform** (aspect ratio preserved) and **covers** the target so
both dimensions reach it; the final crop/fit into the placeholder is left to PSD
Export. If the output would exceed `max_pixels`, the scale is reduced to fit and
`clamped: true` is reported.

## Pipeline

Colour channels only: **denoise → resample → sharpen**. The alpha channel is
split off first, resized on its own track, and recombined afterwards, so
denoise/sharpen never bleed a halo across a matte edge.

- **Upscale** (`scale > 1`): Lanczos resample, then unsharp mask.
- **Downscale** (`scale < 1`): box filter, and the unsharp pass is **skipped**
  (`texture_strength` is reported as `0.0`) — sharpening a shrink only amplifies
  resampling artefacts.
- **Denoise**: an edge-preserving median filter blended in by `denoise_strength`
  (a Gaussian blur would smear the very edges we are about to sharpen).

## Colour space & bit depth

The input is normalised to an 8-bit RGB working space and the original
`source_mode` is recorded:

| Source mode | Handling |
| --- | --- |
| `RGB` / `RGBA` / `L` / `LA` | Used directly; an embedded ICC profile is preserved on output (`icc_preserved: true`). |
| `P` (palette) | Expanded to RGB(A); transparency in `info` is treated as alpha. |
| `CMYK` | Converted to sRGB via the embedded ICC profile when present, else a naive convert; profile not carried over (`icc_preserved: false`). |
| `I` / `I;16*` / `F` (high bit) | Data range normalised down to 8-bit via numpy before RGB conversion. |

## Boundary behaviour

| Condition | Behaviour |
| --- | --- |
| Missing / blank `image` input | Rust handler errors `Image Enhance needs a connected image input` before shelling out. |
| Missing file on disk | `FileNotFoundError: base image not found: <path>`. |
| Unknown `mode` | `ValueError: unknown mode ...`. |
| Input larger than `max_decode_pixels` | `ValueError: input image too large to decode safely: WxH ...` (before decode). |
| Cut-out subject (has alpha) | Alpha isolated; matte stays binary (no semi-transparent halo). |
| EXIF-rotated photo | Orientation normalised; `exif_transposed: true`. |
| Broken EXIF block | Ignored; enhancement proceeds. |
| Unsafe `output_name` (`..`, separators) | Rejected server-side. |

## `enhance_report` fields

`mode`, `scale_factor`, `source_mode`, `output_mode`, `had_alpha`,
`source_size`, `output_size`, `target_size`, `target_dpi`, `max_pixels`,
`max_decode_pixels`, `clamped`, `downscaled`, `exif_transposed`,
`icc_preserved`, `denoise_method`, `denoise_strength`, `texture_strength`,
`preserve_text_logo`, `processing_time_ms`.

## Tests

- `python/bridge/tests/test_image_enhance_cli.py` — alpha isolation, CMYK /
  high-bit handling, downscale path, decode guard, target resolution, clamp,
  logo guard, output naming, ICC preservation (run: `pytest python/bridge/tests`).
- `src-tauri/src/studio/image_enhance.rs` — the connected-image-input guard.
