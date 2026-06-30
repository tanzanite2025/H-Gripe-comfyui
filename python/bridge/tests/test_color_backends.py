"""Unit tests for the Match Light & Color learned-matcher seam (``color_backends``).

Most of these run with neither ``onnxruntime`` nor a model weight present (as on
CI and most dev boxes): the ONNX harmonise backend must report itself
*unavailable* rather than crash, and asking it to run anyway must raise
``MatcherUnavailable`` so the node falls back to the always-on heuristic match.

``test_onnx_harmonize_*`` (and the CLI dispatch case) are gated end-to-end checks:
they synthesise a tiny ONNX harmoniser (so no real weights are needed) and are
skipped unless both ``onnx`` and ``onnxruntime`` are importable, mirroring the
detector / SR opt-in gates.
"""

from __future__ import annotations

import sys
from pathlib import Path

import numpy as np
import pytest

# ``color_backends`` lives one directory up (``python/bridge``).
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import color_backends as cb  # noqa: E402
from color_backends import MatcherUnavailable, known_engines, probe, resolve  # noqa: E402
from color_backends.onnx_harmonize import OnnxHarmonizeBackend  # noqa: E402


def test_resolve_cpu_and_blank_return_none() -> None:
    # The heuristic match is not a registered backend; the caller runs it.
    assert resolve("cpu") is None
    assert resolve("") is None
    assert resolve(None) is None
    assert resolve("CPU") is None  # case-insensitive


def test_resolve_unknown_engine_returns_none() -> None:
    # A stale / bogus engine name must not raise -- the caller records the
    # reason and keeps the heuristic result.
    assert resolve("does_not_exist") is None


def test_resolve_known_backend() -> None:
    backend = resolve("onnx_harmonize")
    assert backend is not None
    assert backend.id == "onnx_harmonize"


def test_known_engines_lists_cpu_first() -> None:
    engines = known_engines()
    assert engines[0] == "cpu"
    assert "onnx_harmonize" in engines


def test_probe_always_reports_cpu_available() -> None:
    report = probe()
    assert report["engines"]["cpu"]["available"] is True
    assert "onnx_harmonize" in report["engines"]
    assert "model_cache_dir" in report


def test_weight_path_prefers_env_override(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    weight = tmp_path / "custom_harmonize.onnx"
    monkeypatch.setenv("HGRIPE_COLOR_MODEL", str(weight))
    assert OnnxHarmonizeBackend().weight_path() == weight


def test_onnx_harmonize_unavailable_without_weight(monkeypatch: pytest.MonkeyPatch) -> None:
    # No weight on disk -> unavailable with a helpful reason, never a crash.
    monkeypatch.delenv("HGRIPE_COLOR_MODEL", raising=False)
    monkeypatch.setenv("HGRIPE_MODEL_CACHE", "/definitely/not/here")
    ok, reason = OnnxHarmonizeBackend().available()
    assert ok is False
    assert reason  # non-empty explanation


def test_onnx_harmonize_match_raises_when_unavailable(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("HGRIPE_COLOR_MODEL", raising=False)
    monkeypatch.setenv("HGRIPE_MODEL_CACHE", "/definitely/not/here")
    backend = OnnxHarmonizeBackend()
    rgb = np.zeros((16, 16, 3), dtype=np.uint8)
    alpha = np.ones((16, 16), dtype=np.float32)
    bg = np.zeros((16, 16, 3), dtype=np.uint8)
    with pytest.raises(MatcherUnavailable):
        backend.match(rgb, alpha, bg)


def test_probe_survives_a_broken_backend(monkeypatch: pytest.MonkeyPatch) -> None:
    # A backend whose available() explodes must be reported unavailable, not
    # crash the whole capability probe.
    class Boom:
        id = "boom"

        def available(self) -> tuple[bool, str]:
            raise RuntimeError("kaboom")

    monkeypatch.setattr(cb, "_registry", lambda: {"boom": Boom()})
    report = probe()
    assert report["engines"]["boom"]["available"] is False
    assert "kaboom" in report["engines"]["boom"]["reason"]


# --- gated end-to-end: synthesise a tiny ONNX harmoniser ------------------


def _onnx_stack_importable() -> bool:
    """True only if both ``onnx`` and ``onnxruntime`` actually import.

    ``find_spec`` is not enough: a wheel can be installed yet fail at import
    (e.g. a missing native runtime DLL). A broad ``except`` skips those boxes
    instead of erroring collection, and CI without the deps skips cleanly.
    """
    try:
        import onnx  # noqa: F401
        import onnxruntime  # noqa: F401
    except Exception:
        return False
    return True


requires_onnx = pytest.mark.skipif(
    not _onnx_stack_importable(),
    reason="onnx / onnxruntime not importable (opt-in ML harmoniser gate)",
)


def _make_tiny_harmoniser(path: Path, size: int = 64) -> None:
    """A minimal ONNX graph: ``image`` ``[1,3,size,size]`` + ``mask``
    ``[1,1,size,size]`` float inputs and a ``harmonized`` output that is the
    image passed through (Identity). Enough to exercise dispatch + telemetry
    without a real network. ``mask`` is declared but unused (ONNX allows it).
    """
    import onnx
    from onnx import TensorProto, helper

    image = helper.make_tensor_value_info("image", TensorProto.FLOAT, [1, 3, size, size])
    mask = helper.make_tensor_value_info("mask", TensorProto.FLOAT, [1, 1, size, size])
    out = helper.make_tensor_value_info("harmonized", TensorProto.FLOAT, [1, 3, size, size])
    node = helper.make_node("Identity", ["image"], ["harmonized"])
    graph = helper.make_graph([node], "tiny_harmoniser", [image, mask], [out])
    model = helper.make_model(graph, opset_imports=[helper.make_opsetid("", 13)])
    onnx.checker.check_model(model)
    onnx.save(model, str(path))


@requires_onnx
def test_onnx_harmonize_runs_with_synthetic_model(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    weight = tmp_path / "color_harmonize.onnx"
    _make_tiny_harmoniser(weight)
    monkeypatch.setenv("HGRIPE_COLOR_MODEL", str(weight))

    backend = OnnxHarmonizeBackend()
    ok, reason = backend.available()
    assert ok, reason

    rgb = (np.random.rand(40, 32, 3) * 255).astype(np.uint8)
    alpha = np.ones((40, 32), dtype=np.float32)
    bg = (np.random.rand(50, 60, 3) * 255).astype(np.uint8)

    out = backend.match(rgb, alpha, bg)
    # Geometry contract: harmonised output matches the source subject size.
    assert out.shape == rgb.shape
    assert out.dtype == np.uint8


@requires_onnx
def test_onnx_harmonize_dispatch_via_match(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # End-to-end through the CLI dispatch helper: an available learned engine
    # runs and stamps the report with its telemetry.
    import color_match_cli as cli
    from PIL import Image

    weight = tmp_path / "color_harmonize.onnx"
    _make_tiny_harmoniser(weight)
    monkeypatch.setenv("HGRIPE_COLOR_MODEL", str(weight))

    subj = tmp_path / "subject.png"
    Image.fromarray((np.random.rand(40, 32, 3) * 255).astype(np.uint8), "RGB").save(subj)
    bg = tmp_path / "background.png"
    Image.fromarray((np.random.rand(50, 60, 3) * 255).astype(np.uint8), "RGB").save(bg)

    args = cli.build_parser().parse_args(
        [
            "--image",
            str(subj),
            "--background",
            str(bg),
            "--mode",
            "color_transfer",
            "--engine",
            "onnx_harmonize",
            "--output-dir",
            str(tmp_path),
        ]
    )
    report = cli.match(args)["match_report"]
    assert report["engine"] == "onnx_harmonize"
    assert report["engine_requested"] == "onnx_harmonize"
    assert report["engine_fallback_reason"] is None
    assert report["backend_model"] == "color_harmonize.onnx"
    assert report["applied"] is True
