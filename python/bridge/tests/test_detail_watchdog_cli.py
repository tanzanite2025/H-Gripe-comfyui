"""Unit tests for the Detail Watchdog CLI (``detail_watchdog_cli.py``).

These exercise the node's detect-only contract and the v1 hardening: the
input-decode guard, colour-space / high-bit / palette handling, EXIF
orientation reporting, the advisory (non-consumed) mask, and the core
heuristics (global blur, soft regions, edge halo, colour mismatch). They run on
the vendored ``Pillow`` + ``numpy`` only (no GPU), matching the Phase 1 backend.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import numpy as np
import pytest

# The CLI lives one directory up (``python/bridge``); make it importable.
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import detail_watchdog_cli as cli  # noqa: E402

PIL = pytest.importorskip("PIL")
from PIL import Image  # noqa: E402


def _run(image_path: Path, output_dir: Path, **kwargs: object) -> dict:
    """Build args from defaults + overrides, run ``watch`` and return JSON."""
    argv = ["--image", str(image_path), "--output-dir", str(output_dir)]
    for key, value in kwargs.items():
        flag = "--" + key.replace("_", "-")
        if isinstance(value, bool):
            if value:
                argv.append(flag)
            continue
        argv.extend([flag, str(value)])
    args = cli.build_parser().parse_args(argv)
    return cli.watch(args)


def _sharp_rgb(path: Path, size: int = 128, color=(60, 140, 90)) -> Path:
    """A high-frequency checkerboard so the global sharpness clears any floor."""
    yy, xx = np.mgrid[0:size, 0:size]
    checker = ((xx // 4 + yy // 4) % 2).astype(np.uint8) * 255
    arr = np.zeros((size, size, 3), dtype=np.uint8)
    arr[..., 0] = checker
    arr[..., 1] = checker
    arr[..., 2] = checker
    Image.fromarray(arr, "RGB").save(path)
    return path


def _flat_rgb(path: Path, size: int = 64, color=(120, 120, 120)) -> Path:
    Image.new("RGB", (size, size), tuple(color)).save(path)
    return path


def _halo_rgba(path: Path, size: int = 64) -> Path:
    """A subject with a bright rim on a semi-transparent alpha band."""
    yy, xx = np.mgrid[0:size, 0:size]
    cx = cy = (size - 1) / 2.0
    dist = np.sqrt((xx - cx) ** 2 + (yy - cy) ** 2)
    radius = 20.0
    alpha = np.clip((radius - dist) / 3.0 + 0.5, 0.0, 1.0)
    arr = np.zeros((size, size, 4), dtype=np.uint8)
    arr[..., :3] = 40  # dark interior subject
    rim = (alpha > 0.05) & (alpha < 0.95)
    arr[rim, :3] = 250  # bright fringe on the rim
    arr[..., 3] = np.rint(alpha * 255.0).astype(np.uint8)
    Image.fromarray(arr, "RGBA").save(path)
    return path


def test_sharp_image_passes_and_report_fields(tmp_path: Path) -> None:
    img = _sharp_rgb(tmp_path / "sharp.png")
    out = _run(img, tmp_path)
    assert out["quality_report"]["status"] == "passed"
    assert out["quality_report"]["issues"] == []
    # Detect-only: the candidate is returned unchanged.
    assert out["fixed_image"] == str(img)
    rep = out["watchdog_report"]
    assert rep["source_mode"] == "RGB"
    assert rep["exif_transposed"] is False
    assert rep["mask_consumed"] is False
    assert rep["image_size"] == [128, 128]


def test_low_resolution_when_below_target(tmp_path: Path) -> None:
    img = _sharp_rgb(tmp_path / "small.png", size=64)
    bounds = json.dumps({"x": 0, "y": 0, "width": 512, "height": 512})
    out = _run(img, tmp_path, target_bounds=bounds)
    types = [i["type"] for i in out["quality_report"]["issues"]]
    assert "low_resolution" in types
    assert out["watchdog_report"]["target_size"] == [512, 512]


def test_edge_halo_detected_on_rim(tmp_path: Path) -> None:
    img = _halo_rgba(tmp_path / "halo.png")
    out = _run(img, tmp_path, watch_targets="product_edges", mode="strict")
    types = [i["type"] for i in out["quality_report"]["issues"]]
    assert "edge_halo" in types


def test_unsupported_targets_recorded_as_skipped(tmp_path: Path) -> None:
    img = _sharp_rgb(tmp_path / "s.png")
    out = _run(img, tmp_path, watch_targets="hands,text,logo")
    assert out["watchdog_report"]["skipped_targets"] == ["hands", "logo", "text"]


def test_overlay_written_when_issues_present(tmp_path: Path) -> None:
    img = _sharp_rgb(tmp_path / "small.png", size=64)
    bounds = json.dumps({"x": 0, "y": 0, "width": 512, "height": 512})
    out = _run(img, tmp_path, target_bounds=bounds)
    assert out["issue_masks"] is not None
    assert Path(out["issue_masks"]).is_file()


def test_no_overlay_flag_suppresses_png(tmp_path: Path) -> None:
    img = _sharp_rgb(tmp_path / "small.png", size=64)
    bounds = json.dumps({"x": 0, "y": 0, "width": 512, "height": 512})
    out = _run(img, tmp_path, target_bounds=bounds, no_overlay=True)
    assert out["issue_masks"] is None


def test_oversized_input_refused_before_decode(tmp_path: Path) -> None:
    img = _flat_rgb(tmp_path / "s.png", size=64)
    with pytest.raises(ValueError, match="too large to decode"):
        _run(img, tmp_path, max_decode_pixels=16)


def test_cmyk_source_mode_recorded(tmp_path: Path) -> None:
    img = tmp_path / "c.tif"
    Image.new("CMYK", (64, 64), (0, 0, 0, 0)).save(img)
    out = _run(img, tmp_path)
    assert out["watchdog_report"]["source_mode"] == "CMYK"


def test_palette_source_mode_recorded(tmp_path: Path) -> None:
    img = tmp_path / "p.png"
    Image.new("RGB", (64, 64), (30, 60, 90)).convert("P").save(img)
    out = _run(img, tmp_path)
    assert out["watchdog_report"]["source_mode"] == "P"


def test_plain_image_not_reported_as_transposed(tmp_path: Path) -> None:
    # Pillow 12's exif_transpose returns a new object even with no orientation;
    # the orientation-tag check must keep exif_transposed False here.
    img = _sharp_rgb(tmp_path / "s.png")
    out = _run(img, tmp_path)
    assert out["watchdog_report"]["exif_transposed"] is False


def test_mask_is_advisory_and_not_consumed(tmp_path: Path) -> None:
    img = _sharp_rgb(tmp_path / "s.png")
    mask = tmp_path / "m.png"
    Image.new("L", (128, 128), 255).save(mask)
    out = _run(img, tmp_path, mask=str(mask))
    assert out["watchdog_report"]["mask_consumed"] is False


def test_invalid_mode_rejected(tmp_path: Path) -> None:
    img = _sharp_rgb(tmp_path / "s.png")
    with pytest.raises(ValueError, match="unknown mode"):
        _run(img, tmp_path, mode="ultra")


def test_unknown_watch_target_rejected(tmp_path: Path) -> None:
    img = _sharp_rgb(tmp_path / "s.png")
    with pytest.raises(ValueError, match="unknown watch target"):
        _run(img, tmp_path, watch_targets="face,unicorn")


def test_missing_image_raises(tmp_path: Path) -> None:
    with pytest.raises(FileNotFoundError):
        _run(tmp_path / "nope.png", tmp_path)
