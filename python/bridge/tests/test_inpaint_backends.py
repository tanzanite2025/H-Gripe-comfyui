"""Unit tests for the Detail Repaint local inpaint seam (``inpaint_backends``).

These run with neither ``torch`` / ``diffusers`` nor an inpaint weight present
(as on CI and most dev boxes): the Stable Diffusion backend must report itself
*unavailable* rather than crash, and asking it to run anyway must raise
``InpaintUnavailable`` so the orchestrator falls back to the always-available
remote ``image.edit`` provider path. ``provider`` is never a registered local
backend -- it is the default and the fallback.
"""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

# ``inpaint_backends`` lives one directory up (``python/bridge``).
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import inpaint_backends as ib  # noqa: E402
from inpaint_backends import (  # noqa: E402
    InpaintUnavailable,
    known_engines,
    probe,
    resolve,
)
from inpaint_backends.sd_inpaint import StableDiffusionInpaintBackend  # noqa: E402

PIL = pytest.importorskip("PIL")
from PIL import Image  # noqa: E402


def test_resolve_provider_and_blank_return_none() -> None:
    # ``provider`` is the remote path the orchestrator owns, not a local
    # backend; the caller runs it directly.
    assert resolve("provider") is None
    assert resolve("") is None
    assert resolve(None) is None
    assert resolve("PROVIDER") is None  # case-insensitive


def test_resolve_unknown_engine_returns_none() -> None:
    # A stale / bogus engine name must not raise -- the caller records the
    # reason and emits an empty repaint set.
    assert resolve("does_not_exist") is None


def test_resolve_known_backend() -> None:
    backend = resolve("sd_inpaint")
    assert backend is not None
    assert backend.id == "sd_inpaint"


def test_known_engines_lists_provider_first() -> None:
    engines = known_engines()
    assert engines[0] == "provider"
    assert "sd_inpaint" in engines


def test_probe_always_reports_provider_available() -> None:
    report = probe()
    assert report["engines"]["provider"]["available"] is True
    assert "sd_inpaint" in report["engines"]
    assert "model_cache_dir" in report


def test_sd_inpaint_unavailable_without_weight(monkeypatch: pytest.MonkeyPatch) -> None:
    # No weight on disk -> unavailable with a helpful reason, never a crash.
    monkeypatch.delenv("HGRIPE_INPAINT_MODEL", raising=False)
    monkeypatch.setenv("HGRIPE_MODEL_CACHE", "/definitely/not/here")
    backend = StableDiffusionInpaintBackend()
    ok, reason = backend.available()
    assert ok is False
    assert reason  # non-empty explanation


def test_sd_inpaint_inpaint_raises_when_unavailable(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.delenv("HGRIPE_INPAINT_MODEL", raising=False)
    monkeypatch.setenv("HGRIPE_MODEL_CACHE", "/definitely/not/here")
    backend = StableDiffusionInpaintBackend()
    crop = Image.new("RGB", (16, 16))
    mask = Image.new("L", (16, 16), 255)
    with pytest.raises(InpaintUnavailable):
        backend.inpaint(crop, mask, "restore")


def test_weight_path_prefers_env_override(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("HGRIPE_INPAINT_MODEL", "/models/my-inpaint")
    assert StableDiffusionInpaintBackend().weight_path() == Path("/models/my-inpaint")


def test_probe_survives_a_broken_backend(monkeypatch: pytest.MonkeyPatch) -> None:
    # A backend whose available() explodes must be reported unavailable, not
    # crash the whole capability probe.
    class Boom:
        id = "boom"

        def available(self) -> tuple[bool, str]:
            raise RuntimeError("kaboom")

    monkeypatch.setattr(ib, "_registry", lambda: {"boom": Boom()})
    report = probe()
    assert report["engines"]["boom"]["available"] is False
    assert "kaboom" in report["engines"]["boom"]["reason"]
