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

from . import CONTROLNET_OFF, InpaintUnavailable, model_cache_dir

_DEFAULT_WEIGHT_DIR = "sd-inpaint"
_DEFAULT_CONTROLNET_WEIGHT_DIR = "controlnet-canny"


def controlnet_weight_path() -> Path:
    """Resolved path of the (non-bundled) canny ControlNet weight.

    ``HGRIPE_CONTROLNET_MODEL`` overrides for dev / CI; otherwise the weight is
    fetched into the model cache like every other opt-in weight.
    """
    override = (os.environ.get("HGRIPE_CONTROLNET_MODEL") or "").strip()
    if override:
        return Path(override)
    return model_cache_dir() / _DEFAULT_CONTROLNET_WEIGHT_DIR


def canny_condition(rgb: Any, low: float = 0.1, high: float = 0.2) -> Any:
    """Canny-style edge conditioning image for a PIL ``RGB`` crop.

    A dependency-light stand-in for OpenCV's Canny (the bridge deliberately
    avoids a cv2 dependency): Gaussian smoothing + Sobel gradient magnitude
    with a double threshold and hysteresis via a dilation pass. White edges on
    black, replicated to 3 channels — the layout ControlNet canny models
    expect.
    """
    import numpy as np
    from PIL import Image, ImageFilter

    gray = np.asarray(
        rgb.convert("L").filter(ImageFilter.GaussianBlur(radius=1.4)), dtype=np.float32
    ) / 255.0
    gx = np.zeros_like(gray)
    gy = np.zeros_like(gray)
    gx[:, 1:-1] = gray[:, 2:] - gray[:, :-2]
    gy[1:-1, :] = gray[2:, :] - gray[:-2, :]
    magnitude = np.hypot(gx, gy)

    strong = magnitude >= high
    weak = magnitude >= low
    # One hysteresis pass: keep weak edges that touch a strong edge.
    padded = np.pad(strong, 1)
    neighbour = np.zeros_like(strong)
    for dy in (-1, 0, 1):
        for dx in (-1, 0, 1):
            neighbour |= padded[1 + dy : padded.shape[0] - 1 + dy, 1 + dx : padded.shape[1] - 1 + dx]
    edges = strong | (weak & neighbour)

    edge_img = Image.fromarray((edges * 255).astype(np.uint8), "L")
    return Image.merge("RGB", (edge_img, edge_img, edge_img))

#: Process-global warm cache of constructed inpaint pipelines (shared by every
#: diffusers backend in this package), keyed by ``(weight, device, precision)``.
#: ``from_pretrained`` loads a multi-GB checkpoint and moves it onto the device;
#: in a long-lived host (the torch worker, staged-rollout step 4 of
#: ``docs/cards/editor-resource-model.md``) this cache means that happens once
#: per ``(weight, device, precision)`` instead of on every run. In a one-shot
#: CLI process it is simply built once and discarded.
_WARM_PIPELINES: dict[tuple[str, str, str], Any] = {}


def _construct_pipeline(weight: str, device: str, precision: str) -> Any:
    """Build a Stable Diffusion inpaint pipeline (imports heavy deps lazily).

    Split out from :func:`_warm_pipeline` so the warm cache can be exercised in
    tests without ``torch`` / ``diffusers`` installed (the test monkeypatches
    this constructor and counts calls).
    """
    import torch
    from diffusers import StableDiffusionInpaintPipeline

    dtype = torch.float16 if precision == "fp16" else torch.float32
    pipe = StableDiffusionInpaintPipeline.from_pretrained(
        weight,
        torch_dtype=dtype,
        safety_checker=None,
    )
    return pipe.to(device)


def _construct_controlnet_pipeline(weight: str, device: str, precision: str) -> Any:
    """Build a ControlNet-conditioned SD inpaint pipeline.

    ``weight`` is the composite ``<sd weight>::<controlnet weight>`` cache key
    produced by :meth:`StableDiffusionInpaintBackend.inpaint`, so the shared
    warm cache distinguishes the plain and the conditioned pipeline.
    """
    import torch
    from diffusers import ControlNetModel, StableDiffusionControlNetInpaintPipeline

    sd_weight, cn_weight = weight.split("::", 1)
    dtype = torch.float16 if precision == "fp16" else torch.float32
    controlnet = ControlNetModel.from_pretrained(cn_weight, torch_dtype=dtype)
    pipe = StableDiffusionControlNetInpaintPipeline.from_pretrained(
        sd_weight,
        controlnet=controlnet,
        torch_dtype=dtype,
        safety_checker=None,
    )
    return pipe.to(device)


def _warm_pipeline(
    weight: str,
    device: str,
    precision: str,
    constructor: Any = None,
) -> Any:
    """Return a cached inpaint pipeline for the key, building it on first use.

    ``constructor`` lets the other diffusers backends (SDXL, Flux Fill) share
    this cache with their own pipeline builder; the default is this module's
    SD constructor (resolved at call time so tests can monkeypatch it). Weights
    live in per-engine directories, so keys never collide across backends.
    """
    key = (weight, device, precision)
    cached = _WARM_PIPELINES.get(key)
    if cached is not None:
        return cached
    build = constructor if constructor is not None else _construct_pipeline
    built = build(weight, device, precision)
    _WARM_PIPELINES[key] = built
    return built


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
        precision: str | None = None,
        controlnet: str = CONTROLNET_OFF,
    ) -> tuple[Any, str, str]:
        """Inpaint the white area of ``mask`` over ``crop`` with Stable Diffusion.

        The pipeline works on multiples of 8 px, so the crop+mask are padded up
        to the next multiple, run through the inpaint pipeline, and cropped back
        to the original size so the caller's geometry contract is unchanged.
        ``precision`` selects the compute precision (``auto`` by default — fp16
        on CUDA, fp32 on CPU — see :func:`sr_backends.resolve_precision`).
        ``controlnet='canny'`` conditions the repaint on the crop's edge map (a
        canny ControlNet, weight from ``HGRIPE_CONTROLNET_MODEL``) so structure
        stays stable across the seam. Returns ``(image, device_used,
        precision_used)`` so the caller reports what actually ran. Raises
        :class:`InpaintUnavailable` if deps/weights vanished since the probe,
        or if the requested ControlNet weight is missing.
        """
        ok, reason = self.available()
        if not ok:
            raise InpaintUnavailable(reason)

        controlnet = (controlnet or CONTROLNET_OFF).strip().lower() or CONTROLNET_OFF
        if controlnet not in (CONTROLNET_OFF, "canny"):
            raise InpaintUnavailable(f"unknown controlnet {controlnet!r} (off | canny)")
        cn_weight: Path | None = None
        if controlnet == "canny":
            cn_weight = controlnet_weight_path()
            if not (cn_weight.is_dir() or cn_weight.is_file()):
                raise InpaintUnavailable(
                    f"controlnet weight not found: {cn_weight} "
                    "(set HGRIPE_CONTROLNET_MODEL or fetch into HGRIPE_MODEL_CACHE)"
                )

        import torch
        from PIL import Image

        from sr_backends import resolve_precision

        device = "cuda" if torch.cuda.is_available() else "cpu"
        precision = resolve_precision(precision, device)
        # Reuse a warm pipeline when the host is long-lived (the torch worker); a
        # one-shot CLI run just builds it once. The multi-GB checkpoint load /
        # device move therefore happens once per (weight, device, precision)
        # instead of on every call.
        if cn_weight is not None:
            pipe = _warm_pipeline(
                f"{self.weight_path()}::{cn_weight}",
                device,
                precision,
                constructor=_construct_controlnet_pipeline,
            )
        else:
            pipe = _warm_pipeline(str(self.weight_path()), device, precision)

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

        pipe_kwargs: dict[str, Any] = {
            "prompt": prompt,
            "negative_prompt": negative_prompt or None,
            "image": rgb,
            "mask_image": msk,
            "strength": float(strength),
            "guidance_scale": float(guidance_scale),
            "num_inference_steps": int(steps),
            "generator": generator,
        }
        if cn_weight is not None:
            pipe_kwargs["control_image"] = canny_condition(rgb)
        result = pipe(**pipe_kwargs).images[0]

        if result.size != (orig_w, orig_h):
            result = result.crop((0, 0, orig_w, orig_h))
        return result.convert("RGB"), device, precision
