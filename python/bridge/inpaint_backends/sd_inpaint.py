"""Stable Diffusion inpaint backend (opt-in, GPU/CPU via diffusers + torch).

This is the first local ``engine`` for the Detail Repaint node — the roadmap's
"local GPU inpainting backend as an alternative to the remote provider" (§3.2),
for offline / privacy / cost-controlled runs. It stays **opt-in**: the backend
is only used when the node's ``engine`` param is ``sd_inpaint`` *and* both the
optional deps (``torch`` + ``diffusers``) and the model weight are present;
otherwise the caller emits an empty repaint set and the orchestrator falls back
to the always-available remote ``image.edit`` provider path.

Nothing heavy is imported at module load — ``torch`` / ``diffusers`` are only
touched inside :meth:`available` (via :func:`importlib.util.find_spec`) and
:meth:`inpaint`.

Weight resolution order:
1. ``HGRIPE_INPAINT_MODEL`` (explicit path or HF repo id, for dev / CI), else
2. ``<model cache>/sd-inpaint`` where the cache dir is ``HGRIPE_MODEL_CACHE``
   or the bundled ``resources/models`` dir.

The weight is **not** bundled in the installer (an SD inpaint checkpoint is
~2-7 GB); ``scripts`` can fetch it into the cache dir, exactly like the SAM 2 /
ViTMatte / Real-ESRGAN weights.
"""

from __future__ import annotations

import importlib.util
import os
from pathlib import Path
from typing import Any

from . import InpaintUnavailable, model_cache_dir

_DEFAULT_WEIGHT_DIR = "sd-inpaint"


class StableDiffusionInpaintBackend:
    id = "sd_inpaint"

    def weight_path(self) -> Path:
        override = (os.environ.get("HGRIPE_INPAINT_MODEL") or "").strip()
        if override:
            return Path(override)
        return model_cache_dir() / _DEFAULT_WEIGHT_DIR

    def available(self) -> tuple[bool, str]:
        """Cheap probe: optional deps importable + a weight directory present.

        Uses ``find_spec`` so we never actually import ``torch`` / ``diffusers``
        (slow, and may pull in CUDA) just to report availability.
        """
        for dep in ("torch", "diffusers"):
            if importlib.util.find_spec(dep) is None:
                return False, f"missing optional dependency: {dep}"
        weight = self.weight_path()
        if not (weight.is_dir() or weight.is_file()):
            return (
                False,
                f"weight not found: {weight} "
                "(set HGRIPE_INPAINT_MODEL or fetch into HGRIPE_MODEL_CACHE)",
            )
        return True, "ready"

    def inpaint(
        self,
        crop: Any,
        mask: Any,
        prompt: str,
        *,
        negative_prompt: str = "",
        strength: float = 0.75,
        guidance_scale: float = 7.5,
        steps: int = 30,
        seed: int | None = None,
    ) -> Any:
        """Inpaint the white area of ``mask`` over ``crop`` with Stable Diffusion.

        The pipeline works on multiples of 8 px, so the crop+mask are padded up
        to the next multiple, run through the inpaint pipeline, and cropped back
        to the original size so the caller's geometry contract is unchanged.
        Raises :class:`InpaintUnavailable` if deps/weights vanished since the
        probe.
        """
        ok, reason = self.available()
        if not ok:
            raise InpaintUnavailable(reason)

        import torch
        from diffusers import StableDiffusionInpaintPipeline
        from PIL import Image

        device = "cuda" if torch.cuda.is_available() else "cpu"
        dtype = torch.float16 if device == "cuda" else torch.float32
        pipe = StableDiffusionInpaintPipeline.from_pretrained(
            str(self.weight_path()),
            torch_dtype=dtype,
            safety_checker=None,
        )
        pipe = pipe.to(device)

        rgb = crop.convert("RGB")
        msk = mask.convert("L")
        orig_w, orig_h = rgb.size
        # SD's VAE downsamples by 8; pad up to a multiple of 8 then crop back.
        pad_w = (8 - orig_w % 8) % 8
        pad_h = (8 - orig_h % 8) % 8
        if pad_w or pad_h:
            padded_rgb = Image.new("RGB", (orig_w + pad_w, orig_h + pad_h))
            padded_rgb.paste(rgb, (0, 0))
            padded_msk = Image.new("L", (orig_w + pad_w, orig_h + pad_h), 0)
            padded_msk.paste(msk, (0, 0))
            rgb, msk = padded_rgb, padded_msk

        generator = None
        if seed is not None:
            generator = torch.Generator(device=device).manual_seed(int(seed))

        result = pipe(
            prompt=prompt,
            negative_prompt=negative_prompt or None,
            image=rgb,
            mask_image=msk,
            strength=float(strength),
            guidance_scale=float(guidance_scale),
            num_inference_steps=int(steps),
            generator=generator,
        ).images[0]

        if result.size != (orig_w, orig_h):
            result = result.crop((0, 0, orig_w, orig_h))
        return result.convert("RGB")
