"""Pluggable super-resolution backends for the Image Enhance node.

The Phase 1 CPU path (Lanczos resample + unsharp) lives in
``image_enhance_cli.py`` and is always available. This package is the
``engine`` seam from ``docs/card-executor-split-and-psd-chain-hardening.md``:
additional engines (``realesrgan`` now; ``ccsr`` / ``supir`` later) register
here and are selected per run by the node's ``engine`` param.

Design rules (mirroring the ViTMatte matting backend):

* **Opt-in, never default.** ``cpu`` stays the default and the fallback. A
  backend is only used when the caller explicitly asks for it *and*
  :meth:`SrBackend.available` returns ``True``.
* **CPU-safe import.** Importing this package must not import ``torch`` or any
  heavy/optional dependency — backends import their deps lazily inside
  :meth:`SrBackend.available` / :meth:`SrBackend.upscale`.
* **Weights are not bundled.** A backend resolves its weights from the model
  cache dir (``HGRIPE_MODEL_CACHE``, falling back to ``resources/models``); a
  missing weight makes the backend unavailable, not an error.
* **Graceful degradation.** When a requested backend is unavailable the caller
  records the reason and falls back to the CPU path — the node always produces
  an output.
"""

from __future__ import annotations

import os
from pathlib import Path
from typing import Any, Protocol

CPU_ENGINE = "cpu"


class BackendUnavailable(RuntimeError):
    """Raised by a backend that was asked to run without its deps / weights.

    Carries a short human-readable ``reason`` recorded in the enhance report so
    the UI can explain why the CPU path was used.
    """

    def __init__(self, reason: str) -> None:
        super().__init__(reason)
        self.reason = reason


class SrBackend(Protocol):
    """A super-resolution engine selectable via the node's ``engine`` param."""

    #: Stable id used as the ``engine`` param value (e.g. ``"realesrgan"``).
    id: str
    #: The native integer upscale factor the model was trained for (e.g. 4).
    native_scale: int

    def available(self) -> tuple[bool, str]:
        """Return ``(ok, reason)``; ``reason`` explains *why not* when not ok."""
        ...

    def weight_path(self) -> Path:
        """Resolved path of the (non-bundled) weight this backend would load.

        Used by :func:`probe` to inventory which weights are cached, without
        importing the heavy deps. The path need not exist (a missing weight is
        what makes the backend unavailable).
        """
        ...

    def upscale(self, rgb: Any, scale: float) -> Any:
        """Upscale a PIL ``RGB`` image by ``scale`` (no alpha — handled by the caller).

        Raises :class:`BackendUnavailable` if deps/weights vanished between the
        probe and the call.
        """
        ...


def model_cache_dir() -> Path:
    """Where downloadable (non-bundled) model weights live.

    ``HGRIPE_MODEL_CACHE`` overrides the location for dev / CI; otherwise we use
    the bundled ``resources/models`` dir next to the desktop app, matching where
    ``scripts/fetch-*`` place weights.
    """
    override = (os.environ.get("HGRIPE_MODEL_CACHE") or "").strip()
    if override:
        return Path(override)
    # python/bridge/sr_backends/__init__.py -> repo apps/desktop-tauri/src-tauri/resources/models
    here = Path(__file__).resolve()
    repo = here.parents[2]
    return repo / "apps" / "desktop-tauri" / "src-tauri" / "resources" / "models"


def weight_info(path: Path) -> dict[str, Any]:
    """Cached-weight inventory entry for one backend's resolved weight path.

    The big ML weights are **not** bundled (Issue #2); they are fetched into the
    model cache. This reports whether a backend's weight is already present and
    how big it is, so the capability report can show what is downloaded vs still
    missing rather than only "engine unavailable". A directory weight (e.g. a
    diffusers / HF snapshot) reports ``present`` without a single file size.
    """
    try:
        size_mb = int(path.stat().st_size // (1024 * 1024)) if path.is_file() else None
        return {"path": str(path), "present": path.exists(), "size_mb": size_mb}
    except OSError as err:
        return {"path": str(path), "present": False, "size_mb": None, "reason": f"{type(err).__name__}: {err}"}


#: ONNX Runtime execution providers we know how to accelerate on, best first.
#: CPU is always appended as the fallback so a session never fails to build.
_PREFERRED_ONNX_PROVIDERS = ("CUDAExecutionProvider",)


def onnx_providers(available: list[str] | None = None) -> list[str]:
    """Execution providers for an ONNX session, preferring a GPU when present.

    The ONNX engines (matting / colour harmonise / defect) used to hard-code the
    CPU provider, so they ran on CPU even on a CUDA box — making the device probe
    and the inspector's "runs on GPU" badge lie. This mirrors the torch backends'
    "cuda if available else cpu" auto behaviour: a known accelerator provider is
    used first when ONNX Runtime exposes it, with ``CPUExecutionProvider`` always
    last as the universal fallback.

    ``available`` is the ORT-reported provider list; it is injected for testing
    and queried lazily (``onnxruntime.get_available_providers()``) when omitted.
    """
    if available is None:
        import onnxruntime as ort

        available = [str(p) for p in ort.get_available_providers()]
    preferred = [p for p in _PREFERRED_ONNX_PROVIDERS if p in available]
    return [*preferred, "CPUExecutionProvider"]


def _engine_weight(backend: Any) -> dict[str, Any] | None:
    """Weight inventory for a registered backend, or ``None`` if it has none.

    Never raises: a backend whose ``weight_path`` cannot be resolved simply has
    no inventory entry, so the probe still reports its availability.
    """
    try:
        return weight_info(backend.weight_path())
    except Exception:  # noqa: BLE001 - a broken weight_path must not crash the report
        return None


def device_probe() -> dict[str, Any]:
    """Machine compute capability for the capability report.

    The per-card ``--probe-engines`` calls already say *which engines could run*;
    this says *what accelerator they would run on*, so the UI can explain that a
    GPU engine will fall back to CPU on a box with no CUDA device. It is the same
    for every card (machine-global), so the Tauri aggregator records it once.

    Optional deps are only inspected lazily here (never imported at module load,
    mirroring the backend seams): ``torch`` for CUDA device names / VRAM and
    ``onnxruntime`` for its available execution providers. Every field degrades
    to a safe default when a dep is absent, so a CPU-only box reports
    ``cuda_available=false`` with empty ``devices`` rather than erroring.
    """
    report: dict[str, Any] = {
        "cuda_available": False,
        "devices": [],
        "torch": {"installed": False},
        "onnxruntime": {"installed": False, "providers": []},
    }

    try:
        import torch
    except Exception as err:  # noqa: BLE001 - a missing/broken optional dep is just "unavailable"
        report["torch"] = {"installed": False, "reason": f"{type(err).__name__}: {err}"}
    else:
        cuda = bool(torch.cuda.is_available())
        report["torch"] = {
            "installed": True,
            "version": str(torch.__version__),
            "cuda": cuda,
        }
        if cuda:
            report["cuda_available"] = True
            for index in range(torch.cuda.device_count()):
                props = torch.cuda.get_device_properties(index)
                report["devices"].append(
                    {
                        "index": index,
                        "name": str(props.name),
                        "total_memory_mb": int(props.total_memory // (1024 * 1024)),
                    }
                )

    try:
        import onnxruntime
    except Exception as err:  # noqa: BLE001 - same: report unavailable, never crash the probe
        report["onnxruntime"] = {
            "installed": False,
            "providers": [],
            "reason": f"{type(err).__name__}: {err}",
        }
    else:
        providers = [str(p) for p in onnxruntime.get_available_providers()]
        report["onnxruntime"] = {
            "installed": True,
            "version": str(onnxruntime.__version__),
            "providers": providers,
        }
        # CUDA / TensorRT / ROCm providers all imply a usable accelerator.
        if any(
            any(tag in provider for tag in ("CUDA", "Tensorrt", "TensorRT", "ROCM"))
            for provider in providers
        ):
            report["cuda_available"] = True

    return report


# ---- registry ------------------------------------------------------------

# Imported lazily so this module stays torch-free at import time.
def _registry() -> dict[str, SrBackend]:
    from .realesrgan import RealEsrganBackend

    backends: list[SrBackend] = [RealEsrganBackend()]
    return {b.id: b for b in backends}


def known_engines() -> list[str]:
    """All selectable engine ids, with ``cpu`` first."""
    return [CPU_ENGINE, *sorted(_registry().keys())]


def resolve(engine: str | None) -> SrBackend | None:
    """Return the backend for ``engine`` or ``None`` for the CPU path.

    Unknown engine names resolve to ``None`` (the caller falls back to CPU and
    records the reason) rather than raising, so a stale saved graph never hard
    fails.
    """
    name = (engine or CPU_ENGINE).strip().lower()
    if name in ("", CPU_ENGINE):
        return None
    return _registry().get(name)


def probe() -> dict[str, Any]:
    """Capability report for the UI: which engines are usable right now.

    Lets the inspector grey out GPU engines when their deps / weights are
    missing. Always includes ``cpu`` as available.
    """
    engines: dict[str, Any] = {
        CPU_ENGINE: {"available": True, "reason": "built-in CPU path", "accelerated": False},
    }
    for name, backend in _registry().items():
        try:
            ok, reason = backend.available()
        except Exception as err:  # noqa: BLE001 - a broken probe must not crash the report
            ok, reason = False, f"{type(err).__name__}: {err}"
        engines[name] = {
            "available": bool(ok),
            "reason": reason,
            "native_scale": getattr(backend, "native_scale", None),
            # GPU-capable engine: the UI pairs this with the machine device probe
            # to warn it would fall back to CPU on a box with no CUDA device.
            "accelerated": True,
            # Cached-weight inventory: which non-bundled weight it loads + size.
            "weight": _engine_weight(backend),
        }
    return {"engines": engines, "model_cache_dir": str(model_cache_dir())}
