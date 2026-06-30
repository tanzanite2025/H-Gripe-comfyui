"""Pluggable local inpaint backends for the Detail Repaint node.

The Phase 1 pixel path (``prepare`` crops + masks, ``composite`` feathered
paste-back) lives in ``detail_repaint_cli.py`` and is always available; the
*generative* fix between those two halves is, today, the H-Gripe broker's
remote ``image.edit`` provider call owned by the Rust/TS orchestrator. This
package is the ``engine`` seam from ``docs/phase2-algorithm-roadmap.md`` §3:
additional **local** GPU inpaint engines (``sd_inpaint`` now; SDXL / Flux Fill
later) register here and are selected per run by the node's ``engine`` param,
consuming the *same* prepare manifest so the contract is unchanged.

Design rules (mirroring the ``sr_backends`` / ``detector_backends`` seams):

* **Additive, opt-in, never default.** ``provider`` (the remote ``image.edit``
  path the orchestrator already drives) stays the default and the fallback. A
  local backend is only used when the caller explicitly asks for it *and*
  :meth:`InpaintBackend.available` returns ``True``.
* **CPU-safe import.** Importing this package must not import ``torch`` /
  ``diffusers`` or any heavy/optional dependency — backends import their deps
  lazily inside :meth:`InpaintBackend.available` / :meth:`InpaintBackend.inpaint`.
* **Weights are not bundled.** A backend resolves its weights from the model
  cache dir (``HGRIPE_MODEL_CACHE``, falling back to ``resources/models``); a
  missing weight makes the backend unavailable, not an error.
* **Graceful degradation.** When a requested engine is unavailable the caller
  records the reason and emits an empty repaint set, so the orchestrator falls
  back to the remote provider — the node always produces an output.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any, Protocol

# Reuse the one model-cache resolver (torch-free, defined for the SR seam) so
# downloadable weights for every node land in the same place.
from sr_backends import _engine_weight, model_cache_dir

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
    the UI can explain why the remote-provider path was used.
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

    def weight_path(self) -> Path:
        """Resolved path of the (non-bundled) weight this engine would load."""
        ...

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
        """Inpaint the masked area of a padded ``crop`` and return the result.

        ``crop`` is a PIL ``RGB`` image (the padded window from ``prepare``);
        ``mask`` is a PIL ``L`` image the same size where **white (255) marks
        the area to regenerate** (the diffusers convention). The returned PIL
        ``RGB`` image is the same size as ``crop``. Raises
        :class:`InpaintUnavailable` if deps/weights vanished between the probe
        and the call.
        """
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
    """Return the backend for ``engine`` or ``None`` for the remote-provider path.

    Unknown engine names resolve to ``None`` (the caller emits an empty repaint
    set and records the reason) rather than raising, so a stale saved graph
    never hard fails.
    """
    name = (engine or PROVIDER_ENGINE).strip().lower()
    if name in ("", PROVIDER_ENGINE):
        return None
    return _registry().get(name)


def probe() -> dict[str, Any]:
    """Capability report for the UI: which engines are usable right now.

    Lets the inspector grey out local GPU engines when their deps / weights are
    missing. Always includes ``provider`` as available (the orchestrator owns
    the remote call, so it is always selectable here).
    """
    engines: dict[str, Any] = {
        PROVIDER_ENGINE: {
            "available": True,
            "reason": "remote image.edit provider (orchestrator)",
            # Remote call, not a local accelerator, so no GPU/CPU device note.
            "accelerated": False,
        },
    }
    for name, backend in _registry().items():
        try:
            ok, reason = backend.available()
        except Exception as err:  # noqa: BLE001 - a broken probe must not crash the report
            ok, reason = False, f"{type(err).__name__}: {err}"
        # GPU-capable local engine: the UI pairs this with the machine device
        # probe to warn it would fall back to CPU on a box with no CUDA device.
        # ``weight`` is the cached-weight inventory (which weight it loads).
        engines[name] = {
            "available": bool(ok),
            "reason": reason,
            "accelerated": True,
            "weight": _engine_weight(backend),
        }
    return {"engines": engines, "model_cache_dir": str(model_cache_dir())}
