"""Video probe + poster-frame extraction for the H-Gripe video media card.

This is the thin backend behind the desktop ``video_probe`` Tauri command -- the
ingest helper for the **generic video card**. When a video file is dropped onto
the canvas the card needs two cheap things up front, neither of which the Rust
side can do (it has no video decoder):

* **metadata** -- duration, pixel resolution, frame rate and codec, shown in the
  card's info row, and
* a **poster frame** -- a single decoded frame (the first frame, or a chosen
  timestamp) written to a PNG so the card can show a still using the *existing*
  image-thumbnail pipeline (``generate_thumbnail`` runs on the PNG).

Frame decoding goes through **PyAV** (``av``), which wraps the ffmpeg libraries
and ships them in its wheels, so there is no separate system ffmpeg install. The
import is deliberately *lazy* (inside :func:`probe`) so this module imports --
and its pure-logic helpers stay unit-testable -- on a box where ``av`` is not
installed; only an actual probe needs it.

The emitted JSON is ``{"ok", "width", "height", "duration_sec", "fps", "codec",
"poster_path"?, "frame_timestamp_sec"?}``. On failure the process exits non-zero
with a single message on stderr (``3`` when ``av`` is missing, ``1`` otherwise).
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Probe a video and extract a poster frame.")
    parser.add_argument("--video", required=True, help="absolute path to the video file")
    parser.add_argument("--poster-out", default=None, help="write the poster frame PNG here")
    parser.add_argument(
        "--timestamp",
        type=float,
        default=0.0,
        help="poster frame time in seconds (clamped into the clip)",
    )
    parser.add_argument(
        "--probe-only",
        action="store_true",
        help="report metadata only; do not decode/write a poster frame",
    )
    return parser


def clamp_timestamp(ts: float | None, duration: float | None) -> float:
    """Clamp a requested poster time into ``[0, duration)``.

    Negative / missing values seek the first frame; a time at or past the end is
    pulled back so a frame is still decodable rather than running off the clip.
    """
    if ts is None or ts <= 0 or not (ts == ts):  # NaN-safe
        return 0.0
    if duration is not None and duration > 0 and ts >= duration:
        return max(0.0, duration - 0.1)
    return ts


def stream_fps(stream: Any) -> float | None:
    """Best-effort frame rate for a PyAV video stream.

    ``average_rate`` is preferred; ``base_rate`` / ``guessed_rate`` are fallbacks
    for containers that do not carry an average. Returns ``None`` when unknown
    rather than guessing.
    """
    for attr in ("average_rate", "base_rate", "guessed_rate"):
        rate = getattr(stream, attr, None)
        if rate:
            value = float(rate)
            if value > 0:
                return value
    return None


def _extract_poster(container: Any, stream: Any, ts: float, out_path: str) -> None:
    if ts > 0 and stream.time_base:
        offset = int(ts / float(stream.time_base))
        # backward=True lands on the keyframe at/just-before the time so the
        # following decode yields a complete frame.
        container.seek(offset, stream=stream, any_frame=False, backward=True)
    for frame in container.decode(stream):
        image = frame.to_image()
        out = Path(out_path)
        out.parent.mkdir(parents=True, exist_ok=True)
        image.save(out)
        return
    raise ValueError("could not decode a video frame")


def probe(args: argparse.Namespace) -> dict[str, Any]:
    import av  # lazy: PyAV bundles ffmpeg and is only needed for an actual probe

    with av.open(args.video) as container:
        stream = next((s for s in container.streams if s.type == "video"), None)
        if stream is None:
            raise ValueError("no video stream found")
        codec_ctx = stream.codec_context
        width = int(codec_ctx.width or 0)
        height = int(codec_ctx.height or 0)

        duration_sec: float | None = None
        if container.duration:
            duration_sec = float(container.duration) / float(av.time_base)
        elif stream.duration and stream.time_base:
            duration_sec = float(stream.duration) * float(stream.time_base)

        report: dict[str, Any] = {
            "ok": True,
            "width": width,
            "height": height,
            "duration_sec": duration_sec,
            "fps": stream_fps(stream),
            "codec": codec_ctx.name,
        }

        if not args.probe_only and args.poster_out:
            ts = clamp_timestamp(args.timestamp, duration_sec)
            _extract_poster(container, stream, ts, args.poster_out)
            report["poster_path"] = args.poster_out
            report["frame_timestamp_sec"] = ts
        return report


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        report = probe(args)
    except ImportError:
        sys.stderr.write("PyAV (av) is required for video probing; install with `pip install av`\n")
        return 3
    except Exception as exc:  # noqa: BLE001 - surface a single message to the caller
        sys.stderr.write(f"{exc}\n")
        return 1
    sys.stdout.write(json.dumps(report, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
