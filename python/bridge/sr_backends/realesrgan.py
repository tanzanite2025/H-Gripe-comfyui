"""Real-ESRGAN super-resolution backend (opt-in, GPU/CPU via torch).

This is the first non-CPU ``engine`` for the Image Enhance node. It is the
roadmap's recommended first integration target (light, deterministic, fast,
strong mid-tier quality). It stays **opt-in**: the backend is only used when the
node's ``engine`` param is ``realesrgan`` *and* both the optional deps
(``torch`` + ``realesrgan``) and the model weight are present; otherwise the
caller falls back to the always-available CPU Lanczos path.

Nothing heavy is imported at module load — ``torch`` is only touched inside
:meth:`available` (via :func:`importlib.util.find_spec`) and :meth:`upscale`.

Weight resolution order:
1. ``HGRIPE_REALESRGAN_MODEL`` (explicit path, for dev / CI), else
2. ``<model cache>/RealESRGAN_x4plus.pth`` where the cache dir is
   ``HGRIPE_MODEL_CACHE`` or the bundled ``resources/models`` dir.

The weight is **not** bundled in the installer (it is ~64 MB); ``scripts`` can
fetch it into the cache dir, exactly like the SAM 2 / ViTMatte weights.
"""

from __future__ import annotations

import importlib.util
import os
from pathlib import Path
from typing import Any

from . import BackendUnavailable, model_cache_dir

_DEFAULT_WEIGHT_NAME = "RealESRGAN_x4plus.pth"


class RealEsrganBackend:
    id = "realesrgan"
    native_scale = 4

    def weight_path(self) -> Path:
        override = (os.environ.get("HGRIPE_REALESRGAN_MODEL") or "").strip()
        if override:
            return Path(override)
        return model_cache_dir() / _DEFAULT_WEIGHT_NAME

    def available(self) -> tuple[bool, str]:
        """Cheap probe: optional deps importable + weight present on disk.

        Uses ``find_spec`` so we never actually import ``torch`` (slow, and may
        pull in CUDA) just to report availability.
        """
        for dep in ("torch", "realesrgan", "basicsr"):
            if importlib.util.find_spec(dep) is None:
                return False, f"missing optional dependency: {dep}"
        weight = self.weight_path()
        if not weight.is_file():
            return (
                False,
                f"weight not found: {weight} "
                "(set HGRIPE_REALESRGAN_MODEL or fetch into HGRIPE_MODEL_CACHE)",
            )
        return True, "ready"

    def upscale(self, rgb: Any, scale: float) -> Any:
        """Upscale a PIL ``RGB`` image by ``scale`` using Real-ESRGAN x4.

        The model has a fixed native ``x4`` factor; we run it once and then
        Lanczos-resize to the exact requested factor so the caller's
        target-size contract is unchanged. Tiling keeps VRAM bounded on large
        inputs. Raises :class:`BackendUnavailable` if deps/weights vanished
        since the probe.
        """
        ok, reason = self.available()
        if not ok:
            raise BackendUnavailable(reason)

        import numpy as np
        import torch
        from basicsr.archs.rrdbnet_arch import RRDBNet
        from PIL import Image
        from realesrgan import RealESRGANer

        device = "cuda" if torch.cuda.is_available() else "cpu"
        model = RRDBNet(
            num_in_ch=3,
            num_out_ch=3,
            num_feat=64,
            num_block=23,
            num_grow_ch=32,
            scale=self.native_scale,
        )
        upsampler = RealESRGANer(
            scale=self.native_scale,
            model_path=str(self.weight_path()),
            model=model,
            tile=512,
            tile_pad=10,
            pre_pad=0,
            # fp16 only helps on CUDA; keep full precision on CPU for determinism.
            half=(device == "cuda"),
            device=device,
        )

        src = np.asarray(rgb.convert("RGB"))
        # RealESRGANer expects BGR (it wraps cv2 conventions); convert in/out.
        bgr = src[:, :, ::-1]
        out_bgr, _ = upsampler.enhance(bgr, outscale=self.native_scale)
        out_rgb = np.ascontiguousarray(out_bgr[:, :, ::-1])
        enhanced = Image.fromarray(out_rgb, mode="RGB")

        # Resize from the native x4 result to the exact requested factor.
        target_w = max(1, int(round(rgb.width * scale)))
        target_h = max(1, int(round(rgb.height * scale)))
        if enhanced.size != (target_w, target_h):
            enhanced = enhanced.resize((target_w, target_h), Image.LANCZOS)
        return enhanced
