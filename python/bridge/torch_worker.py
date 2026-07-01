"""Long-lived torch worker that hosts the GPU/torch bridge CLIs so their models stay warm.

This is staged-rollout **step 4** of ``docs/cards/editor-resource-model.md``
("torch long-lived Python worker: Rust spawns and keeps it alive; replaces
per-call subprocess + model reload for realesrgan / sd_inpaint").

The Rust host (``studio/torch_worker.rs``) spawns exactly one of these and keeps
it alive for the life of the desktop process, then talks to it with
newline-delimited JSON: one request object per line on ``stdin``, one response
object per line on ``stdout``. Because the process is long-lived, the torch
backends' process-global warm caches (``sr_backends.realesrgan._WARM_UPSAMPLERS``,
``inpaint_backends.sd_inpaint._WARM_PIPELINES``) survive across requests, so the
~64 MB Real-ESRGAN weight / multi-GB SD-inpaint pipeline is built once per
``(weight, device, precision)`` instead of reloaded on every run — the dominant
latency the old per-call subprocess paid.

Protocol (one JSON object per line, UTF-8):

    request : {"id": <any>, "cmd": "ping"|"image_enhance"|"detail_repaint"|"shutdown",
               "argv": [str, ...]}
    response: {"id": <same>, "ok": bool, "code": int, "stdout": str, "error": str}

``argv`` is the exact argument vector the one-shot CLI would have received, so
the worker is a drop-in host: it invokes the CLI's ``main(argv)`` with
``stdout``/``stderr`` captured and returns the CLI's JSON on ``stdout`` plus its
exit ``code``. All argument parsing, CPU fallback and reporting stay in the CLIs;
the worker adds only warmth (via the backends' caches) and process management.
A request whose ``cmd`` maps to a CLI that fails still yields a well-formed
response (``ok=false`` with the captured ``error``) so one bad run never wedges
the worker — the host keeps sending it more.
"""

from __future__ import annotations

import contextlib
import io
import json
import sys
from typing import Any, Callable

import detail_repaint_cli
import image_enhance_cli

#: ``cmd`` -> the CLI ``main(argv) -> int`` that services it. Both CLIs share the
#: same contract: write a single JSON result line to stdout, a single error line
#: to stderr, and return a POSIX exit code (0 == ok).
_COMMANDS: dict[str, Callable[[list[str]], int]] = {
    "image_enhance": image_enhance_cli.main,
    "detail_repaint": detail_repaint_cli.main,
}


def _run_cli(main: Callable[[list[str]], int], argv: list[str]) -> dict[str, Any]:
    """Invoke a CLI ``main(argv)`` in-process, capturing its stdout/stderr.

    The CLIs write their JSON result to stdout and a single error line to
    stderr; we redirect both so nothing leaks onto the worker's own stdout
    protocol stream. The captured stdout is returned verbatim (the host parses
    it exactly as it parsed the one-shot subprocess output).
    """
    out, err = io.StringIO(), io.StringIO()
    try:
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = int(main(list(argv)))
    except SystemExit as exc:  # argparse errors call sys.exit; treat as a failed run
        code = int(exc.code) if isinstance(exc.code, int) else 1
    except Exception as exc:  # noqa: BLE001 - a crashed run must not kill the worker
        return {"ok": False, "code": 1, "stdout": out.getvalue(), "error": f"{type(exc).__name__}: {exc}"}
    return {
        "ok": code == 0,
        "code": code,
        "stdout": out.getvalue(),
        "error": err.getvalue().strip(),
    }


def handle_request(request: dict[str, Any]) -> dict[str, Any] | None:
    """Service one decoded request, returning the response (``None`` == shut down).

    Never raises: an unknown ``cmd`` or a crashing CLI yields an ``ok=false``
    response rather than propagating, so the host's read loop stays in lock-step
    (one response per request) and a single bad run cannot wedge the worker.
    """
    req_id = request.get("id")
    cmd = request.get("cmd")

    if cmd == "shutdown":
        return None
    if cmd == "ping":
        return {"id": req_id, "ok": True, "code": 0, "stdout": "", "error": ""}

    main = _COMMANDS.get(cmd) if isinstance(cmd, str) else None
    if main is None:
        return {"id": req_id, "ok": False, "code": 1, "stdout": "", "error": f"unknown cmd {cmd!r}"}

    argv = request.get("argv") or []
    if not isinstance(argv, list) or not all(isinstance(a, str) for a in argv):
        return {"id": req_id, "ok": False, "code": 1, "stdout": "", "error": "argv must be a list of strings"}

    result = _run_cli(main, argv)
    result["id"] = req_id
    return result


def serve(stdin: Any = None, stdout: Any = None) -> int:
    """Run the request/response loop until EOF or a ``shutdown`` request.

    Reads one JSON request per line and writes one JSON response per line,
    flushing after each so the host sees results promptly. A line that is not
    valid JSON gets an ``ok=false`` response (with no ``id``) rather than
    crashing the loop. Returns ``0`` on a clean exit.
    """
    stdin = stdin if stdin is not None else sys.stdin
    stdout = stdout if stdout is not None else sys.stdout

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
    return 0


if __name__ == "__main__":
    raise SystemExit(serve())
