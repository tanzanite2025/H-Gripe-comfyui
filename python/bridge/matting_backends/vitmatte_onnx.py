"""ONNX alpha-matting matter (opt-in, via ``onnxruntime``).

This is the first learned ``engine`` for the Mask Edge Refine node. Where the
Phase 1 heuristic snaps the matte to luminance edges with a guided filter, a
learned matter (ViTMatte / IndexNet / MODNet-style) solves true continuous alpha
in the trimap's unknown band, recovering hair / fur / glass detail the guided
filter flattens. It stays **opt-in**: the backend is only used when the node's
``engine`` param is ``onnx_matting`` *and* both the optional dep
(``onnxruntime``) and the model weight are present; otherwise the caller keeps
the always-available heuristic result.

Nothing heavy is imported at module load — ``onnxruntime`` is only touched inside
:meth:`available` (via :func:`importlib.util.find_spec`) and :meth:`matte`.

Weight resolution order:
1. ``HGRIPE_MATTING_MODEL`` (explicit path, for dev / CI), else
2. ``<model cache>/matting.onnx`` where the cache dir is ``HGRIPE_MODEL_CACHE``
   or the bundled ``resources/models`` dir.

The weight is **not** bundled in the installer; ``scripts`` can fetch it into the
cache dir, exactly like the SAM 2 / ViTMatte / Real-ESRGAN weights.

Model contract (a standard trimap-based image-matting network):
* inputs:
  - ``image`` ``[1, 3, H, W]`` float32 RGB in ``0..1``.
  - ``trimap`` ``[1, 1, H, W]`` float32 in ``0..1`` (``0`` bg / ``0.5`` unknown /
    ``1`` fg). The network's fixed spatial size is read from the model; a dynamic
    axis falls back to ``_DEFAULT_SIZE``. Image + trimap are resized into that
    size and the predicted alpha is resized back so the caller's geometry
    contract is unchanged.
* output: ``[1, 1, H, W]`` float32 alpha in ``0..1``. Matched by name
  (``alpha`` / ``output`` / ``matte``), falling back to positional [0].
"""

from __future__ import annotations

import importlib.util
import os
from pathlib import Path
from typing import Any

from . import MattingUnavailable, model_cache_dir

_DEFAULT_WEIGHT_NAME = "matting.onnx"
# Fallback square size when the model declares a dynamic spatial axis.
_DEFAULT_SIZE = 512


class OnnxMattingBackend:
    id = "onnx_matting"

    def weight_path(self) -> Path:
        override = (os.environ.get("HGRIPE_MATTING_MODEL") or "").strip()
        if override:
            return Path(override)
        return model_cache_dir() / _DEFAULT_WEIGHT_NAME

    def available(self) -> tuple[bool, str]:
        """Cheap probe: optional dep importable + weight present on disk.

        Uses ``find_spec`` so we never actually import ``onnxruntime`` (which can
        pull in heavy native providers) just to report availability.
        """
        if importlib.util.find_spec("onnxruntime") is None:
            return False, "missing optional dependency: onnxruntime"
        weight = self.weight_path()
        if not weight.is_file():
            return (
                False,
                f"weight not found: {weight} "
                "(set HGRIPE_MATTING_MODEL or fetch into HGRIPE_MODEL_CACHE)",
            )
        return True, "ready"

    def matte(self, rgb: Any, trimap: Any) -> Any:
        """Solve a refined alpha from ``rgb`` and ``trimap``.

        Raises :class:`MattingUnavailable` if deps / weights vanished since the
        probe. Only the alpha is returned, at the source geometry.
        """
        ok, reason = self.available()
        if not ok:
            raise MattingUnavailable(reason)

        import numpy as np
        import onnxruntime as ort

        weight = self.weight_path()
        session = ort.InferenceSession(
            str(weight), providers=["CPUExecutionProvider"]
        )
        inputs = session.get_inputs()
        image_spec = _pick_input(inputs, "image", "input")
        trimap_spec = _pick_input(inputs, "trimap", "mask")

        # Spatial dims are the trailing two axes of an NCHW input; a non-int
        # (dynamic) axis falls back to the default square size.
        net_h = image_spec.shape[2] if isinstance(image_spec.shape[2], int) else _DEFAULT_SIZE
        net_w = image_spec.shape[3] if isinstance(image_spec.shape[3], int) else _DEFAULT_SIZE

        src_h, src_w = rgb.shape[:2]
        image_t = _to_nchw(rgb, net_w, net_h, np, channels=3)
        feed: dict[str, Any] = {image_spec.name: image_t}
        if trimap_spec is not None:
            tri = np.clip(trimap, 0.0, 1.0).astype(np.float32)
            tri_t = _to_nchw((tri[..., None] * 255.0).astype(np.uint8), net_w, net_h, np, channels=1)
            feed[trimap_spec.name] = tri_t

        raw = session.run(None, feed)
        out = _named_output(session, raw)
        # NCHW float -> HW float in 0..1, resized back to the source geometry.
        out_arr = np.asarray(out)[0]
        out_hw = out_arr[0] if out_arr.ndim == 3 else out_arr
        alpha = _resize_hw(out_hw, src_w, src_h, np)
        return np.clip(alpha, 0.0, 1.0).astype(np.float32)


def _pick_input(inputs: list[Any], *keys: str) -> Any | None:
    """Return the model input whose name contains one of ``keys`` (else None).

    The first call passes the primary keys and is expected to match; the trimap
    input is optional, so a miss returns ``None`` and the trimap is simply not
    fed.
    """
    for key in keys:
        for spec in inputs:
            if key in spec.name.lower():
                return spec
    # Fall back to the first input for the primary image when nothing matched.
    if keys and keys[0] in ("image", "input"):
        return inputs[0]
    return None


def _resize_hw(hw: Any, width: int, height: int, np: Any) -> Any:
    """Bilinear resize an (H,W) float (0..1) array to ``width x height``."""
    from PIL import Image

    src_h, src_w = hw.shape[:2]
    if (src_w, src_h) == (width, height):
        return hw
    u8 = np.clip(np.rint(hw * 255.0), 0, 255).astype(np.uint8)
    img = Image.fromarray(u8, "L").resize((width, height), Image.BILINEAR)
    return np.asarray(img, dtype=np.float32) / 255.0


def _to_nchw(hwc_u8: Any, net_w: int, net_h: int, np: Any, channels: int = 3) -> Any:
    """Resize an HWC uint8 array into a ``[1, C, net_h, net_w]`` float32 tensor."""
    from PIL import Image

    arr = hwc_u8
    if channels == 1 and arr.ndim == 3:
        arr = arr[..., 0]
    mode = "L" if channels == 1 else "RGB"
    img = Image.fromarray(arr.astype(np.uint8), mode)
    if img.size != (net_w, net_h):
        img = img.resize((net_w, net_h), Image.BILINEAR)
    resized = np.asarray(img, dtype=np.float32) / 255.0
    if channels == 1:
        resized = resized[..., None]
    tensor = np.transpose(resized, (2, 0, 1))[None, ...].astype(np.float32)
    return tensor


def _named_output(session: Any, raw: list[Any]) -> Any:
    """Pick the alpha from the session outputs by name.

    Falls back to the first output when the model does not name it.
    """
    names = [o.name.lower() for o in session.get_outputs()]
    for key in ("alpha", "matte", "output", "pha"):
        for name, value in zip(names, raw):
            if key in name:
                return value
    return raw[0]
