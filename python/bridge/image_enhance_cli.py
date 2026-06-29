"""Headless CPU image enhancement for the H-Gripe desktop node editor.

This is the thin, ``torch``-free entry point behind the desktop
``enhance_image`` Tauri command -- the backend of the **Image Enhance /
Super Resolution** node, which sits after Mask Edge Refine in the PSD-first
production chain. Cloud image APIs often hand back a low-resolution subject;
dropped into a large print PSD it goes soft, loses texture and is unusable at
300 DPI. This node resizes the subject up to the PSD placeholder's pixel target
and restores high-frequency detail.

Phase 1 is deliberately CPU-only and dependency-light -- only the vendored
``Pillow`` + ``numpy`` (no GPU, no SupIR / CCSR / RealESRGAN): denoise is a
Gaussian-blur blend, the upscale is a Lanczos resample, and detail is an unsharp
mask. The GPU super-resolution backends are left as a future ``profile_ref``
mode behind the same node contract.

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
to fit and ``clamped`` is flagged in the report.

The emitted JSON is ``{"enhanced_image", "scale_factor", "enhance_report"}``
where ``enhance_report`` records the resolved mode, sizes, scale factor and
strengths. On failure the process exits non-zero with a single message on
stderr.
"""

from __future__ import annotations

import argparse
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


def _denoise(img: Any, strength: float) -> Any:
    """Blend a slight Gaussian blur back over the image to suppress noise.

    A full blur would destroy texture, so we mix the blurred copy in by
    ``strength`` only -- enough to soften sensor/compression noise before the
    upscale amplifies it.
    """
    if strength <= 0.0:
        return img
    from PIL import Image, ImageFilter

    blurred = img.filter(ImageFilter.GaussianBlur(radius=1.2))
    return Image.blend(img, blurred, _clip01(strength))


def _sharpen(img: Any, strength: float) -> Any:
    """Restore high-frequency detail lost to the upscale via an unsharp mask."""
    if strength <= 0.0:
        return img
    from PIL import ImageFilter

    percent = int(round(_clip01(strength) * 150.0))
    return img.filter(ImageFilter.UnsharpMask(radius=2.0, percent=percent, threshold=2))


def enhance(args: argparse.Namespace) -> dict[str, Any]:
    from PIL import Image

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

    started = time.perf_counter()
    img = Image.open(image_path).convert("RGBA")
    src_w, src_h = img.size

    scale, clamped = _resolve_scale(
        src_w, src_h, target_w, target_h, fallback_scale, max_pixels
    )
    out_w = max(1, int(round(src_w * scale)))
    out_h = max(1, int(round(src_h * scale)))

    # Pipeline: denoise the small image, upscale (Lanczos), then sharpen so the
    # restored detail lands on the final pixel grid.
    img = _denoise(img, denoise_strength)
    if (out_w, out_h) != (src_w, src_h):
        img = img.resize((out_w, out_h), Image.LANCZOS)
    img = _sharpen(img, texture_strength)

    directory = Path((args.output_dir or "").strip() or ".")
    directory.mkdir(parents=True, exist_ok=True)
    stem = (args.output_name or "").strip() or f"{_safe_stem(image_path)}_enhanced"
    image_out = directory / f"{stem}.png"

    target_dpi = int(max(1, args.target_dpi))
    img.save(str(image_out), format="PNG", dpi=(target_dpi, target_dpi))

    elapsed_ms = int(round((time.perf_counter() - started) * 1000.0))
    scale_factor = round(out_w / src_w, 4)

    enhance_report = {
        "mode": mode,
        "scale_factor": scale_factor,
        "source_size": [src_w, src_h],
        "output_size": [out_w, out_h],
        "target_size": [target_w, target_h] if (target_w > 0 or target_h > 0) else None,
        "target_dpi": target_dpi,
        "max_pixels": max_pixels,
        "clamped": clamped,
        "denoise_strength": round(denoise_strength, 4),
        "texture_strength": round(texture_strength, 4),
        "preserve_text_logo": preserve_text_logo,
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
        help="Gaussian-blur denoise blend 0..1 (custom)",
    )
    parser.add_argument(
        "--texture-strength",
        dest="texture_strength",
        type=float,
        default=0.25,
        help="unsharp-mask detail strength 0..1 (custom)",
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
    try:
        result = enhance(args)
    except Exception as err:  # noqa: BLE001 - surface a single clean error line
        sys.stderr.write(f"{type(err).__name__}: {err}\n")
        return 1
    sys.stdout.write(json.dumps(result, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
