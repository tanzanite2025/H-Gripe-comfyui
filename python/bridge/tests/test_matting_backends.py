"""Unit tests for the Mask Edge Refine learned-matter seam (``matting_backends``).

Most of these run with neither ``onnxruntime`` nor a model weight present (as on
CI and most dev boxes): the ONNX matter must report itself *unavailable* rather
than crash, and asking it to run anyway must raise ``MattingUnavailable`` so the
node falls back to the always-on heuristic refine.

``test_onnx_matting_*`` (and the CLI dispatch case) are gated end-to-end checks:
they synthesise a tiny ONNX matter (so no real weights are needed) and are
skipped unless both ``onnx`` and ``onnxruntime`` are importable, mirroring the
detector / SR / color opt-in gates.
"""

from __future__ import annotations

import sys
from pathlib import Path

import numpy as np
import pytest

# ``matting_backends`` lives one directory up (``python/bridge``).
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import matting_backends as mb  # noqa: E402
from matting_backends import MattingUnavailable, known_engines, probe, resolve  # noqa: E402
from matting_backends.vitmatte_onnx import OnnxMattingBackend  # noqa: E402


def test_resolve_cpu_and_blank_return_none() -> None:
    # The heuristic refine is not a registered backend; the caller runs it.
    assert resolve("cpu") is None
    assert resolve("") is None
    assert resolve(None) is None
    assert resolve("CPU") is None  # case-insensitive


def test_resolve_unknown_engine_returns_none() -> None:
    assert resolve("does_not_exist") is None


def test_resolve_known_backend() -> None:
    backend = resolve("onnx_matting")
    assert backend is not None
    assert backend.id == "onnx_matting"


def test_known_engines_lists_cpu_first() -> None:
    engines = known_engines()
    assert engines[0] == "cpu"
    assert "onnx_matting" in engines


def test_probe_always_reports_cpu_available() -> None:
    report = probe()
    assert report["engines"]["cpu"]["available"] is True
    assert "onnx_matting" in report["engines"]
    assert report["engines"]["cpu"]["accelerated"] is False
    assert report["engines"]["onnx_matting"]["accelerated"] is True
    assert "model_cache_dir" in report


def test_weight_path_prefers_env_override(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    weight = tmp_path / "custom_matting.onnx"
    monkeypatch.setenv("HGRIPE_MATTING_MODEL", str(weight))
    assert OnnxMattingBackend().weight_path() == weight


def test_onnx_matting_unavailable_without_weight(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("HGRIPE_MATTING_MODEL", raising=False)
    monkeypatch.setenv("HGRIPE_MODEL_CACHE", "/definitely/not/here")
    ok, reason = OnnxMattingBackend().available()
    assert ok is False
    assert reason


def test_onnx_matting_matte_raises_when_unavailable(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("HGRIPE_MATTING_MODEL", raising=False)
    monkeypatch.setenv("HGRIPE_MODEL_CACHE", "/definitely/not/here")
    backend = OnnxMattingBackend()
    rgb = np.zeros((16, 16, 3), dtype=np.uint8)
    trimap = np.full((16, 16), 0.5, dtype=np.float32)
    with pytest.raises(MattingUnavailable):
        backend.matte(rgb, trimap)


def test_probe_survives_a_broken_backend(monkeypatch: pytest.MonkeyPatch) -> None:
    class Boom:
        id = "boom"

        def available(self) -> tuple[bool, str]:
            raise RuntimeError("kaboom")

    monkeypatch.setattr(mb, "_registry", lambda: {"boom": Boom()})
    report = probe()
    assert report["engines"]["boom"]["available"] is False
    assert "kaboom" in report["engines"]["boom"]["reason"]


# --- gated end-to-end: synthesise a tiny ONNX matter ----------------------


def _onnx_stack_importable() -> bool:
    """True only if both ``onnx`` and ``onnxruntime`` actually import."""
    try:
        import onnx  # noqa: F401
        import onnxruntime  # noqa: F401
    except Exception:
        return False
    return True


requires_onnx = pytest.mark.skipif(
    not _onnx_stack_importable(),
    reason="onnx / onnxruntime not importable (opt-in ML matter gate)",
)


def _make_tiny_matter(path: Path, size: int = 64) -> None:
    """A minimal ONNX graph: ``image`` ``[1,3,size,size]`` + ``trimap``
    ``[1,1,size,size]`` float inputs and an ``alpha`` ``[1,1,size,size]`` output
    that is the trimap passed through (Identity). Enough to exercise dispatch +
    telemetry without a real network. ``image`` is declared but unused.
    """
    import onnx
    from onnx import TensorProto, helper

    image = helper.make_tensor_value_info("image", TensorProto.FLOAT, [1, 3, size, size])
    trimap = helper.make_tensor_value_info("trimap", TensorProto.FLOAT, [1, 1, size, size])
    out = helper.make_tensor_value_info("alpha", TensorProto.FLOAT, [1, 1, size, size])
    node = helper.make_node("Identity", ["trimap"], ["alpha"])
    graph = helper.make_graph([node], "tiny_matter", [image, trimap], [out])
    model = helper.make_model(graph, opset_imports=[helper.make_opsetid("", 13)])
    onnx.checker.check_model(model)
    onnx.save(model, str(path))


@requires_onnx
def test_onnx_matting_runs_with_synthetic_model(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    weight = tmp_path / "matting.onnx"
    _make_tiny_matter(weight)
    monkeypatch.setenv("HGRIPE_MATTING_MODEL", str(weight))

    backend = OnnxMattingBackend()
    ok, reason = backend.available()
    assert ok, reason

    rgb = (np.random.rand(40, 32, 3) * 255).astype(np.uint8)
    trimap = np.full((40, 32), 0.5, dtype=np.float32)
    trimap[:8] = 1.0
    trimap[-8:] = 0.0

    alpha = backend.matte(rgb, trimap)
    # Geometry contract: solved alpha matches the source subject size.
    assert alpha.shape == rgb.shape[:2]
    assert alpha.dtype == np.float32
    assert float(alpha.min()) >= 0.0 and float(alpha.max()) <= 1.0


@requires_onnx
def test_onnx_matting_dispatch_via_refine(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # End-to-end through the CLI: an available learned matter runs and stamps the
    # edge report with its telemetry.
    import edge_refine_cli as cli
    from PIL import Image

    weight = tmp_path / "matting.onnx"
    _make_tiny_matter(weight)
    monkeypatch.setenv("HGRIPE_MATTING_MODEL", str(weight))

    size = 48
    yy, xx = np.mgrid[0:size, 0:size]
    dist = np.sqrt((xx - 23.5) ** 2 + (yy - 23.5) ** 2)
    alpha = np.clip((16.0 - dist) / 2.0 + 0.5, 0.0, 1.0).astype(np.float32)
    rgba = np.zeros((size, size, 4), dtype=np.uint8)
    rgba[..., :3] = (40, 160, 60)
    rgba[..., 3] = np.rint(alpha * 255.0).astype(np.uint8)
    subj = tmp_path / "subject.png"
    Image.fromarray(rgba, "RGBA").save(subj)

    tri = np.full((size, size), 128, dtype=np.uint8)
    tri[alpha > 0.9] = 255
    tri[alpha < 0.1] = 0
    trimap_path = tmp_path / "trimap.png"
    Image.fromarray(tri, "L").save(trimap_path)

    args = cli.build_parser().parse_args(
        [
            "--image",
            str(subj),
            "--trimap",
            str(trimap_path),
            "--engine",
            "onnx_matting",
            "--output-dir",
            str(tmp_path),
        ]
    )
    report = cli.refine(args)["edge_report"]
    assert report["engine"] == "onnx_matting"
    assert report["engine_requested"] == "onnx_matting"
    assert report["engine_fallback_reason"] is None
    assert report["backend_model"] == "matting.onnx"
    assert report["trimap_applied"] is True
