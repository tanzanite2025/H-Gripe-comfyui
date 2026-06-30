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
        CPU_ENGINE: {"available": True, "reason": "built-in CPU path"},
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
        }
    return {"engines": engines, "model_cache_dir": str(model_cache_dir())}
