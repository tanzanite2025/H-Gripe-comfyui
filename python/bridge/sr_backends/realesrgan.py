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

from . import BackendUnavailable, model_cache_dir, resolve_device, resolve_precision

_DEFAULT_WEIGHT_NAME = "RealESRGAN_x4plus.pth"

#: Process-global warm cache of constructed Real-ESRGAN upsamplers, keyed by
#: ``(weight, device, precision)``. Building a ``RealESRGANer`` reads the ~64 MB
#: weight and moves it onto the device; in a long-lived host (the torch worker,
#: staged-rollout step 4 of ``docs/cards/editor-resource-model.md``) this cache
#: means that happens once per ``(weight, device, precision)`` instead of on
#: every run. In a one-shot CLI process it is simply built once and discarded.
_WARM_UPSAMPLERS: dict[tuple[str, str, str], Any] = {}


def _construct_upsampler(weight: str, native_scale: int, device: str, precision: str) -> Any:
    """Build a ``RealESRGANer`` (imports the heavy torch deps lazily).

    Split out from :func:`_warm_upsampler` so the warm cache can be exercised in
    tests without ``torch`` / ``realesrgan`` installed (the test monkeypatches
    this constructor and counts calls).
    """
    from basicsr.archs.rrdbnet_arch import RRDBNet
    from realesrgan import RealESRGANer

    model = RRDBNet(
        num_in_ch=3,
        num_out_ch=3,
        num_feat=64,
        num_block=23,
        num_grow_ch=32,
        scale=native_scale,
    )
    return RealESRGANer(
        scale=native_scale,
        model_path=weight,
        model=model,
        tile=512,
        tile_pad=10,
        pre_pad=0,
        # fp16 only helps on CUDA; resolve_precision already degraded an
        # explicit fp16 to fp32 on a CPU run, so this is safe.
        half=(precision == "fp16"),
        device=device,
    )


def _warm_upsampler(weight: str, native_scale: int, device: str, precision: str) -> Any:
    """Return a cached ``RealESRGANer`` for the key, building it on first use."""
    key = (weight, device, precision)
    cached = _WARM_UPSAMPLERS.get(key)
    if cached is not None:
        return cached
    built = _construct_upsampler(weight, native_scale, device, precision)
    _WARM_UPSAMPLERS[key] = built
    return built


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

    def upscale(
        self,
        rgb: Any,
        scale: float,
        device: str | None = None,
        precision: str | None = None,
    ) -> tuple[Any, str, str]:
        """Upscale a PIL ``RGB`` image by ``scale`` using Real-ESRGAN x4.

        The model has a fixed native ``x4`` factor; we run it once and then
        Lanczos-resize to the exact requested factor so the caller's
        target-size contract is unchanged. Tiling keeps VRAM bounded on large
        inputs. ``device`` selects the compute device (``auto`` by default) and
        ``precision`` the compute precision (``auto`` by default — fp16 on CUDA,
        fp32 on CPU); returns ``(image, device_used, precision_used)`` so the
        caller reports what actually ran. Raises :class:`BackendUnavailable` if
        deps/weights vanished since the probe.
        """
        ok, reason = self.available()
        if not ok:
            raise BackendUnavailable(reason)

        import numpy as np
        import torch
        from PIL import Image

        device = resolve_device(device, torch.cuda.is_available())
        precision = resolve_precision(precision, device)
        # Reuse a warm upsampler when the host is long-lived (the torch worker);
        # a one-shot CLI run just builds it once. The ~64 MB weight load / device
        # move therefore happens once per (weight, device, precision) instead of
        # on every call.
        upsampler = _warm_upsampler(
            str(self.weight_path()), self.native_scale, device, precision
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
        return enhanced, device, precision
