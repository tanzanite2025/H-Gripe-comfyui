# Detail Repaint card

Executor: **API** (orchestrates a provider `image.edit` between two local pixel
steps). Pixel backend: `prepare_repaint_regions` / `composite_repaint` Tauri
commands → `detail_repaint_cli.py` (Pillow + numpy, CPU-only).

The Phase-2 follow-up to the detect-only **Detail Watchdog**: Watchdog reports
*where* an image breaks down (a `QualityReport`); Detail Repaint takes those
issue regions and actually fixes them. Because the provider call (`image.edit`
through the H-Gripe broker) is owned by the Rust/TS orchestration layer, the
pixel work is a **two-stage, stateless contract** the orchestrator drives around
the broker call:

1. **prepare** — for each repaintable issue region, crop a padded window out of
   the candidate and write a same-size inpaint `mask` marking the (un-padded)
   issue core as the edit area. Emits a manifest of regions (crop + mask paths,
   geometry).
2. *(orchestrator)* — send each `crop` + `mask` + repaint prompt to the
   provider's `image.edit`; collect the repainted crops.
3. **composite** — paste each repainted crop back inside a *feathered* version
   of its issue core (a secondary edge fusion at the seam), leaving the padding
   context untouched, and write the fixed image. Emits a `repaint_report`.

When no `image.edit`-capable provider is configured (empty or `mock`), the
provider loop is skipped and the node passes the image through unchanged
(`repaint_report.status == "unchanged"`).

## Inputs (ports)

| Port | Type | Required | Notes |
| --- | --- | --- | --- |
| `image` | image path | yes | The candidate to repaint. |
| `quality_report` | JSON | no | The Detail Watchdog `QualityReport`; its `issues` drive region selection. |

## Parameters

| Param | Type | Default | Range / values | Notes |
| --- | --- | --- | --- | --- |
| `provider` / `operation` / `credentials_ref` | str | — / `image.edit` / — | | Provider routing; empty or `mock` passes through unchanged. |
| `repaint_prompt_base` | str | auto | | Prepended to the per-issue repaint prompt. |
| `repaint_actions` | csv | `detail_redraw` | `suggested_action` values | Which issue actions are repaintable locally (global `image_enhance` / `color_match` are other nodes). |
| `min_confidence` | float | `0.0` | `0..1` | Skip issues below this confidence. |
| `region_padding` | int | `24` | `>= 0` | Context padding (px) around each issue bbox in the crop. |
| `max_regions` | int | `8` | `>= 1` | Cap on repainted regions (highest confidence first). |
| `feather_px` | float | `0.0` | `>= 0` (0 = auto) | Seam feather radius; auto ≈ 6% of the issue's short side (2..24). |
| `max_decode_pixels` | int | `96_000_000` | `>= 0` (0 disables) | Rejects an **input** larger than this before decoding (decompression-bomb guard). |
| `output_dir` | path | run output dir | | Validated server-side. |
| `output_name` | basename | `<image>_repaint(ed)` | plain basename | Rejected if it contains `..` or a path separator. |

## Inpaint-mask polarity

The crop-sized mask marks the edit area. By default (OpenAI `image.edit`
convention) the issue core is punched **transparent** (alpha 0 = regenerate) and
the padding kept opaque; `--invert-mask` flips this for providers that treat
opaque/white as the edit area. The chosen polarity is reported as
`mask_edit_is_transparent`.

## Alpha isolation (Method A)

The composite blends **only the RGB channels** of the repainted patch into the
candidate; the candidate's **original alpha is preserved**. A cut-out subject
therefore keeps its exact matte and never gains a soft seam halo from a provider
crop whose own alpha differs. The feathered weight applies to RGB only.

## Patch resampling

A provider crop is reused as-is when it matches the crop size. When it differs:
shrinking uses a **box (area-average) filter** (avoids the ringing/aliasing
Lanczos introduces on downsample); growing uses Lanczos.

## Colour space & bit depth

The candidate decode is normalised to an 8-bit RGBA working space so crops and
the paste-back carry honest colour; the source's original mode is recorded as
`source_mode`:

| Source mode | Handling |
| --- | --- |
| `RGB` / `RGBA` / `L` / `LA` | Used directly; alpha (when present) is preserved. |
| `P` (palette) | Expanded to RGBA; transparency in `info` is treated as alpha. |
| `CMYK` | Converted to sRGB via the embedded ICC profile when present, else a naive convert. |
| `I` / `I;16*` / `F` (high bit) | Data range normalised down to 8-bit via numpy before RGB conversion. |

## Boundary behaviour

| Condition | Behaviour |
| --- | --- |
| Missing / blank `image` input | Rust handler errors `Detail Repaint needs a connected image input` before shelling out. |
| Missing image file on disk | `FileNotFoundError: candidate image not found: <path>`. |
| Input larger than `max_decode_pixels` | `ValueError: input image too large to decode safely: WxH ...` (before decode). |
| Invalid `quality_report` / `manifest` / `repainted` JSON | `ValueError: invalid <label> JSON: ...`. |
| No repaintable issues | `prepare.selected_count == 0`; composite is `unchanged`. |
| Issue not repaintable / below confidence / over cap / no bbox | Recorded in `prepare.skipped` with a `reason`. |
| Provider returned nothing for a region | That region's composite status is `no_repaint`; others still composite (`partial`). |
| EXIF-rotated input | Orientation normalised via the orientation tag; `exif_transposed: true`. |
| Unsafe `output_name` (`..`, separators) | Rejected server-side. |

## Report fields

`prepare` returns: `regions` (each `index`, `type`, `confidence`,
`suggested_action`, `bbox`, `crop_box`, `inner_box`, `size`, `crop_path`,
`mask_path`), `skipped`, `image_size`, `selected_count`,
`mask_edit_is_transparent`, `source_mode`, `exif_transposed`,
`max_decode_pixels`.

`composite` returns `fixed_image` and `repaint_report`: `status`
(`repainted` \| `partial` \| `unchanged`), `regions` (each `index`, `type`,
`bbox`, `status`, optional `feather_px`), `repainted_count`, `requested_count`,
`image_size`, `source_mode`, `exif_transposed`, `max_decode_pixels`.

## Outputs (ports)

| Port | Type | Notes |
| --- | --- | --- |
| `fixed_image` | image path | The candidate with the selected issue cores repainted and edge-fused. |
| `repaint_report` | JSON | The report above. |

## Tests

- `python/bridge/tests/test_detail_repaint_cli.py` — issue selection / action /
  confidence / region cap, mask polarity (+ invert), padding, the feathered
  paste-back, **alpha isolation** (RGB-only blend, original alpha preserved),
  **box-filter downsampling**, no-repaint passthrough, decode guard, CMYK source
  mode, EXIF reporting, invalid JSON, missing image (run:
  `pytest python/bridge/tests`).
- `src-tauri/src/studio/exec.rs` — `PrepareRepaintResult` / `RepaintReport`
  deserialization of the v1 hardening fields (plus legacy JSON defaults).
