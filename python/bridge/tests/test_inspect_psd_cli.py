"""Unit tests for the PSD inspect CLI (``inspect_psd_cli.py``).

These cover the validate-before-run contract -- canvas size, the flattened
layer list, the requested-name existence check, the missing-file path
(``{"exists": false}`` rather than an error) -- and the v1 decode guard that
refuses an oversized canvas (aligned with ``analyze_psd_cli`` /
``compose_psd_cli``). They run on the vendored ``psd_tools`` + ``Pillow`` only.

A synthetic single-layer PSD is built from a PIL image via ``PSDImage.frompil``
so the tests need no checked-in binary fixtures.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import inspect_psd_cli as cli  # noqa: E402

pytest.importorskip("PIL")
pytest.importorskip("psd_tools")
from PIL import Image  # noqa: E402
from psd_tools import PSDImage  # noqa: E402


def _make_template(path: Path, size: tuple[int, int] = (64, 48)) -> Path:
    PSDImage.frompil(Image.new("RGBA", size, (20, 30, 40, 255))).save(str(path))
    return path


def _run(template: Path, **kwargs: object) -> dict:
    argv = ["--template", str(template)]
    for key, value in kwargs.items():
        flag = "--" + key.replace("_", "-")
        argv.extend([flag, str(value)])
    args = cli.build_parser().parse_args(argv)
    return cli.inspect(args)


def test_reports_canvas_and_layers(tmp_path: Path) -> None:
    out = _run(_make_template(tmp_path / "t.psd"))
    assert out["status"] == "succeeded"
    assert out["exists"] is True
    assert out["width"] == 64 and out["height"] == 48
    assert isinstance(out["layers"], list)
    assert all({"name", "kind"} <= row.keys() for row in out["layers"])


def test_missing_name_is_reported(tmp_path: Path) -> None:
    out = _run(_make_template(tmp_path / "t.psd"), names=json.dumps(["nope"]))
    assert out["missing"] == ["nope"]


def test_missing_file_is_not_an_error(tmp_path: Path) -> None:
    out = _run(tmp_path / "nope.psd", names=json.dumps(["a"]))
    assert out["exists"] is False
    assert out["width"] == 0 and out["height"] == 0
    assert out["missing"] == ["a"]


def test_oversized_canvas_refused(tmp_path: Path) -> None:
    with pytest.raises(ValueError, match="too large to inspect"):
        _run(_make_template(tmp_path / "t.psd"), max_decode_pixels=16)


def test_max_decode_pixels_zero_disables_guard(tmp_path: Path) -> None:
    out = _run(_make_template(tmp_path / "t.psd"), max_decode_pixels=0)
    assert out["exists"] is True


def test_default_max_decode_pixels_matches_constant() -> None:
    args = cli.build_parser().parse_args(["--template", "x.psd"])
    assert args.max_decode_pixels == cli._DEFAULT_MAX_DECODE_PIXELS
