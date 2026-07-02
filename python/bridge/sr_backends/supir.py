"""SupIR super-resolution backend (opt-in, diffusion-based, GPU/CPU via torch).

SupIR is the roadmap's "state of the art perceptual quality" diffusion SR
option — heavy, SDXL-scale, capable of restoring detail beyond what the input
carries. Like every engine on this seam it stays **opt-in**: the backend only
runs when the node's ``engine`` param is ``supir`` *and* the optional deps
(``torch`` + ``diffusers``) and the weight snapshot are present; otherwise the
caller falls back to the always-available CPU Lanczos path with a recorded
reason.

Nothing heavy is imported at module load; the loading/inference path is the
shared diffusion SR helper in :mod:`sr_backends.ccsr` (generic
``DiffusionPipeline.from_pretrained`` over the snapshot's declared pipeline,
with the process-global warm cache).

Weight resolution order:
1. ``HGRIPE_SUPIR_MODEL`` (explicit snapshot dir, for dev / CI), else
2. ``<model cache>/supir`` where the cache dir is ``HGRIPE_MODEL_CACHE`` or the
   bundled ``resources/models`` dir.
"""

from __future__ import annotations

import os
from pathlib import Path
from typing import Any

from . import model_cache_dir
from .ccsr import diffusion_sr_available, diffusion_sr_upscale

_DEFAULT_WEIGHT_DIR = "supir"


class SupirBackend:
    id = "supir"
    native_scale = 4

    def weight_path(self) -> Path:
        override = (os.environ.get("HGRIPE_SUPIR_MODEL") or "").strip()
        if override:
            return Path(override)
        return model_cache_dir() / _DEFAULT_WEIGHT_DIR

    def available(self) -> tuple[bool, str]:
        return diffusion_sr_available(self.weight_path())

    def upscale(
        self,
        rgb: Any,
        scale: float,
        device: str | None = None,
        precision: str | None = None,
    ) -> tuple[Any, str, str]:
        """Upscale a PIL ``RGB`` image by ``scale`` using the SupIR snapshot.

        Raises :class:`BackendUnavailable` if deps/weights vanished since the
        probe (the caller degrades to the CPU path with the recorded reason).
        """
        return diffusion_sr_upscale(self, rgb, scale, device, precision)
