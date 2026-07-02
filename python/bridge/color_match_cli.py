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

Phase 2 adds an **opt-in** learned matcher behind the ``--engine`` seam
(``color_backends/``): ``--engine onnx_harmonize`` runs a learned light/colour
harmoniser when its optional dep + weight are present, otherwise it degrades to
the heuristic above and the report records why (``engine_fallback_reason``).
``cpu`` stays the default and the always-available baseline; weights are not
bundled. ``--probe-engines`` prints which engines are usable so the UI can grey
out the unavailable ones.

The emitted JSON is ``{"matched_image", "prompt_suffix", "match_report"}`` where
``match_report`` records the before/after mean colour, colour temperature and
contrast plus the Lab statistics used, plus the ``engine`` telemetry (which
engine ran, what was requested, any ``engine_fallback_reason`` and the resolved
``backend_model``). On failure the process exits non-zero with a single message
on stderr.
"""

from __future__ import annotations

import argparse
import io
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

# Default ceiling on *input* pixels (~96 MP); a larger image is refused before
# it is decoded so a decompression-bomb workflow asset cannot exhaust memory.
_DEFAULT_MAX_DECODE_PIXELS = 96_000_000

# Pillow modes carrying per-pixel transparency / high-bit integer-or-float data.
_ALPHA_MODES = {"RGBA", "LA", "La", "PA"}
_HIGHBIT_MODES = {"I", "I;16", "I;16B", "I;16L", "I;16N", "F"}


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


def _load_rgb_alpha(
    path: str, max_decode_pixels: int = _DEFAULT_MAX_DECODE_PIXELS
) -> tuple["np.ndarray", "np.ndarray", str, bool]:
    """Load an image as (H,W,3 uint8 RGB, H,W float alpha in 0..1, source_mode, exif_transposed).

    Refuses oversized inputs before decoding, normalises EXIF orientation, and
    maps CMYK / high-bit / palette / grayscale sources into an 8-bit RGB working
    space (so colour matching is not skewed by a naive ``convert``). The alpha
    channel is taken from the source when present, else a fully-opaque plane.
    """
    from PIL import Image, ImageOps

    img = Image.open(path)
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
    except Exception:  # noqa: BLE001 - a broken EXIF block must not abort matching
        fixed = img
    if fixed is not img:
        transposed = True
    img = fixed

    # A ProPhoto-tagged manual product (the Rust chain's 16-bit output) is
    # colour-managed into sRGB; everything else passes through untouched.
    from wide_gamut import managed_to_srgb

    img, _ = managed_to_srgb(img)

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


def _prompt_suffix(
    context: dict[str, Any] | None,
    background_rgb: "np.ndarray | None",
    background_weight: "np.ndarray | None" = None,
) -> str:
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
        if background_weight is None:
            background_weight = np.ones(background_rgb.shape[:2], dtype=np.float32)
        mean_rgb, _ = _region_stats(background_rgb.astype(np.float32), background_weight)
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
    bg_region: "np.ndarray | None" = None,
) -> tuple["np.ndarray", dict[str, Any]]:
    """Reinhard mean/std transfer in Lab toward the background statistics.

    ``bg_region`` (a 0..1 weight over the background, default fully opaque)
    excludes transparent background pixels so their colour does not skew the
    target mean/std.
    """
    src_mean, src_std = _region_stats(subj_lab, region)
    if bg_region is None:
        bg_region = np.ones(bg_lab.shape[:2], dtype=np.float32)
    dst_mean, dst_std = _region_stats(bg_lab, bg_region)
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


# The always-available CPU heuristic is the default "engine"; learned matchers
# register in ``color_backends`` and are selected by ``--engine``.
_CPU_ENGINE = "cpu"
# Default compute-device selection for the learned matcher (mirrors
# sr_backends.DEVICE_AUTO; kept local so the module stays import-light).
_DEVICE_AUTO = "auto"


def _run_engine(
    engine_requested: str,
    rgb: "np.ndarray",
    alpha: "np.ndarray",
    background_rgb: "np.ndarray",
    device_requested: str,
) -> tuple[dict[str, Any], "np.ndarray | None"]:
    """Run the opt-in learned matcher for ``engine_requested`` (or the CPU path).

    Returns ``(telemetry, harmonized_rgb)``. Any unavailability (unknown engine,
    missing deps / weight, runtime error) degrades to the heuristic path
    (``harmonized_rgb is None``) and records ``fallback_reason`` -- the node never
    crashes on a box without the model.
    """
    state: dict[str, Any] = {
        "engine": _CPU_ENGINE,
        "fallback_reason": None,
        "backend_model": None,
        "device": None,
    }

    from color_backends import MatcherUnavailable, resolve

    backend = resolve(engine_requested)
    if backend is None:
        state["fallback_reason"] = f"unknown engine {engine_requested!r}"
        return state, None

    ok, reason = backend.available()
    if not ok:
        state["fallback_reason"] = reason
        return state, None

    try:
        harmonized, state["device"] = backend.match(
            rgb, alpha, background_rgb, device=device_requested
        )
    except MatcherUnavailable as err:
        state["fallback_reason"] = err.reason
        return state, None
    except Exception as err:  # noqa: BLE001 - degrade to heuristic, never crash
        state["fallback_reason"] = f"{type(err).__name__}: {err}"
        return state, None

    state["engine"] = backend.id
    try:
        state["backend_model"] = Path(backend.weight_path()).name
    except Exception:  # noqa: BLE001 - the model name is best-effort telemetry
        state["backend_model"] = None
    return state, harmonized


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

    max_decode_pixels = int(getattr(args, "max_decode_pixels", _DEFAULT_MAX_DECODE_PIXELS))
    rgb, alpha, source_mode, exif_transposed = _load_rgb_alpha(image_path, max_decode_pixels)
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
    background_region: "np.ndarray | None" = None
    background_mode: str | None = None
    bg_path = (args.background or "").strip()
    if bg_path:
        if not Path(bg_path).is_file():
            raise FileNotFoundError(f"background image not found: {bg_path}")
        background_rgb, bg_alpha, background_mode, _ = _load_rgb_alpha(bg_path, max_decode_pixels)
        # Only opaque background pixels describe the target lighting; transparent
        # regions (a cut-out background plate) must not skew the statistics.
        background_region = (bg_alpha > 0.0).astype(np.float32)
        if float(background_region.sum()) < _EPS:
            background_region = np.ones(background_rgb.shape[:2], dtype=np.float32)

    prompt_suffix = _prompt_suffix(context, background_rgb, background_region)
    before = _appearance(rgb, region)

    report: dict[str, Any] = {
        "mode": mode,
        "strength": round(strength, 4),
        "shadow_strength": round(shadow_strength, 4),
        "highlight_strength": round(highlight_strength, 4),
        "protect_saturation": bool(args.protect_saturation),
        "protect_brand_color": bool(args.protect_brand_color),
        "source_mode": source_mode,
        "background_mode": background_mode,
        "exif_transposed": exif_transposed,
        "max_decode_pixels": max_decode_pixels,
        "applied": False,
        "before": before,
        "after": before,
    }

    # Learned-matcher ``engine`` seam (``color_backends``). ``cpu`` is the
    # always-on heuristic baseline; a learned engine is opt-in and degrades back
    # to the heuristic (recording why) when its deps / weights are missing.
    engine_requested = (getattr(args, "engine", _CPU_ENGINE) or _CPU_ENGINE).strip().lower() or _CPU_ENGINE
    # ``device`` selects the ONNX execution provider for the learned matcher (the
    # CPU heuristic always runs on CPU). ``device`` in the report is the one the
    # session actually bound, which can differ from the request (an explicit
    # ``cuda`` degrades to ``cpu`` when ORT exposes no accelerator).
    device_requested = (getattr(args, "device", None) or _DEVICE_AUTO).strip().lower() or _DEVICE_AUTO
    report["engine"] = _CPU_ENGINE
    report["engine_requested"] = engine_requested
    report["engine_fallback_reason"] = None
    report["backend_model"] = None
    report["device"] = None
    report["device_requested"] = device_requested

    pixels_change = mode != "prompt_only" and strength > 0.0

    engine_rgb: "np.ndarray | None" = None
    if engine_requested != _CPU_ENGINE:
        if pixels_change and background_rgb is not None:
            engine_state, engine_rgb = _run_engine(
                engine_requested, rgb, alpha, background_rgb, device_requested
            )
            report["engine"] = engine_state["engine"]
            report["engine_fallback_reason"] = engine_state["fallback_reason"]
            report["backend_model"] = engine_state["backend_model"]
            report["device"] = engine_state["device"]
        else:
            # A learned matcher was requested but there is nothing for it to
            # harmonise against; keep the passthrough path and say why.
            report["engine_fallback_reason"] = (
                "no background reference" if background_rgb is None else "mode does not change pixels"
            )

    if engine_rgb is not None:
        # Apply the learned correction only inside the subject region, scaled by
        # ``strength``, so the matcher honours the same region/strength contract
        # as the heuristic path.
        weight = (region * strength)[..., None]
        blended = rgb.astype(np.float32) * (1.0 - weight) + engine_rgb.astype(np.float32) * weight
        out_rgb = np.clip(np.rint(blended), 0, 255).astype(np.uint8)
        report["applied"] = True
        report["after"] = _appearance(out_rgb, region)
    elif not pixels_change or background_rgb is None:
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

        bg_sel = (background_region > 0.5) if background_region is not None else None
        if mode in ("color_transfer", "hybrid"):
            transferred, stats = _apply_color_transfer(
                subj_lab, bg_lab, region, bool(args.protect_saturation), background_region
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
                ref_channel = np.rint(bg_lab[..., ch]).astype(np.int64)
                if bg_sel is not None:
                    ref_channel = ref_channel[bg_sel]
                matched[..., ch] = _histogram_match(
                    np.rint(base[..., ch]).astype(np.int64),
                    ref_channel,
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
    parser.add_argument(
        "--max-decode-pixels",
        dest="max_decode_pixels",
        type=int,
        default=_DEFAULT_MAX_DECODE_PIXELS,
        help="refuse inputs larger than this many pixels before decoding (0 disables)",
    )
    parser.add_argument(
        "--device",
        default=_DEVICE_AUTO,
        help=(
            "compute device for the learned matcher: auto (default, cuda provider "
            "if present else cpu) | cpu | cuda (degrades to cpu without an "
            "accelerator provider)"
        ),
    )
    parser.add_argument(
        "--engine",
        default=_CPU_ENGINE,
        help="match engine: cpu (default heuristic) | onnx_harmonize (opt-in learned, falls back to cpu)",
    )
    parser.add_argument(
        "--probe-engines",
        dest="probe_engines",
        action="store_true",
        help="print engine availability JSON and exit (UI capability probe)",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    if getattr(args, "probe_engines", False):
        from color_backends import probe

        sys.stdout.write(json.dumps(probe(), ensure_ascii=False))
        return 0
    try:
        result = match(args)
    except Exception as err:  # noqa: BLE001 - surface a single clean error line
        sys.stderr.write(f"{type(err).__name__}: {err}\n")
        return 1
    sys.stdout.write(json.dumps(result, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
