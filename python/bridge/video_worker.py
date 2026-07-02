"""Long-lived video worker that keeps a decoder open so seeks stay warm.

This is staged-rollout **step 5** of ``docs/cards/editor-resource-model.md``
("Video media engine … decode / playback threads + frame cache; foundation for
the manual clip editor"). It is the *first backend* behind the Rust media
engine's decoder seam: a persistent Python process that hosts PyAV so the ffmpeg
container/stream is opened **once per file** and reused across every probe and
frame request, instead of the old per-call ``video_probe_cli.py`` subprocess
that reopened (and re-demuxed from the start) on every poster. A native-Rust
ffmpeg backend can replace this later behind the same seam without touching the
Rust engine above it.

The Rust host (``studio/video_worker.rs``) spawns exactly one of these and keeps
it alive for the life of the desktop process, then talks to it with
newline-delimited JSON — one request object per line on ``stdin``, one response
object per line on ``stdout``:

    request : {"id": <any>, "cmd": "ping"|"probe"|"frame"|"close"|"shutdown",
               "args": {...}}
    response: {"id": <same>, "ok": bool, "code": int, "stdout": str, "error": str}

``stdout`` carries the result payload as a JSON string (exactly the shape the
one-shot ``video_probe_cli.py`` printed), so the Rust host decodes a worker
response identically to the torch worker's. A request that fails yields a
well-formed ``ok=false`` response with the message in ``error`` rather than
crashing the loop, so one bad file never wedges the worker.

PyAV (``av``) is imported lazily inside the decoder factory so this module — and
its protocol loop — stays importable and unit-testable on a box where ``av`` is
not installed (the CI ``python bridge`` lane installs neither ``av`` nor
``onnx``). Tests swap :data:`_DECODER_FACTORY` for a fake decoder to exercise the
protocol and the warm-open cache without ffmpeg.
"""

from __future__ import annotations

import json
import sys
from typing import Any, Callable

import video_probe_cli

#: path -> the open decoder serving it. Process-global so it survives across
#: requests: this is the whole point of the long-lived worker (open once, seek
#: many). Bounded implicitly by how many distinct clips a session touches; a
#: ``close`` request (sent by the host when a clip leaves the editor) frees one.
_OPEN: dict[str, "Decoder"] = {}


class Decoder:
    """A warm PyAV decoder for one file: the container + its video stream.

    Holds the open container so a seek reuses it instead of reopening the file.
    Construction is lazy (:func:`_open_decoder`) so importing this module never
    needs ``av``; only an actual decode does.
    """

    def __init__(self, container: Any, stream: Any) -> None:
        self._container = container
        self._stream = stream

    def meta(self) -> dict[str, Any]:
        """Metadata for the info row: resolution, duration, fps, codec."""
        import av  # lazy; only reached once a real decode is requested

        codec_ctx = self._stream.codec_context
        duration_sec: float | None = None
        if self._container.duration:
            duration_sec = float(self._container.duration) / float(av.time_base)
        elif self._stream.duration and self._stream.time_base:
            duration_sec = float(self._stream.duration) * float(self._stream.time_base)
        return {
            "width": int(codec_ctx.width or 0),
            "height": int(codec_ctx.height or 0),
            "duration_sec": duration_sec,
            "fps": video_probe_cli.stream_fps(self._stream),
            "codec": codec_ctx.name,
        }

    def frame_at(self, timestamp: float, out_path: str) -> float:
        """Seek to ``timestamp`` (clamped) and write that frame to ``out_path``.

        Returns the clamped timestamp actually decoded. Reuses the open
        container: ``seek`` lands on the keyframe at/just-before the time and the
        following decode yields a complete frame.
        """
        meta = self.meta()
        ts = video_probe_cli.clamp_timestamp(timestamp, meta.get("duration_sec"))
        video_probe_cli._extract_poster(self._container, self._stream, ts, out_path)
        return ts

    def close(self) -> None:
        try:
            self._container.close()
        except Exception:  # noqa: BLE001 - closing must never raise into the loop
            pass


def _open_decoder(path: str) -> Decoder:
    """Open ``path`` with PyAV and wrap its first video stream (default factory)."""
    import av  # lazy: PyAV bundles ffmpeg and is only needed for a real open

    container = av.open(path)
    stream = next((s for s in container.streams if s.type == "video"), None)
    if stream is None:
        container.close()
        raise ValueError("no video stream found")
    return Decoder(container, stream)


#: Indirection so tests can inject a fake decoder (no ``av`` needed). Production
#: always uses :func:`_open_decoder`.
_DECODER_FACTORY: Callable[[str], Decoder] = _open_decoder


def _decoder_for(path: str) -> Decoder:
    """Get the warm decoder for ``path``, opening (and caching) it on first use."""
    decoder = _OPEN.get(path)
    if decoder is None:
        decoder = _DECODER_FACTORY(path)
        _OPEN[path] = decoder
    return decoder


def _drop(path: str) -> None:
    """Close and forget the decoder for ``path`` if one is open."""
    decoder = _OPEN.pop(path, None)
    if decoder is not None:
        decoder.close()


def _assemble_frames(
    frames: list[str], out_path: str, fps: float, codec: str
) -> dict[str, Any]:
    """Encode ``frames`` (image paths, in order) into ``out_path`` via PyAV.

    Every frame is normalised to the first frame's size (rounded down to even
    dimensions, as yuv420p requires) so a mixed-size sequence still encodes.
    Returns the payload for the ``assemble`` response.
    """
    import av  # lazy: PyAV bundles ffmpeg and is only needed for a real encode
    from fractions import Fraction

    from PIL import Image

    rate = Fraction(fps).limit_denominator(1001)
    container = av.open(out_path, mode="w")
    try:
        stream = container.add_stream(codec, rate=rate)
        stream.pix_fmt = "yuv420p"
        width = height = 0
        for path in frames:
            with Image.open(path) as img:
                rgb = img.convert("RGB")
                if width == 0:
                    width = max(2, rgb.width - (rgb.width % 2))
                    height = max(2, rgb.height - (rgb.height % 2))
                    stream.width = width
                    stream.height = height
                if (rgb.width, rgb.height) != (width, height):
                    rgb = rgb.resize((width, height))
                frame = av.VideoFrame.from_image(rgb)
                for packet in stream.encode(frame):
                    container.mux(packet)
        for packet in stream.encode():
            container.mux(packet)
    finally:
        container.close()
    return {
        "video_path": out_path,
        "frame_count": len(frames),
        "width": width,
        "height": height,
        "fps": float(rate),
        "duration_sec": len(frames) / float(rate) if rate else None,
        "codec": codec,
    }


def _do_assemble(args: dict[str, Any]) -> dict[str, Any]:
    """Validate an ``assemble`` request and encode its frame list to a video."""
    frames = args.get("frames")
    if (
        not isinstance(frames, list)
        or not frames
        or not all(isinstance(f, str) and f for f in frames)
    ):
        raise ValueError("args.frames must be a non-empty list of image paths")
    out_path = args.get("out")
    if not isinstance(out_path, str) or not out_path:
        raise ValueError("args.out must be a non-empty string")
    fps = args.get("fps", 24.0)
    try:
        fps = float(fps)
    except (TypeError, ValueError):
        raise ValueError("args.fps must be a number") from None
    if fps <= 0:
        raise ValueError("args.fps must be positive")
    codec = args.get("codec", "libx264")
    if not isinstance(codec, str) or not codec:
        raise ValueError("args.codec must be a non-empty string")
    return _ASSEMBLER(frames, out_path, fps, codec)


#: Indirection so tests can inject a fake encoder (no ``av``/PIL needed).
#: Production always uses :func:`_assemble_frames`.
_ASSEMBLER: Callable[[list[str], str, float, str], dict[str, Any]] = _assemble_frames


def _require_video(args: dict[str, Any]) -> str:
    video = args.get("video")
    if not isinstance(video, str) or not video:
        raise ValueError("args.video must be a non-empty string")
    return video


def _do_probe(args: dict[str, Any]) -> dict[str, Any]:
    return _decoder_for(_require_video(args)).meta()


def _do_frame(args: dict[str, Any]) -> dict[str, Any]:
    video = _require_video(args)
    poster_out = args.get("poster_out")
    if not isinstance(poster_out, str) or not poster_out:
        raise ValueError("args.poster_out must be a non-empty string")
    timestamp = args.get("timestamp", 0.0)
    try:
        timestamp = float(timestamp)
    except (TypeError, ValueError):
        raise ValueError("args.timestamp must be a number") from None
    decoder = _decoder_for(video)
    ts = decoder.frame_at(timestamp, poster_out)
    meta = decoder.meta()
    return {
        "poster_path": poster_out,
        "frame_timestamp_sec": ts,
        "width": meta["width"],
        "height": meta["height"],
    }


#: ``cmd`` -> handler returning the JSON-able result payload placed in ``stdout``.
_COMMANDS: dict[str, Callable[[dict[str, Any]], dict[str, Any]]] = {
    "probe": _do_probe,
    "frame": _do_frame,
    "assemble": _do_assemble,
}


def handle_request(request: dict[str, Any]) -> dict[str, Any] | None:
    """Service one decoded request, returning the response (``None`` == shut down).

    Never raises: an unknown ``cmd`` or a decode error yields an ``ok=false``
    response so the host's read loop stays in lock-step (one response per
    request) and a single bad file cannot wedge the worker.
    """
    req_id = request.get("id")
    cmd = request.get("cmd")
    args = request.get("args") or {}
    if not isinstance(args, dict):
        return {"id": req_id, "ok": False, "code": 1, "stdout": "", "error": "args must be an object"}

    if cmd == "shutdown":
        return None
    if cmd == "ping":
        return {"id": req_id, "ok": True, "code": 0, "stdout": "", "error": ""}
    if cmd == "close":
        try:
            _drop(_require_video(args))
        except ValueError as exc:
            return {"id": req_id, "ok": False, "code": 1, "stdout": "", "error": str(exc)}
        return {"id": req_id, "ok": True, "code": 0, "stdout": "", "error": ""}

    handler = _COMMANDS.get(cmd) if isinstance(cmd, str) else None
    if handler is None:
        return {"id": req_id, "ok": False, "code": 1, "stdout": "", "error": f"unknown cmd {cmd!r}"}

    try:
        payload = handler(args)
    except ImportError:
        return {"id": req_id, "ok": False, "code": 3, "stdout": "", "error": "PyAV (av) is required for video decoding; install with `pip install av`"}
    except Exception as exc:  # noqa: BLE001 - a bad file must not kill the worker
        return {"id": req_id, "ok": False, "code": 1, "stdout": "", "error": f"{type(exc).__name__}: {exc}"}
    return {
        "id": req_id,
        "ok": True,
        "code": 0,
        "stdout": json.dumps(payload, ensure_ascii=False),
        "error": "",
    }


def serve(stdin: Any = None, stdout: Any = None) -> int:
    """Run the request/response loop until EOF or a ``shutdown`` request.

    Reads one JSON request per line and writes one JSON response per line,
    flushing after each so the host sees results promptly. A line that is not
    valid JSON gets an ``ok=false`` response rather than crashing the loop. On
    exit every still-open decoder is closed. Returns ``0`` on a clean exit.
    """
    stdin = stdin if stdin is not None else sys.stdin
    stdout = stdout if stdout is not None else sys.stdout

    try:
        for line in stdin:
            line = line.strip()
            if not line:
                continue
            try:
                request = json.loads(line)
            except json.JSONDecodeError as exc:
                response: dict[str, Any] | None = {
                    "id": None,
                    "ok": False,
                    "code": 1,
                    "stdout": "",
                    "error": f"invalid request json: {exc}",
                }
            else:
                response = handle_request(request)
                if response is None:  # shutdown
                    break
            stdout.write(json.dumps(response, ensure_ascii=False) + "\n")
            stdout.flush()
    finally:
        for path in list(_OPEN):
            _drop(path)
    return 0


if __name__ == "__main__":
    raise SystemExit(serve())
