"""Local PSD-first production nodes for H-Gripe.

These nodes implement the MVP of the PSD-first production workflow: load a PSD
template, compose a generated image into a placeholder while preserving
template/reference/candidate layers, and export ``final.psd`` +
``preview.png`` + ``metadata.json``.

PSD reading and writing use ``psd-tools``, vendored under ``third_party/`` so the
nodes depend on a copy we control. Heavy imports are deferred to call time so the
module can be loaded even when the vendored copy's dependencies are missing.
"""

from __future__ import annotations

import json
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

# Prefer the vendored ``psd_tools`` (third_party/) over any pip-installed copy so
# our local modifications (e.g. smart-object content replacement) are used.
_VENDOR_DIR = Path(__file__).resolve().parent.parent / "third_party"
if _VENDOR_DIR.is_dir() and str(_VENDOR_DIR) not in sys.path:
    sys.path.insert(0, str(_VENDOR_DIR))

PSD_TEMPLATE_TYPE = "HGRIPE_PSD_TEMPLATE"
PSD_DOC_TYPE = "HGRIPE_PSD"


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


def _tensor_to_pil(image: Any, image_index: int):
    import numpy as np
    from PIL import Image

    if len(image.shape) == 3:
        selected = image
    else:
        batch_size = int(image.shape[0])
        if image_index < 0 or image_index >= batch_size:
            raise ValueError(f"image_index must be between 0 and {batch_size - 1}")
        selected = image[image_index]

    array = selected.detach().cpu().numpy()
    array = np.clip(array * 255.0, 0, 255).astype(np.uint8)
    if array.ndim == 3 and array.shape[2] == 4:
        return Image.fromarray(array, mode="RGBA")
    return Image.fromarray(array).convert("RGBA")


def _pil_to_tensor(pil_image):
    import numpy as np
    import torch

    array = np.asarray(pil_image.convert("RGB")).astype(np.float32) / 255.0
    return torch.from_numpy(array)[None,]


def _is_smart_object(layer) -> bool:
    return type(layer).__name__ == "SmartObjectLayer" or getattr(layer, "kind", "") == "smartobject"


def _layer_descriptor(layer) -> dict[str, Any]:
    left, top, right, bottom = layer.bbox
    if layer.is_group():
        kind = "group"
    elif _is_smart_object(layer):
        kind = "smartobject"
    else:
        kind = "pixel"
    descriptor: dict[str, Any] = {
        "name": layer.name,
        "kind": kind,
        "visible": bool(layer.visible),
        "has_mask": bool(layer.has_mask()),
        "bounds": [int(left), int(top), int(right), int(bottom)],
        "size": [int(right - left), int(bottom - top)],
    }
    if layer.is_group():
        descriptor["children"] = [_layer_descriptor(child) for child in layer]
    return descriptor


def _find_layer(node, name: str):
    """Recursively find a layer by name; return (layer, parent, index) or None."""
    for index, layer in enumerate(node):
        if layer.name == name:
            return layer, node, index
        if layer.is_group():
            found = _find_layer(layer, name)
            if found is not None:
                return found
    return None


def _mask_tensor_to_pil(mask, mask_index: int):
    """Convert a ComfyUI MASK tensor to an 'L' image (white = visible)."""
    import numpy as np
    from PIL import Image

    if len(mask.shape) == 2:
        selected = mask
    else:
        batch_size = int(mask.shape[0])
        index = max(0, min(mask_index, batch_size - 1))
        selected = mask[index]
    array = selected.detach().cpu().numpy()
    array = np.clip(array * 255.0, 0, 255).astype(np.uint8)
    return Image.fromarray(array, mode="L")


def _render_text_image(text: str, size: tuple[int, int]):
    """Render text onto a transparent raster image for an in-PSD metadata layer."""
    from PIL import Image, ImageDraw

    width, height = max(1, int(size[0])), max(1, int(size[1]))
    image = Image.new("RGBA", (width, height), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)
    margin = 4
    lines: list[str] = []
    for raw_line in text.splitlines() or [""]:
        line = raw_line
        while len(line) > 0:
            lines.append(line[:64])
            line = line[64:]
        if not raw_line:
            lines.append("")
    y = margin
    for line in lines:
        draw.text((margin, y), line, fill=(255, 255, 255, 255))
        y += 12
        if y > height - margin:
            break
    return image


def _fit_into_box(pil_image, box_w: int, box_h: int, fit_mode: str):
    """Return (resized_image, offset_x, offset_y) for placing inside the box."""
    from PIL import Image

    box_w = max(1, int(box_w))
    box_h = max(1, int(box_h))
    src_w, src_h = pil_image.size

    if fit_mode == "stretch":
        return pil_image.resize((box_w, box_h), Image.LANCZOS), 0, 0

    if fit_mode == "cover":
        scale = max(box_w / src_w, box_h / src_h)
        new_w, new_h = max(1, round(src_w * scale)), max(1, round(src_h * scale))
        resized = pil_image.resize((new_w, new_h), Image.LANCZOS)
        crop_x = (new_w - box_w) // 2
        crop_y = (new_h - box_h) // 2
        cropped = resized.crop((crop_x, crop_y, crop_x + box_w, crop_y + box_h))
        return cropped, 0, 0

    # contain (default)
    scale = min(box_w / src_w, box_h / src_h)
    new_w, new_h = max(1, round(src_w * scale)), max(1, round(src_h * scale))
    resized = pil_image.resize((new_w, new_h), Image.LANCZOS)
    return resized, (box_w - new_w) // 2, (box_h - new_h) // 2


class HGripePsdTemplateLoad:
    """Load a PSD template: read layers/bounds and render a preview."""

    @classmethod
    def INPUT_TYPES(cls):
        return {
            "required": {
                "psd_path": ("STRING", {"default": ""}),
            }
        }

    RETURN_TYPES = (PSD_TEMPLATE_TYPE, "IMAGE", "STRING")
    RETURN_NAMES = ("template", "preview", "info_json")
    FUNCTION = "run"
    CATEGORY = "H-Gripe/PSD"

    def run(self, psd_path: str):
        from psd_tools import PSDImage

        path = psd_path.strip()
        if not path:
            raise ValueError("psd_path is required")
        if not Path(path).is_file():
            raise FileNotFoundError(f"PSD template not found: {path}")

        psd = PSDImage.open(path)
        layers = [_layer_descriptor(layer) for layer in psd]
        preview = _pil_to_tensor(psd.composite())
        template = {
            "psd_path": path,
            "size": [int(psd.width), int(psd.height)],
            "layers": layers,
        }
        info_json = json.dumps(
            {"size": template["size"], "layers": layers}, ensure_ascii=False, indent=2
        )
        return (template, preview, info_json)


class HGripePsdCompose:
    """Compose a generated image into a template placeholder.

    Template layers are preserved; the generated image is inserted (by default
    just above the bottom/background layer so template borders and text stay on
    top). Optional reference and candidate images are added as hidden layers.
    """

    @classmethod
    def INPUT_TYPES(cls):
        return {
            "required": {
                "template": (PSD_TEMPLATE_TYPE,),
                "image": ("IMAGE",),
                "placeholder_json": (
                    "STRING",
                    {
                        "multiline": True,
                        "default": '{"left": 0, "top": 0, "width": 0, "height": 0}',
                    },
                ),
                "generated_layer_name": ("STRING", {"default": "generated"}),
                "fit_mode": (["contain", "cover", "stretch"], {"default": "contain"}),
                "z_order": (
                    ["above_background", "placeholder", "top"],
                    {"default": "above_background"},
                ),
                "image_index": ("INT", {"default": 0, "min": 0, "max": 4095, "step": 1}),
            },
            "optional": {
                "reference_image": ("IMAGE",),
                "candidates": ("IMAGE",),
                "mask": ("MASK",),
                "mask_index": ("INT", {"default": 0, "min": 0, "max": 4095, "step": 1}),
                "inherit_placeholder_mask": (["enable", "disable"], {"default": "disable"}),
                "hide_placeholder": (["enable", "disable"], {"default": "enable"}),
                "visible_candidate": ("INT", {"default": 1, "min": 1, "max": 4096, "step": 1}),
                "smart_object_mode": (["disable", "replace_content"], {"default": "disable"}),
                "write_metadata_layer": (["enable", "disable"], {"default": "disable"}),
                "metadata_json": ("STRING", {"multiline": True, "default": "{}"}),
            },
        }

    RETURN_TYPES = (PSD_DOC_TYPE, "IMAGE", "STRING")
    RETURN_NAMES = ("composed", "preview", "metadata_json")
    FUNCTION = "run"
    CATEGORY = "H-Gripe/PSD"

    def _resolve_placeholder(self, psd, plan: dict[str, Any]):
        """Return (left, top, w, h, placeholder_layer, parent, index)."""
        canvas_w, canvas_h = int(psd.width), int(psd.height)
        name = plan.get("name")
        if isinstance(name, str) and name.strip():
            found = _find_layer(psd, name.strip())
            if found is None:
                raise ValueError(f"placeholder layer '{name}' was not found in template")
            layer, parent, index = found
            left, top, right, bottom = layer.bbox
            box_w, box_h = int(right - left), int(bottom - top)
            if box_w <= 0 or box_h <= 0:
                box_w, box_h = canvas_w, canvas_h
            return int(left), int(top), box_w, box_h, layer, parent, index

        left = int(plan.get("left", 0))
        top = int(plan.get("top", 0))
        width = int(plan.get("width", 0)) or canvas_w
        height = int(plan.get("height", 0)) or canvas_h
        return left, top, width, height, None, None, None

    def run(
        self,
        template: dict[str, Any],
        image,
        placeholder_json: str,
        generated_layer_name: str,
        fit_mode: str,
        z_order: str,
        image_index: int,
        reference_image=None,
        candidates=None,
        mask=None,
        mask_index: int = 0,
        inherit_placeholder_mask: str = "disable",
        hide_placeholder: str = "enable",
        visible_candidate: int = 1,
        smart_object_mode: str = "disable",
        write_metadata_layer: str = "disable",
        metadata_json: str = "{}",
    ):
        from PIL import Image

        from psd_tools import PSDImage
        from psd_tools.api.layers import Group, PixelLayer

        psd_path = template.get("psd_path")
        if not psd_path or not Path(psd_path).is_file():
            raise FileNotFoundError("composed template is missing a valid psd_path")

        psd = PSDImage.open(psd_path)
        canvas_w, canvas_h = int(psd.width), int(psd.height)

        plan = _parse_json_object(placeholder_json, "placeholder_json")
        left, top, box_w, box_h, ph_layer, ph_parent, ph_index = self._resolve_placeholder(
            psd, plan
        )
        placeholder_kind = None
        if ph_layer is not None:
            placeholder_kind = "smartobject" if _is_smart_object(ph_layer) else "pixel"

        gen_pil = _tensor_to_pil(image, image_index)
        fitted, off_x, off_y = _fit_into_box(gen_pil, box_w, box_h, fit_mode)
        main_name = generated_layer_name.strip() or "generated"

        # True smart-object replacement: write the generated image *inside* the
        # template's smart object (kept editable in Photoshop) instead of laying
        # a flat pixel copy on top. Falls back to the pixel path otherwise.
        so_replace = (
            smart_object_mode == "replace_content"
            and placeholder_kind == "smartobject"
            and ph_layer is not None
        )
        candidate_layers = []
        applied_mask = "none"
        choice = 1

        if so_replace:
            box_img = Image.new("RGBA", (box_w, box_h), (0, 0, 0, 0))
            box_img.paste(fitted, (off_x, off_y))
            ph_layer.replace_with_image(box_img)
            main_name = ph_layer.name
        else:
            generated_group = Group.new(psd, "03_GENERATED")
            main_layer = PixelLayer.frompil(fitted, psd, main_name, top + off_y, left + off_x)
            generated_group.append(main_layer)

            if candidates is not None:
                batch = int(candidates.shape[0]) if len(candidates.shape) == 4 else 1
                for index in range(batch):
                    candidate_pil = _tensor_to_pil(candidates, index)
                    cand_fitted, cand_x, cand_y = _fit_into_box(
                        candidate_pil, box_w, box_h, fit_mode
                    )
                    candidate_layer = PixelLayer.frompil(
                        cand_fitted, psd, f"candidate_{index + 2:02d}", top + cand_y, left + cand_x
                    )
                    generated_group.append(candidate_layer)
                    candidate_layers.append(candidate_layer)

            # Placeholder-aware z-ordering: drop the group into the placeholder's slot.
            if z_order == "placeholder" and ph_parent is not None:
                ph_parent.insert(ph_index, generated_group)
            elif z_order == "above_background":
                psd.insert(min(1, len(psd)), generated_group)
            else:
                psd.append(generated_group)

            if ph_layer is not None and hide_placeholder == "enable":
                ph_layer.visible = False

            # Multi-candidate visibility: exactly one of [main, candidate_02, ...] is shown.
            gallery = [main_layer, *candidate_layers]
            choice = visible_candidate if 1 <= visible_candidate <= len(gallery) else 1
            for position, layer in enumerate(gallery, start=1):
                layer.visible = position == choice

            # Mask: explicit MASK input wins; otherwise optionally inherit placeholder mask.
            if mask is not None:
                mask_pil = _mask_tensor_to_pil(mask, mask_index).resize(fitted.size)
                main_layer.create_mask(mask_pil, top + off_y, left + off_x)
                applied_mask = "input"
            elif (
                inherit_placeholder_mask == "enable"
                and ph_layer is not None
                and ph_layer.has_mask()
            ):
                placeholder_mask = ph_layer.mask
                mask_image = placeholder_mask.topil()
                if mask_image is not None:
                    mleft, mtop = int(placeholder_mask.bbox[0]), int(placeholder_mask.bbox[1])
                    main_layer.create_mask(mask_image.convert("L"), mtop, mleft)
                    applied_mask = "inherited"

        has_reference = False
        if reference_image is not None:
            reference_group = Group.new(psd, "02_REFERENCE")
            ref_pil = _tensor_to_pil(reference_image, 0)
            reference_layer = PixelLayer.frompil(ref_pil, psd, "reference_image", 0, 0)
            reference_layer.visible = False
            reference_group.append(reference_layer)
            reference_group.visible = False
            psd.insert(min(1, len(psd)), reference_group)
            has_reference = True

        metadata = _parse_json_object(metadata_json, "metadata_json")
        metadata.update(
            {
                "created_at": datetime.now(timezone.utc).isoformat(),
                "template_path": psd_path,
                "canvas": [canvas_w, canvas_h],
                "placeholder": {"left": left, "top": top, "width": box_w, "height": box_h},
                "placeholder_name": plan.get("name"),
                "placeholder_kind": placeholder_kind,
                "generated_layer": main_name,
                "fit_mode": fit_mode,
                "z_order": z_order,
                "candidate_count": len(candidate_layers),
                "visible_candidate": choice,
                "applied_mask": applied_mask,
                "has_reference": has_reference,
                "smart_object_mode": "replace_content" if so_replace else "disable",
            }
        )

        # In-PSD metadata: psd-tools cannot reliably write editable text (TypeLayer),
        # so the prompt and generation info are rendered to hidden raster layers.
        if write_metadata_layer == "enable":
            meta_group = Group.new(psd, "00_META")
            prompt_text = str(metadata.get("prompt") or "(no prompt)")
            info_text = json.dumps(metadata, ensure_ascii=False, indent=2)
            prompt_layer = PixelLayer.frompil(
                _render_text_image(prompt_text, (canvas_w, canvas_h)), psd, "prompt", 0, 0
            )
            prompt_layer.visible = False
            meta_group.append(prompt_layer)
            info_layer = PixelLayer.frompil(
                _render_text_image(info_text, (canvas_w, canvas_h)), psd, "generation_info", 0, 0
            )
            info_layer.visible = False
            meta_group.append(info_layer)
            meta_group.visible = False
            psd.append(meta_group)
            metadata["metadata_layer"] = True

        preview = _pil_to_tensor(psd.composite())
        composed = {"psd": psd, "metadata": metadata}
        return (composed, preview, json.dumps(metadata, ensure_ascii=False, indent=2))


class HGripePsdExport:
    """Export a composed PSD as final.psd + preview.png + metadata.json."""

    @classmethod
    def INPUT_TYPES(cls):
        return {
            "required": {
                "composed": (PSD_DOC_TYPE,),
                "output_dir": ("STRING", {"default": ""}),
                "filename": ("STRING", {"default": "final"}),
                "save_preview": (["enable", "disable"], {"default": "enable"}),
            }
        }

    RETURN_TYPES = ("STRING", "STRING", "STRING", "STRING")
    RETURN_NAMES = ("psd_path", "preview_path", "metadata_path", "status")
    FUNCTION = "run"
    CATEGORY = "H-Gripe/PSD"

    def run(self, composed: dict[str, Any], output_dir: str, filename: str, save_preview: str):
        psd = composed.get("psd")
        if psd is None:
            raise ValueError("composed input does not contain a PSD document")
        metadata = composed.get("metadata") or {}

        directory = Path(output_dir.strip() or ".")
        directory.mkdir(parents=True, exist_ok=True)
        base = filename.strip() or "final"

        psd_path = directory / f"{base}.psd"
        psd.save(str(psd_path))

        preview_path = ""
        if save_preview == "enable":
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

        return (str(psd_path), preview_path, str(metadata_file), "succeeded")


NODE_CLASS_MAPPINGS = {
    "HGripePsdTemplateLoad": HGripePsdTemplateLoad,
    "HGripePsdCompose": HGripePsdCompose,
    "HGripePsdExport": HGripePsdExport,
}

NODE_DISPLAY_NAME_MAPPINGS = {
    "HGripePsdTemplateLoad": "H-Gripe PSD Template Load",
    "HGripePsdCompose": "H-Gripe PSD Compose",
    "HGripePsdExport": "H-Gripe PSD Export",
}
