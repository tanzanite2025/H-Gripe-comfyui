"""Local PSD-first production nodes for H-Gripe.

These nodes implement the MVP of the PSD production workflow described in
``PSD_AI_PRODUCTION_WORKFLOW_RESEARCH.md``: load a PSD template, compose a
generated image into a placeholder while preserving template/reference/candidate
layers, and export ``final.psd`` + ``preview.png`` + ``metadata.json``.

PSD reading and writing use ``psd-tools``. Heavy imports are deferred to call
time so the module can be loaded even when ``psd-tools`` is not installed.
"""

from __future__ import annotations

import json
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

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


def _layer_descriptor(layer) -> dict[str, Any]:
    left, top, right, bottom = layer.bbox
    descriptor: dict[str, Any] = {
        "name": layer.name,
        "kind": "group" if layer.is_group() else "pixel",
        "visible": bool(layer.visible),
        "bounds": [int(left), int(top), int(right), int(bottom)],
        "size": [int(right - left), int(bottom - top)],
    }
    if layer.is_group():
        descriptor["children"] = [_layer_descriptor(child) for child in layer]
    return descriptor


def _find_layer_bounds(layers: list[dict[str, Any]], name: str) -> list[int] | None:
    for layer in layers:
        if layer.get("name") == name:
            return layer.get("bounds")
        children = layer.get("children")
        if children:
            found = _find_layer_bounds(children, name)
            if found is not None:
                return found
    return None


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
                "z_order": (["above_background", "top"], {"default": "above_background"}),
                "image_index": ("INT", {"default": 0, "min": 0, "max": 4095, "step": 1}),
            },
            "optional": {
                "reference_image": ("IMAGE",),
                "candidates": ("IMAGE",),
                "metadata_json": ("STRING", {"multiline": True, "default": "{}"}),
            },
        }

    RETURN_TYPES = (PSD_DOC_TYPE, "IMAGE", "STRING")
    RETURN_NAMES = ("composed", "preview", "metadata_json")
    FUNCTION = "run"
    CATEGORY = "H-Gripe/PSD"

    def _resolve_box(self, template: dict[str, Any], plan: dict[str, Any]):
        canvas_w, canvas_h = template["size"]
        name = plan.get("name")
        if isinstance(name, str) and name.strip():
            bounds = _find_layer_bounds(template["layers"], name.strip())
            if bounds is None:
                raise ValueError(f"placeholder layer '{name}' was not found in template")
            left, top, right, bottom = bounds
            return int(left), int(top), int(right - left), int(bottom - top)

        left = int(plan.get("left", 0))
        top = int(plan.get("top", 0))
        width = int(plan.get("width", 0)) or int(canvas_w)
        height = int(plan.get("height", 0)) or int(canvas_h)
        return left, top, width, height

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
        metadata_json: str = "{}",
    ):
        from psd_tools import PSDImage
        from psd_tools.api.layers import Group, PixelLayer

        psd_path = template.get("psd_path")
        if not psd_path or not Path(psd_path).is_file():
            raise FileNotFoundError("composed template is missing a valid psd_path")

        psd = PSDImage.open(psd_path)
        canvas_w, canvas_h = int(psd.width), int(psd.height)

        plan = _parse_json_object(placeholder_json, "placeholder_json")
        left, top, box_w, box_h = self._resolve_box(template, plan)

        generated_group = Group.new(psd, "03_GENERATED")
        gen_pil = _tensor_to_pil(image, image_index)
        fitted, off_x, off_y = _fit_into_box(gen_pil, box_w, box_h, fit_mode)
        main_layer = PixelLayer.frompil(
            fitted, psd, generated_layer_name.strip() or "generated", top + off_y, left + off_x
        )
        generated_group.append(main_layer)

        candidate_count = 0
        if candidates is not None:
            batch = int(candidates.shape[0]) if len(candidates.shape) == 4 else 1
            for index in range(batch):
                candidate_pil = _tensor_to_pil(candidates, index)
                cand_fitted, cand_x, cand_y = _fit_into_box(candidate_pil, box_w, box_h, fit_mode)
                candidate_layer = PixelLayer.frompil(
                    cand_fitted, psd, f"candidate_{index + 2:02d}", top + cand_y, left + cand_x
                )
                candidate_layer.visible = False
                generated_group.append(candidate_layer)
                candidate_count += 1

        insert_index = min(1, len(psd)) if z_order == "above_background" else len(psd)
        psd.insert(insert_index, generated_group)

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
                "generated_layer": generated_layer_name.strip() or "generated",
                "fit_mode": fit_mode,
                "z_order": z_order,
                "candidate_count": candidate_count,
                "has_reference": has_reference,
            }
        )

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
