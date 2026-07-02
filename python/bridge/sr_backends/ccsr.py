"""CCSR super-resolution backend (opt-in, diffusion-based, GPU/CPU via torch).

CCSR (Content-Consistent Super-Resolution) is the roadmap's "more faithful /
less hallucinated" diffusion SR option — a good fit for product photography
where over-invention is a risk. Like every engine on this seam it stays
**opt-in**: the backend only runs when the node's ``engine`` param is ``ccsr``
*and* the optional deps (``torch`` + ``diffusers``) and the weight snapshot are
present; otherwise the caller falls back to the always-available CPU Lanczos
path with a recorded reason.

Nothing heavy is imported at module load — ``torch`` / ``diffusers`` are only
touched inside :meth:`available` (via :func:`importlib.util.find_spec`) and
:meth:`upscale`.

Weight resolution order:
1. ``HGRIPE_CCSR_MODEL`` (explicit snapshot dir, for dev / CI), else
2. ``<model cache>/ccsr`` where the cache dir is ``HGRIPE_MODEL_CACHE`` or the
   bundled ``resources/models`` dir.

The weight is **not** bundled (multi-GB); a fetch script places the exported
diffusers-format snapshot into the cache dir, exactly like the inpaint weights.
The snapshot declares its own pipeline class, so it is loaded generically via
``DiffusionPipeline.from_pretrained`` and called with the image-to-image
convention every diffusion SR export shares (``pipe(prompt, image=...)``).
"""

from __future__ import annotations

import importlib.util
import os
from pathlib import Path
from typing import Any

from . import BackendUnavailable, model_cache_dir, resolve_device, resolve_precision

_DEFAULT_WEIGHT_DIR = "ccsr"

#: Process-global warm cache of constructed diffusion SR pipelines, keyed by
#: ``(weight, device, precision)`` — shared by the CCSR and SupIR backends so a
#: long-lived host (the torch worker) loads each multi-GB snapshot once.
_WARM_PIPELINES: dict[tuple[str, str, str], Any] = {}


def _construct_pipeline(weight: str, device: str, precision: str) -> Any:
    """Load a diffusion SR pipeline snapshot (imports the heavy deps lazily).

    Split out so the warm cache can be exercised in tests without ``torch`` /
    ``diffusers`` installed (the test monkeypatches this constructor).
    """
    import torch
    from diffusers import DiffusionPipeline

    dtype = torch.float16 if precision == "fp16" else torch.float32
    pipe = DiffusionPipeline.from_pretrained(weight, torch_dtype=dtype)
    return pipe.to(device)


def warm_pipeline(weight: str, device: str, precision: str) -> Any:
    """Return a cached diffusion SR pipeline for the key, building on first use."""
    key = (weight, device, precision)
    cached = _WARM_PIPELINES.get(key)
    if cached is not None:
        return cached
    built = _construct_pipeline(weight, device, precision)
    _WARM_PIPELINES[key] = built
    return built


def diffusion_sr_available(weight: Path) -> tuple[bool, str]:
    """Shared cheap probe for the diffusion SR engines: deps + snapshot dir.

    Uses ``find_spec`` so ``torch`` is never actually imported (slow, may pull
    in CUDA) just to report availability. The weight is a diffusers-format
    snapshot *directory* (multi-file), unlike Real-ESRGAN's single ``.pth``.
    """
    for dep in ("torch", "diffusers"):
        if importlib.util.find_spec(dep) is None:
            return False, f"missing optional dependency: {dep}"
    if not weight.is_dir():
        return (
            False,
            f"weight snapshot not found: {weight} "
            "(set the engine's model env var or fetch into HGRIPE_MODEL_CACHE)",
        )
    return True, "ready"


def diffusion_sr_upscale(
    backend: Any,
    rgb: Any,
    scale: float,
    device: str | None,
    precision: str | None,
) -> tuple[Any, str, str]:
    """Shared upscale path for the diffusion SR engines (CCSR / SupIR).

    Runs the snapshot's pipeline once at its native factor, then Lanczos-resizes
    to the exact requested factor so the caller's target-size contract is
    unchanged. Returns ``(image, device_used, precision_used)`` so the caller
    reports truthfully what ran.
    """
    ok, reason = backend.available()
    if not ok:
        raise BackendUnavailable(reason)

    import torch
    from PIL import Image

    device = resolve_device(device, torch.cuda.is_available())
    precision = resolve_precision(precision, device)
    pipe = warm_pipeline(str(backend.weight_path()), device, precision)

    src = rgb.convert("RGB")
    enhanced = pipe(prompt="", image=src).images[0].convert("RGB")

    target_w = max(1, int(round(rgb.width * scale)))
    target_h = max(1, int(round(rgb.height * scale)))
    if enhanced.size != (target_w, target_h):
        enhanced = enhanced.resize((target_w, target_h), Image.LANCZOS)
    return enhanced, device, precision


class CcsrBackend:
    id = "ccsr"
    native_scale = 4

    def weight_path(self) -> Path:
        override = (os.environ.get("HGRIPE_CCSR_MODEL") or "").strip()
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
        """Upscale a PIL ``RGB`` image by ``scale`` using the CCSR snapshot.

        Raises :class:`BackendUnavailable` if deps/weights vanished since the
        probe (the caller degrades to the CPU path with the recorded reason).
        """
        return diffusion_sr_upscale(self, rgb, scale, device, precision)
