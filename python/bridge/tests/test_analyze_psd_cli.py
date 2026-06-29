"""Unit tests for the PSD Context Analyze CLI (``analyze_psd_cli.py``).

These exercise the node's contract and the v1 hardening: placeholder geometry /
safe-area, alpha-weighted background statistics (mean colour, brightness,
contrast, dominant palette, colour temperature, light direction), the histogram
PNG artifact, the oversized-canvas decode guard and the missing-file path. They
run on the vendored ``psd_tools`` + ``Pillow`` + ``numpy`` only (no GPU),
matching the Phase 1 backend.

A synthetic single-layer PSD is built from a PIL image via
``PSDImage.frompil`` so the tests need no checked-in binary fixtures.
"""

from __future__ import annotations

import sys
from pathlib import Path

import numpy as np
import pytest

# The CLI lives one directory up (``python/bridge``); importing it also wires up
# the repo root + vendored ``third_party`` onto ``sys.path`` (for ``psd_tools``
# and ``custom_nodes``), so this single insert is enough.
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import analyze_psd_cli as cli  # noqa: E402

pytest.importorskip("PIL")
pytest.importorskip("psd_tools")
from PIL import Image  # noqa: E402
from psd_tools import PSDImage  # noqa: E402


def _save_psd(arr: np.ndarray, path: Path) -> Path:
    """Persist an (H, W, 4) uint8 array as a single-layer RGBA PSD."""
    img = Image.fromarray(arr.astype(np.uint8), "RGBA")
    PSDImage.frompil(img).save(str(path))
    return path


def _flat_rgba(size: int, color, alpha: int = 255) -> np.ndarray:
    arr = np.zeros((size, size, 4), dtype=np.uint8)
    arr[..., 0], arr[..., 1], arr[..., 2] = color
    arr[..., 3] = alpha
    return arr


def _run(template: Path, output_dir: Path, **kwargs: object) -> dict:
    argv = ["--template", str(template), "--output-dir", str(output_dir)]
    for key, value in kwargs.items():
        flag = "--" + key.replace("_", "-")
        argv.extend([flag, str(value)])
    args = cli.build_parser().parse_args(argv)
    return cli.analyze(args)


# --- Pure-helper tests (no PSD needed) ------------------------------------


def test_dominant_palette_ignores_transparent_pixels() -> None:
    # Left half: opaque warm orange. Right half: fully transparent black.
    rgb = np.zeros((16, 16, 3), dtype=np.float32)
    rgb[:, :8] = (210, 120, 50)
    alpha = np.zeros((16, 16), dtype=np.float32)
    alpha[:, :8] = 1.0
    palette = cli._dominant_palette(rgb, alpha)
    assert palette, "expected at least one swatch"
    # The phantom transparent-black region must not become the dominant swatch.
    assert palette[0] == "#d27832"


def test_dominant_palette_all_transparent_falls_back() -> None:
    rgb = np.full((8, 8, 3), 64.0, dtype=np.float32)
    alpha = np.zeros((8, 8), dtype=np.float32)
    palette = cli._dominant_palette(rgb, alpha)
    assert palette  # does not crash / returns something usable


def test_light_direction_finds_bright_corner() -> None:
    gray = np.zeros((30, 30), dtype=np.float32)
    gray[:10, :10] = 255.0  # bright top-left cell
    weight = np.ones_like(gray)
    direction, spread = cli._light_direction(gray, weight)
    assert direction == "top-left"
    assert spread > 0.08


def test_light_direction_uniform_reads_center() -> None:
    gray = np.full((30, 30), 128.0, dtype=np.float32)
    weight = np.ones_like(gray)
    direction, spread = cli._light_direction(gray, weight)
    assert direction == "center"
    assert spread < 0.08


def test_light_direction_weight_suppresses_transparent_cell() -> None:
    # A bright top-left cell that is fully transparent must NOT win once the
    # alpha weight zeroes it out; an opaque bright bottom-right cell should.
    gray = np.full((30, 30), 40.0, dtype=np.float32)
    gray[:10, :10] = 255.0
    gray[20:, 20:] = 200.0
    weight = np.ones_like(gray)
    weight[:10, :10] = 0.0
    direction, _ = cli._light_direction(gray, weight)
    assert direction == "bottom-right"


def test_color_temperature_warm_vs_cool() -> None:
    warm = cli._color_temperature([220.0, 150.0, 70.0])
    cool = cli._color_temperature([70.0, 150.0, 220.0])
    assert warm < cool
    assert cli._warmth_label(warm) == "warm"
    assert cli._warmth_label(cool) == "cool"


# --- End-to-end tests over a synthetic PSD --------------------------------


def test_report_shape_and_histogram_artifact(tmp_path: Path) -> None:
    psd = _save_psd(_flat_rgba(64, (200, 120, 60)), tmp_path / "hero.psd")
    out = _run(psd, tmp_path)
    bg = out["background"]
    assert bg["mean_color"] == [200, 120, 60]
    assert bg["dominant_palette"]
    assert 0.0 <= bg["brightness"] <= 1.0
    hist = Path(bg["histogram_path"])
    assert hist.is_file()
    assert hist.name == "hero_histogram.png"
    assert Path(bg["image_path"]).name == "hero_background.png"
    # Contract surface: lighting + placeholder + prompt_suffix all present.
    assert set(out["lighting"]) >= {"direction", "quality", "color_temperature", "description"}
    assert set(out["placeholder"]) >= {"bounds", "mask_path", "safe_area"}
    assert isinstance(out["prompt_suffix"], str)


def test_safe_area_is_inset_within_bounds(tmp_path: Path) -> None:
    psd = _save_psd(_flat_rgba(80, (10, 10, 10)), tmp_path / "t.psd")
    out = _run(psd, tmp_path)
    bounds = out["placeholder"]["bounds"]
    safe = out["placeholder"]["safe_area"]
    assert safe["x"] >= bounds["x"]
    assert safe["y"] >= bounds["y"]
    assert safe["width"] <= bounds["width"]
    assert safe["height"] <= bounds["height"]


def test_transparent_region_does_not_darken_mean(tmp_path: Path) -> None:
    # Left half opaque mid-grey, right half transparent. The alpha-weighted mean
    # must stay near the opaque grey, not be dragged toward black.
    arr = np.zeros((40, 40, 4), dtype=np.uint8)
    arr[:, :20, :3] = 130
    arr[:, :20, 3] = 255
    psd = _save_psd(arr, tmp_path / "half.psd")
    out = _run(psd, tmp_path)
    assert all(abs(c - 130) <= 2 for c in out["background"]["mean_color"])
    assert out["background"]["brightness"] > 0.45


def test_oversized_canvas_refused(tmp_path: Path) -> None:
    psd = _save_psd(_flat_rgba(64, (128, 128, 128)), tmp_path / "big.psd")
    with pytest.raises(ValueError, match="too large"):
        _run(psd, tmp_path, max_decode_pixels=16)


def test_max_decode_pixels_zero_disables_guard(tmp_path: Path) -> None:
    psd = _save_psd(_flat_rgba(64, (128, 128, 128)), tmp_path / "ok.psd")
    out = _run(psd, tmp_path, max_decode_pixels=0)
    assert out["background"]["mean_color"] == [128, 128, 128]


def test_missing_template_raises(tmp_path: Path) -> None:
    with pytest.raises(FileNotFoundError):
        _run(tmp_path / "nope.psd", tmp_path)


def test_default_max_decode_pixels_matches_constant(tmp_path: Path) -> None:
    args = cli.build_parser().parse_args(["--template", "x.psd"])
    assert args.max_decode_pixels == cli._DEFAULT_MAX_DECODE_PIXELS
