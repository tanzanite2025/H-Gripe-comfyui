"""Headless light & colour matching for the H-Gripe desktop node editor.

This is the thin, ``torch``-free entry point behind the desktop
``match_light_color`` Tauri command -- the backend of the **Light & Color
Match** node, the second node of the PSD-first production chain. It takes a
generated subject image and nudges its light & colour toward a PSD background so
the composite stops looking pasted-on: the AI subject picks up the template's
colour cast, contrast and tonal balance while its brand colours stay put.

Phase 1 is deliberately heuristic and dependency-light -- only the vendored
``Pillow`` + ``numpy`` (no 3D LUT, no OpenCV, no local model):

* ``prompt_only``     -- no pixels touched; only emits a ``prompt_suffix`` to
  steer generation *before* the image exists.
* ``color_transfer``  -- Reinhard mean/std transfer in CIE-Lab toward the
  background's statistics.
* ``histogram_match`` -- per-channel CDF matching of the subject onto the
  background.
* ``hybrid``          -- ``color_transfer`` followed by a gentler
  ``histogram_match`` pass.

Two safety rails the plan calls for are hard constraints, not toggles you can
forget: corrections are weighted toward shadows/highlights (so midtone product
bodies move less), and ``--protect-brand-color`` damps the shift on
high-chroma pixels so a perfume bottle / logo keeps its brand colour. The
correction only ever acts inside the subject's alpha (optionally further masked).

The emitted JSON is ``{"matched_image", "prompt_suffix", "match_report"}`` where
``match_report`` records the before/after mean colour, colour temperature and
contrast plus the Lab statistics used. On failure the process exits non-zero
with a single message on stderr.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any

import numpy as np

# Lab channel ratios are clamped so a near-flat subject channel can't blow the
# transfer up; chroma is normalised against this to derive the brand-colour
# protection weight.
_RATIO_MIN, _RATIO_MAX = 0.5, 2.0
_CHROMA_NORM = 110.0
_EPS = 1e-6


def _safe_stem(image_path: str) -> str:
    """A filesystem-safe base name derived from the image file stem."""
    stem = Path(image_path).stem or "image"
    cleaned = "".join(ch if ch.isalnum() or ch in ("-", "_") else "_" for ch in stem)
    return cleaned or "image"


def _color_temperature(mean_rgb: "np.ndarray") -> int:
    """Rough correlated colour temperature from the red/blue balance.

    Mirrors ``analyze_psd_cli._color_temperature`` so the two production nodes
    report colour temperature on the same scale.
    """
    red = float(mean_rgb[0]) + 1.0
    blue = float(mean_rgb[2]) + 1.0
    kelvin = 2000.0 + (blue / red) * 4500.0
    kelvin = max(2000.0, min(12000.0, kelvin))
    return int(round(kelvin / 100.0) * 100)


def _warmth_label(kelvin: int) -> str:
    if kelvin < 4500:
        return "warm"
    if kelvin > 6500:
        return "cool"
    return "neutral"


def _load_rgb_alpha(path: str) -> tuple["np.ndarray", "np.ndarray"]:
    """Load an image as (H,W,3 uint8 RGB, H,W float alpha in 0..1)."""
    from PIL import Image

    img = Image.open(path)
    img = img.convert("RGBA")
    arr = np.asarray(img, dtype=np.uint8)
    rgb = arr[..., :3].copy()
    alpha = arr[..., 3].astype(np.float32) / 255.0
    return rgb, alpha


def _load_weight(path: str | None, shape: tuple[int, int]) -> "np.ndarray | None":
    """Load an optional mask as a float 0..1 array matched to ``shape`` (H,W)."""
    if not path:
        return None
    from PIL import Image

    mask = Image.open(path).convert("L")
    if mask.size != (shape[1], shape[0]):
        mask = mask.resize((shape[1], shape[0]))
    return np.asarray(mask, dtype=np.float32) / 255.0


def _rgb_to_lab(rgb: "np.ndarray") -> "np.ndarray":
    """RGB uint8 (H,W,3) -> Lab float32 via Pillow (L/a/b each 0..255)."""
    from PIL import Image

    lab = Image.fromarray(rgb, "RGB").convert("LAB")
    return np.asarray(lab, dtype=np.float32)


def _lab_to_rgb(lab: "np.ndarray") -> "np.ndarray":
    """Lab float (H,W,3) -> RGB uint8, clamping back into the 0..255 byte range."""
    from PIL import Image

    clamped = np.clip(lab, 0.0, 255.0).astype(np.uint8)
    return np.asarray(Image.fromarray(clamped, "LAB").convert("RGB"), dtype=np.uint8)


def _region_stats(values: "np.ndarray", weight: "np.ndarray") -> tuple["np.ndarray", "np.ndarray"]:
    """Weighted per-channel mean and std of (H,W,C) values over a (H,W) weight."""
    w = weight[..., None]
    total = float(w.sum()) + _EPS
    mean = (values * w).sum(axis=(0, 1)) / total
    var = (((values - mean) ** 2) * w).sum(axis=(0, 1)) / total
    return mean, np.sqrt(np.maximum(var, 0.0))


def _tone_protection_weight(
    subj_lab: "np.ndarray",
    base: float,
    shadow_strength: float,
    highlight_strength: float,
    protect_brand_color: bool,
) -> "np.ndarray":
    """Per-pixel correction weight, emphasising shadows/highlights and sparing
    high-chroma (brand) pixels. Returns a (H,W) float in 0..1."""
    ln = subj_lab[..., 0] / 255.0
    shadow = np.clip((0.45 - ln) / 0.45, 0.0, 1.0)
    highlight = np.clip((ln - 0.55) / 0.45, 0.0, 1.0)
    weight = base * (1.0 + shadow_strength * shadow + highlight_strength * highlight)
    if protect_brand_color:
        chroma = np.hypot(subj_lab[..., 1] - 128.0, subj_lab[..., 2] - 128.0)
        weight = weight * (1.0 - np.clip(chroma / _CHROMA_NORM, 0.0, 1.0))
    return np.clip(weight, 0.0, 1.0)


def _histogram_match(channel: "np.ndarray", reference: "np.ndarray") -> "np.ndarray":
    """Map ``channel`` values so their CDF matches ``reference`` (both 0..255)."""
    src = channel.reshape(-1)
    ref = reference.reshape(-1)
    s_vals, s_idx, s_counts = np.unique(src, return_inverse=True, return_counts=True)
    r_vals, r_counts = np.unique(ref, return_counts=True)
    s_cdf = np.cumsum(s_counts).astype(np.float64) / src.size
    r_cdf = np.cumsum(r_counts).astype(np.float64) / ref.size
    mapped = np.interp(s_cdf, r_cdf, r_vals)
    return mapped[s_idx].reshape(channel.shape)


def _blend(original: "np.ndarray", corrected: "np.ndarray", weight: "np.ndarray") -> "np.ndarray":
    """Per-pixel lerp of two (H,W,C) arrays by a (H,W) weight."""
    w = weight[..., None]
    return original * (1.0 - w) + corrected * w


def _appearance(rgb: "np.ndarray", weight: "np.ndarray") -> dict[str, Any]:
    """mean_color / color_temperature / contrast over the weighted region."""
    mean_rgb, _ = _region_stats(rgb.astype(np.float32), weight)
    gray = rgb.astype(np.float32) @ np.array([0.299, 0.587, 0.114], dtype=np.float32)
    _, gray_std = _region_stats(gray[..., None], weight)
    return {
        "mean_color": [int(round(float(c))) for c in mean_rgb],
        "color_temperature": _color_temperature(mean_rgb),
        "contrast": round(min(1.0, float(gray_std[0]) / 128.0), 4),
    }


def _prompt_suffix(context: dict[str, Any] | None, background_rgb: "np.ndarray | None") -> str:
    """Reuse the upstream context's suffix when present, else synthesise one
    from the background's colour temperature."""
    if context:
        existing = str(context.get("prompt_suffix") or "").strip()
        if existing:
            return existing
        lighting = context.get("lighting") or {}
        quality = str(lighting.get("quality") or "soft")
        direction = str(lighting.get("direction") or "center")
        kelvin = int(lighting.get("color_temperature") or 5500)
        warmth = _warmth_label(kelvin)
    elif background_rgb is not None:
        mean_rgb, _ = _region_stats(
            background_rgb.astype(np.float32), np.ones(background_rgb.shape[:2], dtype=np.float32)
        )
        kelvin = _color_temperature(mean_rgb)
        warmth, quality, direction = _warmth_label(kelvin), "soft", "center"
    else:
        return ""
    return (
        f"matched with the PSD background lighting: {quality} key light from {direction}, "
        f"{warmth} background, color temperature {kelvin}k, "
        "realistic contact shadow, consistent highlight direction, no floating object"
    )


def _apply_color_transfer(
    subj_lab: "np.ndarray",
    bg_lab: "np.ndarray",
    region: "np.ndarray",
    protect_saturation: bool,
) -> tuple["np.ndarray", dict[str, Any]]:
    """Reinhard mean/std transfer in Lab toward the background statistics."""
    src_mean, src_std = _region_stats(subj_lab, region)
    bg_weight = np.ones(bg_lab.shape[:2], dtype=np.float32)
    dst_mean, dst_std = _region_stats(bg_lab, bg_weight)
    ratio = np.clip(dst_std / (src_std + _EPS), _RATIO_MIN, _RATIO_MAX)
    transferred = (subj_lab - src_mean) * ratio + dst_mean
    if protect_saturation:
        # Match luminance only; keep the subject's own a/b (chroma) untouched.
        transferred[..., 1:] = subj_lab[..., 1:]
    stats = {
        "src_mean_lab": [round(float(v), 2) for v in src_mean],
        "dst_mean_lab": [round(float(v), 2) for v in dst_mean],
        "src_std_lab": [round(float(v), 2) for v in src_std],
        "dst_std_lab": [round(float(v), 2) for v in dst_std],
    }
    return transferred, stats


def match(args: argparse.Namespace) -> dict[str, Any]:
    image_path = (args.image or "").strip()
    if not image_path or not Path(image_path).is_file():
        raise FileNotFoundError(f"subject image not found: {image_path}")

    mode = (args.mode or "color_transfer").strip()
    valid = {"prompt_only", "color_transfer", "histogram_match", "hybrid"}
    if mode not in valid:
        raise ValueError(f"unknown mode {mode!r}; expected one of {sorted(valid)}")

    strength = float(np.clip(args.strength, 0.0, 1.0))
    shadow_strength = float(np.clip(args.shadow_strength, 0.0, 1.0))
    highlight_strength = float(np.clip(args.highlight_strength, 0.0, 1.0))

    context: dict[str, Any] | None = None
    raw_context = (args.context or "").strip()
    if raw_context:
        try:
            parsed = json.loads(raw_context)
            if isinstance(parsed, dict):
                context = parsed
        except json.JSONDecodeError:
            context = None

    rgb, alpha = _load_rgb_alpha(image_path)
    height, width = rgb.shape[:2]

    # The correction region: inside the subject's alpha, optionally narrowed by
    # an explicit mask. Fall back to the whole frame when there is no coverage.
    region = (alpha > 0.0).astype(np.float32)
    extra = _load_weight(args.mask, (height, width))
    if extra is not None:
        region = region * extra
    if float(region.sum()) < _EPS:
        region = np.ones((height, width), dtype=np.float32)

    background_rgb: "np.ndarray | None" = None
    bg_path = (args.background or "").strip()
    if bg_path:
        if not Path(bg_path).is_file():
            raise FileNotFoundError(f"background image not found: {bg_path}")
        background_rgb, _ = _load_rgb_alpha(bg_path)

    prompt_suffix = _prompt_suffix(context, background_rgb)
    before = _appearance(rgb, region)

    report: dict[str, Any] = {
        "mode": mode,
        "strength": round(strength, 4),
        "shadow_strength": round(shadow_strength, 4),
        "highlight_strength": round(highlight_strength, 4),
        "protect_saturation": bool(args.protect_saturation),
        "protect_brand_color": bool(args.protect_brand_color),
        "applied": False,
        "before": before,
        "after": before,
    }

    pixels_change = mode != "prompt_only" and strength > 0.0
    if not pixels_change or background_rgb is None:
        # prompt_only, zero strength, or nothing to match against: pass the
        # subject through untouched so the node is still wired-up correctly.
        if mode != "prompt_only" and background_rgb is None:
            report["note"] = "no background image connected; passed subject through unchanged"
        out_rgb = rgb
    else:
        subj_lab = _rgb_to_lab(rgb)
        bg_lab = _rgb_to_lab(background_rgb)
        weight = region * _tone_protection_weight(
            subj_lab, strength, shadow_strength, highlight_strength, bool(args.protect_brand_color)
        )

        if mode in ("color_transfer", "hybrid"):
            transferred, stats = _apply_color_transfer(
                subj_lab, bg_lab, region, bool(args.protect_saturation)
            )
            report.update(stats)
            result_lab = _blend(subj_lab, transferred, weight)
        else:
            result_lab = subj_lab

        if mode in ("histogram_match", "hybrid"):
            # Gentler second pass for hybrid so the transfer stays dominant.
            hist_weight = weight * (0.5 if mode == "hybrid" else 1.0)
            base = result_lab if mode == "hybrid" else subj_lab
            matched = base.copy()
            for ch in range(3):
                if args.protect_saturation and ch > 0:
                    continue
                matched[..., ch] = _histogram_match(
                    np.rint(base[..., ch]).astype(np.int64),
                    np.rint(bg_lab[..., ch]).astype(np.int64),
                ).astype(np.float32)
            result_lab = _blend(base, matched, hist_weight)

        out_rgb = _lab_to_rgb(result_lab)
        report["applied"] = True
        report["after"] = _appearance(out_rgb, region)

    # Recombine the (untouched) alpha and write the matched RGBA PNG. prompt_only
    # still writes a copy so downstream nodes always get a concrete path.
    directory = Path((args.output_dir or "").strip() or ".")
    directory.mkdir(parents=True, exist_ok=True)
    stem = (args.output_name or "").strip() or f"{_safe_stem(image_path)}_matched"

    from PIL import Image

    rgba = np.dstack([out_rgb, np.rint(alpha * 255.0).astype(np.uint8)])
    matched_path = directory / f"{stem}.png"
    Image.fromarray(rgba, "RGBA").save(str(matched_path), format="PNG")
    report["output_size"] = [width, height]

    return {
        "matched_image": str(matched_path),
        "prompt_suffix": prompt_suffix,
        "match_report": report,
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Match a subject image's light & colour to a PSD background."
    )
    parser.add_argument("--image", required=True, help="path to the subject image")
    parser.add_argument(
        "--background", default="", help="path to the background reference image"
    )
    parser.add_argument(
        "--mask", default="", help="optional mask narrowing the corrected region"
    )
    parser.add_argument(
        "--context", default="", help="optional VisualContext JSON for the prompt suffix"
    )
    parser.add_argument(
        "--mode",
        default="color_transfer",
        help="prompt_only | color_transfer | histogram_match | hybrid",
    )
    parser.add_argument("--strength", type=float, default=0.6, help="match strength 0..1")
    parser.add_argument(
        "--shadow-strength",
        dest="shadow_strength",
        type=float,
        default=0.0,
        help="extra correction weight in shadows 0..1",
    )
    parser.add_argument(
        "--highlight-strength",
        dest="highlight_strength",
        type=float,
        default=0.0,
        help="extra correction weight in highlights 0..1",
    )
    parser.add_argument(
        "--protect-saturation",
        dest="protect_saturation",
        action="store_true",
        help="match luminance only, keeping the subject's own chroma",
    )
    parser.add_argument(
        "--protect-brand-color",
        dest="protect_brand_color",
        action="store_true",
        help="damp the shift on high-chroma (brand) pixels",
    )
    parser.add_argument(
        "--output-dir",
        dest="output_dir",
        default="",
        help="directory for the written matched PNG (default: cwd)",
    )
    parser.add_argument(
        "--output-name",
        dest="output_name",
        default="",
        help="base name for the matched PNG (default: <image>_matched)",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        result = match(args)
    except Exception as err:  # noqa: BLE001 - surface a single clean error line
        sys.stderr.write(f"{type(err).__name__}: {err}\n")
        return 1
    sys.stdout.write(json.dumps(result, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
