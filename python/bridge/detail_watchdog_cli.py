"""Headless quality watchdog for the H-Gripe desktop node editor.

This is the thin, ``torch``-free entry point behind the desktop
``detect_quality_issues`` Tauri command -- the backend of the **Detail
Watchdog** node, the fifth node of the PSD-first production chain. It scans a
candidate image for local breakdowns (blur, halos, colour mismatch, missing
resolution) and emits a structured :class:`QualityReport` so the workflow can
decide whether to re-run or hand-fix a region before composing into the PSD.

Phase 1 is deliberately **detect + report only** (no automatic repaint) and
dependency-light -- only the vendored ``Pillow`` + ``numpy`` (no OpenCV, no
ML). That bounds what can be detected honestly on the CPU:

* ``low_resolution`` -- global Laplacian-variance blur and/or the image being
  smaller than the connected PSD placeholder bounds (needs upscaling).
* ``face_blur`` -- soft local regions found by a per-tile sharpness grid and
  merged into bounding boxes (reported as ``face_blur`` when ``face`` is a
  watch target, otherwise ``low_resolution``).
* ``edge_halo`` -- a bright fringe on the semi-transparent alpha rim of a
  cut-out subject (residual white/old-background matte).
* ``color_mismatch`` -- the subject's mean colour drifting from the connected
  ``visual_context`` background colour.

Semantic detection of hands, packaging text and logo deformation needs the
later GPU/VLM backend and is intentionally **not** attempted here; those watch
targets are recorded as skipped in the report rather than guessed at.

The emitted JSON is ``{"fixed_image", "quality_report", "issue_masks"}`` where
``fixed_image`` is the unchanged input (Phase 1 never repaints), the report
follows the shared ``QualityReport`` contract, and ``issue_masks`` is an
optional overlay PNG highlighting the flagged boxes. On failure the process
exits non-zero with a single message on stderr.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any

import numpy as np

_EPS = 1e-6

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
# Watch targets that the CPU Phase-1 heuristics cannot honestly detect.
_UNSUPPORTED_TARGETS = ("hands", "text", "logo")


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
    skipped = sorted(watch_set & set(_UNSUPPORTED_TARGETS))

    visual_context = _load_json_arg(args.visual_context, "visual_context")
    target_bounds = _load_json_arg(args.target_bounds, "target_bounds")
    target = _resolve_target(visual_context, target_bounds)
    background_mean = _background_mean(visual_context)

    rgb, alpha = _load_rgb_alpha(image_path)
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
    try:
        result = watch(args)
    except Exception as err:  # noqa: BLE001 - surface a single clean error line
        sys.stderr.write(f"{type(err).__name__}: {err}\n")
        return 1
    sys.stdout.write(json.dumps(result, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
