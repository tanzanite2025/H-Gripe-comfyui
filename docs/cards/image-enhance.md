# Image Enhance / Super Resolution card

Executor: **local** (always `python/bridge/image_enhance_cli.py`, never networks).
Backend: `enhance_image` Tauri command → `image_enhance_cli.py` (Pillow + numpy CPU default; opt-in model engines via `python/bridge/sr_backends/`).

Upscales and restores a low-resolution subject so it fills a PSD placeholder at
print DPI without going soft. This document is the card's contract: what it
accepts, what it guarantees, and how it behaves at the edges. Heavier GPU
super-resolution backends (SupIR / CCSR; **Real-ESRGAN landed as opt-in**) slot
in behind the `engine` param without changing this contract — see
[Engines](#engines).

## Inputs (ports)

| Port | Type | Required | Notes |
| --- | --- | --- | --- |
| `image` | image path | yes | The subject to enhance. |
| `target_bounds` | `{x, y, width, height}` | no | Connected PSD placeholder rect; used to derive the target size when no explicit target is set. |

## Parameters

| Param | Type | Default | Range / values | Notes |
| --- | --- | --- | --- | --- |
| `mode` | enum | `conservative` | `conservative` \| `texture_rebuild` \| `print_ready` \| `custom` | Presets set denoise/texture; `custom` uses the sliders below. |
| `engine` | enum | `cpu` | `cpu` \| `realesrgan` | Upscale backend. `cpu` is the built-in Lanczos+sharpen (always available); `realesrgan` is opt-in and **falls back to `cpu`** when its deps/weight are missing. See [Engines](#engines). |
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

### `engine = cpu` in-process fast path (Rust)

The default `cpu` engine no longer always shells out: `studio/image_enhance_cpu.rs`
reproduces the CLI's `cpu` pipeline **in-process** (Lanczos3 / box resample,
unsharp, edge-preserving median denoise, independent alpha track) so a run of
common inputs skips the Python subprocess entirely. It is behaviour-preserving
by construction — anything it cannot reproduce faithfully returns `Ok(None)` and
falls straight through to `image_enhance_cli.py`, so no input regresses.

| Source colour | In-process (Rust) | Notes |
| --- | --- | --- |
| 8-bit `RGB` / `RGBA` / `L` / `LA` | ✅ | Embedded ICC re-embedded on output (iCCP) + DPI (pHYs), matching the CLI `save`. |
| 16-bit `Rgb16` / `Rgba16` / `La16` | ✅ | High byte kept (PIL / `into_rgba8` parity). |
| single-channel 16-bit (`I;16`, `L16`) | ✅ | Range-scaled by the source's own peak to 8-bit (numpy parity), not a naive `>>8`. |
| `CMYK` (TIFF) | ✅ | Raw ink samples + embedded ICC read via `cmyk_decode` (bypassing the `image` crate, which drops them at decode), then colour-managed to sRGB via `cmyk_transform` (the profile's A2B LUT through `moxcms`, else PIL's naive formula). Output is sRGB, so the source CMYK profile is dropped (`icc_preserved: false`), matching Python. See below. |
| `CMYK` (Adobe JPEG) | ✅ | APP14 transform-0 JPEGs store *inverted* ink (0 = full ink); `cmyk_decode` undoes it (`255 - v`) so the samples match TIFF Separated, then the same `cmyk_transform` path applies. |
| `CMYK` (YCCK / unmarked JPEG) | ⛔ defers to Python | `zune-jpeg`'s YCCK→RGB drops the embedded ICC, and an unmarked CMYK JPEG is too rare to validate a round-trip for, so both stay on the colour-managed Python path. |
| `Rgb32F` / `Rgba32F` (float) | ⛔ defers to Python | |

Landed: [#172](https://github.com/tanzanite2025/H-Gripe-Studio/pull/172)
(8-bit fast path), [#174](https://github.com/tanzanite2025/H-Gripe-Studio/pull/174)
(16-bit range-scale + ICC/DPI preserve),
[#176](https://github.com/tanzanite2025/H-Gripe-Studio/pull/176) /
[#177](https://github.com/tanzanite2025/H-Gripe-Studio/pull/177) /
[#178](https://github.com/tanzanite2025/H-Gripe-Studio/pull/178) (CMYK TIFF c1–c3).

### CMYK → sRGB in-process (landed: TIFF + Adobe JPEG)

CMYK samples and the embedded profile are read straight from the container
(bypassing the `image` crate, which converts CMYK→RGB and drops the profile at
decode) and colour-managed to sRGB before the normal pipeline. Shipped as small,
independently reviewable, CI-verifiable steps:

- **c1 — raw CMYK decoder ([#176](https://github.com/tanzanite2025/H-Gripe-Studio/pull/176)).**
  `studio/cmyk_decode.rs` returns the raw 4-channel CMYK samples + optional ICC
  from JPEG (`zune-jpeg`, output colourspace pinned to CMYK) and TIFF (`tiff`,
  `ColorType::CMYK(8)`) sources, reusing the shared decompression-bomb budget.
- **c2 — `moxcms` CMYK→sRGB transform ([#177](https://github.com/tanzanite2025/H-Gripe-Studio/pull/177)).**
  `cmyk_transform::cmyk_to_rgb8` runs the embedded profile's A2B LUT into sRGB
  (perceptual intent, mirroring the CLI's `ImageCms.profileToProfile`), and
  falls back to PIL's *naive* formula (`out = (255-K) - muldiv255(255-K, ink)`,
  byte-exact) when there is no usable profile.
- **c3 — wired behind the gate ([#178](https://github.com/tanzanite2025/H-Gripe-Studio/pull/178)).**
  `try_enhance` routes **TIFF** CMYK through `cmyk_decode` + `cmyk_to_rgb8` →
  the normal pipeline → sRGB PNG (source profile dropped, `icc_preserved: false`,
  matching Python). CMYK **JPEGs** and any decode/transform miss return
  `Ok(None)` → Python.
- **c3b — Adobe CMYK JPEG in-process.**
  `cmyk_decode` now also takes **Adobe** CMYK JPEGs (an APP14 marker with
  transform 0): Adobe stores inverted ink (0 = full ink) that PIL/libjpeg
  normalise on load, so we apply `255 - v` after `zune-jpeg` decode to land in
  the device direction (0 = no ink) that TIFF Separated and `cmyk_transform`
  expect. **YCCK** JPEGs (transform 2; `zune` reports a non-CMYK input
  colourspace, and its YCCK→RGB drops the embedded ICC) and CMYK JPEGs **without**
  an Adobe marker still return `Ok(None)` → Python. A committed PIL-generated
  fixture (`tests/fixtures/cmyk_adobe_app14.jpg`, regenerable via
  `scripts/gen_cmyk_jpeg_fixture.py`) is decoded + transformed in Rust and
  compared to Pillow's RGB within tolerance, so an inversion-direction error
  fails CI immediately.
- **c4 — colour-accuracy regression + docs (this section).** The naive CMYK→sRGB
  table is asserted **byte-for-byte on both sides** — Rust
  (`cmyk_transform` test `naive_matches_pil_convert_rgb`) and Python
  (`test_cmyk_naive_transform_matches_rust_reference`, running live Pillow) — so
  the CMYK fast path is a zero-ΔE cross-language regression: a shift in
  either engine breaks CI. The ICC (profiled) path is checked against a
  littleCMS reference locally (moxcms is not byte-identical to littleCMS; small
  ΔE), skipped on runners without a system CMYK profile.

Because there is no local Rust toolchain, each step leans on CI + the fixture
assertions rather than manual inspection.

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

## Engines

The `engine` param is the **local-card backend seam** from
`docs/card-executor-split-and-psd-chain-hardening.md` (§2.5 / §3.4): adding an
engine extends the registry + the CLI only, with no dispatch changes.

| Engine | Deps | Weight | Behaviour |
| --- | --- | --- | --- |
| `cpu` (default) | vendored Pillow + numpy | none | Lanczos resample + unsharp mask + edge-preserving median denoise. Always available; the fallback for every other engine. |
| `realesrgan` | `torch` + `realesrgan` (optional, **not** bundled) | `RealESRGAN_x4plus.pth` in the model cache | Real-ESRGAN x4 in one pass (tiled), then Lanczos to the exact requested factor. CUDA when present, else CPU. |

Rules (`python/bridge/sr_backends/`):

- **Opt-in & CPU-safe.** A non-`cpu` engine is used only when explicitly
  requested *and* its deps + weight are present. On any miss (no deps, no
  weight, a downscale target, an unknown name, or a runtime error) the node
  **falls back to the `cpu` path** and records `engine_fallback_reason` — it
  never hard-fails.
- **Weights are not bundled.** `realesrgan` resolves its weight from
  `HGRIPE_REALESRGAN_MODEL` (explicit path) or `<model cache>/RealESRGAN_x4plus.pth`,
  where the cache dir is `HGRIPE_MODEL_CACHE` or the bundled `resources/models`
  dir (same convention as the SAM 2 / ViTMatte weights).
- **Model replaces the CPU steps.** When a model engine runs it performs
  restoration + upscaling itself, so the CPU denoise/unsharp passes are skipped
  (`denoise_method` is the engine id, `texture_strength` reported as `0.0`).
- **Capability probe.** `image_enhance_cli.py --probe-engines` prints which
  engines are usable right now (`{engines: {<id>: {available, reason, …}}, model_cache_dir}`)
  so the UI can grey out unavailable engines.

## `enhance_report` fields

`mode`, `scale_factor`, `source_mode`, `output_mode`, `had_alpha`,
`source_size`, `output_size`, `target_size`, `target_dpi`, `max_pixels`,
`max_decode_pixels`, `clamped`, `downscaled`, `exif_transposed`,
`icc_preserved`, `denoise_method`, `denoise_strength`, `texture_strength`,
`preserve_text_logo`, `engine`, `engine_requested`, `engine_fallback_reason`,
`backend_model`, `processing_time_ms`.

## Tests

- `python/bridge/tests/test_image_enhance_cli.py` — alpha isolation, CMYK /
  high-bit handling, downscale path, decode guard, target resolution, clamp,
  logo guard, output naming, ICC preservation, and the **engine seam** (default
  `cpu`, unknown-engine fallback, `realesrgan` unavailable fallback, downscale
  skip, a fake-backend dispatch + telemetry, `--probe-engines`) — run:
  `pytest python/bridge/tests`.
- `python/bridge/tests/test_sr_backends.py` — registry `resolve`, capability
  `probe`, weight-path resolution, and the Real-ESRGAN unavailable/raise paths.
- `src-tauri/src/studio/image_enhance.rs` — the connected-image-input guard.

## Verifying `realesrgan` end-to-end

Real inference needs `torch` + `realesrgan` + the weight, which CI does not
install, so it is verified manually (mirroring the ViTMatte e2e):

```
pip install torch realesrgan
export HGRIPE_MODEL_CACHE=/path/to/models   # or HGRIPE_REALESRGAN_MODEL=/path/to/RealESRGAN_x4plus.pth
python python/bridge/image_enhance_cli.py --probe-engines        # realesrgan -> available: true
python python/bridge/image_enhance_cli.py --image in.png --engine realesrgan \
  --target-width 1024 --output-dir out                          # enhance_report.engine == "realesrgan"
```
