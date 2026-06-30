"""Unit tests for the Detail Watchdog ML detector seam (``detector_backends``).

Most of these run with neither ``onnxruntime`` nor a model weight present (as on
CI and most dev boxes): the ONNX defect backend must report itself *unavailable*
rather than crash, and asking it to run anyway must raise ``DetectorUnavailable``
so the watchdog falls back to the always-on rule layer.

``test_onnx_defect_detects_*`` is a gated end-to-end check: it synthesises a tiny
ONNX detector (so no real weights are needed) and is skipped unless both ``onnx``
and ``onnxruntime`` are importable, mirroring the ViTMatte opt-in gate.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import numpy as np
import pytest

# ``detector_backends`` lives one directory up (``python/bridge``).
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import detector_backends as db  # noqa: E402
from detector_backends import DetectorUnavailable, known_engines, probe, resolve  # noqa: E402
from detector_backends.onnx_defect import OnnxDefectBackend  # noqa: E402


def test_resolve_rules_and_blank_return_none() -> None:
    # The rule layer is not a registered backend; the caller runs it directly.
    assert resolve("rules") is None
    assert resolve("") is None
    assert resolve(None) is None
    assert resolve("RULES") is None  # case-insensitive


def test_resolve_unknown_engine_returns_none() -> None:
    # A stale / bogus engine name must not raise -- the caller records the reason
    # and keeps the rule-only report.
    assert resolve("does_not_exist") is None


def test_resolve_known_backend() -> None:
    backend = resolve("onnx_defect")
    assert backend is not None
    assert backend.id == "onnx_defect"
    assert set(backend.targets) == {"hands", "text", "logo"}


def test_known_engines_lists_rules_first() -> None:
    engines = known_engines()
    assert engines[0] == "rules"
    assert "onnx_defect" in engines


def test_probe_always_reports_rules_available() -> None:
    report = probe()
    assert report["engines"]["rules"]["available"] is True
    assert "onnx_defect" in report["engines"]
    assert report["engines"]["rules"]["accelerated"] is False
    assert report["engines"]["onnx_defect"]["accelerated"] is True
    # Cached-weight inventory: the rule baseline loads none; the ML engine names it.
    assert "weight" not in report["engines"]["rules"]
    weight = report["engines"]["onnx_defect"]["weight"]
    assert weight["path"].endswith("watchdog_defect.onnx")
    assert isinstance(weight["present"], bool)
    assert "model_cache_dir" in report


def test_onnx_defect_unavailable_without_weight(monkeypatch: pytest.MonkeyPatch) -> None:
    # No weight on disk -> unavailable with a helpful reason, never a crash.
    monkeypatch.delenv("HGRIPE_WATCHDOG_MODEL", raising=False)
    monkeypatch.setenv("HGRIPE_MODEL_CACHE", "/definitely/not/here")
    backend = OnnxDefectBackend()
    ok, reason = backend.available()
    assert ok is False
    assert reason  # non-empty explanation


def test_onnx_defect_detect_raises_when_unavailable(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.delenv("HGRIPE_WATCHDOG_MODEL", raising=False)
    monkeypatch.setenv("HGRIPE_MODEL_CACHE", "/definitely/not/here")
    backend = OnnxDefectBackend()
    rgb = np.zeros((16, 16, 3), dtype=np.uint8)
    alpha = np.ones((16, 16), dtype=np.float32)
    with pytest.raises(DetectorUnavailable):
        backend.detect(rgb, alpha, {"hands"})


def test_probe_survives_a_broken_backend(monkeypatch: pytest.MonkeyPatch) -> None:
    # A backend whose available() explodes must be reported unavailable, not
    # crash the whole capability probe.
    class Boom:
        id = "boom"
        targets = ("hands",)

        def available(self) -> tuple[bool, str]:
            raise RuntimeError("kaboom")

    monkeypatch.setattr(db, "_registry", lambda: {"boom": Boom()})
    report = probe()
    assert report["engines"]["boom"]["available"] is False
    assert "kaboom" in report["engines"]["boom"]["reason"]


def test_label_map_prefers_sidecar(tmp_path: Path) -> None:
    weight = tmp_path / "watchdog_defect.onnx"
    weight.write_bytes(b"not-a-real-model")
    sidecar = tmp_path / "watchdog_defect.onnx.labels.json"
    sidecar.write_text(json.dumps({"0": "logo", "1": "hands"}), encoding="utf-8")
    backend = OnnxDefectBackend()
    assert backend._label_map(weight) == {0: "logo", 1: "hands"}


def test_label_map_falls_back_to_target_order(tmp_path: Path) -> None:
    weight = tmp_path / "watchdog_defect.onnx"
    backend = OnnxDefectBackend()
    assert backend._label_map(weight) == {0: "hands", 1: "text", 2: "logo"}


# --- gated end-to-end: synthesise a tiny ONNX detector --------------------


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
    reason="onnx / onnxruntime not importable (opt-in ML detector gate)",
)


def _make_tiny_detector(path: Path) -> None:
    """A minimal ONNX graph: a fixed [1,3,64,64] float input, constant
    ``boxes`` / ``scores`` / ``labels`` outputs (one detection, class 0).
    """
    import onnx
    from onnx import TensorProto, helper, numpy_helper

    inp = helper.make_tensor_value_info(
        "images", TensorProto.FLOAT, [1, 3, 64, 64]
    )
    boxes_out = helper.make_tensor_value_info("boxes", TensorProto.FLOAT, [1, 4])
    scores_out = helper.make_tensor_value_info("scores", TensorProto.FLOAT, [1])
    labels_out = helper.make_tensor_value_info("labels", TensorProto.INT64, [1])

    boxes_c = numpy_helper.from_array(
        np.array([[8, 8, 40, 40]], dtype=np.float32), name="boxes_const"
    )
    scores_c = numpy_helper.from_array(
        np.array([0.9], dtype=np.float32), name="scores_const"
    )
    labels_c = numpy_helper.from_array(
        np.array([0], dtype=np.int64), name="labels_const"
    )
    nodes = [
        helper.make_node("Constant", [], ["boxes"], value=boxes_c),
        helper.make_node("Constant", [], ["scores"], value=scores_c),
        helper.make_node("Constant", [], ["labels"], value=labels_c),
    ]
    graph = helper.make_graph(
        nodes, "tiny_detector", [inp], [boxes_out, scores_out, labels_out]
    )
    model = helper.make_model(graph, opset_imports=[helper.make_opsetid("", 13)])
    onnx.checker.check_model(model)
    onnx.save(model, str(path))


@requires_onnx
def test_onnx_defect_detects_with_synthetic_model(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    weight = tmp_path / "watchdog_defect.onnx"
    _make_tiny_detector(weight)
    # class 0 -> hands per the sidecar map.
    (tmp_path / "watchdog_defect.onnx.labels.json").write_text(
        json.dumps({"0": "hands"}), encoding="utf-8"
    )
    monkeypatch.setenv("HGRIPE_WATCHDOG_MODEL", str(weight))

    backend = OnnxDefectBackend()
    ok, reason = backend.available()
    assert ok, reason

    rgb = (np.random.rand(96, 80, 3) * 255).astype(np.uint8)
    alpha = np.ones((96, 80), dtype=np.float32)

    # Watching hands -> the detection graduates into a real issue.
    issues = backend.detect(rgb, alpha, {"hands"})
    assert len(issues) == 1
    issue = issues[0]
    assert issue["type"] == "malformed_hands"
    assert issue["suggested_action"] == "detail_redraw"
    assert 0.0 < issue["confidence"] <= 0.99
    x1, y1, x2, y2 = issue["bbox"]
    assert 0 <= x1 < x2 <= 80
    assert 0 <= y1 < y2 <= 96

    # The same detection is dropped when its target is not being watched.
    assert backend.detect(rgb, alpha, {"text"}) == []


@requires_onnx
def test_onnx_defect_dispatch_via_watch(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # End-to-end through the CLI dispatch helper: an available ML engine merges
    # findings and graduates its covered targets out of skipped_targets.
    import detail_watchdog_cli as cli
    from PIL import Image

    weight = tmp_path / "watchdog_defect.onnx"
    _make_tiny_detector(weight)
    (tmp_path / "watchdog_defect.onnx.labels.json").write_text(
        json.dumps({"0": "hands"}), encoding="utf-8"
    )
    monkeypatch.setenv("HGRIPE_WATCHDOG_MODEL", str(weight))

    image_path = tmp_path / "candidate.png"
    Image.fromarray(
        (np.random.rand(96, 80, 3) * 255).astype(np.uint8), "RGB"
    ).save(image_path)

    args = cli.build_parser().parse_args(
        [
            "--image",
            str(image_path),
            "--output-dir",
            str(tmp_path),
            "--engine",
            "onnx_defect",
            "--watch-targets",
            "hands,text",
            "--no-overlay",
        ]
    )
    result = cli.watch(args)
    report = result["watchdog_report"]
    assert report["engine"] == "onnx_defect"
    assert report["engine_requested"] == "onnx_defect"
    assert report["engine_fallback_reason"] is None
    assert report["detectors"] == ["onnx_defect"]
    assert report["backend_model"] == "watchdog_defect.onnx"
    # hands is covered and graduates out of skipped; text stays skipped.
    assert "hands" not in report["skipped_targets"]
    assert "text" in report["skipped_targets"]
    types = {issue["type"] for issue in result["quality_report"]["issues"]}
    assert "malformed_hands" in types
