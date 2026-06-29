"""Headless PSD context analysis for the H-Gripe desktop node editor.

This is the thin, ``torch``-free entry point behind the desktop
``analyze_psd_context`` Tauri command -- the backend of the **PSD Context
Analyze** node, the first node of the PSD-first production chain. It reads a PSD
*template* and distils it into a machine-usable ``VisualContext``: background
colour / lighting heuristics, the target placeholder's geometry (+ an inset
"safe area"), a written placeholder mask and background preview PNG, and a
ready-to-append ``prompt_suffix`` describing the template's light & colour. The
goal is that downstream nodes (Light & Color Match, etc.) -- and the user -- no
longer have to hand-describe the template's lighting.

Phase 1 is deliberately heuristic and dependency-light: it uses only the
vendored ``psd_tools`` + ``Pillow`` (no local VLM, no OpenCV). It reuses
``HGripePsdCompose._resolve_placeholder`` / ``_find_layer`` from
``custom_nodes/hgripe_psd_nodes.py`` so placeholder + layer resolution stays a
single source of truth with the ComfyUI nodes and the other bridge CLIs.

The emitted JSON object matches the ``VisualContext`` contract defined once in
``apps/desktop-tauri/src-tauri/src/contracts.rs`` and mirrored in
``apps/desktop-tauri/studio-ui/src/types/production.ts``. On failure the process
exits non-zero with a message on stderr.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any

# Resolve the repo root (this file lives at <root>/python/bridge/) and make both
# the root (for ``custom_nodes``) and the vendored ``third_party`` importable,
# exactly like the other bridge CLIs do.
_ROOT_DIR = Path(__file__).resolve().parents[2]
for _candidate in (_ROOT_DIR, _ROOT_DIR / "third_party"):
    if _candidate.is_dir() and str(_candidate) not in sys.path:
        sys.path.insert(0, str(_candidate))

# These helpers import cleanly without torch (heavy imports inside
# hgripe_psd_nodes are deferred to call time), so reusing them keeps placeholder
# + layer resolution a single source of truth with the ComfyUI nodes.
from custom_nodes.hgripe_psd_nodes import (  # noqa: E402
    HGripePsdCompose,
    _find_layer,
)

# The 3x3 grid cells, row-major, mapped to a light-direction label.
_DIRECTIONS = [
    "top-left",
    "top",
    "top-right",
    "left",
    "center",
    "right",
    "bottom-left",
    "bottom",
    "bottom-right",
]


def _safe_stem(template_path: str) -> str:
    """A filesystem-safe base name derived from the template file stem."""
    stem = Path(template_path).stem or "template"
    cleaned = "".join(ch if ch.isalnum() or ch in ("-", "_") else "_" for ch in stem)
    return cleaned or "template"


def _hex(rgb: tuple[int, int, int]) -> str:
    return "#{:02x}{:02x}{:02x}".format(*(max(0, min(255, int(c))) for c in rgb))


def _dominant_palette(rgb_image: Any, count: int = 5) -> list[str]:
    """Top ``count`` colours via median-cut quantisation, most frequent first."""
    small = rgb_image.resize((128, 128))
    quantised = small.quantize(colors=max(2, count))
    palette = quantised.getpalette() or []
    colors = quantised.getcolors() or []  # list of (pixel_count, palette_index)
    ordered = sorted(colors, key=lambda item: item[0], reverse=True)
    result: list[str] = []
    for _, index in ordered[:count]:
        base = index * 3
        rgb = (palette[base], palette[base + 1], palette[base + 2])
        result.append(_hex(rgb))
    return result


def _light_direction(gray_image: Any) -> tuple[str, float]:
    """Brightest cell of a 3x3 downscale -> direction label + normalised spread.

    The spread (max-min cell luminance, 0-1) doubles as a hardness cue: a strong
    bright/dark split implies a harder, more directional key light.
    """
    grid = gray_image.resize((3, 3))
    # Row-major (top-left first) to match _DIRECTIONS.
    cells = [float(grid.getpixel((x, y))) for y in range(3) for x in range(3)]
    if not cells:
        return "center", 0.0
    brightest = max(range(len(cells)), key=lambda i: cells[i])
    spread = (max(cells) - min(cells)) / 255.0
    # Near-uniform luminance reads as flat/ambient rather than directional.
    direction = "center" if spread < 0.08 else _DIRECTIONS[brightest]
    return direction, spread


def _color_temperature(mean_rgb: list[float]) -> int:
    """Rough correlated colour temperature from the red/blue balance.

    Equal R/B reads ~6500K (neutral daylight); a red-heavy image trends warm
    (lower K), a blue-heavy image trends cool (higher K). Clamped to a sane
    photographic range and rounded to the nearest 100K -- a heuristic, not a
    calibrated measurement.
    """
    red = mean_rgb[0] + 1.0
    blue = mean_rgb[2] + 1.0
    kelvin = 2000.0 + (blue / red) * 4500.0
    kelvin = max(2000.0, min(12000.0, kelvin))
    return int(round(kelvin / 100.0) * 100)


def _warmth_label(kelvin: int) -> str:
    if kelvin < 4500:
        return "warm"
    if kelvin > 6500:
        return "cool"
    return "neutral"


def _select_background(psd: Any, background_layer: str) -> Any:
    """Composite the named background layer if given/found, else the whole PSD."""
    name = (background_layer or "").strip()
    if name:
        found = _find_layer(psd, name)
        if found is not None:
            layer, _parent, _index = found
            composed = layer.composite()
            if composed is not None:
                return composed.convert("RGB")
    return psd.composite().convert("RGB")


def analyze(args: argparse.Namespace) -> dict[str, Any]:
    from PIL import Image, ImageDraw, ImageStat

    from psd_tools import PSDImage

    template_path = (args.template or "").strip()
    if not template_path or not Path(template_path).is_file():
        raise FileNotFoundError(f"PSD template not found: {template_path}")

    psd = PSDImage.open(template_path)
    canvas_w, canvas_h = int(psd.width), int(psd.height)

    # --- Placeholder geometry (reuse the node's resolver). Empty name -> the
    # whole canvas, so the node still produces a usable context unconfigured.
    target_name = (args.target_placeholder or "").strip()
    plan = {"name": target_name} if target_name else {}
    left, top, box_w, box_h, _layer, _parent, _index = HGripePsdCompose()._resolve_placeholder(
        psd, plan
    )
    margin_x = int(round(box_w * 0.05))
    margin_y = int(round(box_h * 0.05))
    safe_area = {
        "x": left + margin_x,
        "y": top + margin_y,
        "width": max(0, box_w - 2 * margin_x),
        "height": max(0, box_h - 2 * margin_y),
    }

    # --- Background appearance heuristics.
    background = _select_background(psd, args.background_layer)
    gray = background.convert("L")
    rgb_stat = ImageStat.Stat(background)
    gray_stat = ImageStat.Stat(gray)
    mean_rgb = [float(channel) for channel in rgb_stat.mean[:3]]
    mean_color = [int(round(channel)) for channel in mean_rgb]
    brightness = round(gray_stat.mean[0] / 255.0, 4)
    contrast = round(min(1.0, gray_stat.stddev[0] / 128.0), 4)

    palette = _dominant_palette(background)
    color_temperature = _color_temperature(mean_rgb)
    direction, spread = _light_direction(gray)
    # Hard light: either a wide luminance spread or globally high contrast.
    quality = "hard" if (spread >= 0.35 or contrast >= 0.45) else "soft"
    warmth = _warmth_label(color_temperature)
    description = (
        f"{warmth} background with {quality} key light from {direction}, "
        f"color temperature {color_temperature}k"
    )
    prompt_suffix = (
        f"matched with the PSD background lighting: {quality} key light from {direction}, "
        f"{warmth} background, color temperature {color_temperature}k, "
        "realistic contact shadow, consistent highlight direction, no floating object"
    )

    # --- Written artifacts (node outputs): placeholder mask + background preview.
    directory = Path((args.output_dir or "").strip() or ".")
    directory.mkdir(parents=True, exist_ok=True)
    stem = _safe_stem(template_path)

    mask = Image.new("L", (canvas_w, canvas_h), 0)
    if box_w > 0 and box_h > 0:
        ImageDraw.Draw(mask).rectangle(
            [left, top, left + box_w - 1, top + box_h - 1], fill=255
        )
    mask_path = directory / f"{stem}_placeholder_mask.png"
    mask.save(str(mask_path), format="PNG")

    background_path = directory / f"{stem}_background.png"
    background.save(str(background_path), format="PNG")

    return {
        "background": {
            "mean_color": mean_color,
            "dominant_palette": palette,
            "brightness": brightness,
            "contrast": contrast,
            "histogram_path": None,
            "image_path": str(background_path),
        },
        "lighting": {
            "direction": direction,
            "quality": quality,
            "color_temperature": color_temperature,
            "description": description,
        },
        "placeholder": {
            "layer_name": target_name,
            "bounds": {"x": left, "y": top, "width": box_w, "height": box_h},
            "mask_path": str(mask_path),
            "safe_area": safe_area,
        },
        "prompt_suffix": prompt_suffix,
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Analyze a PSD template into a structured VisualContext."
    )
    parser.add_argument("--template", required=True, help="path to the .psd template")
    parser.add_argument(
        "--background-layer",
        dest="background_layer",
        default="",
        help="background layer name to sample (empty: composite the whole PSD)",
    )
    parser.add_argument(
        "--target-placeholder",
        dest="target_placeholder",
        default="",
        help="placeholder layer name (empty: use the whole canvas)",
    )
    parser.add_argument(
        "--reference-layers",
        dest="reference_layers",
        default="[]",
        help="JSON array of reference layer names (advisory in Phase 1)",
    )
    parser.add_argument(
        "--output-dir",
        dest="output_dir",
        default="",
        help="directory for the written mask + background preview (default: cwd)",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        result = analyze(args)
    except Exception as err:  # noqa: BLE001 - surface a single clean error line
        sys.stderr.write(f"{type(err).__name__}: {err}\n")
        return 1
    sys.stdout.write(json.dumps(result, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
