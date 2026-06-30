"""Pluggable local inpaint backends for the Detail Repaint node.

The Phase 1 path for Detail Repaint is *provider-driven*: the
``prepare``/``composite`` halves in ``detail_repaint_cli.py`` are a thin,
``torch``-free pixel split around a remote ``image.edit`` provider call owned by
the Rust/TS orchestration layer. This package is the ``engine`` seam from
``docs/phase2-algorithm-roadmap.md`` §3: a **local** generative inpaint backend
(``sd_inpaint`` now; ControlNet / Flux Fill later) registers here and is
selected per run by the node's ``engine`` param, consuming the *same* crop +
mask + prompt manifest the provider path already produces — so ``composite``
and the ``RepaintReport`` contract need no change.

Design rules (mirroring the ``sr_backends`` / ``detector_backends`` seams):

* **Additive, opt-in, never default.** ``provider`` stays the default and the
  always-available baseline (the orchestrator's remote ``image.edit`` call). A
  local backend is only used when the caller explicitly asks for it *and*
  :meth:`InpaintBackend.available` returns ``True``.
* **CPU-safe import.** Importing this package must not import ``torch`` /
  ``diffusers`` or any heavy/optional dependency — backends import their deps
  lazily inside :meth:`InpaintBackend.available` / :meth:`InpaintBackend.inpaint`.
* **Weights are not bundled.** A backend resolves its weights from the model
  cache dir (``HGRIPE_MODEL_CACHE``, falling back to ``resources/models``); a
  missing weight makes the backend unavailable, not an error.
* **Graceful degradation.** When a requested engine is unavailable the caller
  records the reason and the regions are left for the provider path / pass
  through unrepainted — the node always produces an output.
"""

from __future__ import annotations

from typing import Any, Protocol

# Reuse the one model-cache resolver (torch-free, defined for the SR seam) so
# downloadable weights for every node land in the same place.
from sr_backends import model_cache_dir

PROVIDER_ENGINE = "provider"

__all__ = [
    "PROVIDER_ENGINE",
    "InpaintUnavailable",
    "InpaintBackend",
    "model_cache_dir",
    "known_engines",
    "resolve",
    "probe",
]


class InpaintUnavailable(RuntimeError):
    """Raised by a backend that was asked to run without its deps / weights.

    Carries a short human-readable ``reason`` recorded in the repaint report so
    the UI can explain why the provider / passthrough path was used.
    """

    def __init__(self, reason: str) -> None:
        super().__init__(reason)
        self.reason = reason


class InpaintBackend(Protocol):
    """A local inpaint engine selectable via the node's ``engine`` param."""

    #: Stable id used as the ``engine`` param value (e.g. ``"sd_inpaint"``).
    id: str

    def available(self) -> tuple[bool, str]:
        """Return ``(ok, reason)``; ``reason`` explains *why not* when not ok."""
        ...

    def inpaint(
        self,
        rgb: Any,
        mask: Any,
        prompt: str,
        **options: Any,
    ) -> Any:
        """Inpaint the masked region of an 8-bit RGB crop.

        ``rgb`` is an ``(H, W, 3)`` uint8 array, ``mask`` an ``(H, W)`` uint8
        array where ``255`` marks the pixels to regenerate (the issue core) and
        ``0`` the context to preserve. Returns an ``(H, W, 3)`` uint8 array the
        same size as ``rgb``. Raises :class:`InpaintUnavailable` if deps /
        weights vanished between the probe and the call.
        """
        ...

    def backend_model(self) -> str | None:
        """Short identifier of the resolved weight, for report telemetry."""
        ...


# ---- registry ------------------------------------------------------------

# Imported lazily so this module stays torch/diffusers-free at import time.
def _registry() -> dict[str, InpaintBackend]:
    from .sd_inpaint import StableDiffusionInpaintBackend

    backends: list[InpaintBackend] = [StableDiffusionInpaintBackend()]
    return {b.id: b for b in backends}


def known_engines() -> list[str]:
    """All selectable engine ids, with ``provider`` first."""
    return [PROVIDER_ENGINE, *sorted(_registry().keys())]


def resolve(engine: str | None) -> InpaintBackend | None:
    """Return the backend for ``engine`` or ``None`` for the provider path.

    Unknown engine names resolve to ``None`` (the caller records the reason and
    keeps the provider / passthrough path) rather than raising, so a stale saved
    graph never hard fails.
    """
    name = (engine or PROVIDER_ENGINE).strip().lower()
    if name in ("", PROVIDER_ENGINE):
        return None
    return _registry().get(name)


def probe() -> dict[str, Any]:
    """Capability report for the UI: which engines are usable right now.

    Lets the inspector grey out the local engine when its deps / weights are
    missing. Always includes ``provider`` as available (the remote ``image.edit``
    baseline; whether a provider is *configured* is a separate credentials
    concern the UI already surfaces).
    """
    engines: dict[str, Any] = {
        PROVIDER_ENGINE: {
            "available": True,
            "reason": "remote image.edit provider",
        },
    }
    for name, backend in _registry().items():
        try:
            ok, reason = backend.available()
        except Exception as err:  # noqa: BLE001 - a broken probe must not crash the report
            ok, reason = False, f"{type(err).__name__}: {err}"
        engines[name] = {"available": bool(ok), "reason": reason}
    return {"engines": engines, "model_cache_dir": str(model_cache_dir())}
