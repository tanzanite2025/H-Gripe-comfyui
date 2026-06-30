"""Headless quality watchdog for the H-Gripe desktop node editor.

This is the thin, ``torch``-free entry point behind the desktop
``detect_quality_issues`` Tauri command -- the backend of the **Detail
Watchdog** node, the fifth node of the PSD-first production chain. It scans a
candidate image for local breakdowns (blur, halos, colour mismatch, missing
resolution) and emits a structured :class:`QualityReport` so the workflow can
decide whether to re-run or hand-fix a region before composing into the PSD.

Phase 1 is deliberately **detect + report only** (no automatic repaint) and
dependency-light -- only the vendored ``Pillow`` + ``numpy`` (no OpenCV, no
ML). The decode is **input-hardened** so the heuristics see honest 8-bit RGB on
real production assets: CMYK (via its embedded ICC profile when present),
16-bit / float, palette and grayscale sources are normalised to an 8-bit RGB
working space, the alpha channel is taken from the source (or a fully-opaque
plane), EXIF orientation is applied, and an input larger than
``--max-decode-pixels`` is refused before it is decoded so a crafted/huge image
cannot exhaust memory. The resolved ``source_mode`` and ``exif_transposed`` are
recorded in ``watchdog_report``.

The optional ``--mask`` is **advisory only** in Phase 1 (``mask_consumed`` is
``false`` in the report); detection runs on the image's own alpha rim. That
bounds what can be detected honestly on the CPU:

* ``low_resolution`` -- global Laplacian-variance blur and/or the image being
  smaller than the connected PSD placeholder bounds (needs upscaling).
* ``face_blur`` -- soft local regions found by a per-tile sharpness grid and
  merged into bounding boxes (reported as ``face_blur`` when ``face`` is a
  watch target, otherwise ``low_resolution``).
* ``edge_halo`` -- a bright fringe on the semi-transparent alpha rim of a
  cut-out subject (residual white/old-background matte).
* ``color_mismatch`` -- the subject's mean colour drifting from the connected
  ``visual_context`` background colour.

Semantic detection of hands, packaging text and logo deformation needs a
learned detector. The rule layer never attempts them on its own; those watch
targets are recorded as skipped in the report rather than guessed at. They are
graduated to real findings by an **opt-in** ML detector behind the ``--engine``
seam (``detector_backends/``): ``--engine onnx_defect`` runs a learned detector
when its optional dep (``onnxruntime``) and weight are present, merging its
findings on top of the rule findings; otherwise the rule layer runs alone and
the report records why (``engine_fallback_reason``). ``rules`` stays the default
and the always-available baseline, so the node never hard-fails on a box without
the model. ``--probe-engines`` prints which engines are usable so the UI can
grey out unavailable ones.

The emitted JSON is ``{"fixed_image", "quality_report", "issue_masks"}`` where
``fixed_image`` is the unchanged input (Phase 1 never repaints), the report
follows the shared ``QualityReport`` contract, and ``issue_masks`` is an
optional overlay PNG highlighting the flagged boxes. On failure the process
exits non-zero with a single message on stderr.
"""

from __future__ import annotations

import argparse
import io
import json
import sys
from pathlib import Path
from typing import Any

import numpy as np

_EPS = 1e-6

# Refuse to decode an input larger than this many pixels (decompression-bomb
# guard). 0 disables the check.
_DEFAULT_MAX_DECODE_PIXELS = 96_000_000
_ALPHA_MODES = {"RGBA", "LA", "La", "PA"}
_HIGHBIT_MODES = {"I", "I;16", "I;16B", "I;16L", "I;16N", "F"}
# EXIF tag holding the orientation (1 = normal, 2..8 = a flip/rotation).
_EXIF_ORIENTATION_TAG = 0x0112

# Per-mode detection thresholds. ``strict`` flags more aggressively, ``lenient``
# only the obvious breakdowns; ``balanced`` is the default middle ground.
_MODES: dict[str, dict[str, float]] = {
    "strict": {
        "blur_floor": 120.0,
        "region_ratio": 0.6,
        "region_floor": 90.0,
        "halo_delta": 0.10,
        "color_delta": 28.0,
    },
    "balanced": {
        "blur_floor": 80.0,
        "region_ratio": 0.45,
        "region_floor": 60.0,
        "halo_delta": 0.16,
        "color_delta": 40.0,
    },
    "lenient": {
        "blur_floor": 50.0,
        "region_ratio": 0.3,
        "region_floor": 35.0,
        "halo_delta": 0.24,
        "color_delta": 55.0,
    },
}

_ALL_TARGETS = ("face", "hands", "text", "logo", "product_edges")
# Watch targets that the CPU rule layer cannot honestly detect on its own; an
# opt-in ML detector (``--engine``) graduates the ones it covers out of this set.
_UNSUPPORTED_TARGETS = ("hands", "text", "logo")

# The always-available rule layer is the default "engine"; learned detectors
# register in ``detector_backends`` and are selected by ``--engine``.
_RULES_ENGINE = "rules"


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
    except Exception:  # noqa: BLE001 - a broken EXIF block must not abort detection
        orientation = 1
    if orientation in (None, 0, 1):
        return img, False
    try:
        fixed = ImageOps.exif_transpose(img)
    except Exception:  # noqa: BLE001 - fall back to the un-rotated image
        return img, False
    return fixed, fixed is not img


def _load_rgb_alpha(
    path: str, max_decode_pixels: int = _DEFAULT_MAX_DECODE_PIXELS
) -> tuple["np.ndarray", "np.ndarray", str, bool]:
    """Load an image as (H,W,3 uint8 RGB, H,W float alpha in 0..1, source_mode, exif_transposed).

    Refuses oversized inputs before decoding, applies EXIF orientation, and maps
    CMYK / high-bit / palette / grayscale sources into an 8-bit RGB working
    space so the sharpness / halo / colour heuristics sample honest luminance
    and colour. The alpha channel is taken from the source when present, else a
    fully-opaque plane.
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

    source_mode = img.mode
    had_alpha = source_mode in _ALPHA_MODES or (
        source_mode == "P" and "transparency" in img.info
    )

    if had_alpha:
        rgba = img.convert("RGBA")
        alpha = np.asarray(rgba.getchannel("A"), dtype=np.float32) / 255.0
        rgb = np.asarray(rgba.convert("RGB"), dtype=np.uint8).copy()
    else:
        if source_mode == "CMYK":
            rgb_img = _cmyk_to_rgb(img)
        elif source_mode in _HIGHBIT_MODES:
            rgb_img = _highbit_to_rgb(img)
        else:
            rgb_img = img.convert("RGB")
        rgb = np.asarray(rgb_img, dtype=np.uint8).copy()
        alpha = np.ones(rgb.shape[:2], dtype=np.float32)
    return rgb, alpha, source_mode, transposed


def _luminance(rgb: "np.ndarray") -> "np.ndarray":
    """Rec.601 luminance as a float (H,W) array on the 0..255 scale."""
    return rgb.astype(np.float32) @ np.array([0.299, 0.587, 0.114], np.float32)


def _laplacian(lum: "np.ndarray") -> "np.ndarray":
    """4-neighbour Laplacian high-pass response (edge replicated)."""
    padded = np.pad(lum, 1, mode="edge")
    return (
        padded[:-2, 1:-1]
        + padded[2:, 1:-1]
        + padded[1:-1, :-2]
        + padded[1:-1, 2:]
        - 4.0 * lum
    )


def _confidence(value: float, threshold: float, span: float) -> float:
    """Map how far ``value`` crosses ``threshold`` to a 0.5..0.95 confidence."""
    over = (value - threshold) / max(span, _EPS)
    return round(float(min(0.95, max(0.5, 0.5 + 0.45 * over))), 2)


def _bbox_from_mask(mask: "np.ndarray") -> list[int] | None:
    """Tight ``[x1, y1, x2, y2]`` around the True pixels, or None when empty."""
    rows = np.any(mask, axis=1)
    cols = np.any(mask, axis=0)
    if not rows.any() or not cols.any():
        return None
    y1, y2 = np.where(rows)[0][[0, -1]]
    x1, x2 = np.where(cols)[0][[0, -1]]
    return [int(x1), int(y1), int(x2) + 1, int(y2) + 1]


def _label_grid(flagged: "np.ndarray") -> list[list[tuple[int, int]]]:
    """4-connected components over a small boolean tile grid (flood fill)."""
    rows, cols = flagged.shape
    seen = np.zeros_like(flagged, dtype=bool)
    components: list[list[tuple[int, int]]] = []
    for r in range(rows):
        for c in range(cols):
            if not flagged[r, c] or seen[r, c]:
                continue
            stack = [(r, c)]
            seen[r, c] = True
            cells: list[tuple[int, int]] = []
            while stack:
                cr, cc = stack.pop()
                cells.append((cr, cc))
                for dr, dc in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                    nr, nc = cr + dr, cc + dc
                    if 0 <= nr < rows and 0 <= nc < cols and flagged[nr, nc] and not seen[nr, nc]:
                        seen[nr, nc] = True
                        stack.append((nr, nc))
            components.append(cells)
    return components


def _detect_low_resolution(
    lap: "np.ndarray",
    size: tuple[int, int],
    target: tuple[int, int] | None,
    thresholds: dict[str, float],
) -> dict[str, Any] | None:
    """Global blur (Laplacian variance) and/or below the target placeholder size."""
    width, height = size
    sharpness = float(lap.var())
    floor = thresholds["blur_floor"]
    too_small = target is not None and (width < target[0] * 0.9 or height < target[1] * 0.9)
    if sharpness >= floor and not too_small:
        return None
    if too_small and target is not None:
        action = "image_enhance"
        confidence = round(
            float(min(0.95, max(0.5, 0.5 + 0.45 * (1.0 - min(width / target[0], height / target[1]))))),
            2,
        )
    else:
        action = "image_enhance"
        confidence = _confidence(floor - sharpness, 0.0, floor)
    return {
        "type": "low_resolution",
        "confidence": confidence,
        "bbox": [0, 0, width, height],
        "suggested_action": action,
    }


def _detect_soft_regions(
    lap: "np.ndarray",
    watch: set[str],
    thresholds: dict[str, float],
) -> list[dict[str, Any]]:
    """Per-tile sharpness grid -> merged boxes for locally soft areas."""
    height, width = lap.shape
    cols = 8
    rows = max(1, min(8, int(round(cols * height / max(width, 1)))))
    ys = np.linspace(0, height, rows + 1, dtype=int)
    xs = np.linspace(0, width, cols + 1, dtype=int)

    tile_sharp = np.zeros((rows, cols), dtype=np.float32)
    for r in range(rows):
        for c in range(cols):
            tile = lap[ys[r] : ys[r + 1], xs[c] : xs[c + 1]]
            tile_sharp[r, c] = float(tile.var()) if tile.size else 0.0

    median = float(np.median(tile_sharp))
    if median <= _EPS:
        return []
    flagged = (tile_sharp < thresholds["region_ratio"] * median) & (
        tile_sharp < thresholds["region_floor"]
    )
    if not flagged.any():
        return []

    upper_half = "face" in watch
    issue_type = "face_blur" if "face" in watch else "low_resolution"
    issues: list[dict[str, Any]] = []
    for cells in _label_grid(flagged):
        r1 = min(r for r, _ in cells)
        r2 = max(r for r, _ in cells)
        c1 = min(c for _, c in cells)
        c2 = max(c for _, c in cells)
        # When watching faces, prefer regions that overlap the upper portion of
        # the frame (where faces usually sit); skip purely-bottom soft areas.
        if upper_half and r1 > rows // 2:
            continue
        region = tile_sharp[r1 : r2 + 1, c1 : c2 + 1]
        sharp = float(region.mean())
        issues.append(
            {
                "type": issue_type,
                "confidence": _confidence(
                    thresholds["region_floor"] - sharp, 0.0, thresholds["region_floor"]
                ),
                "bbox": [int(xs[c1]), int(ys[r1]), int(xs[c2 + 1]), int(ys[r2 + 1])],
                "suggested_action": "detail_redraw",
            }
        )
    return issues


def _detect_edge_halo(
    lum: "np.ndarray",
    alpha: "np.ndarray",
    thresholds: dict[str, float],
) -> dict[str, Any] | None:
    """Bright fringe on the semi-transparent rim of a cut-out subject."""
    rim = (alpha > 0.05) & (alpha < 0.95)
    interior = alpha >= 0.95
    if rim.sum() < 16 or interior.sum() < 16:
        return None
    norm = lum / 255.0
    rim_brightness = float(norm[rim].mean())
    interior_brightness = float(norm[interior].mean())
    delta = rim_brightness - interior_brightness
    if delta < thresholds["halo_delta"]:
        return None
    bbox = _bbox_from_mask(alpha > 0.05)
    if bbox is None:
        return None
    return {
        "type": "edge_halo",
        "confidence": _confidence(delta, thresholds["halo_delta"], 0.4),
        "bbox": bbox,
        "suggested_action": "edge_refine",
    }


def _detect_color_mismatch(
    rgb: "np.ndarray",
    alpha: "np.ndarray",
    background_mean: list[float] | None,
    thresholds: dict[str, float],
) -> dict[str, Any] | None:
    """Subject mean colour drifting from the connected background colour."""
    if background_mean is None or len(background_mean) < 3:
        return None
    subject = alpha >= 0.5
    pixels = rgb[subject] if subject.any() else rgb.reshape(-1, 3)
    subject_mean = pixels.astype(np.float32).mean(axis=0)
    bg = np.array(background_mean[:3], dtype=np.float32)
    delta = float(np.linalg.norm(subject_mean - bg))
    if delta < thresholds["color_delta"]:
        return None
    bbox = _bbox_from_mask(subject) or [0, 0, rgb.shape[1], rgb.shape[0]]
    return {
        "type": "color_mismatch",
        "confidence": _confidence(delta, thresholds["color_delta"], 80.0),
        "bbox": bbox,
        "suggested_action": "color_match",
    }


def _status(issues: list[dict[str, Any]]) -> str:
    """Aggregate issues into ``passed | warning | failed``."""
    if not issues:
        return "passed"
    if len(issues) >= 3 or any(i["confidence"] >= 0.85 for i in issues):
        return "failed"
    return "warning"


def _write_issue_overlay(
    rgb: "np.ndarray", issues: list[dict[str, Any]], path: Path
) -> None:
    """Draw red boxes around each flagged region for UI / PSD review."""
    from PIL import Image, ImageDraw

    img = Image.fromarray(rgb, "RGB").convert("RGBA")
    draw = ImageDraw.Draw(img)
    for issue in issues:
        x1, y1, x2, y2 = issue["bbox"]
        draw.rectangle([x1, y1, max(x1, x2 - 1), max(y1, y2 - 1)], outline=(255, 64, 64, 255), width=3)
    img.save(str(path), format="PNG")


def _resolve_target(
    visual_context: dict[str, Any] | None, target_bounds: dict[str, Any] | None
) -> tuple[int, int] | None:
    """Target placeholder pixel size from explicit bounds or visual_context."""
    bounds = target_bounds
    if bounds is None and visual_context is not None:
        placeholder = visual_context.get("placeholder") or {}
        bounds = placeholder.get("bounds")
    if not isinstance(bounds, dict):
        return None
    width = int(bounds.get("width") or 0)
    height = int(bounds.get("height") or 0)
    if width <= 0 or height <= 0:
        return None
    return width, height


def _background_mean(visual_context: dict[str, Any] | None) -> list[float] | None:
    """The connected background mean RGB colour, when available."""
    if visual_context is None:
        return None
    background = visual_context.get("background") or {}
    mean = background.get("mean_color")
    if isinstance(mean, list) and len(mean) >= 3:
        return [float(v) for v in mean[:3]]
    return None


def _load_json_arg(raw: str | None, label: str) -> dict[str, Any] | None:
    """Parse an inline JSON object argument, raising on malformed input."""
    text = (raw or "").strip()
    if not text:
        return None
    try:
        parsed = json.loads(text)
    except json.JSONDecodeError as err:
        raise ValueError(f"invalid {label} JSON: {err}") from err
    return parsed if isinstance(parsed, dict) else None


def _run_engine(
    engine_requested: str,
    rgb: "np.ndarray",
    alpha: "np.ndarray",
    watch_set: set[str],
) -> dict[str, Any]:
    """Run the opt-in ML detector for ``engine_requested`` (or the rule-only path).

    Returns the dispatch telemetry plus any merged issues. Any unavailability
    (unknown engine, missing deps / weight, runtime error) degrades to the
    rule-only report and records ``fallback_reason`` -- the node never crashes
    on a box without the model.
    """
    result: dict[str, Any] = {
        "engine": _RULES_ENGINE,
        "fallback_reason": None,
        "detectors": [],
        "backend_model": None,
        "issues": [],
        "covered": set(),
    }
    if engine_requested == _RULES_ENGINE:
        return result

    from detector_backends import DetectorUnavailable, resolve

    backend = resolve(engine_requested)
    if backend is None:
        result["fallback_reason"] = f"unknown engine {engine_requested!r}"
        return result

    ok, reason = backend.available()
    if not ok:
        result["fallback_reason"] = reason
        return result

    try:
        issues = backend.detect(rgb, alpha, watch_set)
    except DetectorUnavailable as err:
        result["fallback_reason"] = err.reason
        return result
    except Exception as err:  # noqa: BLE001 - degrade to rules, never crash
        result["fallback_reason"] = f"{type(err).__name__}: {err}"
        return result

    result["engine"] = backend.id
    result["detectors"] = [backend.id]
    result["issues"] = issues
    # Targets the backend honestly covers (whether or not it found a defect
    # there) graduate out of ``skipped_targets``. Prefer ``covered_targets()``
    # when the backend reports what its *loaded weight* can actually detect, so
    # a partial weight does not claim targets it cannot find; fall back to the
    # static capability set otherwise.
    covered_fn = getattr(backend, "covered_targets", None)
    covered = covered_fn() if callable(covered_fn) else getattr(backend, "targets", ())
    result["covered"] = set(covered) & watch_set
    try:
        from pathlib import Path as _Path

        result["backend_model"] = _Path(backend.weight_path()).name
    except Exception:  # noqa: BLE001 - the model name is best-effort telemetry
        result["backend_model"] = None
    return result


def watch(args: argparse.Namespace) -> dict[str, Any]:
    image_path = (args.image or "").strip()
    if not image_path or not Path(image_path).is_file():
        raise FileNotFoundError(f"candidate image not found: {image_path}")

    mode = (args.mode or "balanced").strip()
    if mode not in _MODES:
        raise ValueError(f"unknown mode {mode!r}; expected one of {sorted(_MODES)}")
    thresholds = _MODES[mode]

    raw_targets = [t.strip() for t in (args.watch_targets or "").split(",") if t.strip()]
    watch_set = set(raw_targets) if raw_targets else set(_ALL_TARGETS)
    unknown = sorted(watch_set - set(_ALL_TARGETS))
    if unknown:
        raise ValueError(f"unknown watch target(s): {unknown}; expected {list(_ALL_TARGETS)}")

    engine_requested = (getattr(args, "engine", _RULES_ENGINE) or _RULES_ENGINE).strip().lower() or _RULES_ENGINE

    max_decode_pixels = int(max(0, getattr(args, "max_decode_pixels", _DEFAULT_MAX_DECODE_PIXELS)))

    visual_context = _load_json_arg(args.visual_context, "visual_context")
    target_bounds = _load_json_arg(args.target_bounds, "target_bounds")
    target = _resolve_target(visual_context, target_bounds)
    background_mean = _background_mean(visual_context)

    rgb, alpha, source_mode, exif_transposed = _load_rgb_alpha(image_path, max_decode_pixels)
    height, width = rgb.shape[:2]
    lum = _luminance(rgb)
    lap = _laplacian(lum)

    issues: list[dict[str, Any]] = []
    low_res = _detect_low_resolution(lap, (width, height), target, thresholds)
    if low_res is not None:
        issues.append(low_res)
    issues.extend(_detect_soft_regions(lap, watch_set, thresholds))
    if "product_edges" in watch_set:
        halo = _detect_edge_halo(lum, alpha, thresholds)
        if halo is not None:
            issues.append(halo)
    mismatch = _detect_color_mismatch(rgb, alpha, background_mean, thresholds)
    if mismatch is not None:
        issues.append(mismatch)

    # Opt-in ML pass: merges semantic findings (hands/text/logo) on top of the
    # rule findings and graduates the targets it covers out of skipped_targets.
    engine = _run_engine(engine_requested, rgb, alpha, watch_set)
    issues.extend(engine["issues"])
    skipped = sorted((watch_set & set(_UNSUPPORTED_TARGETS)) - engine["covered"])

    quality_report = {
        "status": _status(issues),
        "issues": issues,
    }

    issue_masks: str | None = None
    if issues and not args.no_overlay:
        directory = Path((args.output_dir or "").strip() or ".")
        directory.mkdir(parents=True, exist_ok=True)
        stem = (args.output_name or "").strip() or f"{_safe_stem(image_path)}_issues"
        overlay_path = directory / f"{stem}.png"
        _write_issue_overlay(rgb, issues, overlay_path)
        issue_masks = str(overlay_path)

    return {
        # Phase 1 is detect-only: the candidate is returned unchanged.
        "fixed_image": image_path,
        "quality_report": quality_report,
        "issue_masks": issue_masks,
        "watchdog_report": {
            "mode": mode,
            "watch_targets": sorted(watch_set),
            "skipped_targets": skipped,
            "image_size": [width, height],
            "target_size": list(target) if target else None,
            "global_sharpness": round(float(lap.var()), 2),
            "source_mode": source_mode,
            "exif_transposed": exif_transposed,
            "max_decode_pixels": max_decode_pixels,
            # The optional --mask is advisory in Phase 1; detection runs on the
            # image's own alpha rim, so the supplied matte is not consumed.
            "mask_consumed": False,
            # ML detector seam telemetry. ``engine`` is what actually ran
            # (``rules`` when the requested engine was unavailable);
            # ``detectors`` lists the learned passes that ran on top of rules.
            "engine": engine["engine"],
            "engine_requested": engine_requested,
            "engine_fallback_reason": engine["fallback_reason"],
            "detectors": engine["detectors"],
            "backend_model": engine["backend_model"],
        },
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Detect local quality issues in a candidate image (detect + report)."
    )
    parser.add_argument("--image", required=True, help="path to the candidate image")
    parser.add_argument(
        "--mask", default="", help="optional subject matte (advisory in Phase 1)"
    )
    parser.add_argument(
        "--visual-context",
        dest="visual_context",
        default="",
        help="inline VisualContext JSON (background colour + placeholder bounds)",
    )
    parser.add_argument(
        "--target-bounds",
        dest="target_bounds",
        default="",
        help="inline placeholder bounds JSON {x,y,width,height}",
    )
    parser.add_argument(
        "--watch-targets",
        dest="watch_targets",
        default="",
        help="comma list: face,hands,text,logo,product_edges (default: all)",
    )
    parser.add_argument(
        "--mode", default="balanced", help="strict | balanced | lenient"
    )
    parser.add_argument(
        "--engine",
        default=_RULES_ENGINE,
        help="detection engine: rules (default) | onnx_defect (opt-in ML, falls back to rules)",
    )
    parser.add_argument(
        "--probe-engines",
        dest="probe_engines",
        action="store_true",
        help="print engine availability JSON and exit (UI capability probe)",
    )
    parser.add_argument(
        "--max-decode-pixels",
        dest="max_decode_pixels",
        type=int,
        default=_DEFAULT_MAX_DECODE_PIXELS,
        help="reject input images larger than this many pixels (0 disables)",
    )
    parser.add_argument(
        "--no-overlay",
        dest="no_overlay",
        action="store_true",
        help="skip writing the issue overlay PNG",
    )
    parser.add_argument(
        "--output-dir",
        dest="output_dir",
        default="",
        help="directory for the issue overlay PNG (default: cwd)",
    )
    parser.add_argument(
        "--output-name",
        dest="output_name",
        default="",
        help="base name for the overlay PNG (default: <image>_issues)",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    if getattr(args, "probe_engines", False):
        from detector_backends import probe

        sys.stdout.write(json.dumps(probe(), ensure_ascii=False))
        return 0
    try:
        result = watch(args)
    except Exception as err:  # noqa: BLE001 - surface a single clean error line
        sys.stderr.write(f"{type(err).__name__}: {err}\n")
        return 1
    sys.stdout.write(json.dumps(result, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
