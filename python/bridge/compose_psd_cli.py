"""Headless PSD compose + export for the H-Gripe desktop node editor.

This is the thin, ``torch``-free entry point behind the desktop ``compose_psd``
Tauri command (route 2 of the PSD export design): it takes a PSD *template* and
a *generated image on disk*, composes the image into the template's placeholder
-- using true smart-object content replacement when the placeholder is a smart
object -- and writes ``<name>.psd`` + ``<name>_preview.png`` + ``<name>_metadata.json``.

It deliberately reuses the proven logic from ``custom_nodes/hgripe_psd_nodes.py``
(placeholder resolution, fit math, smart-object detection) but reads the
generated image from a file via PIL instead of from a ComfyUI tensor, so it runs
without ``torch`` -- meaning it can be exercised by CI / a plain Python with just
``Pillow`` + the vendored ``psd_tools`` + ``attrs``.

As the **final assembler** of the PSD chain (every upstream card feeds it) the
loader is hardened so it behaves on real production assets, not just clean 8-bit
RGB PNGs:

* CMYK, 16-bit (``I;16``), float (``F``), grayscale and palette (``P``)
  generated images are converted to an 8-bit RGBA working space first (CMYK via
  its embedded ICC profile when present); the resolved ``source_mode`` is
  recorded in the export report.
* EXIF orientation is normalised so a phone-camera/raw export lands upright.
* An input larger than ``--max-decode-pixels`` is rejected *before* it is
  decoded so a crafted/huge image cannot exhaust memory; the same guard covers
  the optional mask.
* The optional matte is read as grayscale, high-bit-normalised, resized to the
  image and multiplied into any existing alpha so a pre-cut subject is never
  re-opened.

Input is passed as CLI flags; a single JSON object is printed to stdout on
success (the original fields plus an additive ``export_report``). On failure the
process exits non-zero with a single message on stderr.
"""

from __future__ import annotations

import argparse
import io
import json
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

# Resolve the repo root (this file lives at <root>/python/bridge/) and make both
# the root (for ``custom_nodes``) and the vendored ``third_party`` importable,
# exactly like the ComfyUI nodes and the offline examples do.
_ROOT_DIR = Path(__file__).resolve().parents[2]
for _candidate in (_ROOT_DIR, _ROOT_DIR / "third_party"):
    if _candidate.is_dir() and str(_candidate) not in sys.path:
        sys.path.insert(0, str(_candidate))

# These helpers import cleanly without torch (heavy imports inside hgripe_psd_nodes
# are deferred to call time), so reusing them keeps this CLI a single source of
# truth with the ComfyUI nodes for the tricky placeholder / fit / smart-object
# logic.
from custom_nodes.hgripe_psd_nodes import (  # noqa: E402
    HGripePsdCompose,
    _fit_into_box,
    _is_smart_object,
)

# Refuse to decode an input larger than this many pixels (decompression-bomb
# guard). 0 disables the check. Tunable via ``--max-decode-pixels``.
_DEFAULT_MAX_DECODE_PIXELS = 96_000_000
# Pillow modes that carry per-pixel transparency.
_ALPHA_MODES = {"RGBA", "LA", "La", "PA"}
# High-bit-depth integer/float modes we normalise down to 8-bit.
_HIGHBIT_MODES = {"I", "I;16", "I;16B", "I;16L", "I;16N", "F"}
# EXIF tag holding the orientation (1 = normal, 2..8 = a flip/rotation).
_EXIF_ORIENTATION_TAG = 0x0112


def _parse_json_object(raw: str, field_name: str) -> dict[str, Any]:
    text = (raw or "").strip()
    if not text:
        return {}
    try:
        value = json.loads(text)
    except json.JSONDecodeError as err:
        raise ValueError(f"{field_name} must be valid JSON: {err}") from err
    if not isinstance(value, dict):
        raise ValueError(f"{field_name} must be a JSON object")
    return value


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


def _highbit_to_l(img: Any) -> Any:
    """Normalise a 16-bit / 32-bit / float image down to an 8-bit ``L`` image.

    ``convert("L"/"RGB")`` on an ``I;16`` image clips to 0..255 and destroys the
    tonal range, so we scale the actual data range into 8 bits first. Used for
    both high-bit colour sources (then promoted to RGB) and high-bit mattes.
    """
    import numpy as np

    arr = np.asarray(img).astype(np.float64)
    if arr.size == 0:
        return img.convert("L")
    peak = float(arr.max())
    if peak > 255.0:
        arr = arr * (255.0 / peak)
    arr = np.clip(arr, 0.0, 255.0).astype(np.uint8)
    from PIL import Image

    return Image.fromarray(arr, mode="L")


def _apply_exif_orientation(img: Any) -> tuple[Any, bool]:
    """Apply EXIF orientation, returning ``(image, transposed)``.

    Pillow's ``ImageOps.exif_transpose`` returns a *new* object even when there
    is no orientation to apply, so ``fixed is not img`` over-reports a transpose
    on every plain image. We read the orientation tag directly and only
    transpose for a real, non-identity orientation.
    """
    from PIL import ImageOps

    try:
        orientation = img.getexif().get(_EXIF_ORIENTATION_TAG, 1)
    except Exception:  # noqa: BLE001 - a broken EXIF block must not abort compose
        orientation = 1
    if orientation in (None, 0, 1):
        return img, False
    try:
        fixed = ImageOps.exif_transpose(img)
    except Exception:  # noqa: BLE001 - fall back to the un-rotated image
        return img, False
    return fixed, fixed is not img


def _guard_decode_size(img: Any, path: str, max_decode_pixels: int) -> None:
    """Refuse an oversized input before it is decoded (``Image.open`` is lazy)."""
    width, height = img.size
    if max_decode_pixels > 0 and width * height > max_decode_pixels:
        raise ValueError(
            f"input image too large to decode safely: {path} {width}x{height} "
            f"({width * height} px > max {max_decode_pixels})"
        )


def _load_rgba(image_path: str, max_decode_pixels: int) -> tuple[Any, str, bool]:
    """Load the generated image as 8-bit RGBA, hardened for real assets.

    Returns ``(rgba_image, source_mode, exif_transposed)``. CMYK / high-bit /
    palette / grayscale sources are mapped into an 8-bit RGB working space (and
    then promoted to RGBA) so the composite has an honest colour + alpha; the
    decode-size guard and EXIF orientation are applied first.
    """
    from PIL import Image

    img = Image.open(image_path)
    _guard_decode_size(img, image_path, max_decode_pixels)
    img.load()
    img, transposed = _apply_exif_orientation(img)

    source_mode = img.mode
    had_alpha = source_mode in _ALPHA_MODES or (
        source_mode == "P" and "transparency" in img.info
    )
    if had_alpha:
        return img.convert("RGBA"), source_mode, transposed
    if source_mode == "CMYK":
        rgb = _cmyk_to_rgb(img)
    elif source_mode in _HIGHBIT_MODES:
        rgb = _highbit_to_l(img).convert("RGB")
    else:
        rgb = img.convert("RGB")
    return rgb.convert("RGBA"), source_mode, transposed


def _load_mask(mask_path: str, max_decode_pixels: int) -> tuple[Any, str]:
    """Load a matte as an 8-bit ``L`` image, hardened for real assets.

    Returns ``(mask_L, source_mode)``. High-bit mattes are tone-scaled rather
    than clipped; the decode guard applies as for the colour input.
    """
    from PIL import Image

    img = Image.open(mask_path)
    _guard_decode_size(img, mask_path, max_decode_pixels)
    img.load()
    source_mode = img.mode
    if source_mode in _HIGHBIT_MODES:
        return _highbit_to_l(img), source_mode
    return img.convert("L"), source_mode


def _apply_mask(gen_pil: Any, mask_img: Any) -> Any:
    """Use an explicit matte (e.g. Mask Edge Refine's ``refined_mask``) as the
    image's alpha. The mask is resized to the image and multiplied into any
    existing alpha so a pre-cut subject is not re-opened."""
    from PIL import Image, ImageChops

    mask = mask_img
    if mask.size != gen_pil.size:
        mask = mask.resize(gen_pil.size, Image.LANCZOS)
    alpha = ImageChops.multiply(gen_pil.getchannel("A"), mask)
    out = gen_pil.copy()
    out.putalpha(alpha)
    return out


def compose_and_export(args: argparse.Namespace) -> dict[str, Any]:
    from PIL import Image

    from psd_tools import PSDImage
    from psd_tools.api.layers import Group, PixelLayer

    template_path = args.template.strip()
    image_path = args.image.strip()
    if not template_path or not Path(template_path).is_file():
        raise FileNotFoundError(f"PSD template not found: {template_path}")
    if not image_path or not Path(image_path).is_file():
        raise FileNotFoundError(f"generated image not found: {image_path}")

    mask_path = (args.mask or "").strip()
    if mask_path and not Path(mask_path).is_file():
        raise FileNotFoundError(f"mask not found: {mask_path}")

    max_decode_pixels = int(max(0, args.max_decode_pixels))
    started = time.perf_counter()

    psd = PSDImage.open(template_path)
    canvas_w, canvas_h = int(psd.width), int(psd.height)

    plan = _parse_json_object(args.placeholder, "placeholder")
    # Reuse the node's placeholder resolution (by layer name or explicit box).
    left, top, box_w, box_h, ph_layer, ph_parent, ph_index = HGripePsdCompose()._resolve_placeholder(
        psd, plan
    )
    placeholder_kind = None
    if ph_layer is not None:
        placeholder_kind = "smartobject" if _is_smart_object(ph_layer) else "pixel"

    gen_pil, source_mode, exif_transposed = _load_rgba(image_path, max_decode_pixels)
    image_size = list(gen_pil.size)
    mask_source_mode = None
    if mask_path:
        mask_img, mask_source_mode = _load_mask(mask_path, max_decode_pixels)
        gen_pil = _apply_mask(gen_pil, mask_img)
    fitted, off_x, off_y = _fit_into_box(gen_pil, box_w, box_h, args.fit_mode)
    main_name = (args.generated_layer_name or "generated").strip() or "generated"

    so_replace = (
        args.smart_object_mode == "replace_content"
        and placeholder_kind == "smartobject"
        and ph_layer is not None
    )

    if so_replace:
        # True smart-object replacement: write the generated image *inside* the
        # template's smart object so it stays editable in Photoshop.
        box_img = Image.new("RGBA", (box_w, box_h), (0, 0, 0, 0))
        box_img.paste(fitted, (off_x, off_y))
        ph_layer.replace_with_image(box_img)
        main_name = ph_layer.name
    else:
        # Pixel fallback: insert the generated image as a new layer, optionally
        # hiding the placeholder so it does not show through.
        generated_group = Group.new(psd, "03_GENERATED")
        main_layer = PixelLayer.frompil(fitted, psd, main_name, top + off_y, left + off_x)
        generated_group.append(main_layer)

        if args.z_order == "placeholder" and ph_parent is not None:
            ph_parent.insert(ph_index, generated_group)
        elif args.z_order == "above_background":
            psd.insert(min(1, len(psd)), generated_group)
        else:
            psd.append(generated_group)

        if ph_layer is not None and args.hide_placeholder == "enable":
            ph_layer.visible = False

    metadata = _parse_json_object(args.metadata, "metadata")
    metadata.update(
        {
            "created_at": datetime.now(timezone.utc).isoformat(),
            "template_path": template_path,
            "source_image": image_path,
            "source_mode": source_mode,
            "exif_transposed": exif_transposed,
            "image_size": image_size,
            "mask_applied": bool(mask_path),
            "mask_source": mask_path or None,
            "mask_source_mode": mask_source_mode,
            "canvas": [canvas_w, canvas_h],
            "placeholder": {"left": left, "top": top, "width": box_w, "height": box_h},
            "placeholder_name": plan.get("name"),
            "placeholder_kind": placeholder_kind,
            "generated_layer": main_name,
            "fit_mode": args.fit_mode,
            "fit_offset": [off_x, off_y],
            "z_order": args.z_order,
            "smart_object_mode": "replace_content" if so_replace else "disable",
        }
    )

    directory = Path(args.output_dir.strip() or ".")
    directory.mkdir(parents=True, exist_ok=True)
    base = (args.filename or "final").strip() or "final"

    psd_path = directory / f"{base}.psd"
    psd.save(str(psd_path))

    preview_path = ""
    if args.save_preview == "enable":
        preview_file = directory / f"{base}_preview.png"
        psd.composite().convert("RGB").save(str(preview_file), format="PNG")
        preview_path = str(preview_file)

    metadata_file = directory / f"{base}_metadata.json"
    metadata_payload = dict(metadata)
    metadata_payload["psd_path"] = str(psd_path)
    if preview_path:
        metadata_payload["preview_path"] = preview_path
    metadata_file.write_text(
        json.dumps(metadata_payload, ensure_ascii=False, indent=2), encoding="utf-8"
    )

    elapsed_ms = int(round((time.perf_counter() - started) * 1000.0))
    export_report = {
        "source_mode": source_mode,
        "mask_source_mode": mask_source_mode,
        "exif_transposed": exif_transposed,
        "max_decode_pixels": max_decode_pixels,
        "image_size": image_size,
        "canvas": [canvas_w, canvas_h],
        "placeholder": {"left": left, "top": top, "width": box_w, "height": box_h},
        "placeholder_kind": placeholder_kind,
        "fit_mode": args.fit_mode,
        "fit_offset": [off_x, off_y],
        "mask_applied": bool(mask_path),
        "triplet": {
            "psd": True,
            "preview": bool(preview_path),
            "metadata": True,
        },
        "processing_time_ms": elapsed_ms,
    }

    return {
        "status": "succeeded",
        "psd_path": str(psd_path),
        "preview_path": preview_path,
        "metadata_path": str(metadata_file),
        "placeholder_kind": placeholder_kind,
        "smart_object_mode": metadata["smart_object_mode"],
        "export_report": export_report,
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Compose a generated image into a PSD template and export.")
    parser.add_argument("--template", required=True, help="path to the .psd template")
    parser.add_argument("--image", required=True, help="path to the generated image to place")
    parser.add_argument("--mask", default="", help="optional matte applied as the image's alpha")
    parser.add_argument("--output-dir", dest="output_dir", required=True, help="directory for the exported files")
    parser.add_argument("--filename", default="final", help="base name for the exported files")
    parser.add_argument(
        "--placeholder",
        default="{}",
        help='JSON: {"name": "<layer>"} or {"left","top","width","height"}',
    )
    parser.add_argument("--generated-layer-name", dest="generated_layer_name", default="generated")
    parser.add_argument("--fit-mode", dest="fit_mode", choices=["contain", "cover", "stretch"], default="contain")
    parser.add_argument(
        "--z-order", dest="z_order", choices=["above_background", "placeholder", "top"], default="above_background"
    )
    parser.add_argument(
        "--smart-object-mode",
        dest="smart_object_mode",
        choices=["disable", "replace_content"],
        default="disable",
    )
    parser.add_argument("--hide-placeholder", dest="hide_placeholder", choices=["enable", "disable"], default="enable")
    parser.add_argument("--metadata", default="{}", help="JSON object merged into the exported metadata")
    parser.add_argument("--save-preview", dest="save_preview", choices=["enable", "disable"], default="enable")
    parser.add_argument(
        "--max-decode-pixels",
        dest="max_decode_pixels",
        type=int,
        default=_DEFAULT_MAX_DECODE_PIXELS,
        help="reject input image/mask larger than this many pixels (0 disables)",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        result = compose_and_export(args)
    except Exception as err:  # noqa: BLE001 - surface a single clean error line
        sys.stderr.write(f"{type(err).__name__}: {err}\n")
        return 1
    sys.stdout.write(json.dumps(result, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
