"""Headless CPU image enhancement for the H-Gripe desktop node editor.

This is the thin, ``torch``-free entry point behind the desktop
``enhance_image`` Tauri command -- the backend of the **Image Enhance /
Super Resolution** node, which sits after Mask Edge Refine in the PSD-first
production chain. Cloud image APIs often hand back a low-resolution subject;
dropped into a large print PSD it goes soft, loses texture and is unusable at
300 DPI. This node resizes the subject up to the PSD placeholder's pixel target
and restores high-frequency detail.

The default ``cpu`` engine is dependency-light -- only the vendored ``Pillow`` +
``numpy`` (no GPU). The upscale is a Lanczos resample, detail is an unsharp
mask, and denoise is an *edge-preserving* median blend (a plain Gaussian blur
would smear the very edges we are about to sharpen).

GPU super-resolution backends slot in behind the ``--engine`` seam
(``python/bridge/sr_backends/``): ``--engine realesrgan`` runs Real-ESRGAN when
its optional deps (``torch`` + ``realesrgan``) and weight are present, otherwise
it **falls back to the CPU path** and records why in the report. ``cpu`` stays
the default and the always-available fallback, so the node never hard-fails on a
box without the model. ``--probe-engines`` prints which engines are usable so
the UI can grey out unavailable ones.

The enhancement is **alpha-aware and colour-space aware** so the node behaves on
real production assets, not just clean 8-bit RGB PNGs:

* The alpha channel of a cut-out subject is split off, resized on its own and
  recombined *after* enhancement, so denoise/sharpen never bleed a halo across
  the matte edge.
* CMYK, 16-bit (``I;16``), float (``F``), grayscale and palette (``P``) inputs
  are converted to an 8-bit RGB working space first (CMYK via its embedded ICC
  profile when present), and the resolved ``source_mode`` is recorded in the
  report. An ICC profile is preserved on the output when the working space did
  not change colour model.
* EXIF orientation is normalised, and an input larger than ``--max-decode-pixels``
  is rejected before it is decoded so a crafted/huge image cannot exhaust memory.

Presets keep the UI to "pick an enhance style + target size"; the per-step
strengths live behind an advanced toggle:

* ``conservative``    -- gentle denoise, light sharpen (safe default).
* ``texture_rebuild`` -- minimal denoise, strong sharpen (rebuild texture).
* ``print_ready``     -- balanced denoise + sharpen for 300 DPI print.
* ``custom``          -- use the explicit ``--denoise-strength`` /
  ``--texture-strength`` / ``--scale`` args.

The target size is resolved as: explicit ``--target-width`` / ``--target-height``
win, else the connected ``target_bounds`` (a PSD placeholder rectangle) via
``--target-bounds-json``, else a preset scale factor. The upscale is uniform
(aspect ratio preserved) and "covers" the target so both dimensions reach it;
the final fit into the placeholder is left to PSD Export. ``--max-pixels`` caps
the output so a huge placeholder cannot blow up memory -- the scale is reduced
to fit and ``clamped`` is flagged in the report. When the resolved scale is < 1
(the target is smaller than the source) the node down-samples with a box filter
and skips sharpening, which would only amplify the resampling artefacts.

The emitted JSON is ``{"enhanced_image", "scale_factor", "enhance_report"}``
where ``enhance_report`` records the resolved mode, sizes, scale factor and
strengths. On failure the process exits non-zero with a single message on
stderr.
"""

from __future__ import annotations

import argparse
import io
import json
import sys
import time
from pathlib import Path
from typing import Any

# Resolved (scale, denoise_strength, texture_strength) for each named preset.
# ``scale`` is only used when no explicit target size is resolved. ``custom``
# is handled by falling through to the explicit CLI args.
_PRESETS: dict[str, dict[str, float]] = {
    "conservative": {"scale": 2.0, "denoise_strength": 0.3, "texture_strength": 0.25},
    "texture_rebuild": {"scale": 2.0, "denoise_strength": 0.15, "texture_strength": 0.7},
    "print_ready": {"scale": 2.0, "denoise_strength": 0.2, "texture_strength": 0.5},
}

_VALID = {"conservative", "texture_rebuild", "print_ready", "custom"}

# Engine ids are validated lazily against the sr_backends registry; "cpu" is the
# always-available default and fallback.
_CPU_ENGINE = "cpu"

# Default ceiling on *input* pixels (~96 MP). The decode is refused above this
# so a decompression-bomb workflow asset cannot exhaust memory before we even
# look at it. Tunable via ``--max-decode-pixels`` (0 disables).
_DEFAULT_MAX_DECODE_PIXELS = 96_000_000

# Pillow modes that carry per-pixel transparency.
_ALPHA_MODES = {"RGBA", "LA", "La", "PA"}
# High-bit-depth integer/float modes we normalise down to 8-bit RGB.
_HIGHBIT_MODES = {"I", "I;16", "I;16B", "I;16L", "I;16N", "F"}


def _clip01(value: float) -> float:
    return float(min(1.0, max(0.0, value)))


def _safe_stem(image_path: str) -> str:
    """A filesystem-safe base name derived from the image file stem."""
    stem = Path(image_path).stem or "image"
    cleaned = "".join(ch if ch.isalnum() or ch in ("-", "_") else "_" for ch in stem)
    return cleaned or "image"


def _target_from_bounds(bounds_json: str) -> tuple[int, int]:
    """Pull ``(width, height)`` out of a connected placeholder bounds object.

    Accepts the ``{x, y, width, height}`` rectangle emitted by PSD Context
    Analyze's ``placeholder_bounds`` port; returns ``(0, 0)`` when absent or
    unparseable so the caller falls back to the preset scale.
    """
    text = (bounds_json or "").strip()
    if not text:
        return 0, 0
    try:
        data = json.loads(text)
    except (ValueError, TypeError):
        return 0, 0
    if not isinstance(data, dict):
        return 0, 0
    width = int(round(float(data.get("width", 0) or 0)))
    height = int(round(float(data.get("height", 0) or 0)))
    return max(0, width), max(0, height)


def _resolve_scale(
    src_w: int,
    src_h: int,
    target_w: int,
    target_h: int,
    fallback_scale: float,
    max_pixels: int,
) -> tuple[float, bool]:
    """Pick a uniform upscale factor and whether it was clamped by max pixels.

    With a target size the factor "covers" it (both dimensions reach the
    target); otherwise the preset/custom ``fallback_scale`` is used. The factor
    is reduced when the output would exceed ``max_pixels`` so a large
    placeholder cannot exhaust memory.
    """
    if target_w > 0 or target_h > 0:
        ratios = []
        if target_w > 0:
            ratios.append(target_w / src_w)
        if target_h > 0:
            ratios.append(target_h / src_h)
        scale = max(ratios)
    else:
        scale = max(0.01, fallback_scale)

    clamped = False
    if max_pixels > 0:
        out_pixels = (src_w * scale) * (src_h * scale)
        if out_pixels > max_pixels:
            scale *= (max_pixels / out_pixels) ** 0.5
            clamped = True
    return scale, clamped


def _load_image(image_path: str, max_decode_pixels: int) -> tuple[Any, bool]:
    """Open an image, refusing oversized inputs and fixing EXIF orientation.

    ``Image.open`` is lazy, so we read the declared size first and bail before
    decoding when it exceeds ``max_decode_pixels`` (0 disables the guard).
    Returns ``(image, exif_transposed)``.
    """
    from PIL import Image, ImageOps

    img = Image.open(image_path)
    width, height = img.size
    if max_decode_pixels > 0 and width * height > max_decode_pixels:
        raise ValueError(
            f"input image too large to decode safely: {width}x{height} "
            f"({width * height} px > max {max_decode_pixels})"
        )
    img.load()

    transposed = False
    try:
        fixed = ImageOps.exif_transpose(img)
    except Exception:  # noqa: BLE001 - a broken EXIF block must not abort enhance
        fixed = img
    if fixed is not img:
        transposed = True
    return fixed, transposed


def _cmyk_to_rgb(img: Any) -> Any:
    """Convert CMYK to sRGB, honouring an embedded ICC profile when present.

    A bare ``convert("RGB")`` uses a naive transform that visibly shifts colour;
    when the file carries a CMYK ICC profile we run a real profile-to-profile
    transform into sRGB instead, falling back to the naive path on any error.
    """
    icc = img.info.get("icc_profile")
    if icc:
        try:
            from PIL import ImageCms

            src = ImageCms.ImageCmsProfile(io.BytesIO(icc))
            dst = ImageCms.createProfile("sRGB")
            return ImageCms.profileToProfile(img, src, dst, outputMode="RGB")
        except Exception:  # noqa: BLE001 - fall back to the naive conversion
            pass
    return img.convert("RGB")


def _highbit_to_rgb(img: Any) -> Any:
    """Normalise a 16-bit / 32-bit / float image down to 8-bit RGB.

    ``Image.convert("RGB")`` on an ``I;16`` image clips to 0..255 and destroys
    the tonal range, so we scale the actual data range into 8 bits with numpy
    first.
    """
    import numpy as np

    arr = np.asarray(img).astype(np.float64)
    if arr.size == 0:
        return img.convert("RGB")
    peak = float(arr.max())
    if peak > 255.0:
        # 16-bit data spans 0..65535; map to 0..255 preserving relative tone.
        arr = arr * (255.0 / peak)
    arr = np.clip(arr, 0.0, 255.0).astype(np.uint8)
    from PIL import Image

    gray = Image.fromarray(arr, mode="L")
    return gray.convert("RGB")


def _split_alpha_and_rgb(img: Any) -> tuple[Any, Any | None, bool]:
    """Split an image into an 8-bit RGB working image and an optional alpha.

    Returns ``(rgb, alpha_or_none, had_alpha)``. The alpha channel is kept as a
    separate ``L`` image so enhancement only ever touches colour data.
    """
    mode = img.mode
    had_alpha = mode in _ALPHA_MODES or (mode == "P" and "transparency" in img.info)

    if had_alpha:
        rgba = img.convert("RGBA")
        alpha = rgba.getchannel("A")
        return rgba.convert("RGB"), alpha, True

    if mode == "CMYK":
        return _cmyk_to_rgb(img), None, False
    if mode in _HIGHBIT_MODES:
        return _highbit_to_rgb(img), None, False
    return img.convert("RGB"), None, False


def _denoise(img: Any, strength: float) -> Any:
    """Edge-preserving denoise: blend a median-filtered copy back in.

    A median filter removes speckle/compression noise while keeping edges
    crisp; mixing it in by ``strength`` softens noise without the global smear a
    Gaussian blur would leave for the unsharp mask to then re-amplify.
    """
    if strength <= 0.0:
        return img
    from PIL import Image, ImageFilter

    cleaned = img.filter(ImageFilter.MedianFilter(size=3))
    return Image.blend(img, cleaned, _clip01(strength))


def _sharpen(img: Any, strength: float) -> Any:
    """Restore high-frequency detail lost to the upscale via an unsharp mask."""
    if strength <= 0.0:
        return img
    from PIL import ImageFilter

    percent = int(round(_clip01(strength) * 150.0))
    return img.filter(ImageFilter.UnsharpMask(radius=2.0, percent=percent, threshold=2))


def _resample(img: Any, out_w: int, out_h: int, downscaling: bool) -> Any:
    """Resize, using a box filter when shrinking and Lanczos when enlarging."""
    from PIL import Image

    if (out_w, out_h) == img.size:
        return img
    resample = Image.BOX if downscaling else Image.LANCZOS
    return img.resize((out_w, out_h), resample)


def enhance(args: argparse.Namespace) -> dict[str, Any]:
    image_path = (args.image or "").strip()
    if not image_path or not Path(image_path).is_file():
        raise FileNotFoundError(f"base image not found: {image_path}")

    mode = (args.mode or "conservative").strip()
    if mode not in _VALID:
        raise ValueError(f"unknown mode {mode!r}; expected one of {sorted(_VALID)}")

    # A named preset wins over the sliders; ``custom`` uses the explicit args.
    if mode == "custom":
        denoise_strength = _clip01(args.denoise_strength)
        texture_strength = _clip01(args.texture_strength)
        fallback_scale = max(0.01, float(args.scale))
    else:
        p = _PRESETS[mode]
        denoise_strength = _clip01(p["denoise_strength"])
        texture_strength = _clip01(p["texture_strength"])
        fallback_scale = float(p["scale"])

    # Over-sharpening mangles logos and packaging text; honour the guard by
    # capping the texture strength when the subject carries protected marks.
    preserve_text_logo = bool(args.preserve_text_logo)
    if preserve_text_logo:
        texture_strength = min(texture_strength, 0.4)

    # Resolve the target size: explicit params win, else the connected bounds.
    target_w = int(max(0, args.target_width))
    target_h = int(max(0, args.target_height))
    if target_w <= 0 and target_h <= 0:
        target_w, target_h = _target_from_bounds(args.target_bounds_json)

    max_pixels = int(max(0, args.max_pixels))
    max_decode_pixels = int(max(0, args.max_decode_pixels))

    started = time.perf_counter()

    raw, exif_transposed = _load_image(image_path, max_decode_pixels)
    source_mode = raw.mode
    # Preserve an ICC profile only when we stay in the same colour model; a
    # CMYK/high-bit conversion produces sRGB data the old profile no longer
    # describes.
    icc_profile = raw.info.get("icc_profile") if source_mode in ("RGB", "RGBA", "L", "LA") else None

    rgb, alpha, had_alpha = _split_alpha_and_rgb(raw)
    src_w, src_h = rgb.size

    scale, clamped = _resolve_scale(
        src_w, src_h, target_w, target_h, fallback_scale, max_pixels
    )
    out_w = max(1, int(round(src_w * scale)))
    out_h = max(1, int(round(src_h * scale)))
    downscaling = out_w < src_w or out_h < src_h

    # Resolve the requested upscale engine. A non-cpu engine only applies to a
    # genuine upscale; downscales always use the CPU box filter. Any
    # unavailability (missing deps / weight, downscale, unknown name, runtime
    # error) falls back to the CPU path and is recorded in the report.
    engine_requested = (args.engine or _CPU_ENGINE).strip().lower() or _CPU_ENGINE
    engine_used = _CPU_ENGINE
    engine_fallback_reason: str | None = None
    backend_model: str | None = None
    used_backend = False

    if engine_requested != _CPU_ENGINE:
        from sr_backends import BackendUnavailable, resolve

        if downscaling or scale <= 1.0:
            engine_fallback_reason = "engine skipped: target is not an upscale"
        else:
            backend = resolve(engine_requested)
            if backend is None:
                engine_fallback_reason = f"unknown engine {engine_requested!r}"
            else:
                ok, reason = backend.available()
                if not ok:
                    engine_fallback_reason = reason
                else:
                    try:
                        rgb = backend.upscale(rgb, scale)
                        used_backend = True
                        engine_used = backend.id
                        backend_model = Path(backend.weight_path()).name
                    except BackendUnavailable as err:
                        engine_fallback_reason = err.reason
                    except Exception as err:  # noqa: BLE001 - degrade to CPU, never crash
                        engine_fallback_reason = f"{type(err).__name__}: {err}"

    if used_backend:
        # The model performs restoration + upscaling in one pass, so the CPU
        # denoise / unsharp steps are skipped; the result is already at the
        # requested factor (the backend resizes to the exact target).
        applied_denoise = 0.0
        applied_texture = 0.0
        denoise_method = engine_used
    else:
        # CPU pipeline (colour channels only): denoise the small image,
        # resample, then sharpen so restored detail lands on the final grid.
        # When downscaling we skip the unsharp pass -- it would only amplify
        # resampling artefacts.
        rgb = _denoise(rgb, denoise_strength)
        rgb = _resample(rgb, out_w, out_h, downscaling)
        applied_texture = 0.0 if downscaling else texture_strength
        rgb = _sharpen(rgb, applied_texture)
        applied_denoise = denoise_strength
        denoise_method = "median" if denoise_strength > 0.0 else "none"

    # Recombine the untouched alpha, resized on its own track so the matte edge
    # never picks up a denoise/sharpen halo.
    if had_alpha and alpha is not None:
        alpha_resized = _resample(alpha, out_w, out_h, downscaling)
        out_img = rgb.convert("RGBA")
        out_img.putalpha(alpha_resized)
    else:
        out_img = rgb

    directory = Path((args.output_dir or "").strip() or ".")
    directory.mkdir(parents=True, exist_ok=True)
    stem = (args.output_name or "").strip() or f"{_safe_stem(image_path)}_enhanced"
    image_out = directory / f"{stem}.png"

    target_dpi = int(max(1, args.target_dpi))
    save_kwargs: dict[str, Any] = {"format": "PNG", "dpi": (target_dpi, target_dpi)}
    icc_preserved = False
    if icc_profile:
        save_kwargs["icc_profile"] = icc_profile
        icc_preserved = True
    out_img.save(str(image_out), **save_kwargs)

    elapsed_ms = int(round((time.perf_counter() - started) * 1000.0))
    scale_factor = round(out_w / src_w, 4)

    enhance_report = {
        "mode": mode,
        "scale_factor": scale_factor,
        "source_mode": source_mode,
        "output_mode": out_img.mode,
        "had_alpha": had_alpha,
        "source_size": [src_w, src_h],
        "output_size": [out_w, out_h],
        "target_size": [target_w, target_h] if (target_w > 0 or target_h > 0) else None,
        "target_dpi": target_dpi,
        "max_pixels": max_pixels,
        "max_decode_pixels": max_decode_pixels,
        "clamped": clamped,
        "downscaled": downscaling,
        "exif_transposed": exif_transposed,
        "icc_preserved": icc_preserved,
        "denoise_method": denoise_method,
        "denoise_strength": round(applied_denoise, 4),
        "texture_strength": round(applied_texture, 4),
        "preserve_text_logo": preserve_text_logo,
        "engine": engine_used,
        "engine_requested": engine_requested,
        "engine_fallback_reason": engine_fallback_reason,
        "backend_model": backend_model,
        "processing_time_ms": elapsed_ms,
    }

    return {
        "enhanced_image": str(image_out),
        "scale_factor": scale_factor,
        "enhance_report": enhance_report,
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Upscale and sharpen a subject image for PSD placement (CPU)."
    )
    parser.add_argument("--image", required=True, help="path to the base image")
    parser.add_argument(
        "--mode",
        default="conservative",
        help="conservative | texture_rebuild | print_ready | custom",
    )
    parser.add_argument(
        "--target-width",
        dest="target_width",
        type=int,
        default=0,
        help="target width px (0 = auto from target bounds / preset scale)",
    )
    parser.add_argument(
        "--target-height",
        dest="target_height",
        type=int,
        default=0,
        help="target height px (0 = auto from target bounds / preset scale)",
    )
    parser.add_argument(
        "--target-bounds-json",
        dest="target_bounds_json",
        default="",
        help="connected PSD placeholder bounds {x,y,width,height} as JSON",
    )
    parser.add_argument(
        "--target-dpi",
        dest="target_dpi",
        type=int,
        default=300,
        help="DPI written into the output PNG metadata",
    )
    parser.add_argument(
        "--max-pixels",
        dest="max_pixels",
        type=int,
        default=48_000_000,
        help="cap on output pixels; scale is reduced to fit (0 disables)",
    )
    parser.add_argument(
        "--max-decode-pixels",
        dest="max_decode_pixels",
        type=int,
        default=_DEFAULT_MAX_DECODE_PIXELS,
        help="reject input images larger than this many pixels (0 disables)",
    )
    parser.add_argument(
        "--scale",
        type=float,
        default=2.0,
        help="upscale factor used when no target size is given (custom)",
    )
    parser.add_argument(
        "--denoise-strength",
        dest="denoise_strength",
        type=float,
        default=0.3,
        help="edge-preserving median denoise blend 0..1 (custom)",
    )
    parser.add_argument(
        "--texture-strength",
        dest="texture_strength",
        type=float,
        default=0.25,
        help="unsharp-mask detail strength 0..1 (custom)",
    )
    parser.add_argument(
        "--engine",
        default=_CPU_ENGINE,
        help="upscale engine: cpu (default) | realesrgan (opt-in, falls back to cpu)",
    )
    parser.add_argument(
        "--probe-engines",
        dest="probe_engines",
        action="store_true",
        help="print engine availability JSON and exit (UI capability probe)",
    )
    parser.add_argument(
        "--preserve-text-logo",
        dest="preserve_text_logo",
        action="store_true",
        help="cap sharpening so logos / packaging text are not mangled",
    )
    parser.add_argument(
        "--output-dir",
        dest="output_dir",
        default="",
        help="directory for the written PNG (default: cwd)",
    )
    parser.add_argument(
        "--output-name",
        dest="output_name",
        default="",
        help="base name for the PNG (default: <image>_enhanced)",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    if getattr(args, "probe_engines", False):
        from sr_backends import probe

        sys.stdout.write(json.dumps(probe(), ensure_ascii=False))
        return 0
    try:
        result = enhance(args)
    except Exception as err:  # noqa: BLE001 - surface a single clean error line
        sys.stderr.write(f"{type(err).__name__}: {err}\n")
        return 1
    sys.stdout.write(json.dumps(result, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
