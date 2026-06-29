"""Unit tests for the Image Enhance CLI (``image_enhance_cli.py``).

These exercise the node's contract and the v1 hardening: alpha isolation,
colour-space / high-bit handling, the input-decode guard, the down-sample path,
target resolution and the enhance report. They run on the vendored
``Pillow`` + ``numpy`` only (no GPU), matching the Phase 1 backend.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest

# The CLI lives one directory up (``python/bridge``); make it importable.
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import image_enhance_cli as cli  # noqa: E402

PIL = pytest.importorskip("PIL")
from PIL import Image  # noqa: E402


def _run(image_path: Path, **kwargs: object) -> dict:
    """Build args from defaults + overrides, run ``enhance`` and return JSON."""
    argv = ["--image", str(image_path)]
    for key, value in kwargs.items():
        flag = "--" + key.replace("_", "-")
        if isinstance(value, bool):
            if value:
                argv.append(flag)
            continue
        argv.extend([flag, str(value)])
    args = cli.build_parser().parse_args(argv)
    return cli.enhance(args)


def _make_rgb(path: Path, size: tuple[int, int] = (32, 24), color=(180, 90, 40)) -> Path:
    Image.new("RGB", size, color).save(path)
    return path


def test_rgb_upscale_to_explicit_target(tmp_path: Path) -> None:
    src = _make_rgb(tmp_path / "in.png", (40, 30))
    out = _run(src, target_width=80, target_height=60, output_dir=tmp_path)

    report = out["enhance_report"]
    assert report["source_size"] == [40, 30]
    assert report["output_size"] == [80, 60]
    assert report["scale_factor"] == 2.0
    assert report["source_mode"] == "RGB"
    assert report["had_alpha"] is False
    assert report["downscaled"] is False
    written = Image.open(out["enhanced_image"])
    assert written.size == (80, 60)


def test_alpha_is_preserved_and_isolated(tmp_path: Path) -> None:
    # A subject with a hard matte edge: left half opaque, right half transparent.
    img = Image.new("RGBA", (20, 20), (200, 50, 50, 255))
    for x in range(10, 20):
        for y in range(20):
            img.putpixel((x, y), (200, 50, 50, 0))
    src = tmp_path / "cutout.png"
    img.save(src)

    out = _run(src, target_width=40, target_height=40, mode="texture_rebuild", output_dir=tmp_path)
    report = out["enhance_report"]
    assert report["had_alpha"] is True
    assert report["output_mode"] == "RGBA"

    written = Image.open(out["enhanced_image"])
    assert written.mode == "RGBA"
    alpha = written.getchannel("A")
    # The matte stays binary: enhancement must not introduce a semi-transparent
    # halo of intermediate alpha values along the edge.
    assert set(alpha.getextrema()) <= {0, 255}


def test_cmyk_source_is_converted_and_reported(tmp_path: Path) -> None:
    src = tmp_path / "cmyk.tiff"
    Image.new("CMYK", (24, 24), (0, 0, 0, 0)).save(src)
    out = _run(src, target_width=48, target_height=48, output_dir=tmp_path)

    report = out["enhance_report"]
    assert report["source_mode"] == "CMYK"
    assert report["icc_preserved"] is False
    assert Image.open(out["enhanced_image"]).mode == "RGB"


def test_high_bit_depth_source_is_normalised(tmp_path: Path) -> None:
    src = tmp_path / "depth.tiff"
    Image.new("I", (16, 16), 40000).save(src)
    out = _run(src, target_width=32, target_height=32, output_dir=tmp_path)

    report = out["enhance_report"]
    assert report["source_mode"] in {"I", "I;16"}
    assert Image.open(out["enhanced_image"]).mode == "RGB"


def test_downscale_uses_box_and_skips_sharpen(tmp_path: Path) -> None:
    src = _make_rgb(tmp_path / "big.png", (100, 100))
    out = _run(src, target_width=50, target_height=50, mode="texture_rebuild", output_dir=tmp_path)

    report = out["enhance_report"]
    assert report["downscaled"] is True
    assert report["output_size"] == [50, 50]
    # Sharpening is suppressed when shrinking, regardless of the preset.
    assert report["texture_strength"] == 0.0


def test_oversized_input_is_rejected_before_decode(tmp_path: Path) -> None:
    src = _make_rgb(tmp_path / "in.png", (64, 64))
    with pytest.raises(ValueError, match="too large to decode"):
        _run(src, target_width=128, target_height=128, max_decode_pixels=100, output_dir=tmp_path)


def test_target_bounds_json_resolves_size(tmp_path: Path) -> None:
    src = _make_rgb(tmp_path / "in.png", (50, 50))
    bounds = json.dumps({"x": 0, "y": 0, "width": 150, "height": 100})
    out = _run(src, target_bounds_json=bounds, output_dir=tmp_path)

    report = out["enhance_report"]
    # "cover" both dimensions: max(150/50, 100/50) = 3x.
    assert report["scale_factor"] == 3.0
    assert report["output_size"] == [150, 150]


def test_max_pixels_clamps_scale(tmp_path: Path) -> None:
    src = _make_rgb(tmp_path / "in.png", (100, 100))
    out = _run(src, target_width=1000, target_height=1000, max_pixels=40000, output_dir=tmp_path)

    report = out["enhance_report"]
    assert report["clamped"] is True
    w, h = report["output_size"]
    assert w * h <= 40000


def test_preserve_text_logo_caps_texture(tmp_path: Path) -> None:
    src = _make_rgb(tmp_path / "in.png", (32, 32))
    out = _run(
        src,
        target_width=64,
        target_height=64,
        mode="texture_rebuild",
        preserve_text_logo=True,
        output_dir=tmp_path,
    )
    # texture_rebuild is 0.7; the logo guard caps it at 0.4.
    assert out["enhance_report"]["texture_strength"] == 0.4


def test_unknown_mode_rejected(tmp_path: Path) -> None:
    src = _make_rgb(tmp_path / "in.png")
    with pytest.raises(ValueError, match="unknown mode"):
        _run(src, mode="bogus", output_dir=tmp_path)


def test_missing_image_rejected(tmp_path: Path) -> None:
    with pytest.raises(FileNotFoundError):
        _run(tmp_path / "nope.png", output_dir=tmp_path)


def test_default_output_name_from_stem(tmp_path: Path) -> None:
    src = _make_rgb(tmp_path / "subject.png", (20, 20))
    out = _run(src, target_width=40, target_height=40, output_dir=tmp_path)
    assert Path(out["enhanced_image"]).name == "subject_enhanced.png"


def test_icc_preserved_for_rgb_with_profile(tmp_path: Path) -> None:
    pytest.importorskip("PIL.ImageCms")
    from PIL import ImageCms

    src = tmp_path / "tagged.png"
    profile = ImageCms.createProfile("sRGB")
    icc = ImageCms.ImageCmsProfile(profile).tobytes()
    Image.new("RGB", (24, 24), (120, 120, 120)).save(src, icc_profile=icc)

    out = _run(src, target_width=48, target_height=48, output_dir=tmp_path)
    assert out["enhance_report"]["icc_preserved"] is True
    assert "icc_profile" in Image.open(out["enhanced_image"]).info
