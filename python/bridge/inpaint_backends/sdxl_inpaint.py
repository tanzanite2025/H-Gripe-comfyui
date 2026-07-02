"""Stable Diffusion XL inpaint backend (opt-in, GPU/CPU via diffusers + torch).

The roadmap's SDXL follow-up to ``sd_inpaint`` (§3.2/§3.3): same seam, same
contract, a higher-quality 1024-native inpaint checkpoint. Stays **opt-in**:
used only when the node's ``engine`` param is ``sdxl_inpaint`` *and* both the
optional deps (``torch`` + ``diffusers``) and the model weight are present;
otherwise the caller emits an empty repaint set and the orchestrator falls
back to the always-available remote ``image.edit`` provider path.

Nothing heavy is imported at module load — ``torch`` / ``diffusers`` are only
touched inside :meth:`available` (via :func:`importlib.util.find_spec`) and
:meth:`inpaint`.

Weight resolution order:
1. ``HGRIPE_SDXL_INPAINT_MODEL`` (explicit path or HF repo id, for dev / CI), else
2. ``<model cache>/sdxl-inpaint`` where the cache dir is ``HGRIPE_MODEL_CACHE``
   or the bundled ``resources/models`` dir.

The weight is **not** bundled in the installer (an SDXL inpaint checkpoint is
~7 GB); ``scripts`` can fetch it into the cache dir, exactly like the SD
inpaint / SAM 2 / ViTMatte / Real-ESRGAN weights.
"""

from __future__ import annotations

import importlib.util
import os
from pathlib import Path
from typing import Any

from . import InpaintUnavailable, model_cache_dir
from .sd_inpaint import _warm_pipeline

_DEFAULT_WEIGHT_DIR = "sdxl-inpaint"


def _construct_pipeline(weight: str, device: str, precision: str) -> Any:
    """Build an SDXL inpaint pipeline (imports heavy deps lazily)."""
    import torch
    from diffusers import StableDiffusionXLInpaintPipeline

    dtype = torch.float16 if precision == "fp16" else torch.float32
    pipe = StableDiffusionXLInpaintPipeline.from_pretrained(
        weight,
        torch_dtype=dtype,
    )
    return pipe.to(device)


class StableDiffusionXLInpaintBackend:
    id = "sdxl_inpaint"

    def weight_path(self) -> Path:
        override = (os.environ.get("HGRIPE_SDXL_INPAINT_MODEL") or "").strip()
        if override:
            return Path(override)
        return model_cache_dir() / _DEFAULT_WEIGHT_DIR

    def available(self) -> tuple[bool, str]:
        """Cheap probe: optional deps importable + a weight directory present."""
        for dep in ("torch", "diffusers"):
            if importlib.util.find_spec(dep) is None:
                return False, f"missing optional dependency: {dep}"
        weight = self.weight_path()
        if not (weight.is_dir() or weight.is_file()):
            return (
                False,
                f"weight not found: {weight} "
                "(set HGRIPE_SDXL_INPAINT_MODEL or fetch into HGRIPE_MODEL_CACHE)",
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
        precision: str | None = None,
    ) -> tuple[Any, str, str]:
        """Inpaint the white area of ``mask`` over ``crop`` with SDXL.

        Same geometry contract as ``sd_inpaint``: the crop+mask are padded up
        to the pipeline's multiple-of-8 requirement, run, and cropped back.
        Returns ``(image, device_used, precision_used)``. Raises
        :class:`InpaintUnavailable` if deps/weights vanished since the probe.
        """
        ok, reason = self.available()
        if not ok:
            raise InpaintUnavailable(reason)

        import torch
        from PIL import Image

        from sr_backends import resolve_precision

        device = "cuda" if torch.cuda.is_available() else "cpu"
        precision = resolve_precision(precision, device)
        pipe = _warm_pipeline(
            str(self.weight_path()), device, precision, constructor=_construct_pipeline
        )

        rgb = crop.convert("RGB")
        msk = mask.convert("L")
        orig_w, orig_h = rgb.size
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
            # SDXL inpaint expects the working resolution passed explicitly so
            # it does not resize the crop to its 1024 native size.
            width=rgb.size[0],
            height=rgb.size[1],
            strength=float(strength),
            guidance_scale=float(guidance_scale),
            num_inference_steps=int(steps),
            generator=generator,
        ).images[0]

        if result.size != (orig_w, orig_h):
            result = result.crop((0, 0, orig_w, orig_h))
        return result.convert("RGB"), device, precision
