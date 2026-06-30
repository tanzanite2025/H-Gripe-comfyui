"""Pluggable ML defect detectors for the Detail Watchdog node.

The Phase 1 rule layer (Laplacian-variance blur, tile-sharpness grid, alpha-rim
halo, mean-colour drift) lives in ``detail_watchdog_cli.py`` and is always
available. This package is the ``engine`` seam from
``docs/phase2-algorithm-roadmap.md`` §2: additional *learned* detectors register
here and are selected per run by the node's ``engine`` param. They graduate the
semantic targets the rule layer honestly records as ``skipped`` today (hands /
text / logo) into real findings, emitting into the **same** ``QualityReport``
contract so the downstream Detail Repaint consumer needs no change.

Design rules (mirroring the ``sr_backends`` super-resolution seam):

* **Additive, opt-in, never default.** ``rules`` stays the default and the
  always-on baseline; an ML detector is only run when the caller explicitly asks
  for it *and* :meth:`DetectorBackend.available` returns ``True``. Its findings
  are merged on top of the rule findings — the rule layer always runs.
* **CPU-safe import.** Importing this package must not import ``onnxruntime`` /
  ``torch`` or any heavy/optional dependency — backends import their deps lazily
  inside :meth:`DetectorBackend.available` / :meth:`DetectorBackend.detect`.
* **Weights are not bundled.** A backend resolves its weights from the model
  cache dir (``HGRIPE_MODEL_CACHE``, falling back to ``resources/models``); a
  missing weight makes the backend unavailable, not an error.
* **Graceful degradation.** When a requested engine is unavailable the caller
  records the reason and keeps the rule-only report — the node always produces
  an output and the uncovered semantic targets stay ``skipped`` exactly as today.
"""

from __future__ import annotations

from typing import Any, Protocol

# Reuse the one model-cache resolver (torch-free, defined for the SR seam) so
# downloadable weights for every node land in the same place.
from sr_backends import model_cache_dir

RULES_ENGINE = "rules"

__all__ = [
    "RULES_ENGINE",
    "DetectorUnavailable",
    "DetectorBackend",
    "model_cache_dir",
    "known_engines",
    "resolve",
    "probe",
]


class DetectorUnavailable(RuntimeError):
    """Raised by a detector that was asked to run without its deps / weights.

    Carries a short human-readable ``reason`` recorded in the watchdog report so
    the UI can explain why the rule-only path was used.
    """

    def __init__(self, reason: str) -> None:
        super().__init__(reason)
        self.reason = reason


class DetectorBackend(Protocol):
    """A learned defect detector selectable via the node's ``engine`` param."""

    #: Stable id used as the ``engine`` param value (e.g. ``"onnx_defect"``).
    id: str
    #: Semantic watch targets this backend can honestly cover (e.g. hands/text/logo).
    targets: tuple[str, ...]

    def available(self) -> tuple[bool, str]:
        """Return ``(ok, reason)``; ``reason`` explains *why not* when not ok."""
        ...

    def detect(
        self, rgb: Any, alpha: Any, watch: set[str]
    ) -> list[dict[str, Any]]:
        """Detect defects on an 8-bit RGB image, restricted to ``watch`` targets.

        ``rgb`` is an ``(H, W, 3)`` uint8 array, ``alpha`` an ``(H, W)`` float
        array in 0..1. Returns a list of issue dicts in the shared
        ``QualityReport`` shape (``type`` / ``confidence`` / ``bbox`` /
        ``suggested_action``). Raises :class:`DetectorUnavailable` if deps /
        weights vanished between the probe and the call.
        """
        ...


# ---- registry ------------------------------------------------------------

# Imported lazily so this module stays onnxruntime/torch-free at import time.
def _registry() -> dict[str, DetectorBackend]:
    from .onnx_defect import OnnxDefectBackend

    backends: list[DetectorBackend] = [OnnxDefectBackend()]
    return {b.id: b for b in backends}


def known_engines() -> list[str]:
    """All selectable engine ids, with ``rules`` first."""
    return [RULES_ENGINE, *sorted(_registry().keys())]


def resolve(engine: str | None) -> DetectorBackend | None:
    """Return the backend for ``engine`` or ``None`` for the rule-only path.

    Unknown engine names resolve to ``None`` (the caller keeps the rule-only
    report and records the reason) rather than raising, so a stale saved graph
    never hard fails.
    """
    name = (engine or RULES_ENGINE).strip().lower()
    if name in ("", RULES_ENGINE):
        return None
    return _registry().get(name)


def probe() -> dict[str, Any]:
    """Capability report for the UI: which engines are usable right now.

    Lets the inspector grey out ML engines when their deps / weights are
    missing. Always includes ``rules`` as available.
    """
    engines: dict[str, Any] = {
        RULES_ENGINE: {
            "available": True,
            "reason": "built-in CPU rule layer",
            "accelerated": False,
        },
    }
    for name, backend in _registry().items():
        try:
            ok, reason = backend.available()
        except Exception as err:  # noqa: BLE001 - a broken probe must not crash the report
            ok, reason = False, f"{type(err).__name__}: {err}"
        engines[name] = {
            "available": bool(ok),
            "reason": reason,
            "targets": list(getattr(backend, "targets", ())),
            # GPU-capable engine: the UI pairs this with the machine device probe
            # to warn it would fall back to CPU on a box with no CUDA device.
            "accelerated": True,
        }
    return {"engines": engines, "model_cache_dir": str(model_cache_dir())}
