"""Unit tests for the super-resolution engine seam (``sr_backends``).

These run without ``torch`` / ``realesrgan`` installed (as on CI and most dev
boxes): the Real-ESRGAN backend must report itself *unavailable* rather than
crash, and asking it to run anyway must raise ``BackendUnavailable`` so the
caller can fall back to the CPU path.
"""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

# ``sr_backends`` lives one directory up (``python/bridge``); make it importable.
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import sr_backends  # noqa: E402
from sr_backends import BackendUnavailable, probe, resolve  # noqa: E402
from sr_backends.realesrgan import RealEsrganBackend  # noqa: E402


def test_onnx_providers_prefers_cuda_when_present() -> None:
    # On a CUDA box ORT exposes the CUDA provider: it is used first, with CPU
    # always last as the universal fallback (mirrors the torch "cuda if
    # available else cpu" auto behaviour).
    assert sr_backends.onnx_providers(
        ["CUDAExecutionProvider", "CPUExecutionProvider"]
    ) == ["CUDAExecutionProvider", "CPUExecutionProvider"]


def test_onnx_providers_cpu_only_box() -> None:
    # CPU-only ORT (the test/dev default): just the CPU provider, never empty.
    assert sr_backends.onnx_providers(["CPUExecutionProvider"]) == ["CPUExecutionProvider"]
    # An unknown accelerator we don't drive is ignored; CPU still backs it.
    assert sr_backends.onnx_providers(["SomeOtherProvider"]) == ["CPUExecutionProvider"]


def test_onnx_providers_device_cpu_pins_cpu_provider() -> None:
    # An explicit cpu pins the CPU provider even on a CUDA box, so an operator
    # can keep the GPU free for another engine.
    assert sr_backends.onnx_providers(
        ["CUDAExecutionProvider", "CPUExecutionProvider"], device="cpu"
    ) == ["CPUExecutionProvider"]


def test_onnx_providers_device_cuda_degrades_without_accelerator() -> None:
    # cuda requested but ORT exposes no accelerator: still just CPU (the session
    # always builds), same as auto.
    assert sr_backends.onnx_providers(["CPUExecutionProvider"], device="cuda") == [
        "CPUExecutionProvider"
    ]
    # cuda requested and present: accelerator first, CPU as the fallback.
    assert sr_backends.onnx_providers(
        ["CUDAExecutionProvider", "CPUExecutionProvider"], device="cuda"
    ) == ["CUDAExecutionProvider", "CPUExecutionProvider"]


def test_provider_device_labels_the_bound_provider() -> None:
    # The label tracks the *first* (bound) provider, so a degraded cuda request
    # that fell back to the CPU provider is reported truthfully as cpu.
    assert sr_backends.provider_device(["CUDAExecutionProvider", "CPUExecutionProvider"]) == "cuda"
    assert sr_backends.provider_device(["CPUExecutionProvider"]) == "cpu"
    assert sr_backends.provider_device([]) == "cpu"


def test_resolve_device_auto_follows_availability() -> None:
    # auto (the default) mirrors the torch backends' long-standing behaviour:
    # cuda when a device is present, else cpu. Blank / None / unknown == auto.
    assert sr_backends.resolve_device("auto", True) == "cuda"
    assert sr_backends.resolve_device("auto", False) == "cpu"
    assert sr_backends.resolve_device(None, True) == "cuda"
    assert sr_backends.resolve_device("", False) == "cpu"
    assert sr_backends.resolve_device("bogus", True) == "cuda"


def test_resolve_device_explicit_cpu_always_cpu() -> None:
    # An explicit cpu never touches the GPU even when one is present.
    assert sr_backends.resolve_device("cpu", True) == "cpu"
    assert sr_backends.resolve_device("CPU", False) == "cpu"


def test_resolve_device_explicit_cuda_degrades_truthfully() -> None:
    # cuda is honoured only when a device exists; on a CPU-only box it degrades
    # to cpu (reported truthfully by the caller) rather than crashing the run.
    assert sr_backends.resolve_device("cuda", True) == "cuda"
    assert sr_backends.resolve_device("cuda", False) == "cpu"


def test_resolve_cpu_and_blank_return_none() -> None:
    assert resolve("cpu") is None
    assert resolve("") is None
    assert resolve(None) is None
    # Case-insensitive.
    assert resolve("CPU") is None


def test_resolve_unknown_engine_returns_none() -> None:
    assert resolve("nope") is None


def test_resolve_realesrgan_returns_backend() -> None:
    backend = resolve("realesrgan")
    assert backend is not None
    assert backend.id == "realesrgan"
    assert backend.native_scale == 4


def test_probe_reports_cpu_available_and_realesrgan_entry() -> None:
    report = probe()
    engines = report["engines"]
    assert engines["cpu"]["available"] is True
    assert "realesrgan" in engines
    # availability is a bool either way; reason is always a non-empty string.
    assert isinstance(engines["realesrgan"]["available"], bool)
    assert engines["realesrgan"]["reason"]
    # The CPU baseline is not GPU-accelerated; the ML engine is.
    assert engines["cpu"]["accelerated"] is False
    assert engines["realesrgan"]["accelerated"] is True
    # Cached-weight inventory: the CPU baseline loads none; the ML engine names
    # its (not-yet-downloaded) weight.
    assert "weight" not in engines["cpu"]
    weight = engines["realesrgan"]["weight"]
    assert weight["path"].endswith("RealESRGAN_x4plus.pth")
    assert isinstance(weight["present"], bool)
    assert "model_cache_dir" in report


def test_model_cache_dir_honours_env(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    monkeypatch.setenv("HGRIPE_MODEL_CACHE", str(tmp_path))
    assert sr_backends.model_cache_dir() == tmp_path


def test_realesrgan_unavailable_without_deps_or_weight() -> None:
    # torch / realesrgan are not installed in the test environment, so the probe
    # must say so (and never import torch just to answer).
    ok, reason = RealEsrganBackend().available()
    assert ok is False
    assert reason  # human-readable explanation


def test_realesrgan_upscale_raises_when_unavailable() -> None:
    pytest.importorskip("PIL")
    from PIL import Image

    img = Image.new("RGB", (8, 8), (120, 60, 30))
    with pytest.raises(BackendUnavailable):
        RealEsrganBackend().upscale(img, 4.0)


def test_realesrgan_weight_path_env_override(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    weight = tmp_path / "custom.pth"
    monkeypatch.setenv("HGRIPE_REALESRGAN_MODEL", str(weight))
    assert RealEsrganBackend().weight_path() == weight
