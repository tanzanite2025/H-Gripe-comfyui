"""Unit tests for the machine device probe (``sr_backends.device_probe``).

These run with neither ``torch`` nor ``onnxruntime`` necessarily present (as on
CI and most dev boxes): the probe must report a safe, fully-shaped report rather
than crash, so the capability report always has the keys the UI reads. When a
dep *is* importable the corresponding section is filled in, but availability is
never asserted (CI has no GPU).
"""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path

# ``sr_backends`` / ``device_probe_cli`` live one directory up (``python/bridge``).
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import device_probe_cli  # noqa: E402
from sr_backends import device_probe  # noqa: E402


def test_device_probe_is_fully_shaped_without_gpu() -> None:
    report = device_probe()
    # The UI always reads these keys, so they must exist regardless of deps.
    assert set(report) >= {"cuda_available", "devices", "torch", "onnxruntime"}
    assert isinstance(report["cuda_available"], bool)
    assert isinstance(report["devices"], list)
    assert isinstance(report["torch"], dict)
    assert isinstance(report["onnxruntime"], dict)
    assert "installed" in report["torch"]
    assert "installed" in report["onnxruntime"]
    assert isinstance(report["onnxruntime"].get("providers", []), list)


def test_device_probe_torch_section_matches_dep_presence() -> None:
    report = device_probe()
    torch_present = importlib.util.find_spec("torch") is not None
    assert report["torch"]["installed"] is torch_present
    # No CUDA on CI -> no devices reported; a torch-less box can't be cuda either.
    if not torch_present:
        assert report["cuda_available"] is False
        assert report["devices"] == []


def test_device_probe_onnxruntime_section_matches_dep_presence() -> None:
    report = device_probe()
    ort_present = importlib.util.find_spec("onnxruntime") is not None
    assert report["onnxruntime"]["installed"] is ort_present
    if ort_present:
        # When present we at least get the always-available CPU provider.
        assert "CPUExecutionProvider" in report["onnxruntime"]["providers"]


def test_cli_prints_same_report_as_function() -> None:
    captured: list[str] = []
    original = sys.stdout.write
    sys.stdout.write = captured.append  # type: ignore[assignment]
    try:
        rc = device_probe_cli.main([])
    finally:
        sys.stdout.write = original  # type: ignore[assignment]
    assert rc == 0
    printed = json.loads("".join(captured))
    assert set(printed) == set(device_probe())
