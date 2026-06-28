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

Input is passed as CLI flags; a single JSON object is printed to stdout on
success. On failure the process exits non-zero with a message on stderr.
"""

from __future__ import annotations

import argparse
import json
import sys
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

    gen_pil = Image.open(image_path).convert("RGBA")
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
            "canvas": [canvas_w, canvas_h],
            "placeholder": {"left": left, "top": top, "width": box_w, "height": box_h},
            "placeholder_name": plan.get("name"),
            "placeholder_kind": placeholder_kind,
            "generated_layer": main_name,
            "fit_mode": args.fit_mode,
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

    return {
        "status": "succeeded",
        "psd_path": str(psd_path),
        "preview_path": preview_path,
        "metadata_path": str(metadata_file),
        "placeholder_kind": placeholder_kind,
        "smart_object_mode": metadata["smart_object_mode"],
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Compose a generated image into a PSD template and export.")
    parser.add_argument("--template", required=True, help="path to the .psd template")
    parser.add_argument("--image", required=True, help="path to the generated image to place")
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
