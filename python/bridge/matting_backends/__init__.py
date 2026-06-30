"""Pluggable learned alpha matters for the Mask Edge Refine node.

The Phase 1 heuristic layer (erode/dilate morphology, the numpy guided filter,
feather, colour decontamination, trimap-band protection) lives in
``edge_refine_cli.py`` and is always available. This package is the ``engine``
seam called out in ``docs/implementation-status.md`` (the "learned-matting /
``guidedFilter`` ``profile_ref`` engine mode"): a *learned* alpha matter
registers here and is selected per run by the node's ``engine`` param, emitting
into the **same** ``{refined_image, refined_mask, edge_report}`` contract so the
downstream PSD Export consumer needs no change.

A learned matter solves the genuinely-soft **unknown band** of a matting trimap
(hair / fur / glass) far better than the global guided filter, so the engine is
only meaningful when a trimap is connected; without one the caller keeps the
heuristic result.

Design rules (mirroring the ``sr_backends`` / ``detector_backends`` /
``inpaint_backends`` / ``color_backends`` seams):

* **Additive, opt-in, never default.** ``cpu`` (the built-in heuristic refine)
  stays the default and always-on baseline; a learned matter is only run when
  the caller explicitly asks for it *and* :meth:`MattingBackend.available`
  returns ``True``.
* **CPU-safe import.** Importing this package must not import ``onnxruntime`` /
  ``torch`` or any heavy/optional dependency — backends import their deps lazily
  inside :meth:`MattingBackend.available` / :meth:`MattingBackend.matte`.
* **Weights are not bundled.** A backend resolves its weights from the model
  cache dir (``HGRIPE_MODEL_CACHE``, falling back to ``resources/models``); a
  missing weight makes the backend unavailable, not an error.
* **Graceful degradation.** When a requested engine is unavailable the caller
  records the reason and keeps the heuristic result — the node always produces
  an output exactly as today.
"""

from __future__ import annotations

from typing import Any, Protocol

# Reuse the one model-cache resolver (torch-free, defined for the SR seam) so
# downloadable weights for every node land in the same place.
from sr_backends import model_cache_dir

CPU_ENGINE = "cpu"

__all__ = [
    "CPU_ENGINE",
    "MattingUnavailable",
    "MattingBackend",
    "model_cache_dir",
    "known_engines",
    "resolve",
    "probe",
]


class MattingUnavailable(RuntimeError):
    """Raised by a matter that was asked to run without its deps / weights.

    Carries a short human-readable ``reason`` recorded in the edge report so the
    UI can explain why the heuristic path was used.
    """

    def __init__(self, reason: str) -> None:
        super().__init__(reason)
        self.reason = reason


class MattingBackend(Protocol):
    """A learned alpha matter selectable via the node's ``engine`` param."""

    #: Stable id used as the ``engine`` param value (e.g. ``"onnx_matting"``).
    id: str

    def available(self) -> tuple[bool, str]:
        """Return ``(ok, reason)``; ``reason`` explains *why not* when not ok."""
        ...

    def matte(self, rgb: Any, trimap: Any) -> Any:
        """Solve a refined alpha from a subject image and a matting trimap.

        ``rgb`` is the subject as an ``(H, W, 3)`` uint8 array and ``trimap`` its
        ``(H, W)`` float trimap in 0..1 (``0`` = background, ``~0.5`` = unknown,
        ``1`` = foreground). Returns an ``(H, W)`` float alpha in 0..1 the same
        size as ``rgb``. Raises :class:`MattingUnavailable` if deps / weights
        vanished between the probe and the call.
        """
        ...


# ---- registry ------------------------------------------------------------

# Imported lazily so this module stays onnxruntime/torch-free at import time.
def _registry() -> dict[str, MattingBackend]:
    from .vitmatte_onnx import OnnxMattingBackend

    backends: list[MattingBackend] = [OnnxMattingBackend()]
    return {b.id: b for b in backends}


def known_engines() -> list[str]:
    """All selectable engine ids, with ``cpu`` first."""
    return [CPU_ENGINE, *sorted(_registry().keys())]


def resolve(engine: str | None) -> MattingBackend | None:
    """Return the backend for ``engine`` or ``None`` for the heuristic path.

    Unknown engine names resolve to ``None`` (the caller keeps the heuristic
    result and records the reason) rather than raising, so a stale saved graph
    never hard fails.
    """
    name = (engine or CPU_ENGINE).strip().lower()
    if name in ("", CPU_ENGINE):
        return None
    return _registry().get(name)


def probe() -> dict[str, Any]:
    """Capability report for the UI: which engines are usable right now.

    Lets the inspector grey out learned engines when their deps / weights are
    missing. Always includes ``cpu`` as available.
    """
    engines: dict[str, Any] = {
        CPU_ENGINE: {"available": True, "reason": "built-in CPU heuristic refine"},
    }
    for name, backend in _registry().items():
        try:
            ok, reason = backend.available()
        except Exception as err:  # noqa: BLE001 - a broken probe must not crash the report
            ok, reason = False, f"{type(err).__name__}: {err}"
        engines[name] = {"available": bool(ok), "reason": reason}
    return {"engines": engines, "model_cache_dir": str(model_cache_dir())}
