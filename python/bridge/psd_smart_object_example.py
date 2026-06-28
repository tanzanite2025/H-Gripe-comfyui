"""Offline demo: true smart-object content replacement in H-Gripe PSD Compose.

Builds a self-contained template PSD whose placeholder is an *embedded smart
object* (not a flat pixel layer), then runs Load -> Compose -> Export with
``smart_object_mode="replace_content"`` and verifies that the generated image
ends up *inside* the smart object (still editable in Photoshop) while the
template's top decoration is preserved.

The template is synthesized from scratch using the vendored ``psd_tools`` so the
example needs no external assets and no network access.
"""

from __future__ import annotations

import io
import json
import sys
import tempfile
import uuid as uuidlib
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
from psd_tools.api.layers import PixelLayer, SmartObjectLayer
from psd_tools.constants import LinkedLayerType, Tag
from psd_tools.psd.descriptor import DescriptorBlock, String
from psd_tools.psd.linked_layer import LinkedLayer, LinkedLayers

from custom_nodes.hgripe_psd_nodes import (
    HGripePsdCompose,
    HGripePsdExport,
    HGripePsdTemplateLoad,
)

SMART_NAME = "smart_placeholder"
SMART_BOX = (8, 16, 32, 24)  # left, top, width, height


def _attach_embedded_smart_object(psd: PSDImage, layer: PixelLayer, content: Image.Image) -> None:
    """Turn ``layer`` into an embedded smart object holding ``content``.

    Adds the layer-level ``SoLd`` descriptor and a matching document-level
    ``lnkD`` (embedded data) block, mirroring how Photoshop stores an embedded
    smart object. This is what makes the placeholder a real smart object that
    ``replace_content`` can rewrite.
    """
    unique_id = str(uuidlib.uuid4())
    buffer = io.BytesIO()
    content.convert("RGBA").save(buffer, format="PNG")

    layer._record.tagged_blocks.set_data(
        Tag.SMART_OBJECT_LAYER_DATA2,
        kind=b"soLD",
        version=5,
        data=DescriptorBlock(items=[(b"Idnt", String(value=unique_id))]),
    )
    linked = LinkedLayer(
        kind=LinkedLayerType.DATA,
        version=2,
        uuid=unique_id,
        filename="placeholder.png",
        filetype=b"png ",
        creator=b"8BPB",
        data=buffer.getvalue(),
    )
    doc_blocks = psd._record.layer_and_mask_information.tagged_blocks
    doc_blocks.set_data(Tag.LINKED_LAYER1, LinkedLayers([linked]))


def build_template(path: Path) -> None:
    psd = PSDImage.new(mode="RGBA", size=(64, 48))
    background = PixelLayer.frompil(
        Image.new("RGBA", (64, 48), (240, 240, 240, 255)), psd, "background", 0, 0
    )
    psd.append(background)

    left, top, width, height = SMART_BOX
    # The placeholder is a smart object whose original content is a grey box.
    placeholder = PixelLayer.frompil(
        Image.new("RGBA", (width, height), (180, 180, 180, 255)), psd, SMART_NAME, top, left
    )
    psd.append(placeholder)
    _attach_embedded_smart_object(psd, placeholder, Image.new("RGBA", (width, height), (180, 180, 180, 255)))

    # Decorative border that must stay on top of the replaced smart object.
    border = Image.new("RGBA", (64, 48), (0, 0, 0, 0))
    border.paste((255, 0, 0, 255), (0, 0, 64, 2))
    decoration = PixelLayer.frompil(border, psd, "decoration", 0, 0)
    psd.append(decoration)

    psd._update_record()
    psd.save(str(path))


work_dir = Path(tempfile.mkdtemp(prefix="hgripe-psd-so-"))
template_path = work_dir / "template.psd"
build_template(template_path)

# Confirm the template placeholder really is an embedded smart object.
template_psd = PSDImage.open(str(template_path))
template_layer = next(layer for layer in template_psd if layer.name == SMART_NAME)
template_is_smart_object = isinstance(template_layer, SmartObjectLayer)
template_so_kind = template_layer.smart_object.kind

# Load -> Compose (replace the smart object's content) -> Export.
template, _preview, info_json = HGripePsdTemplateLoad().run(psd_path=str(template_path))
info = json.loads(info_json)

generated = torch.zeros((1, 24, 32, 3), dtype=torch.float32)
generated[:, :, :, 2] = 1.0  # solid blue

composed, composed_preview, metadata_json = HGripePsdCompose().run(
    template=template,
    image=generated,
    placeholder_json=json.dumps({"name": SMART_NAME}),
    generated_layer_name="generated",
    fit_mode="stretch",
    z_order="placeholder",
    image_index=0,
    smart_object_mode="replace_content",
    metadata_json=json.dumps({"prompt": "a blue square", "seed": 7}),
)
metadata = json.loads(metadata_json)

psd_path, preview_path, metadata_path, status = HGripePsdExport().run(
    composed=composed,
    output_dir=str(work_dir / "out"),
    filename="final",
    save_preview="enable",
)

reloaded = PSDImage.open(psd_path)
target = next(layer for layer in reloaded if layer.name == SMART_NAME)
still_smart_object = isinstance(target, SmartObjectLayer)
embedded = Image.open(io.BytesIO(target.smart_object.data)).convert("RGBA")
embedded_center = embedded.getpixel((embedded.width // 2, embedded.height // 2))

# Composite at the smart object's centre should now be the generated blue.
ph_left, ph_top, ph_w, ph_h = SMART_BOX
composite = reloaded.composite().convert("RGB")
composite_center = composite.getpixel((ph_left + ph_w // 2, ph_top + ph_h // 2))
# Decoration (top red border) must survive on top.
top_border = composite.getpixel((10, 0))

print(
    {
        "status": status,
        "template_is_smart_object": template_is_smart_object,
        "template_so_kind": template_so_kind,
        "template_layers": [layer["name"] for layer in info["layers"]],
        "composed_preview_shape": tuple(composed_preview.shape),
        "metadata_smart_object_mode": metadata.get("smart_object_mode"),
        "metadata_placeholder_kind": metadata.get("placeholder_kind"),
        "reloaded_still_smart_object": still_smart_object,
        "embedded_filetype": target.smart_object.filetype,
        "embedded_center_is_blue": embedded_center == (0, 0, 255, 255),
        "composite_center_is_blue": composite_center == (0, 0, 255),
        "decoration_preserved": top_border == (255, 0, 0),
        "psd_exists": Path(psd_path).is_file(),
        "preview_exists": Path(preview_path).is_file(),
    }
)
