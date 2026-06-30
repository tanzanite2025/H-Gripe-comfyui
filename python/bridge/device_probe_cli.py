"""One-shot machine compute-capability probe for the capability report.

The per-card ``*_cli.py --probe-engines`` calls deliberately stay light (they use
``importlib.util.find_spec`` so they never import ``onnxruntime`` / ``torch`` just
to report which engines *could* run). This CLI is the complementary *device*
probe: it reports *what accelerator those engines would run on* (CUDA device
names / VRAM, and the ONNX Runtime execution providers available here) so the UI
can warn that a GPU engine will fall back to CPU on a box with no CUDA device.

It is machine-global, so the Tauri ``probe_engines`` aggregator runs it **once**
(rather than per card) and merges the result into the cross-card report. It
prints the :func:`sr_backends.device_probe` JSON to stdout and exits ``0``;
inspecting the optional deps is intentional and only happens on this on-demand
call, never on a normal node run.
"""

from __future__ import annotations

import json
import sys

from sr_backends import device_probe


def main(argv: list[str] | None = None) -> int:
    sys.stdout.write(json.dumps(device_probe(), ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
