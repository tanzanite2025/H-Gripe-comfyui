"""Headless mask edge refinement for the H-Gripe desktop node editor.

This is the thin, ``torch``-free entry point behind the desktop
``refine_mask_edge`` Tauri command -- the backend of the **Mask Edge Refine**
node, the third node of the PSD-first production chain. It cleans up the matte
of a cut-out subject so it drops into a PSD placeholder without white halos,
fringing or jagged, semi-transparent dirt at the edges.

Phase 1 is deliberately heuristic and dependency-light -- only the vendored
``Pillow`` + ``numpy`` (no OpenCV): erosion / dilation are ``MinFilter`` /
``MaxFilter`` morphology, feather is a Gaussian falloff, the edge-following
clean-up is a numpy box-filter guided filter, and colour decontamination pulls
opaque subject colour (or the target background colour) into the partial-alpha
band. OpenCV's ``bilateralFilter`` / ``guidedFilter`` are left as a future
optional high-quality backend.

Presets keep the UI to "pick an edge style + strength"; the per-step pixel
counts live behind an advanced toggle:

* ``clean``   -- bite 1px in, tight feather, decontaminate (kills white edges).
* ``natural`` -- bite 1px in, medium feather, decontaminate.
* ``soft``    -- no bite, wide feather, no decontaminate (dreamy edges).
* ``custom``  -- use the explicit ``--erode-px`` / ``--feather-px`` / ... args.

The emitted JSON is ``{"refined_image", "refined_mask", "edge_report"}`` where
``edge_report`` records the resolved morphology parameters, the edge-band size
and the mask coverage before/after. On failure the process exits non-zero with
a single message on stderr.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any

import numpy as np

_EPS = 1e-6

# Resolved (erode_px, dilate_px, feather_px, guided_radius, decontaminate,
# background_blend_strength) for each named preset. ``custom`` is handled by
# falling through to the explicit CLI args.
_PRESETS: dict[str, dict[str, Any]] = {
    "clean": {
        "erode_px": 1,
        "dilate_px": 0,
        "feather_px": 2,
        "guided_radius": 4,
        "edge_decontaminate": True,
        "background_blend_strength": 0.5,
    },
    "natural": {
        "erode_px": 1,
        "dilate_px": 0,
        "feather_px": 6,
        "guided_radius": 8,
        "edge_decontaminate": True,
        "background_blend_strength": 0.4,
    },
    "soft": {
        "erode_px": 0,
        "dilate_px": 0,
        "feather_px": 12,
        "guided_radius": 12,
        "edge_decontaminate": False,
        "background_blend_strength": 0.3,
    },
}


def _safe_stem(image_path: str) -> str:
    """A filesystem-safe base name derived from the image file stem."""
    stem = Path(image_path).stem or "image"
    cleaned = "".join(ch if ch.isalnum() or ch in ("-", "_") else "_" for ch in stem)
    return cleaned or "image"


def _load_rgb_alpha(path: str) -> tuple["np.ndarray", "np.ndarray"]:
    """Load an image as (H,W,3 uint8 RGB, H,W float alpha in 0..1)."""
    from PIL import Image

    arr = np.asarray(Image.open(path).convert("RGBA"), dtype=np.uint8)
    rgb = arr[..., :3].copy()
    alpha = arr[..., 3].astype(np.float32) / 255.0
    return rgb, alpha


def _load_mask(path: str | None, shape: tuple[int, int]) -> "np.ndarray | None":
    """Load an explicit matte as a float 0..1 array matched to ``shape`` (H,W)."""
    if not path:
        return None
    from PIL import Image

    mask = Image.open(path).convert("L")
    if mask.size != (shape[1], shape[0]):
        mask = mask.resize((shape[1], shape[0]))
    return np.asarray(mask, dtype=np.float32) / 255.0


def _morphology(mask: "np.ndarray", erode_px: int, dilate_px: int) -> "np.ndarray":
    """Erode then dilate a 0..1 matte by N pixels via Pillow Min/Max filters.

    Eroding first bites the white/background fringe off the edge; dilating after
    can win back coverage when a preset asks for it. Each pixel of radius is one
    pass of a 3x3 filter so non-square amounts stay well-defined.
    """
    from PIL import Image, ImageFilter

    img = Image.fromarray(np.rint(np.clip(mask, 0.0, 1.0) * 255.0).astype(np.uint8), "L")
    for _ in range(max(0, erode_px)):
        img = img.filter(ImageFilter.MinFilter(3))
    for _ in range(max(0, dilate_px)):
        img = img.filter(ImageFilter.MaxFilter(3))
    return np.asarray(img, dtype=np.float32) / 255.0


def _feather(mask: "np.ndarray", feather_px: float) -> "np.ndarray":
    """Soften matte edges with a Gaussian falloff (0 = leave hard)."""
    if feather_px <= 0.0:
        return mask
    from PIL import Image, ImageFilter

    img = Image.fromarray(np.rint(np.clip(mask, 0.0, 1.0) * 255.0).astype(np.uint8), "L")
    img = img.filter(ImageFilter.GaussianBlur(radius=float(feather_px)))
    return np.asarray(img, dtype=np.float32) / 255.0


def _box_filter(values: "np.ndarray", radius: int) -> "np.ndarray":
    """O(1)-per-pixel mean over a (2r+1) square via an integral image.

    Works on (H,W) or (H,W,C) float arrays; edges use the true window area so
    the result stays an honest local mean without darkening the border.
    """
    if radius <= 0:
        return values
    arr = values.astype(np.float64)
    pad = ((radius + 1, radius), (radius + 1, radius)) + ((0, 0),) * (arr.ndim - 2)
    padded = np.pad(arr, pad, mode="edge")
    integral = padded.cumsum(axis=0).cumsum(axis=1)
    size = 2 * radius + 1
    lower = integral[size:, size:]
    upper = integral[:-size, size:]
    left = integral[size:, :-size]
    corner = integral[:-size, :-size]
    summed = lower - upper - left + corner
    return (summed / float(size * size)).astype(np.float32)


def _guided_filter(guide: "np.ndarray", src: "np.ndarray", radius: int, eps: float) -> "np.ndarray":
    """Edge-aware smoothing of ``src`` (H,W) guided by ``guide`` (H,W), both 0..1.

    A numpy reimplementation of He et al.'s guided filter: it follows the
    subject's own luminance edges, so the refined matte hugs real contours
    instead of being uniformly blurred.
    """
    if radius <= 0:
        return src
    mean_i = _box_filter(guide, radius)
    mean_p = _box_filter(src, radius)
    corr_i = _box_filter(guide * guide, radius)
    corr_ip = _box_filter(guide * src, radius)
    var_i = corr_i - mean_i * mean_i
    cov_ip = corr_ip - mean_i * mean_p
    a = cov_ip / (var_i + eps)
    b = mean_p - a * mean_i
    mean_a = _box_filter(a, radius)
    mean_b = _box_filter(b, radius)
    return np.clip(mean_a * guide + mean_b, 0.0, 1.0)


def _decontaminate(rgb: "np.ndarray", opaque: "np.ndarray", band: "np.ndarray") -> "np.ndarray":
    """Pull opaque subject colour into the edge band to kill residual fringe.

    The partial-alpha rim of a cut-out keeps a smear of the old background
    (often white). We estimate a clean foreground colour by blurring the
    confidently-opaque pixels and bleed it outward, then replace the band's RGB
    with that estimate weighted by how transitional each pixel is.
    """
    w = opaque[..., None]
    blurred = _box_filter(rgb.astype(np.float32) * w, 6)
    norm = _box_filter(w, 6) + _EPS
    foreground = blurred / norm
    weight = band[..., None]
    return rgb.astype(np.float32) * (1.0 - weight) + foreground * weight


def _coverage(mask: "np.ndarray") -> float:
    """Mean matte coverage 0..1 (how much of the frame the subject occupies)."""
    return round(float(np.clip(mask, 0.0, 1.0).mean()), 4)


def refine(args: argparse.Namespace) -> dict[str, Any]:
    image_path = (args.image or "").strip()
    if not image_path or not Path(image_path).is_file():
        raise FileNotFoundError(f"subject image not found: {image_path}")

    preset = (args.preset or "natural").strip()
    valid = {"clean", "natural", "soft", "custom"}
    if preset not in valid:
        raise ValueError(f"unknown preset {preset!r}; expected one of {sorted(valid)}")

    # A named preset wins over the sliders; ``custom`` uses the explicit args.
    if preset == "custom":
        erode_px = int(max(0, args.erode_px))
        dilate_px = int(max(0, args.dilate_px))
        feather_px = float(max(0.0, args.feather_px))
        guided_radius = int(max(0, args.guided_radius))
        decontaminate = bool(args.edge_decontaminate)
        blend_strength = float(np.clip(args.background_blend_strength, 0.0, 1.0))
    else:
        p = _PRESETS[preset]
        erode_px = int(p["erode_px"])
        dilate_px = int(p["dilate_px"])
        feather_px = float(p["feather_px"])
        guided_radius = int(p["guided_radius"])
        decontaminate = bool(p["edge_decontaminate"])
        blend_strength = float(p["background_blend_strength"])

    rgb, alpha = _load_rgb_alpha(image_path)
    height, width = rgb.shape[:2]

    # Prefer an explicit matte; otherwise refine the subject's own alpha. A fully
    # opaque image with no mask has no edge to work on, so we flag that.
    explicit = _load_mask(args.mask, (height, width))
    mask = explicit if explicit is not None else alpha
    source_mask = "explicit" if explicit is not None else "alpha"

    background_rgb: "np.ndarray | None" = None
    bg_path = (args.background or "").strip()
    if bg_path:
        if not Path(bg_path).is_file():
            raise FileNotFoundError(f"background image not found: {bg_path}")
        background_rgb, _ = _load_rgb_alpha(bg_path)
        if background_rgb.shape[:2] != (height, width):
            from PIL import Image

            resized = Image.fromarray(background_rgb, "RGB").resize((width, height))
            background_rgb = np.asarray(resized, dtype=np.uint8)

    coverage_before = _coverage(mask)

    # 1) Morphology: bite the fringe in (erode), optionally grow back (dilate).
    refined = _morphology(mask, erode_px, dilate_px)
    # 2) Guided filter: snap the matte to the subject's own luminance edges.
    if guided_radius > 0:
        guide = rgb.astype(np.float32) @ np.array([0.299, 0.587, 0.114], np.float32) / 255.0
        refined = _guided_filter(guide, refined, guided_radius, eps=1e-3)
    # 3) Feather: soft transition so the composite has no stair-stepping.
    refined = _feather(refined, feather_px)
    refined = np.clip(refined, 0.0, 1.0)

    # The edge band: pixels that are neither solidly in nor solidly out -- this
    # is where fringe lives and where decontamination / background blend act.
    band = (np.minimum(refined, 1.0 - refined) * 2.0).astype(np.float32)

    out_rgb = rgb.astype(np.float32)
    if decontaminate:
        opaque = (refined > 0.9).astype(np.float32)
        if float(opaque.sum()) > _EPS:
            out_rgb = _decontaminate(rgb, opaque, band)
    if background_rgb is not None and blend_strength > 0.0:
        # Replace lingering old-background colour in the band with the target
        # background's colour, so the seam matches once composited.
        weight = (band * blend_strength)[..., None]
        out_rgb = out_rgb * (1.0 - weight) + background_rgb.astype(np.float32) * weight
    out_rgb = np.clip(out_rgb, 0.0, 255.0).astype(np.uint8)

    directory = Path((args.output_dir or "").strip() or ".")
    directory.mkdir(parents=True, exist_ok=True)
    stem = (args.output_name or "").strip() or f"{_safe_stem(image_path)}_refined"

    from PIL import Image

    refined_u8 = np.rint(refined * 255.0).astype(np.uint8)
    rgba = np.dstack([out_rgb, refined_u8])
    image_out = directory / f"{stem}.png"
    mask_out = directory / f"{stem}_mask.png"
    Image.fromarray(rgba, "RGBA").save(str(image_out), format="PNG")
    Image.fromarray(refined_u8, "L").save(str(mask_out), format="PNG")

    edge_report = {
        "preset": preset,
        "source_mask": source_mask,
        "erode_px": erode_px,
        "dilate_px": dilate_px,
        "feather_px": round(feather_px, 2),
        "guided_radius": guided_radius,
        "edge_decontaminate": decontaminate,
        "background_blend_strength": round(blend_strength, 4),
        "background_applied": background_rgb is not None and blend_strength > 0.0,
        "edge_band_px": int(round(float((band > 0.05).sum()))),
        "coverage_before": coverage_before,
        "coverage_after": _coverage(refined),
        "output_size": [width, height],
    }

    return {
        "refined_image": str(image_out),
        "refined_mask": str(mask_out),
        "edge_report": edge_report,
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Refine a cut-out subject's mask edges for PSD compositing."
    )
    parser.add_argument("--image", required=True, help="path to the subject image")
    parser.add_argument(
        "--mask", default="", help="explicit matte (default: the image's own alpha)"
    )
    parser.add_argument(
        "--background", default="", help="target background for edge colour blending"
    )
    parser.add_argument(
        "--placeholder-mask",
        dest="placeholder_mask",
        default="",
        help="PSD placeholder mask (advisory in Phase 1)",
    )
    parser.add_argument(
        "--preset", default="natural", help="clean | natural | soft | custom"
    )
    parser.add_argument(
        "--erode-px", dest="erode_px", type=int, default=1, help="bite N px in (custom)"
    )
    parser.add_argument(
        "--dilate-px", dest="dilate_px", type=int, default=0, help="grow N px out (custom)"
    )
    parser.add_argument(
        "--feather-px",
        dest="feather_px",
        type=float,
        default=4.0,
        help="Gaussian edge feather radius (custom)",
    )
    parser.add_argument(
        "--guided-radius",
        dest="guided_radius",
        type=int,
        default=8,
        help="guided-filter radius, 0 disables (custom)",
    )
    parser.add_argument(
        "--edge-decontaminate",
        dest="edge_decontaminate",
        action="store_true",
        help="pull opaque subject colour into the edge band (custom)",
    )
    parser.add_argument(
        "--background-blend-strength",
        dest="background_blend_strength",
        type=float,
        default=0.4,
        help="blend the edge band toward the target background 0..1 (custom)",
    )
    parser.add_argument(
        "--output-dir",
        dest="output_dir",
        default="",
        help="directory for the written PNGs (default: cwd)",
    )
    parser.add_argument(
        "--output-name",
        dest="output_name",
        default="",
        help="base name for the PNGs (default: <image>_refined)",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        result = refine(args)
    except Exception as err:  # noqa: BLE001 - surface a single clean error line
        sys.stderr.write(f"{type(err).__name__}: {err}\n")
        return 1
    sys.stdout.write(json.dumps(result, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
