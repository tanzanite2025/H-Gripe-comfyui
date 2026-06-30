"""Stable Diffusion local inpaint backend (opt-in, GPU/CPU via diffusers).

This is the first local ``engine`` for the Detail Repaint node — the offline /
privacy / cost-controlled alternative to the remote ``image.edit`` provider. It
stays **opt-in**: the backend is only used when the node's ``engine`` param is
``sd_inpaint`` *and* both the optional deps (``torch`` + ``diffusers``) and the
model weight are present; otherwise the caller keeps the always-available
provider / passthrough path.

Nothing heavy is imported at module load — ``torch`` / ``diffusers`` are only
touched inside :meth:`available` (via :func:`importlib.util.find_spec`) and
:meth:`inpaint`.

Weight resolution order:
1. ``HGRIPE_INPAINT_MODEL`` (explicit path or HF repo id, for dev / CI), else
2. ``<model cache>/sd_inpaint`` where the cache dir is ``HGRIPE_MODEL_CACHE`` or
   the bundled ``resources/models`` dir.

The weight is **not** bundled in the installer (an SD/SDXL inpaint checkpoint is
several GB); ``scripts`` can fetch it into the cache dir, exactly like the SAM 2
/ ViTMatte / Real-ESRGAN weights.
"""

from __future__ import annotations

import importlib.util
import os
from pathlib import Path
from typing import Any

from . import InpaintUnavailable, model_cache_dir

_DEFAULT_WEIGHT_DIR = "sd_inpaint"
# Conservative defaults: tight masks + low-ish denoise preserve identity inside
# the issue core (the roadmap's identity-drift mitigation), the orchestrator can
# override per run.
_DEFAULT_STEPS = 30
_DEFAULT_GUIDANCE = 7.5
_DEFAULT_STRENGTH = 0.85


class StableDiffusionInpaintBackend:
    id = "sd_inpaint"

    def weight_path(self) -> Path:
        override = (os.environ.get("HGRIPE_INPAINT_MODEL") or "").strip()
        if override:
            return Path(override)
        return model_cache_dir() / _DEFAULT_WEIGHT_DIR

    def backend_model(self) -> str | None:
        override = (os.environ.get("HGRIPE_INPAINT_MODEL") or "").strip()
        if override:
            # A bare HF repo id (``org/name``) has no filesystem path; report it
            # as-is, otherwise the directory/file name.
            return override if not Path(override).exists() else Path(override).name
        return _DEFAULT_WEIGHT_DIR

    def _is_repo_id(self) -> bool:
        """True when the weight is an un-downloaded Hugging Face repo id.

        A repo id (``org/name``) is fetched/cached by ``diffusers`` at load time
        rather than resolved on disk, so we must not require it to be a local
        path in :meth:`available`.
        """
        override = (os.environ.get("HGRIPE_INPAINT_MODEL") or "").strip()
        return bool(override) and not Path(override).exists()

    def available(self) -> tuple[bool, str]:
        """Cheap probe: optional deps importable + weight present on disk.

        Uses ``find_spec`` so we never actually import ``torch`` / ``diffusers``
        (slow, and may pull in CUDA) just to report availability. When the weight
        is an explicit Hugging Face repo id we cannot cheaply verify it without a
        network call, so we treat the deps being present as ``ready`` and let a
        real fetch failure degrade gracefully at call time.
        """
        for dep in ("torch", "diffusers"):
            if importlib.util.find_spec(dep) is None:
                return False, f"missing optional dependency: {dep}"
        if self._is_repo_id():
            return True, "ready (Hugging Face repo id)"
        weight = self.weight_path()
        if not weight.exists():
            return (
                False,
                f"weight not found: {weight} "
                "(set HGRIPE_INPAINT_MODEL or fetch into HGRIPE_MODEL_CACHE)",
            )
        return True, "ready"

    def inpaint(self, rgb: Any, mask: Any, prompt: str, **options: Any) -> Any:
        """Inpaint the ``255`` region of ``mask`` in ``rgb`` with diffusion.

        ``rgb`` is ``(H, W, 3)`` uint8, ``mask`` is ``(H, W)`` uint8 (255 = the
        issue core to regenerate). Returns an ``(H, W, 3)`` uint8 array the same
        size as ``rgb``. Raises :class:`InpaintUnavailable` if deps / weights
        vanished since the probe.
        """
        ok, reason = self.available()
        if not ok:
            raise InpaintUnavailable(reason)

        import numpy as np
        import torch
        from diffusers import StableDiffusionInpaintPipeline
        from PIL import Image

        src_h, src_w = rgb.shape[:2]
        device = "cuda" if torch.cuda.is_available() else "cpu"
        dtype = torch.float16 if device == "cuda" else torch.float32

        pipe = StableDiffusionInpaintPipeline.from_pretrained(
            str(self.weight_path()), torch_dtype=dtype
        )
        pipe = pipe.to(device)
        # SD inpaint works on a multiple-of-8 canvas; remember the source size to
        # resize the result back so the composite geometry is unchanged.
        work_w = max(8, (src_w // 8) * 8)
        work_h = max(8, (src_h // 8) * 8)

        image = Image.fromarray(np.ascontiguousarray(rgb), "RGB")
        mask_img = Image.fromarray(np.ascontiguousarray(mask), "L")
        if (work_w, work_h) != (src_w, src_h):
            image = image.resize((work_w, work_h), Image.LANCZOS)
            mask_img = mask_img.resize((work_w, work_h), Image.NEAREST)

        seed = options.get("seed")
        generator = None
        if seed is not None:
            generator = torch.Generator(device=device).manual_seed(int(seed))

        result = pipe(
            prompt=prompt or "restore this region with clean, realistic detail",
            image=image,
            mask_image=mask_img,
            num_inference_steps=int(options.get("steps", _DEFAULT_STEPS)),
            guidance_scale=float(options.get("guidance", _DEFAULT_GUIDANCE)),
            strength=float(options.get("strength", _DEFAULT_STRENGTH)),
            generator=generator,
        ).images[0]

        if result.size != (src_w, src_h):
            result = result.resize((src_w, src_h), Image.LANCZOS)
        return np.asarray(result.convert("RGB"), dtype=np.uint8)
