"""Unit tests for the long-lived torch worker and the backends' warm caches.

These run without ``torch`` / ``realesrgan`` / ``diffusers`` installed (as on CI
and most dev boxes): the worker hosts the existing CLIs, so its protocol,
dispatch and error handling are exercised with the always-available ``cpu`` /
probe paths, and the backends' warm caches are exercised by monkeypatching the
(heavy) model constructor and counting how often it is called.
"""

from __future__ import annotations

import io
import json
import sys
from pathlib import Path

import pytest

# The bridge modules live one directory up (``python/bridge``); make importable.
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import torch_worker  # noqa: E402
from inpaint_backends import sd_inpaint  # noqa: E402
from sr_backends import realesrgan  # noqa: E402


# ---- worker protocol -----------------------------------------------------


def test_ping_roundtrips_the_id() -> None:
    resp = torch_worker.handle_request({"id": 7, "cmd": "ping"})
    assert resp == {"id": 7, "ok": True, "code": 0, "stdout": "", "error": ""}


def test_shutdown_returns_none() -> None:
    # ``None`` is the loop's signal to stop; nothing is written for it.
    assert torch_worker.handle_request({"id": 1, "cmd": "shutdown"}) is None


def test_unknown_cmd_is_a_failed_response_not_a_crash() -> None:
    resp = torch_worker.handle_request({"id": "x", "cmd": "nope"})
    assert resp["id"] == "x"
    assert resp["ok"] is False
    assert "unknown cmd" in resp["error"]


def test_missing_cmd_is_a_failed_response() -> None:
    resp = torch_worker.handle_request({"id": 2})
    assert resp["ok"] is False


def test_bad_argv_type_is_rejected() -> None:
    resp = torch_worker.handle_request({"id": 3, "cmd": "image_enhance", "argv": "not-a-list"})
    assert resp["ok"] is False
    assert "argv" in resp["error"]


def test_image_enhance_probe_runs_through_the_worker() -> None:
    # The probe path exits before reading the image (and needs no torch), so it
    # verifies the worker really invokes the CLI main() and hands back its JSON
    # stdout + code. ``--image`` is a required arg but the probe never opens it.
    resp = torch_worker.handle_request(
        {"id": 9, "cmd": "image_enhance", "argv": ["--image", "unused.png", "--probe-engines"]}
    )
    assert resp["id"] == 9
    assert resp["ok"] is True
    assert resp["code"] == 0
    report = json.loads(resp["stdout"])
    assert "engines" in report and "cpu" in report["engines"]


def test_detail_repaint_probe_runs_through_the_worker() -> None:
    resp = torch_worker.handle_request(
        {"id": 10, "cmd": "detail_repaint", "argv": ["--probe-engines"]}
    )
    assert resp["ok"] is True
    report = json.loads(resp["stdout"])
    assert "engines" in report


def test_cli_failure_is_captured_not_propagated() -> None:
    # A missing required image makes the enhance CLI exit non-zero and write to
    # stderr; the worker must surface that as ok=false, never raise.
    resp = torch_worker.handle_request(
        {"id": 11, "cmd": "image_enhance", "argv": ["--image", "/no/such/file.png"]}
    )
    assert resp["ok"] is False
    assert resp["code"] != 0
    assert resp["error"]


def test_serve_loop_processes_lines_then_shuts_down() -> None:
    # Two pings then a shutdown: exactly two response lines, in order, and the
    # loop returns cleanly on the shutdown request.
    stdin = io.StringIO(
        json.dumps({"id": 1, "cmd": "ping"})
        + "\n"
        + json.dumps({"id": 2, "cmd": "ping"})
        + "\n"
        + json.dumps({"id": 3, "cmd": "shutdown"})
        + "\n"
        + json.dumps({"id": 4, "cmd": "ping"})  # after shutdown: never serviced
        + "\n"
    )
    stdout = io.StringIO()
    assert torch_worker.serve(stdin, stdout) == 0
    lines = [line for line in stdout.getvalue().splitlines() if line]
    assert [json.loads(line)["id"] for line in lines] == [1, 2]


def test_serve_loop_reports_invalid_json_and_keeps_going() -> None:
    stdin = io.StringIO("not json\n" + json.dumps({"id": 5, "cmd": "ping"}) + "\n")
    stdout = io.StringIO()
    torch_worker.serve(stdin, stdout)
    responses = [json.loads(line) for line in stdout.getvalue().splitlines() if line]
    assert responses[0]["ok"] is False and "invalid request json" in responses[0]["error"]
    assert responses[1]["id"] == 5 and responses[1]["ok"] is True


# ---- backend warm caches -------------------------------------------------


def test_realesrgan_warm_cache_builds_once_per_key(monkeypatch: pytest.MonkeyPatch) -> None:
    realesrgan._WARM_UPSAMPLERS.clear()
    calls: list[tuple[str, int, str, str]] = []

    def fake_construct(weight: str, native_scale: int, device: str, precision: str) -> object:
        calls.append((weight, native_scale, device, precision))
        return object()

    monkeypatch.setattr(realesrgan, "_construct_upsampler", fake_construct)

    first = realesrgan._warm_upsampler("w.pth", 4, "cpu", "fp32")
    second = realesrgan._warm_upsampler("w.pth", 4, "cpu", "fp32")
    # Same key: constructed once, same object handed back both times.
    assert first is second
    assert len(calls) == 1
    # A different key (device) builds a second, distinct upsampler.
    other = realesrgan._warm_upsampler("w.pth", 4, "cuda", "fp16")
    assert other is not first
    assert len(calls) == 2


def test_sd_inpaint_warm_cache_builds_once_per_key(monkeypatch: pytest.MonkeyPatch) -> None:
    sd_inpaint._WARM_PIPELINES.clear()
    calls: list[tuple[str, str, str]] = []

    def fake_construct(weight: str, device: str, precision: str) -> object:
        calls.append((weight, device, precision))
        return object()

    monkeypatch.setattr(sd_inpaint, "_construct_pipeline", fake_construct)

    first = sd_inpaint._warm_pipeline("sd-inpaint", "cpu", "fp32")
    second = sd_inpaint._warm_pipeline("sd-inpaint", "cpu", "fp32")
    assert first is second
    assert len(calls) == 1
    other = sd_inpaint._warm_pipeline("sd-inpaint", "cuda", "fp16")
    assert other is not first
    assert len(calls) == 2
