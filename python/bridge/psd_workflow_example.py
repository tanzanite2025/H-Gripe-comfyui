from __future__ import annotations

import json
import sys
import tempfile
from pathlib import Path

import torch
from PIL import Image

ROOT_DIR = Path(__file__).resolve().parents[2]
if str(ROOT_DIR) not in sys.path:
    sys.path.insert(0, str(ROOT_DIR))
# Use the vendored psd_tools (third_party/) like the nodes do.
_VENDOR_DIR = ROOT_DIR / "third_party"
if _VENDOR_DIR.is_dir() and str(_VENDOR_DIR) not in sys.path:
    sys.path.insert(0, str(_VENDOR_DIR))

from psd_tools import PSDImage
from psd_tools.api.layers import PixelLayer

from custom_nodes.hgripe_psd_nodes import (
    HGripePsdCompose,
    HGripePsdExport,
    HGripePsdTemplateLoad,
)


def build_template(path: Path) -> None:
    psd = PSDImage.new(mode="RGBA", size=(64, 48))
    background = PixelLayer.frompil(
        Image.new("RGBA", (64, 48), (240, 240, 240, 255)), psd, "background", 0, 0
    )
    psd.append(background)
    # A named placeholder region the generated image should fill.
    placeholder = PixelLayer.frompil(
        Image.new("RGBA", (32, 24), (0, 0, 0, 0)), psd, "main_placeholder", 8, 16
    )
    psd.append(placeholder)
    # A decorative border that must stay on top of the generated content.
    border = Image.new("RGBA", (64, 48), (0, 0, 0, 0))
    border.paste((255, 0, 0, 255), (0, 0, 64, 2))
    decoration = PixelLayer.frompil(border, psd, "decoration", 0, 0)
    psd.append(decoration)
    psd.save(str(path))


work_dir = Path(tempfile.mkdtemp(prefix="hgripe-psd-"))
template_path = work_dir / "template.psd"
build_template(template_path)

# Load
template, preview, info_json = HGripePsdTemplateLoad().run(psd_path=str(template_path))
info = json.loads(info_json)

# Compose (phase 2): place into the named placeholder slot, hide the placeholder,
# attach a mask, add candidates with a chosen visible one, and write a metadata layer.
generated = torch.zeros((1, 24, 32, 3), dtype=torch.float32)
generated[:, :, :, 2] = 1.0  # blue
reference = torch.zeros((1, 16, 16, 3), dtype=torch.float32)
reference[:, :, :, 1] = 1.0  # green
candidates = torch.zeros((2, 24, 32, 3), dtype=torch.float32)
candidates[0, :, :, 0] = 1.0  # candidate_02 red
candidates[1, :, :, 1] = 1.0  # candidate_03 green
# Mask: left half visible, right half hidden.
mask = torch.zeros((1, 24, 32), dtype=torch.float32)
mask[:, :, :16] = 1.0

composed, composed_preview, metadata_json = HGripePsdCompose().run(
    template=template,
    image=generated,
    placeholder_json=json.dumps({"name": "main_placeholder"}),
    generated_layer_name="generated",
    fit_mode="stretch",
    z_order="placeholder",
    image_index=0,
    reference_image=reference,
    candidates=candidates,
    mask=mask,
    mask_index=0,
    hide_placeholder="enable",
    visible_candidate=2,
    write_metadata_layer="enable",
    metadata_json=json.dumps({"prompt": "a blue square", "seed": 42}),
)

# Export
psd_path, preview_path, metadata_path, status = HGripePsdExport().run(
    composed=composed,
    output_dir=str(work_dir / "out"),
    filename="final",
    save_preview="enable",
)

reloaded = PSDImage.open(psd_path)


def walk(node):
    names = []
    for layer in node:
        names.append(layer.name)
        if layer.is_group():
            names.extend(walk(layer))
    return names


def visibility(node):
    state = {}
    for layer in node:
        state[layer.name] = bool(layer.visible)
        if layer.is_group():
            state.update(visibility(layer))
    return state


metadata = json.loads(Path(metadata_path).read_text(encoding="utf-8"))
generated_has_mask = False
for layer in reloaded.descendants():
    if layer.name == "generated":
        generated_has_mask = bool(layer.has_mask())
vis = visibility(reloaded)
print(
    {
        "status": status,
        "template_layers": [layer["name"] for layer in info["layers"]],
        "composed_preview_shape": tuple(composed_preview.shape),
        "exported_layers": walk(reloaded),
        "generated_has_mask": generated_has_mask,
        "placeholder_hidden": vis.get("main_placeholder"),
        "main_visible": vis.get("generated"),
        "candidate_02_visible": vis.get("candidate_02"),
        "has_meta_group": "00_META" in vis,
        "psd_exists": Path(psd_path).is_file(),
        "preview_exists": Path(preview_path).is_file(),
        "metadata_applied_mask": metadata.get("applied_mask"),
        "metadata_visible_candidate": metadata.get("visible_candidate"),
        "metadata_placeholder": metadata.get("placeholder"),
    }
)
