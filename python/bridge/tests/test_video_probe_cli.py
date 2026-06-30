"""Unit tests for the video probe CLI (``video_probe_cli.py``).

These exercise the pure-logic helpers (timestamp clamping, fps resolution and
arg parsing) which must work even where PyAV (``av``) is not installed -- the
import is lazy on purpose. The decode/poster path needs ``av`` and is covered by
``importorskip`` so CI without that optional dep stays green.
"""

from __future__ import annotations

import sys
from pathlib import Path

# The CLI lives one directory up (``python/bridge``); make it importable.
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import video_probe_cli as cli  # noqa: E402


class _FakeStream:
    def __init__(self, average_rate=None, base_rate=None, guessed_rate=None):
        self.average_rate = average_rate
        self.base_rate = base_rate
        self.guessed_rate = guessed_rate


def test_clamp_timestamp_negative_and_missing_seek_first_frame():
    assert cli.clamp_timestamp(None, 10.0) == 0.0
    assert cli.clamp_timestamp(-5.0, 10.0) == 0.0
    assert cli.clamp_timestamp(0.0, 10.0) == 0.0


def test_clamp_timestamp_within_clip_is_passthrough():
    assert cli.clamp_timestamp(3.5, 10.0) == 3.5


def test_clamp_timestamp_past_end_is_pulled_back():
    # At/after the duration we pull back so a frame is still decodable.
    assert cli.clamp_timestamp(10.0, 10.0) == 9.9
    assert cli.clamp_timestamp(99.0, 10.0) == 9.9


def test_clamp_timestamp_unknown_duration_keeps_request():
    assert cli.clamp_timestamp(42.0, None) == 42.0


def test_stream_fps_prefers_average_then_falls_back():
    assert cli.stream_fps(_FakeStream(average_rate=30)) == 30.0
    assert cli.stream_fps(_FakeStream(base_rate=24)) == 24.0
    assert cli.stream_fps(_FakeStream(guessed_rate=25)) == 25.0
    assert cli.stream_fps(_FakeStream()) is None
    # A zero/garbage rate is treated as unknown, not 0 fps.
    assert cli.stream_fps(_FakeStream(average_rate=0)) is None


def test_build_parser_defaults_and_required_video():
    args = cli.build_parser().parse_args(["--video", "/tmp/x.mp4"])
    assert args.video == "/tmp/x.mp4"
    assert args.timestamp == 0.0
    assert args.poster_out is None
    assert args.probe_only is False
