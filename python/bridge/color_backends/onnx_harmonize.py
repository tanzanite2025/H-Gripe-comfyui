"""ONNX image-harmonisation matcher (opt-in, via ``onnxruntime``).

This is the first learned ``engine`` for the Match Light & Color node. Where the
Phase 1 heuristic nudges the subject's Lab statistics toward the background, a
learned harmoniser predicts a per-pixel correction that keeps brand colours and
material cues consistent while matching the background's light & colour. It stays
**opt-in**: the backend is only used when the node's ``engine`` param is
``onnx_harmonize`` *and* both the optional dep (``onnxruntime``) and the model
weight are present; otherwise the caller keeps the always-available heuristic
result.

Nothing heavy is imported at module load — ``onnxruntime`` is only touched inside
:meth:`available` (via :func:`importlib.util.find_spec`) and :meth:`match`.

Weight resolution order:
1. ``HGRIPE_COLOR_MODEL`` (explicit path, for dev / CI), else
2. ``<model cache>/color_harmonize.onnx`` where the cache dir is
   ``HGRIPE_MODEL_CACHE`` or the bundled ``resources/models`` dir.

The weight is **not** bundled in the installer; ``scripts`` can fetch it into the
cache dir, exactly like the SAM 2 / ViTMatte / Real-ESRGAN weights.

Model contract (a standard foreground-harmonisation network):
* inputs:
  - ``image`` ``[1, 3, H, W]`` float32 RGB in ``0..1`` — the subject composited
    over the (resized) background reference, so the network sees both.
  - ``mask`` ``[1, 1, H, W]`` float32 — the subject alpha (``1`` = foreground).
  The network's fixed spatial size is read from the model; a dynamic axis falls
  back to ``_DEFAULT_SIZE``. The composite + mask are resized into that size and
  the output is resized back so the caller's geometry contract is unchanged.
* output: ``[1, 3, H, W]`` float32 RGB in ``0..1`` — the harmonised image.
  Matched by name (``output`` / ``harmonized``), falling back to positional [0].
"""

from __future__ import annotations

import importlib.util
import os
from pathlib import Path
from typing import Any

from sr_backends import onnx_providers

from . import MatcherUnavailable, model_cache_dir

_DEFAULT_WEIGHT_NAME = "color_harmonize.onnx"
# Fallback square size when the model declares a dynamic spatial axis.
_DEFAULT_SIZE = 512


class OnnxHarmonizeBackend:
    id = "onnx_harmonize"

    def weight_path(self) -> Path:
        override = (os.environ.get("HGRIPE_COLOR_MODEL") or "").strip()
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
                "(set HGRIPE_COLOR_MODEL or fetch into HGRIPE_MODEL_CACHE)",
            )
        return True, "ready"

    def match(self, rgb: Any, alpha: Any, background_rgb: Any) -> Any:
        """Harmonise ``rgb`` toward ``background_rgb`` over the ``alpha`` matte.

        Raises :class:`MatcherUnavailable` if deps / weights vanished since the
        probe. The subject is composited over the resized background so the
        network sees context; only the subject geometry is returned.
        """
        ok, reason = self.available()
        if not ok:
            raise MatcherUnavailable(reason)

        import numpy as np
        import onnxruntime as ort

        weight = self.weight_path()
        session = ort.InferenceSession(
            str(weight), providers=onnx_providers(ort.get_available_providers())
        )
        inputs = session.get_inputs()
        image_spec = _pick_input(inputs, "image", "input")
        mask_spec = _pick_input(inputs, "mask")

        # Spatial dims are the trailing two axes of an NCHW input; a non-int
        # (dynamic) axis falls back to the default square size.
        net_h = image_spec.shape[2] if isinstance(image_spec.shape[2], int) else _DEFAULT_SIZE
        net_w = image_spec.shape[3] if isinstance(image_spec.shape[3], int) else _DEFAULT_SIZE

        src_h, src_w = rgb.shape[:2]
        composite = _composite_over_background(rgb, alpha, background_rgb, np)

        image_t = _to_nchw(composite, net_w, net_h, np)
        feed: dict[str, Any] = {image_spec.name: image_t}
        if mask_spec is not None:
            mask_u8 = np.rint(np.clip(alpha, 0.0, 1.0) * 255.0).astype(np.uint8)
            mask_t = _to_nchw(mask_u8[..., None], net_w, net_h, np, channels=1)
            feed[mask_spec.name] = mask_t

        raw = session.run(None, feed)
        out = _named_output(session, raw)
        # NCHW float -> HWC float in 0..1, resized back to the source geometry.
        out_hwc = np.transpose(np.asarray(out)[0], (1, 2, 0))
        harmonized = _resize_hwc(out_hwc, src_w, src_h, np)
        return np.clip(np.rint(harmonized * 255.0), 0, 255).astype(np.uint8)


def _pick_input(inputs: list[Any], *keys: str) -> Any | None:
    """Return the model input whose name contains one of ``keys`` (else None).

    The first call passes the primary keys and is expected to match; the mask
    input is optional, so a miss returns ``None`` and the mask is simply not fed.
    """
    for key in keys:
        for spec in inputs:
            if key in spec.name.lower():
                return spec
    # Fall back to the first input for the primary image when nothing matched.
    if keys and keys[0] in ("image", "input"):
        return inputs[0]
    return None


def _composite_over_background(rgb: Any, alpha: Any, background_rgb: Any, np: Any) -> Any:
    """Composite the subject over a cover-resized background, as an HWC uint8.

    Gives the harmoniser background context at the subject's geometry. With no
    background reference the subject is returned unchanged.
    """
    if background_rgb is None:
        return rgb
    src_h, src_w = rgb.shape[:2]
    bg = _resize_hwc(background_rgb.astype(np.float32) / 255.0, src_w, src_h, np)
    a = np.clip(alpha, 0.0, 1.0)[..., None]
    fg = rgb.astype(np.float32) / 255.0
    comp = fg * a + bg * (1.0 - a)
    return np.clip(np.rint(comp * 255.0), 0, 255).astype(np.uint8)


def _resize_hwc(hwc: Any, width: int, height: int, np: Any) -> Any:
    """Bilinear resize an HWC float (0..1) array to ``width x height``."""
    from PIL import Image

    src_h, src_w = hwc.shape[:2]
    if (src_w, src_h) == (width, height):
        return hwc
    u8 = np.clip(np.rint(hwc * 255.0), 0, 255).astype(np.uint8)
    mode = "L" if u8.ndim == 2 or u8.shape[2] == 1 else "RGB"
    arr = u8[..., 0] if mode == "L" and u8.ndim == 3 else u8
    img = Image.fromarray(arr, mode).resize((width, height), Image.BILINEAR)
    out = np.asarray(img, dtype=np.float32) / 255.0
    return out[..., None] if mode == "L" else out


def _to_nchw(hwc_u8: Any, net_w: int, net_h: int, np: Any, channels: int = 3) -> Any:
    """Resize an HWC uint8 array into a ``[1, C, net_h, net_w]`` float32 tensor."""
    resized = _resize_hwc(hwc_u8.astype(np.float32) / 255.0, net_w, net_h, np)
    if channels == 1 and resized.ndim == 3 and resized.shape[2] != 1:
        resized = resized[..., :1]
    tensor = np.transpose(resized, (2, 0, 1))[None, ...].astype(np.float32)
    return tensor


def _named_output(session: Any, raw: list[Any]) -> Any:
    """Pick the harmonised image from the session outputs by name.

    Falls back to the first output when the model does not name it.
    """
    names = [o.name.lower() for o in session.get_outputs()]
    for key in ("harmoniz", "output", "image", "result"):
        for name, value in zip(names, raw):
            if key in name:
                return value
    return raw[0]
