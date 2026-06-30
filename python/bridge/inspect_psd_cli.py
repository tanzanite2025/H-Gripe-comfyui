"""Headless PSD inspection for the H-Gripe desktop node editor.

This is the thin, ``torch``-free entry point behind the desktop ``inspect_psd``
Tauri command. It opens a PSD *template* and reports whether the file exists,
its canvas size, and a flat list of its layers (name + kind) -- so the node
editor can validate a real PSD on disk *before* a run: that the template path
actually points at a file, and that a configured placeholder layer name truly
exists inside it, instead of only discovering the problem mid-compose.

It deliberately reuses ``_layer_descriptor`` from
``custom_nodes/hgripe_psd_nodes.py`` -- the same helper the ComfyUI PSD Template
node uses to enumerate layers -- so layer naming / kind detection stays a single
source of truth. Those helpers import cleanly without ``torch`` (heavy imports
are deferred to call time), so this CLI runs with just ``Pillow`` + the vendored
``psd_tools`` + ``attrs``.

Input is passed as CLI flags; a single JSON object is printed to stdout on
success. A missing template path is reported as ``{"exists": false}`` (status
``succeeded``) rather than an error, so the caller can cleanly distinguish "no
file on disk" from a crash. On a genuine failure the process exits non-zero with
a message on stderr.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any

# Resolve the repo root (this file lives at <root>/python/bridge/) and make both
# the root (for ``custom_nodes``) and the vendored ``third_party`` importable,
# exactly like ``compose_psd_cli.py`` and the ComfyUI nodes do.
_ROOT_DIR = Path(__file__).resolve().parents[2]
for _candidate in (_ROOT_DIR, _ROOT_DIR / "third_party"):
    if _candidate.is_dir() and str(_candidate) not in sys.path:
        sys.path.insert(0, str(_candidate))

# ``_layer_descriptor`` imports cleanly without torch and is the same helper the
# ComfyUI PSD Template node uses, so reusing it keeps layer enumeration a single
# source of truth with the nodes.
from custom_nodes.hgripe_psd_nodes import _layer_descriptor  # noqa: E402

# Refuse to open a PSD whose declared canvas is larger than this many pixels
# (decompression-bomb guard, aligned with ``analyze_psd_cli`` / ``compose_psd_cli``).
# 0 disables the check. Tunable via ``--max-decode-pixels``.
_DEFAULT_MAX_DECODE_PIXELS = 96_000_000


def _flatten_layers(descriptors: list[dict[str, Any]]) -> list[dict[str, str]]:
    """Flatten the nested ``_layer_descriptor`` tree into ``{name, kind}`` rows,
    including group children, so a placeholder name nested in any group is
    discoverable."""
    rows: list[dict[str, str]] = []
    for descriptor in descriptors:
        rows.append(
            {
                "name": str(descriptor.get("name", "")),
                "kind": str(descriptor.get("kind", "")),
            }
        )
        children = descriptor.get("children")
        if isinstance(children, list):
            rows.extend(_flatten_layers(children))
    return rows


def _parse_names(raw: str) -> list[str]:
    text = (raw or "").strip()
    if not text:
        return []
    value = json.loads(text)
    if not isinstance(value, list):
        raise ValueError("names must be a JSON array")
    return [str(item) for item in value]


def inspect(args: argparse.Namespace) -> dict[str, Any]:
    requested = _parse_names(args.names)
    template_path = (args.template or "").strip()
    if not template_path or not Path(template_path).is_file():
        # Not an error: the caller distinguishes "no file" from a crash. Every
        # requested name is "missing" because there is nothing to match against.
        return {
            "status": "succeeded",
            "exists": False,
            "width": 0,
            "height": 0,
            "layers": [],
            "missing": requested,
        }

    from psd_tools import PSDImage

    psd = PSDImage.open(template_path)
    canvas_w, canvas_h = int(psd.width), int(psd.height)
    max_decode_pixels = int(max(0, args.max_decode_pixels))
    if max_decode_pixels > 0 and canvas_w * canvas_h > max_decode_pixels:
        raise ValueError(
            f"PSD canvas too large to inspect safely: {canvas_w}x{canvas_h} "
            f"({canvas_w * canvas_h} px > max {max_decode_pixels})"
        )
    layers = _flatten_layers([_layer_descriptor(layer) for layer in psd])
    names = {row["name"] for row in layers}
    missing = [name for name in requested if name and name not in names]

    return {
        "status": "succeeded",
        "exists": True,
        "width": canvas_w,
        "height": canvas_h,
        "layers": layers,
        "missing": missing,
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Inspect a PSD template's layers.")
    parser.add_argument("--template", required=True, help="path to the .psd template")
    parser.add_argument(
        "--names",
        default="",
        help="JSON array of placeholder layer names to check for existence",
    )
    parser.add_argument(
        "--max-decode-pixels",
        dest="max_decode_pixels",
        type=int,
        default=_DEFAULT_MAX_DECODE_PIXELS,
        help="reject a PSD whose canvas exceeds this many pixels (0 disables)",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        result = inspect(args)
    except Exception as err:  # noqa: BLE001 - surface a single clean error line
        sys.stderr.write(f"{type(err).__name__}: {err}\n")
        return 1
    sys.stdout.write(json.dumps(result, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
