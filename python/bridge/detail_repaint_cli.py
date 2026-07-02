"""Headless localized repaint helper for the H-Gripe desktop node editor.

This is the thin, ``torch``-free pixel backend behind the desktop
``prepare_repaint_regions`` / ``composite_repaint`` Tauri commands -- the two
pixel halves of the **Detail Repaint** node, the Phase-2 follow-up to the
detect-only **Detail Watchdog**. Detail Watchdog reports *where* an image
breaks down (a :class:`QualityReport`); Detail Repaint takes those issue
regions and actually fixes them via a GPU/repaint provider.

The generative fix itself (``image.edit`` through the H-Gripe broker) is owned
by the Rust/TS orchestration layer, not this script, so the pixel work is split
into stateless subcommands the orchestrator drives around the broker call:

* ``prepare``   -- for each issue region selected from the quality report, crop
  a padded window out of the candidate image and write a same-size inpaint
  ``mask`` marking the (un-padded) issue core as the edit area. Emits a JSON
  manifest of the regions (crop + mask paths, geometry) so the orchestrator can
  send each ``crop`` + ``mask`` + repaint prompt to the provider.
* ``repaint``   -- the **opt-in local** ``engine`` seam (Phase 2,
  ``docs/phase2-algorithm-roadmap.md`` §3): instead of the remote provider, run
  a local GPU inpaint backend (``python/bridge/inpaint_backends/``) over the
  manifest's crops + masks + prompt, writing repainted crops and emitting the
  same ``{index, path}`` list ``composite`` consumes. ``provider`` stays the
  default and the always-available fallback — when it is selected, or the local
  backend's deps/weights are missing, no local repaint runs and the orchestrator
  uses the remote provider exactly as before. ``--probe-engines`` reports which
  engines are usable so the UI can grey out unavailable ones.
* ``composite`` -- given the repainted crops (from the provider *or* the local
  backend), paste each back into the candidate within a *feathered* version of
  its issue core (a secondary edge fusion at the patch seam), leaving the
  padding context untouched, and write the final fixed image. Emits a
  ``repaint_report``.

The ``prepare`` / ``composite`` pixel halves use only the vendored ``Pillow`` +
``numpy`` (no OpenCV, no ML); the optional ``repaint`` backends import ``torch``
/ ``diffusers`` lazily and only when explicitly selected. The pixel subcommands
**input-harden** the candidate decode: CMYK (via its embedded ICC
profile when present), 16-bit / float, palette and grayscale sources are
normalised to an 8-bit RGBA working space, EXIF orientation is applied, and an
input larger than ``--max-decode-pixels`` is refused before it is decoded. The
``composite`` step is **alpha-isolated** (Method A): only the RGB channels of
the repainted patch are blended in, the candidate's original alpha is preserved
so a cut-out subject keeps its matte and gains no seam halo. On failure the
process exits non-zero with a single message on stderr.
"""

from __future__ import annotations

import argparse
import io
import json
import sys
from pathlib import Path
from typing import Any

import numpy as np

# Refuse to decode an input larger than this many pixels (decompression-bomb
# guard). 0 disables the check.
_DEFAULT_MAX_DECODE_PIXELS = 96_000_000
_ALPHA_MODES = {"RGBA", "LA", "La", "PA"}
_HIGHBIT_MODES = {"I", "I;16", "I;16B", "I;16L", "I;16N", "F"}
# EXIF tag holding the orientation (1 = normal, 2..8 = a flip/rotation).
_EXIF_ORIENTATION_TAG = 0x0112

# Quality-report ``suggested_action`` values that a localized repaint can act
# on. ``image_enhance`` (global low-resolution) and ``color_match`` (global
# colour drift) are whole-image fixes handled by other nodes, not local repaint.
_DEFAULT_REPAINT_ACTIONS = ("detail_redraw",)


def _safe_stem(image_path: str) -> str:
    """A filesystem-safe base name derived from the image file stem."""
    stem = Path(image_path).stem or "image"
    cleaned = "".join(ch if ch.isalnum() or ch in ("-", "_") else "_" for ch in stem)
    return cleaned or "image"


def _cmyk_to_rgb(img: Any) -> Any:
    """Convert CMYK to sRGB, honouring an embedded ICC profile when present.

    A bare ``convert("RGB")`` uses a naive transform that visibly shifts colour;
    with an embedded CMYK profile we run a real profile-to-profile transform
    into sRGB instead, falling back to the naive path on any error.
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

    ``convert("RGB")`` on an ``I;16`` image clips to 0..255 and destroys the
    tonal range, so we scale the actual data range into 8 bits first.
    """
    arr = np.asarray(img).astype(np.float64)
    if arr.size == 0:
        return img.convert("RGB")
    peak = float(arr.max())
    if peak > 255.0:
        arr = arr * (255.0 / peak)
    arr = np.clip(arr, 0.0, 255.0).astype(np.uint8)
    from PIL import Image

    return Image.fromarray(arr, mode="L").convert("RGB")


def _apply_exif_orientation(img: Any) -> tuple[Any, bool]:
    """Apply EXIF orientation, returning ``(image, transposed)``.

    Pillow 12's ``ImageOps.exif_transpose`` returns a *new* object even when
    there is no orientation to apply, so ``fixed is not img`` over-reports a
    transpose on every plain image. We read the orientation tag directly and
    only transpose for a real, non-identity orientation.
    """
    from PIL import ImageOps

    try:
        orientation = img.getexif().get(_EXIF_ORIENTATION_TAG, 1)
    except Exception:  # noqa: BLE001 - a broken EXIF block must not abort repaint
        orientation = 1
    if orientation in (None, 0, 1):
        return img, False
    try:
        fixed = ImageOps.exif_transpose(img)
    except Exception:  # noqa: BLE001 - fall back to the un-rotated image
        return img, False
    return fixed, fixed is not img


def _load_rgba(
    path: str, max_decode_pixels: int = _DEFAULT_MAX_DECODE_PIXELS
) -> tuple["np.ndarray", str, bool]:
    """Load an image as (H,W,4 uint8 RGBA, source_mode, exif_transposed).

    Refuses oversized inputs before decoding, applies EXIF orientation, and maps
    CMYK / high-bit / palette / grayscale sources into an 8-bit RGBA working
    space so crops and the paste-back composite carry honest colour. The alpha
    channel is taken from the source when present, else a fully-opaque plane.
    """
    from PIL import Image

    img = Image.open(path)
    width, height = img.size
    if max_decode_pixels > 0 and width * height > max_decode_pixels:
        raise ValueError(
            f"input image too large to decode safely: {width}x{height} "
            f"({width * height} px > max {max_decode_pixels})"
        )
    img.load()

    img, transposed = _apply_exif_orientation(img)

    # A ProPhoto-tagged manual product (the Rust chain's 16-bit output) is
    # colour-managed into sRGB; everything else passes through untouched.
    from wide_gamut import managed_to_srgb

    img, _ = managed_to_srgb(img)

    source_mode = img.mode
    had_alpha = source_mode in _ALPHA_MODES or (
        source_mode == "P" and "transparency" in img.info
    )

    if had_alpha:
        rgba_img = img.convert("RGBA")
    elif source_mode == "CMYK":
        rgba_img = _cmyk_to_rgb(img).convert("RGBA")
    elif source_mode in _HIGHBIT_MODES:
        rgba_img = _highbit_to_rgb(img).convert("RGBA")
    else:
        rgba_img = img.convert("RGBA")
    return np.asarray(rgba_img, dtype=np.uint8), source_mode, transposed


def _load_json_arg(raw: str | None, label: str) -> Any:
    """Parse an inline JSON argument, raising a clean error on bad input."""
    text = (raw or "").strip()
    if not text:
        return None
    try:
        return json.loads(text)
    except json.JSONDecodeError as err:
        raise ValueError(f"invalid {label} JSON: {err}") from err


def _clamp_box(box: list[int], width: int, height: int) -> list[int]:
    """Clamp ``[x1, y1, x2, y2]`` to the image and guarantee a non-empty box."""
    x1, y1, x2, y2 = box
    x1 = int(max(0, min(x1, width - 1)))
    y1 = int(max(0, min(y1, height - 1)))
    x2 = int(max(x1 + 1, min(x2, width)))
    y2 = int(max(y1 + 1, min(y2, height)))
    return [x1, y1, x2, y2]


def _pad_box(box: list[int], padding: int, width: int, height: int) -> list[int]:
    """Grow ``box`` outward by ``padding`` px, clamped to the image."""
    x1, y1, x2, y2 = box
    return _clamp_box([x1 - padding, y1 - padding, x2 + padding, y2 + padding], width, height)


def _feather_mask(shape: tuple[int, int], inner: list[int], feather_px: float) -> "np.ndarray":
    """Build a 0..1 (H,W) alpha that is 1 inside ``inner`` and falls off softly.

    ``inner`` is ``[x1, y1, x2, y2]`` in the crop's own coordinates. The
    Gaussian falloff at the rectangle edge is the "secondary edge fusion" that
    hides the patch seam when the repainted core is composited back.
    """
    from PIL import Image, ImageFilter

    height, width = shape
    hard = np.zeros((height, width), dtype=np.uint8)
    x1, y1, x2, y2 = inner
    hard[y1:y2, x1:x2] = 255
    if feather_px <= 0.0:
        return hard.astype(np.float32) / 255.0
    img = Image.fromarray(hard, "L").filter(ImageFilter.GaussianBlur(radius=float(feather_px)))
    return np.asarray(img, dtype=np.float32) / 255.0


def _dst1(arr: "np.ndarray", axis: int) -> "np.ndarray":
    """Type-I discrete sine transform along ``axis`` (via an odd-extended FFT).

    DST-I diagonalises the Dirichlet discrete Laplacian on a rectangle, which is
    what lets the Poisson blend solve the seam equation exactly in one pass
    instead of iterating. Self-inverse up to a factor of ``2 / (n + 1)``.
    """
    n = arr.shape[axis]
    shape = list(arr.shape)
    shape[axis] = 2 * n + 2
    ext = np.zeros(shape, dtype=np.float64)
    sl: list[Any] = [slice(None)] * arr.ndim
    sl[axis] = slice(1, n + 1)
    ext[tuple(sl)] = arr
    sl[axis] = slice(n + 2, 2 * n + 2)
    ext[tuple(sl)] = -np.flip(arr, axis=axis)
    sl[axis] = slice(1, n + 1)
    return -np.fft.fft(ext, axis=axis).imag[tuple(sl)] / 2.0


def _poisson_solve(rhs: "np.ndarray") -> "np.ndarray":
    """Solve ``(4u - sum of 4-neighbours) = rhs`` with zero Dirichlet boundary.

    ``rhs`` is (H,W); the known boundary values must already be folded into it.
    The rectangular domain lets the 5-point Poisson system be solved exactly by
    a 2-D DST-I eigen-decomposition (no iterative solver needed).
    """
    height, width = rhs.shape
    transformed = _dst1(_dst1(rhs, 0), 1)
    lam_i = 2.0 - 2.0 * np.cos(np.pi * np.arange(1, height + 1) / (height + 1))
    lam_j = 2.0 - 2.0 * np.cos(np.pi * np.arange(1, width + 1) / (width + 1))
    transformed /= lam_i[:, None] + lam_j[None, :]
    return _dst1(_dst1(transformed, 0), 1) * (4.0 / ((height + 1) * (width + 1)))


def _poisson_blend_rgb(window: "np.ndarray", patch: "np.ndarray", inner: list[int]) -> bool:
    """Gradient-domain (Poisson) clone of the patch RGB into ``window``.

    Seamless-clone formulation: inside the issue core the result keeps the
    repainted patch's *gradients* while its boundary is pinned to the
    surrounding candidate pixels, so illumination/colour offsets at the seam
    diffuse away instead of showing as a visible edge. Only the RGB channels of
    ``window`` (a float32 view into the candidate) are written — the alpha
    isolation contract is untouched. Returns ``False`` (caller falls back to
    the feathered blend) when the core is too small to solve.
    """
    crop_h, crop_w = window.shape[:2]
    x1, y1, x2, y2 = inner
    x1 = max(0, min(x1, crop_w))
    y1 = max(0, min(y1, crop_h))
    x2 = max(x1, min(x2, crop_w))
    y2 = max(y1, min(y2, crop_h))
    core_h, core_w = y2 - y1, x2 - x1
    if core_h < 3 or core_w < 3:
        return False

    # Pad by one replicated pixel so the boundary ring exists even when the
    # issue core touches the crop edge; (y, x) maps to padded (y + 1, x + 1).
    base_pad = np.pad(window[..., :3], ((1, 1), (1, 1), (0, 0)), mode="edge").astype(np.float64)
    patch_pad = np.pad(patch[..., :3], ((1, 1), (1, 1), (0, 0)), mode="edge").astype(np.float64)
    bound = base_pad[y1 : y2 + 2, x1 : x2 + 2]
    guide = patch_pad[y1 : y2 + 2, x1 : x2 + 2]

    for channel in range(3):
        g = guide[..., channel]
        # Discrete Laplacian of the patch over the core (the guidance field).
        rhs = 4.0 * g[1:-1, 1:-1] - g[:-2, 1:-1] - g[2:, 1:-1] - g[1:-1, :-2] - g[1:-1, 2:]
        # Fold the known Dirichlet boundary (candidate pixels) into the RHS.
        b = bound[..., channel]
        rhs[0, :] += b[0, 1:-1]
        rhs[-1, :] += b[-1, 1:-1]
        rhs[:, 0] += b[1:-1, 0]
        rhs[:, -1] += b[1:-1, -1]
        solved = _poisson_solve(rhs)
        window[y1:y2, x1:x2, channel] = np.clip(solved, 0.0, 255.0).astype(np.float32)
    return True


def _auto_feather(inner: list[int]) -> float:
    """A feather radius scaled to the issue core (~6% of its short side)."""
    x1, y1, x2, y2 = inner
    short = max(1, min(x2 - x1, y2 - y1))
    return float(max(2.0, min(24.0, round(short * 0.06))))


def _select_issues(
    issues: list[dict[str, Any]],
    actions: set[str],
    min_confidence: float,
) -> tuple[list[tuple[int, dict[str, Any]]], list[dict[str, Any]]]:
    """Split report issues into (selected for repaint, skipped) with reasons."""
    selected: list[tuple[int, dict[str, Any]]] = []
    skipped: list[dict[str, Any]] = []
    for index, issue in enumerate(issues):
        if not isinstance(issue, dict):
            continue
        action = str(issue.get("suggested_action") or "")
        confidence = float(issue.get("confidence") or 0.0)
        bbox = issue.get("bbox")
        if not (isinstance(bbox, list) and len(bbox) == 4):
            skipped.append({"index": index, "type": issue.get("type"), "reason": "no_bbox"})
            continue
        if action not in actions:
            skipped.append({"index": index, "type": issue.get("type"), "reason": "action_not_repaintable"})
            continue
        if confidence < min_confidence:
            skipped.append({"index": index, "type": issue.get("type"), "reason": "below_min_confidence"})
            continue
        selected.append((index, issue))
    return selected, skipped


def prepare(args: argparse.Namespace) -> dict[str, Any]:
    image_path = (args.image or "").strip()
    if not image_path or not Path(image_path).is_file():
        raise FileNotFoundError(f"candidate image not found: {image_path}")

    report = _load_json_arg(args.quality_report, "quality_report") or {}
    issues = report.get("issues") if isinstance(report, dict) else None
    issues = issues if isinstance(issues, list) else []

    actions = {
        a.strip()
        for a in (args.repaint_actions or "").split(",")
        if a.strip()
    } or set(_DEFAULT_REPAINT_ACTIONS)
    min_confidence = float(max(0.0, min(1.0, args.min_confidence)))
    padding = int(max(0, args.padding))
    max_regions = int(max(1, args.max_regions))
    invert_mask = bool(args.invert_mask)
    max_decode_pixels = int(max(0, getattr(args, "max_decode_pixels", _DEFAULT_MAX_DECODE_PIXELS)))

    rgba, source_mode, exif_transposed = _load_rgba(image_path, max_decode_pixels)
    height, width = rgba.shape[:2]

    selected, skipped = _select_issues(issues, actions, min_confidence)
    # Highest-confidence issues first, then cap how many regions we repaint.
    selected.sort(key=lambda pair: float(pair[1].get("confidence") or 0.0), reverse=True)
    capped = selected[max_regions:]
    selected = selected[:max_regions]
    for index, issue in capped:
        skipped.append({"index": index, "type": issue.get("type"), "reason": "over_max_regions"})

    directory = Path((args.output_dir or "").strip() or ".")
    directory.mkdir(parents=True, exist_ok=True)
    stem = (args.output_name or "").strip() or f"{_safe_stem(image_path)}_repaint"

    from PIL import Image

    regions: list[dict[str, Any]] = []
    for index, issue in selected:
        bbox = _clamp_box([int(v) for v in issue["bbox"]], width, height)
        crop_box = _pad_box(bbox, padding, width, height)
        cx1, cy1, cx2, cy2 = crop_box
        inner = [bbox[0] - cx1, bbox[1] - cy1, bbox[2] - cx1, bbox[3] - cy1]

        crop = rgba[cy1:cy2, cx1:cx2]
        crop_w, crop_h = cx2 - cx1, cy2 - cy1
        crop_path = directory / f"{stem}_region{index}.png"
        Image.fromarray(crop, "RGBA").save(str(crop_path), format="PNG")

        # Inpaint mask, crop-sized. OpenAI-style ``image.edit`` reads the
        # *transparent* (alpha 0) pixels as the area to regenerate, so the issue
        # core is punched transparent and the padding kept opaque. ``--invert``
        # flips this for providers that treat opaque/white as the edit area.
        edit_alpha, keep_alpha = (0, 255) if not invert_mask else (255, 0)
        mask = np.full((crop_h, crop_w), keep_alpha, dtype=np.uint8)
        mask[inner[1]:inner[3], inner[0]:inner[2]] = edit_alpha
        mask_rgba = np.dstack([np.full((crop_h, crop_w, 3), 255, np.uint8), mask])
        mask_path = directory / f"{stem}_region{index}_mask.png"
        Image.fromarray(mask_rgba, "RGBA").save(str(mask_path), format="PNG")

        regions.append(
            {
                "index": index,
                "type": issue.get("type"),
                "confidence": round(float(issue.get("confidence") or 0.0), 4),
                "suggested_action": issue.get("suggested_action"),
                "bbox": bbox,
                "crop_box": crop_box,
                "inner_box": inner,
                "size": [crop_w, crop_h],
                "crop_path": str(crop_path),
                "mask_path": str(mask_path),
            }
        )

    return {
        "regions": regions,
        "skipped": skipped,
        "image_size": [width, height],
        "selected_count": len(regions),
        "mask_edit_is_transparent": not invert_mask,
        "source_mode": source_mode,
        "exif_transposed": exif_transposed,
        "max_decode_pixels": max_decode_pixels,
    }


def composite(args: argparse.Namespace) -> dict[str, Any]:
    image_path = (args.image or "").strip()
    if not image_path or not Path(image_path).is_file():
        raise FileNotFoundError(f"candidate image not found: {image_path}")

    manifest = _load_json_arg(args.manifest, "manifest") or {}
    regions = manifest.get("regions") if isinstance(manifest, dict) else None
    regions = regions if isinstance(regions, list) else []

    repainted_raw = _load_json_arg(args.repainted, "repainted") or []
    # Map region index -> repainted crop path (entries with a blank path mean
    # the provider returned nothing for that region, so it stays unrepainted).
    repainted: dict[int, str] = {}
    if isinstance(repainted_raw, list):
        for entry in repainted_raw:
            if isinstance(entry, dict) and entry.get("path"):
                repainted[int(entry.get("index"))] = str(entry["path"])

    from PIL import Image

    blend = (getattr(args, "blend", "") or "feather").strip().lower() or "feather"
    if blend not in ("feather", "poisson"):
        raise ValueError(f"unknown blend mode {blend!r} (expected feather | poisson)")

    max_decode_pixels = int(max(0, getattr(args, "max_decode_pixels", _DEFAULT_MAX_DECODE_PIXELS)))
    base_u8, source_mode, exif_transposed = _load_rgba(image_path, max_decode_pixels)
    base = base_u8.astype(np.float32)
    height, width = base.shape[:2]

    region_results: list[dict[str, Any]] = []
    repainted_count = 0
    for region in regions:
        if not isinstance(region, dict):
            continue
        index = int(region.get("index"))
        result = {
            "index": index,
            "type": region.get("type"),
            "bbox": region.get("bbox"),
            "status": "skipped",
        }
        patch_path = repainted.get(index)
        crop_box = region.get("crop_box")
        inner = region.get("inner_box")
        if not patch_path or not Path(str(patch_path)).is_file():
            result["status"] = "no_repaint"
            region_results.append(result)
            continue
        if not (isinstance(crop_box, list) and len(crop_box) == 4 and isinstance(inner, list)):
            result["status"] = "bad_geometry"
            region_results.append(result)
            continue

        cx1, cy1, cx2, cy2 = (int(v) for v in crop_box)
        crop_w, crop_h = cx2 - cx1, cy2 - cy1
        patch = Image.open(str(patch_path)).convert("RGBA")
        if patch.size != (crop_w, crop_h):
            # Shrinking a provider crop: a box (area-average) filter avoids the
            # ringing/aliasing Lanczos introduces when downsampling; only grow
            # with Lanczos.
            shrinking = crop_w < patch.size[0] or crop_h < patch.size[1]
            resample = Image.BOX if shrinking else Image.LANCZOS
            patch = patch.resize((crop_w, crop_h), resample)
        patch_arr = np.asarray(patch, dtype=np.float32)

        # Alpha isolation (Method A): blend only RGB; keep the candidate's own
        # alpha so a cut-out subject's matte is never softened or haloed.
        window = base[cy1:cy2, cx1:cx2]
        blended_poisson = False
        if blend == "poisson":
            blended_poisson = _poisson_blend_rgb(window, patch_arr, [int(v) for v in inner])
        if blended_poisson:
            result["blend"] = "poisson"
        else:
            feather = float(args.feather_px) if args.feather_px > 0 else _auto_feather([int(v) for v in inner])
            alpha = _feather_mask((crop_h, crop_w), [int(v) for v in inner], feather)[..., None]
            window[..., :3] = window[..., :3] * (1.0 - alpha) + patch_arr[..., :3] * alpha
            result["blend"] = "feather"
            result["feather_px"] = round(feather, 2)
        repainted_count += 1
        result["status"] = "repainted"
        region_results.append(result)

    if repainted_count == 0:
        status = "unchanged"
    elif repainted_count == len([r for r in region_results if r["status"] != "skipped"]):
        status = "repainted"
    else:
        status = "partial"

    directory = Path((args.output_dir or "").strip() or ".")
    directory.mkdir(parents=True, exist_ok=True)
    stem = (args.output_name or "").strip() or f"{_safe_stem(image_path)}_repainted"
    out_path = directory / f"{stem}.png"
    Image.fromarray(np.clip(base, 0.0, 255.0).astype(np.uint8), "RGBA").save(
        str(out_path), format="PNG"
    )

    return {
        "fixed_image": str(out_path),
        "repaint_report": {
            "status": status,
            "regions": region_results,
            "repainted_count": repainted_count,
            "requested_count": len(regions),
            "image_size": [width, height],
            "blend": blend,
            "source_mode": source_mode,
            "exif_transposed": exif_transposed,
            "max_decode_pixels": max_decode_pixels,
        },
    }


_PROVIDER_ENGINE = "provider"
# Default compute-precision selection for the local torch backend: fp16 on a
# CUDA device, fp32 on CPU (mirrors sr_backends.PRECISION_AUTO; kept local so
# this module stays torch-free at import time).
_PRECISION_AUTO = "auto"
# Default structural-conditioning selection for the local backend: no
# ControlNet (mirrors inpaint_backends.CONTROLNET_OFF; kept local so this
# module stays torch-free at import time).
_CONTROLNET_OFF = "off"


def _inner_mask(size: tuple[int, int], inner: list[int]) -> "np.ndarray":
    """A crop-sized ``L`` mask (uint8) that is white (255) inside ``inner``.

    This is the **diffusers convention** (white marks the area to regenerate),
    derived from the manifest geometry directly so it is independent of the
    provider-oriented alpha mask ``prepare`` wrote (whose edit-area polarity is
    flipped by ``--invert-mask``).
    """
    crop_w, crop_h = size
    mask = np.zeros((crop_h, crop_w), dtype=np.uint8)
    x1, y1, x2, y2 = (int(v) for v in inner)
    x1 = max(0, min(x1, crop_w))
    y1 = max(0, min(y1, crop_h))
    x2 = max(x1, min(x2, crop_w))
    y2 = max(y1, min(y2, crop_h))
    mask[y1:y2, x1:x2] = 255
    return mask


def repaint(args: argparse.Namespace) -> dict[str, Any]:
    """Run the opt-in local inpaint backend over a prepare manifest's regions.

    Emits the same ``{index, path}`` repaint list ``composite`` consumes plus
    engine telemetry. ``provider`` (the default) and any unavailable backend
    produce an **empty** list and a recorded ``engine_fallback_reason`` so the
    orchestrator falls back to the remote ``image.edit`` provider — this never
    hard-fails on a box without the model.
    """
    manifest = _load_json_arg(args.manifest, "manifest") or {}
    regions = manifest.get("regions") if isinstance(manifest, dict) else None
    regions = regions if isinstance(regions, list) else []

    prompt = (args.prompt or "").strip()
    prompt_map = _load_json_arg(args.prompt_map, "prompt_map") or {}
    if not isinstance(prompt_map, dict):
        prompt_map = {}

    engine_requested = (args.engine or _PROVIDER_ENGINE).strip().lower() or _PROVIDER_ENGINE
    engine_used = _PROVIDER_ENGINE
    engine_fallback_reason: str | None = None
    backend_model: str | None = None

    # ``precision`` selects fp16/fp32 for the local torch backend (the remote
    # ``provider`` path runs no local session). ``precision_used`` / ``device_used``
    # are what the backend actually ran, which can differ from the request (an
    # explicit ``fp16`` degrades to ``fp32`` on a CPU run).
    precision_requested = (
        getattr(args, "precision", None) or _PRECISION_AUTO
    ).strip().lower() or _PRECISION_AUTO
    precision_used: str | None = None
    device_used: str | None = None

    # ``controlnet`` requests optional structural conditioning from the local
    # backend ("off" | "canny"); a backend that cannot honour the request raises
    # ``InpaintUnavailable`` so the run degrades to the provider (recorded
    # truthfully) instead of silently dropping the conditioning.
    controlnet_requested = (
        getattr(args, "controlnet", None) or _CONTROLNET_OFF
    ).strip().lower() or _CONTROLNET_OFF

    repainted: list[dict[str, Any]] = []
    skipped: list[dict[str, Any]] = []

    backend = None
    if engine_requested == _PROVIDER_ENGINE:
        engine_fallback_reason = "engine 'provider': remote image.edit owned by orchestrator"
    else:
        from inpaint_backends import resolve

        backend = resolve(engine_requested)
        if backend is None:
            engine_fallback_reason = f"unknown engine {engine_requested!r}"
        else:
            ok, reason = backend.available()
            if not ok:
                engine_fallback_reason = reason
                backend = None

    directory = Path((args.output_dir or "").strip() or ".")
    seed = args.seed if getattr(args, "seed", None) is not None and args.seed >= 0 else None

    if backend is not None:
        from inpaint_backends import InpaintUnavailable
        from PIL import Image

        directory.mkdir(parents=True, exist_ok=True)
        backend_model = Path(backend.weight_path()).name if hasattr(backend, "weight_path") else None
        for region in regions:
            if not isinstance(region, dict):
                continue
            index = int(region.get("index"))
            crop_path = region.get("crop_path")
            inner = region.get("inner_box")
            issue_type = region.get("type")
            if not (crop_path and Path(str(crop_path)).is_file()):
                skipped.append({"index": index, "type": issue_type, "reason": "missing_crop"})
                continue
            if not (isinstance(inner, list) and len(inner) == 4):
                skipped.append({"index": index, "type": issue_type, "reason": "bad_geometry"})
                continue

            crop_img = Image.open(str(crop_path)).convert("RGB")
            mask_arr = _inner_mask(crop_img.size, [int(v) for v in inner])
            mask_img = Image.fromarray(mask_arr, "L")
            region_prompt = str(prompt_map.get(str(issue_type)) or prompt)

            try:
                result, device_used, precision_used = backend.inpaint(
                    crop_img,
                    mask_img,
                    region_prompt,
                    negative_prompt=(args.negative_prompt or ""),
                    strength=float(args.strength),
                    guidance_scale=float(args.guidance_scale),
                    steps=int(args.steps),
                    seed=seed,
                    precision=precision_requested,
                    controlnet=controlnet_requested,
                )
            except InpaintUnavailable as err:
                engine_fallback_reason = err.reason
                repainted = []
                skipped = []
                backend = None
                device_used = None
                precision_used = None
                break
            except Exception as err:  # noqa: BLE001 - degrade to provider, never crash
                skipped.append(
                    {"index": index, "type": issue_type, "reason": f"{type(err).__name__}: {err}"}
                )
                continue

            stem = (args.output_name or "").strip() or _safe_stem(str(crop_path))
            out_path = directory / f"{stem}_region{index}_repainted.png"
            result.convert("RGBA").save(str(out_path), format="PNG")
            repainted.append({"index": index, "path": str(out_path)})

        if backend is not None and repainted:
            engine_used = backend.id

    return {
        "repainted": repainted,
        "skipped": skipped,
        "engine": engine_used,
        "engine_requested": engine_requested,
        "engine_fallback_reason": engine_fallback_reason,
        "backend_model": backend_model,
        "device": device_used,
        "precision": precision_used,
        "precision_requested": precision_requested,
        "controlnet_requested": controlnet_requested,
        "requested_count": len(regions),
        "repainted_count": len(repainted),
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Localized repaint pixel helper (crop/mask prepare + paste-back composite)."
    )
    parser.add_argument(
        "--probe-engines",
        dest="probe_engines",
        action="store_true",
        help="print local inpaint engine availability JSON and exit (UI capability probe)",
    )
    sub = parser.add_subparsers(dest="command", required=False)

    prep = sub.add_parser("prepare", help="crop issue regions + write inpaint masks")
    prep.add_argument("--image", required=True, help="path to the candidate image")
    prep.add_argument(
        "--quality-report",
        dest="quality_report",
        default="",
        help="inline QualityReport JSON from the Detail Watchdog node",
    )
    prep.add_argument(
        "--repaint-actions",
        dest="repaint_actions",
        default="",
        help="comma list of suggested_action values to repaint (default: detail_redraw)",
    )
    prep.add_argument(
        "--min-confidence",
        dest="min_confidence",
        type=float,
        default=0.0,
        help="only repaint issues at/above this confidence (0..1)",
    )
    prep.add_argument(
        "--padding",
        type=int,
        default=24,
        help="context padding (px) added around each issue bbox",
    )
    prep.add_argument(
        "--max-regions",
        dest="max_regions",
        type=int,
        default=8,
        help="cap how many regions are repainted (highest confidence first)",
    )
    prep.add_argument(
        "--invert-mask",
        dest="invert_mask",
        action="store_true",
        help="mark the edit area opaque/white instead of transparent",
    )
    prep.add_argument(
        "--max-decode-pixels",
        dest="max_decode_pixels",
        type=int,
        default=_DEFAULT_MAX_DECODE_PIXELS,
        help="reject input images larger than this many pixels (0 disables)",
    )
    prep.add_argument(
        "--output-dir",
        dest="output_dir",
        default="",
        help="directory for the crop + mask PNGs (default: cwd)",
    )
    prep.add_argument(
        "--output-name",
        dest="output_name",
        default="",
        help="base name for the written PNGs (default: <image>_repaint)",
    )

    comp = sub.add_parser("composite", help="paste repainted crops back with edge fusion")
    comp.add_argument("--image", required=True, help="path to the original candidate image")
    comp.add_argument(
        "--manifest",
        default="",
        help="inline manifest JSON returned by the prepare step",
    )
    comp.add_argument(
        "--repainted",
        default="",
        help="inline JSON list of {index, path} repainted crops",
    )
    comp.add_argument(
        "--feather-px",
        dest="feather_px",
        type=float,
        default=0.0,
        help="seam feather radius (0 = auto from the issue size)",
    )
    comp.add_argument(
        "--blend",
        default="feather",
        help=(
            "seam blend mode: feather (default) | poisson (gradient-domain "
            "seamless clone; falls back to feather on a too-small region)"
        ),
    )
    comp.add_argument(
        "--max-decode-pixels",
        dest="max_decode_pixels",
        type=int,
        default=_DEFAULT_MAX_DECODE_PIXELS,
        help="reject input images larger than this many pixels (0 disables)",
    )
    comp.add_argument(
        "--output-dir",
        dest="output_dir",
        default="",
        help="directory for the fixed image (default: cwd)",
    )
    comp.add_argument(
        "--output-name",
        dest="output_name",
        default="",
        help="base name for the fixed image (default: <image>_repainted)",
    )

    rep = sub.add_parser(
        "repaint",
        help="run a local inpaint engine over the manifest (opt-in; falls back to provider)",
    )
    rep.add_argument(
        "--manifest",
        default="",
        help="inline manifest JSON returned by the prepare step",
    )
    rep.add_argument(
        "--engine",
        default=_PROVIDER_ENGINE,
        help=(
            "inpaint engine: provider (default) | sd_inpaint | sdxl_inpaint | "
            "flux_fill (opt-in, falls back to provider)"
        ),
    )
    rep.add_argument(
        "--prompt",
        default="",
        help="repaint prompt applied to every region (per-type override via --prompt-map)",
    )
    rep.add_argument(
        "--prompt-map",
        dest="prompt_map",
        default="",
        help="inline JSON mapping issue type -> prompt (overrides --prompt per region)",
    )
    rep.add_argument(
        "--negative-prompt",
        dest="negative_prompt",
        default="",
        help="negative prompt passed to the backend",
    )
    rep.add_argument(
        "--strength",
        type=float,
        default=0.75,
        help="inpaint denoise strength 0..1 (lower preserves more of the source)",
    )
    rep.add_argument(
        "--guidance-scale",
        dest="guidance_scale",
        type=float,
        default=7.5,
        help="classifier-free guidance scale",
    )
    rep.add_argument(
        "--steps",
        type=int,
        default=30,
        help="number of inference steps",
    )
    rep.add_argument(
        "--seed",
        type=int,
        default=-1,
        help="random seed for reproducible inpaint (<0 = nondeterministic)",
    )
    rep.add_argument(
        "--precision",
        default=_PRECISION_AUTO,
        help=(
            "compute precision for the torch backend: auto (default, fp16 on cuda "
            "else fp32) | fp32 | fp16 (degrades to fp32 on a cpu run)"
        ),
    )
    rep.add_argument(
        "--controlnet",
        default=_CONTROLNET_OFF,
        help=(
            "structural conditioning for the sd_inpaint backend: off (default) | "
            "canny (edge-conditioned; needs the HGRIPE_CONTROLNET_MODEL weight)"
        ),
    )
    rep.add_argument(
        "--output-dir",
        dest="output_dir",
        default="",
        help="directory for the repainted crop PNGs (default: cwd)",
    )
    rep.add_argument(
        "--output-name",
        dest="output_name",
        default="",
        help="base name for the repainted crops (default: from each crop file)",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    if getattr(args, "probe_engines", False):
        from inpaint_backends import probe

        sys.stdout.write(json.dumps(probe(), ensure_ascii=False))
        return 0
    if not getattr(args, "command", None):
        parser.error("a subcommand is required (prepare | repaint | composite)")
    try:
        if args.command == "prepare":
            result = prepare(args)
        elif args.command == "repaint":
            result = repaint(args)
        else:
            result = composite(args)
    except Exception as err:  # noqa: BLE001 - surface a single clean error line
        sys.stderr.write(f"{type(err).__name__}: {err}\n")
        return 1
    sys.stdout.write(json.dumps(result, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
