"""Pluggable learned light & colour matchers for the Match Light & Color node.

The Phase 1 heuristic layer (Reinhard Lab transfer, per-channel histogram
matching, the prompt-only suffix) lives in ``color_match_cli.py`` and is always
available. This package is the ``engine`` seam called out in
``docs/implementation-status.md`` (the "learned matcher ``engine``"): a *learned*
colour/light harmoniser registers here and is selected per run by the node's
``engine`` param, emitting into the **same** ``{matched_image, prompt_suffix,
match_report}`` contract so the downstream PSD Export consumer needs no change.

Design rules (mirroring the ``sr_backends`` / ``detector_backends`` /
``inpaint_backends`` seams):

* **Additive, opt-in, never default.** ``cpu`` (the built-in heuristic match)
  stays the default and the always-on baseline; a learned matcher is only run
  when the caller explicitly asks for it *and* :meth:`ColorMatchBackend.available`
  returns ``True``.
* **CPU-safe import.** Importing this package must not import ``onnxruntime`` /
  ``torch`` or any heavy/optional dependency — backends import their deps lazily
  inside :meth:`ColorMatchBackend.available` / :meth:`ColorMatchBackend.match`.
* **Weights are not bundled.** A backend resolves its weights from the model
  cache dir (``HGRIPE_MODEL_CACHE``, falling back to ``resources/models``); a
  missing weight makes the backend unavailable, not an error.
* **Graceful degradation.** When a requested engine is unavailable the caller
  records the reason and keeps the heuristic result — the node always produces
  an output exactly as today.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any, Protocol

# Reuse the one model-cache resolver (torch-free, defined for the SR seam) so
# downloadable weights for every node land in the same place.
from sr_backends import _engine_weight, model_cache_dir

CPU_ENGINE = "cpu"

__all__ = [
    "CPU_ENGINE",
    "MatcherUnavailable",
    "ColorMatchBackend",
    "model_cache_dir",
    "known_engines",
    "resolve",
    "probe",
]


class MatcherUnavailable(RuntimeError):
    """Raised by a matcher that was asked to run without its deps / weights.

    Carries a short human-readable ``reason`` recorded in the match report so
    the UI can explain why the heuristic path was used.
    """

    def __init__(self, reason: str) -> None:
        super().__init__(reason)
        self.reason = reason


class ColorMatchBackend(Protocol):
    """A learned light/colour matcher selectable via the node's ``engine`` param."""

    #: Stable id used as the ``engine`` param value (e.g. ``"onnx_harmonize"``).
    id: str

    def available(self) -> tuple[bool, str]:
        """Return ``(ok, reason)``; ``reason`` explains *why not* when not ok."""
        ...

    def weight_path(self) -> Path:
        """Resolved path of the (non-bundled) weight this matcher would load."""
        ...

    def match(self, rgb: Any, alpha: Any, background_rgb: Any) -> Any:
        """Harmonise a subject toward a background reference.

        ``rgb`` is the subject as an ``(H, W, 3)`` uint8 array, ``alpha`` its
        ``(H, W)`` float matte in 0..1, and ``background_rgb`` the reference as
        an ``(H', W', 3)`` uint8 array. Returns an ``(H, W, 3)`` uint8 array the
        same size as ``rgb`` with the subject's light/colour nudged toward the
        background. Raises :class:`MatcherUnavailable` if deps / weights vanished
        between the probe and the call.
        """
        ...


# ---- registry ------------------------------------------------------------

# Imported lazily so this module stays onnxruntime/torch-free at import time.
def _registry() -> dict[str, ColorMatchBackend]:
    from .onnx_harmonize import OnnxHarmonizeBackend

    backends: list[ColorMatchBackend] = [OnnxHarmonizeBackend()]
    return {b.id: b for b in backends}


def known_engines() -> list[str]:
    """All selectable engine ids, with ``cpu`` first."""
    return [CPU_ENGINE, *sorted(_registry().keys())]


def resolve(engine: str | None) -> ColorMatchBackend | None:
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
        CPU_ENGINE: {
            "available": True,
            "reason": "built-in CPU heuristic match",
            "accelerated": False,
        },
    }
    for name, backend in _registry().items():
        try:
            ok, reason = backend.available()
        except Exception as err:  # noqa: BLE001 - a broken probe must not crash the report
            ok, reason = False, f"{type(err).__name__}: {err}"
        # GPU-capable engine: the UI pairs this with the machine device probe to
        # warn it would fall back to CPU on a box with no CUDA device. ``weight``
        # is the cached-weight inventory (which non-bundled weight it loads).
        engines[name] = {
            "available": bool(ok),
            "reason": reason,
            "accelerated": True,
            "weight": _engine_weight(backend),
        }
    return {"engines": engines, "model_cache_dir": str(model_cache_dir())}
