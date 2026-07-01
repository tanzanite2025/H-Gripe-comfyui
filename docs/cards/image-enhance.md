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
| `CMYK` | ⛔ defers to Python | The `image` crate discards the raw CMYK samples + ICC at decode, so a faithful transform is impossible on this path today. See below. |
| `Rgb32F` / `Rgba32F` (float) | ⛔ defers to Python | |

Landed: [#172](https://github.com/tanzanite2025/H-Gripe-Studio/pull/172)
(8-bit fast path), [#174](https://github.com/tanzanite2025/H-Gripe-Studio/pull/174)
(16-bit range-scale + ICC/DPI preserve).

### Planned: CMYK → sRGB in-process (phased)

To pull CMYK onto the Rust fast path we must bypass the `image` crate (which
converts CMYK→RGB and drops the profile at decode) and read the **raw CMYK
samples + embedded ICC** ourselves, then run a real CMS transform. Sequenced as
small, independently shippable, CI-verifiable steps:

- **c1 — raw CMYK decoder (dead code + tests, still defers).** Add
  `studio/cmyk_decode.rs` returning `Option<(w, h, Vec<u8> /*4ch CMYK*/, Option<Vec<u8>> /*icc*/)>`
  from JPEG (`zune-jpeg`) and TIFF (`tiff`) CMYK sources. Not wired into the
  enhance path yet; unit-tested against CMYK JPEG/TIFF fixtures. CMYK keeps
  deferring to Python. Zero runtime risk.
- **c2 — `moxcms` CMYK→sRGB transform (isolated, fixture-tested).** Add the
  `moxcms` dep; build `cmyk_to_rgb8(cmyk, icc)` that transforms via the embedded
  profile to sRGB, and replicates PIL's *naive* CMYK→RGB formula
  (`r=(255-c)*(255-k)/255`, …) when **no** profile is present (to match the CLI's
  non-ICC branch). Validated against ImageCms/PIL reference patches within a ΔE
  tolerance. Still not wired.
- **c3 — wire behind the gate, Python fallback on any miss.** In `try_enhance`,
  route CMYK through `cmyk_decode` + `cmyk_to_rgb8` → the normal pipeline →
  sRGB PNG (old CMYK profile dropped, `icc_preserved: false`, matching Python).
  On **any** failure (unsupported container, profile/transform error) return
  `Ok(None)` → Python. `can_handle_in_process` stops deferring CMYK only for the
  containers the decoder covers.
- **c4 — colour-accuracy regression + docs.** Small CMYK fixture set asserting
  Rust vs Python output agree within ΔE; flip the CMYK row above and this
  section to "landed".

Risk lives entirely in c2/c3 (colour fidelity); c1 is inert. Because there is no
local Rust toolchain, each step leans on CI + the fixture assertions rather than
manual inspection.

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
