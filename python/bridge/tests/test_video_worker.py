"""Unit tests for the long-lived video worker (``video_worker.py``).

These exercise the JSON request/response protocol and the warm-open decoder
cache **without PyAV**: a fake decoder is injected via ``_DECODER_FACTORY`` and
``_extract_poster`` is monkeypatched, so the tests run on the default CI bridge
lane (which installs neither ``av`` nor ``onnx``). What they pin down:

* ping / shutdown / unknown-cmd / bad-args protocol shapes,
* a file is opened **once** and reused across probe + frame requests (the warm
  cache — the reason the worker is long-lived), and ``close`` frees it,
* a decoder that raises yields ``ok=false`` rather than crashing the loop,
* ``serve()`` survives a garbled JSON line and closes decoders on exit.
"""

from __future__ import annotations

import io
import json
import sys
from pathlib import Path

import pytest

# The bridge modules live one directory up (``python/bridge``); make importable.
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import video_worker  # noqa: E402


class _FakeDecoder:
    """A decoder that records calls instead of touching ffmpeg."""

    def __init__(self, path: str) -> None:
        self.path = path
        self.frames: list[tuple[float, str]] = []
        self.closed = False

    def meta(self) -> dict[str, object]:
        return {"width": 640, "height": 480, "duration_sec": 12.0, "fps": 24.0, "codec": "h264"}

    def frame_at(self, timestamp: float, out_path: str) -> float:
        ts = min(max(timestamp, 0.0), 11.9)
        self.frames.append((ts, out_path))
        return ts

    def close(self) -> None:
        self.closed = True


@pytest.fixture(autouse=True)
def _reset_worker(monkeypatch):
    """Give every test a clean decoder cache + a counting fake factory."""
    video_worker._OPEN.clear()
    opened: list[_FakeDecoder] = []

    def factory(path: str) -> _FakeDecoder:
        decoder = _FakeDecoder(path)
        opened.append(decoder)
        return decoder

    monkeypatch.setattr(video_worker, "_DECODER_FACTORY", factory)
    yield opened
    video_worker._OPEN.clear()


def test_ping_is_ok():
    resp = video_worker.handle_request({"id": 1, "cmd": "ping"})
    assert resp == {"id": 1, "ok": True, "code": 0, "stdout": "", "error": ""}


def test_shutdown_returns_none():
    assert video_worker.handle_request({"id": 2, "cmd": "shutdown"}) is None


def test_unknown_cmd_is_not_ok():
    resp = video_worker.handle_request({"id": 3, "cmd": "explode"})
    assert resp["ok"] is False
    assert "unknown cmd" in resp["error"]


def test_probe_returns_metadata_json():
    resp = video_worker.handle_request({"id": 4, "cmd": "probe", "args": {"video": "clip.mp4"}})
    assert resp["ok"] is True
    meta = json.loads(resp["stdout"])
    assert meta["width"] == 640
    assert meta["codec"] == "h264"


def test_frame_reports_clamped_timestamp_and_poster():
    resp = video_worker.handle_request(
        {"id": 5, "cmd": "frame", "args": {"video": "clip.mp4", "timestamp": 99.0, "poster_out": "/tmp/p.png"}}
    )
    assert resp["ok"] is True
    payload = json.loads(resp["stdout"])
    assert payload["poster_path"] == "/tmp/p.png"
    assert payload["frame_timestamp_sec"] == pytest.approx(11.9)
    assert payload["width"] == 640


def test_missing_video_arg_is_not_ok():
    resp = video_worker.handle_request({"id": 6, "cmd": "probe", "args": {}})
    assert resp["ok"] is False
    assert "video" in resp["error"]


def test_bad_args_type_is_not_ok():
    resp = video_worker.handle_request({"id": 7, "cmd": "probe", "args": "nope"})
    assert resp["ok"] is False
    assert "args must be an object" in resp["error"]


def test_frame_requires_poster_out():
    resp = video_worker.handle_request({"id": 8, "cmd": "frame", "args": {"video": "clip.mp4"}})
    assert resp["ok"] is False
    assert "poster_out" in resp["error"]


def test_decoder_is_opened_once_and_reused(_reset_worker):
    opened = _reset_worker
    video_worker.handle_request({"id": 1, "cmd": "probe", "args": {"video": "clip.mp4"}})
    video_worker.handle_request(
        {"id": 2, "cmd": "frame", "args": {"video": "clip.mp4", "timestamp": 1.0, "poster_out": "/tmp/a.png"}}
    )
    video_worker.handle_request(
        {"id": 3, "cmd": "frame", "args": {"video": "clip.mp4", "timestamp": 2.0, "poster_out": "/tmp/b.png"}}
    )
    # One open for three requests against the same file: the warm cache works.
    assert len(opened) == 1
    assert len(opened[0].frames) == 2


def test_distinct_files_open_distinct_decoders(_reset_worker):
    opened = _reset_worker
    video_worker.handle_request({"id": 1, "cmd": "probe", "args": {"video": "a.mp4"}})
    video_worker.handle_request({"id": 2, "cmd": "probe", "args": {"video": "b.mp4"}})
    assert {d.path for d in opened} == {"a.mp4", "b.mp4"}


def test_close_frees_the_decoder(_reset_worker):
    opened = _reset_worker
    video_worker.handle_request({"id": 1, "cmd": "probe", "args": {"video": "clip.mp4"}})
    resp = video_worker.handle_request({"id": 2, "cmd": "close", "args": {"video": "clip.mp4"}})
    assert resp["ok"] is True
    assert opened[0].closed is True
    assert "clip.mp4" not in video_worker._OPEN
    # A later request reopens (a second decoder), proving the cache was cleared.
    video_worker.handle_request({"id": 3, "cmd": "probe", "args": {"video": "clip.mp4"}})
    assert len(opened) == 2


def test_decoder_error_yields_not_ok(monkeypatch):
    def boom(path: str):
        raise RuntimeError("corrupt moov atom")

    monkeypatch.setattr(video_worker, "_DECODER_FACTORY", boom)
    resp = video_worker.handle_request({"id": 1, "cmd": "probe", "args": {"video": "clip.mp4"}})
    assert resp["ok"] is False
    assert resp["code"] == 1
    assert "corrupt moov atom" in resp["error"]


def test_assemble_encodes_via_injected_assembler(monkeypatch):
    calls: list[tuple[list[str], str, float, str]] = []

    def fake_assembler(frames, out_path, fps, codec):
        calls.append((frames, out_path, fps, codec))
        return {"video_path": out_path, "frame_count": len(frames)}

    monkeypatch.setattr(video_worker, "_ASSEMBLER", fake_assembler)
    resp = video_worker.handle_request(
        {
            "id": 1,
            "cmd": "assemble",
            "args": {"frames": ["a.png", "b.png"], "out": "/tmp/out.mp4", "fps": 12, "codec": "libx264"},
        }
    )
    assert resp["ok"] is True
    payload = json.loads(resp["stdout"])
    assert payload["video_path"] == "/tmp/out.mp4"
    assert payload["frame_count"] == 2
    assert calls == [(["a.png", "b.png"], "/tmp/out.mp4", 12.0, "libx264")]


def test_assemble_defaults_fps_and_codec(monkeypatch):
    seen: dict[str, object] = {}

    def fake_assembler(frames, out_path, fps, codec):
        seen.update(fps=fps, codec=codec)
        return {"video_path": out_path, "frame_count": len(frames)}

    monkeypatch.setattr(video_worker, "_ASSEMBLER", fake_assembler)
    resp = video_worker.handle_request(
        {"id": 2, "cmd": "assemble", "args": {"frames": ["a.png"], "out": "/tmp/out.mp4"}}
    )
    assert resp["ok"] is True
    assert seen == {"fps": 24.0, "codec": "libx264"}


@pytest.mark.parametrize(
    ("args", "needle"),
    [
        ({"out": "/tmp/out.mp4"}, "frames"),
        ({"frames": [], "out": "/tmp/out.mp4"}, "frames"),
        ({"frames": ["a.png", 7], "out": "/tmp/out.mp4"}, "frames"),
        ({"frames": ["a.png"]}, "out"),
        ({"frames": ["a.png"], "out": "/tmp/out.mp4", "fps": "fast"}, "fps"),
        ({"frames": ["a.png"], "out": "/tmp/out.mp4", "fps": 0}, "fps"),
        ({"frames": ["a.png"], "out": "/tmp/out.mp4", "codec": ""}, "codec"),
    ],
)
def test_assemble_rejects_bad_args(args, needle):
    resp = video_worker.handle_request({"id": 3, "cmd": "assemble", "args": args})
    assert resp["ok"] is False
    assert needle in resp["error"]


def test_trim_cuts_via_injected_trimmer(monkeypatch):
    calls: list[tuple[str, str, float, float | None, str]] = []

    def fake_trimmer(video, out_path, start_sec, end_sec, codec):
        calls.append((video, out_path, start_sec, end_sec, codec))
        return {"video_path": out_path, "frame_count": 5}

    monkeypatch.setattr(video_worker, "_TRIMMER", fake_trimmer)
    resp = video_worker.handle_request(
        {
            "id": 1,
            "cmd": "trim",
            "args": {"video": "clip.mp4", "out": "/tmp/cut.mp4", "start_sec": 1.5, "end_sec": 3, "codec": "libx264"},
        }
    )
    assert resp["ok"] is True
    payload = json.loads(resp["stdout"])
    assert payload["video_path"] == "/tmp/cut.mp4"
    assert calls == [("clip.mp4", "/tmp/cut.mp4", 1.5, 3.0, "libx264")]


def test_trim_defaults_start_end_and_codec(monkeypatch):
    seen: dict[str, object] = {}

    def fake_trimmer(video, out_path, start_sec, end_sec, codec):
        seen.update(start_sec=start_sec, end_sec=end_sec, codec=codec)
        return {"video_path": out_path}

    monkeypatch.setattr(video_worker, "_TRIMMER", fake_trimmer)
    resp = video_worker.handle_request(
        {"id": 2, "cmd": "trim", "args": {"video": "clip.mp4", "out": "/tmp/cut.mp4"}}
    )
    assert resp["ok"] is True
    assert seen == {"start_sec": 0.0, "end_sec": None, "codec": "libx264"}


@pytest.mark.parametrize(
    ("args", "needle"),
    [
        ({"out": "/tmp/cut.mp4"}, "video"),
        ({"video": "clip.mp4"}, "out"),
        ({"video": "clip.mp4", "out": "/tmp/cut.mp4", "start_sec": "soon"}, "start_sec"),
        ({"video": "clip.mp4", "out": "/tmp/cut.mp4", "start_sec": -1}, "start_sec"),
        ({"video": "clip.mp4", "out": "/tmp/cut.mp4", "end_sec": "later"}, "end_sec"),
        ({"video": "clip.mp4", "out": "/tmp/cut.mp4", "start_sec": 2, "end_sec": 2}, "end_sec"),
        ({"video": "clip.mp4", "out": "/tmp/cut.mp4", "codec": ""}, "codec"),
    ],
)
def test_trim_rejects_bad_args(args, needle):
    resp = video_worker.handle_request({"id": 3, "cmd": "trim", "args": args})
    assert resp["ok"] is False
    assert needle in resp["error"]


def test_serve_survives_bad_json_and_closes_on_exit(_reset_worker):
    opened = _reset_worker
    script = "\n".join(
        [
            "not json at all",
            json.dumps({"id": 1, "cmd": "probe", "args": {"video": "clip.mp4"}}),
            json.dumps({"cmd": "shutdown"}),
        ]
    )
    out = io.StringIO()
    assert video_worker.serve(stdin=io.StringIO(script), stdout=out) == 0

    lines = [json.loads(line) for line in out.getvalue().splitlines()]
    assert lines[0]["ok"] is False and "invalid request json" in lines[0]["error"]
    assert lines[1]["ok"] is True
    # shutdown ended the loop and the finally-block closed the open decoder.
    assert opened[0].closed is True
