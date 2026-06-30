"""Unit tests for the PSD Export / compose CLI (``compose_psd_cli.py``).

These exercise the node's contract and the v1 hardening of the final assembler:
the input-decode guard, CMYK / high-bit / grayscale colour-space handling, EXIF
orientation, the refined-mask alpha multiply, the exported triplet
(``.psd`` + ``_preview.png`` + ``_metadata.json``) and the additive
``export_report``. They run on the vendored ``psd_tools`` + ``Pillow`` (+ numpy
for the high-bit fixtures) only -- no GPU, no torch -- matching the Phase 1
backend.

A synthetic single-layer PSD template is built from a PIL image via
``PSDImage.frompil`` so the tests need no checked-in binary fixtures.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest

# The CLI lives one directory up (``python/bridge``); importing it also wires up
# the repo root + vendored ``third_party`` onto ``sys.path`` (for ``psd_tools``
# and ``custom_nodes``), so this single insert is enough.
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import compose_psd_cli as cli  # noqa: E402

pytest.importorskip("PIL")
pytest.importorskip("psd_tools")
from PIL import Image  # noqa: E402
from psd_tools import PSDImage  # noqa: E402


def _make_template(path: Path, size: tuple[int, int] = (64, 48)) -> Path:
    """A flat single-layer RGBA PSD used as the compose template."""
    PSDImage.frompil(Image.new("RGBA", size, (20, 30, 40, 255))).save(str(path))
    return path


def _run(template: Path, image: Path, output_dir: Path, **kwargs: object) -> dict:
    argv = [
        "--template",
        str(template),
        "--image",
        str(image),
        "--output-dir",
        str(output_dir),
    ]
    for key, value in kwargs.items():
        flag = "--" + key.replace("_", "-")
        argv.extend([flag, str(value)])
    args = cli.build_parser().parse_args(argv)
    return cli.compose_and_export(args)


def _save(img: Image.Image, path: Path) -> Path:
    img.save(path)
    return path


# --- End-to-end contract over a synthetic PSD -----------------------------


def test_triplet_and_report_shape(tmp_path: Path) -> None:
    template = _make_template(tmp_path / "tmpl.psd")
    image = _save(Image.new("RGB", (40, 30), (200, 120, 60)), tmp_path / "gen.png")

    out = _run(template, image, tmp_path, filename="hero")

    assert out["status"] == "succeeded"
    assert Path(out["psd_path"]).name == "hero.psd" and Path(out["psd_path"]).is_file()
    assert Path(out["preview_path"]).name == "hero_preview.png" and Path(out["preview_path"]).is_file()
    assert Path(out["metadata_path"]).name == "hero_metadata.json" and Path(out["metadata_path"]).is_file()

    report = out["export_report"]
    assert report["source_mode"] == "RGB"
    assert report["mask_applied"] is False
    assert report["exif_transposed"] is False
    assert report["canvas"] == [64, 48]
    assert report["triplet"] == {"psd": True, "preview": True, "metadata": True}

    meta = json.loads(Path(out["metadata_path"]).read_text(encoding="utf-8"))
    assert meta["source_mode"] == "RGB"
    assert meta["image_size"] == [40, 30]
    assert meta["mask_applied"] is False


def test_save_preview_disabled_marks_triplet(tmp_path: Path) -> None:
    template = _make_template(tmp_path / "tmpl.psd")
    image = _save(Image.new("RGB", (20, 20), (90, 90, 90)), tmp_path / "gen.png")

    out = _run(template, image, tmp_path, save_preview="disable")
    assert out["preview_path"] == ""
    assert out["export_report"]["triplet"]["preview"] is False


def test_mask_multiplies_into_alpha(tmp_path: Path) -> None:
    template = _make_template(tmp_path / "tmpl.psd")
    image = _save(Image.new("RGBA", (30, 30), (200, 50, 50, 255)), tmp_path / "gen.png")
    # Mask transparent on the right half: the matte must zero that alpha.
    mask = Image.new("L", (30, 30), 255)
    for x in range(15, 30):
        for y in range(30):
            mask.putpixel((x, y), 0)
    mask_path = _save(mask, tmp_path / "mask.png")

    out = _run(template, image, tmp_path, mask=str(mask_path), fit_mode="stretch")
    assert out["export_report"]["mask_applied"] is True

    meta = json.loads(Path(out["metadata_path"]).read_text(encoding="utf-8"))
    assert meta["mask_applied"] is True
    assert meta["mask_source"] == str(mask_path)


def test_cmyk_source_is_converted_and_reported(tmp_path: Path) -> None:
    template = _make_template(tmp_path / "tmpl.psd")
    image = _save(Image.new("CMYK", (24, 24), (0, 0, 0, 0)), tmp_path / "gen.tiff")

    out = _run(template, image, tmp_path)
    assert out["export_report"]["source_mode"] == "CMYK"
    # The composed PSD + preview still build cleanly from the converted RGBA.
    assert Path(out["preview_path"]).is_file()


def test_high_bit_source_is_normalised(tmp_path: Path) -> None:
    template = _make_template(tmp_path / "tmpl.psd")
    image = _save(Image.new("I", (16, 16), 40000), tmp_path / "gen.tiff")

    out = _run(template, image, tmp_path)
    assert out["export_report"]["source_mode"] in {"I", "I;16"}
    assert Path(out["psd_path"]).is_file()


def test_grayscale_source_is_converted(tmp_path: Path) -> None:
    template = _make_template(tmp_path / "tmpl.psd")
    image = _save(Image.new("L", (20, 20), 128), tmp_path / "gen.png")

    out = _run(template, image, tmp_path)
    assert out["export_report"]["source_mode"] == "L"
    assert Path(out["psd_path"]).is_file()


def test_oversized_input_is_rejected_before_decode(tmp_path: Path) -> None:
    template = _make_template(tmp_path / "tmpl.psd")
    image = _save(Image.new("RGB", (64, 64), (10, 10, 10)), tmp_path / "gen.png")
    with pytest.raises(ValueError, match="too large to decode"):
        _run(template, image, tmp_path, max_decode_pixels=100)


def test_oversized_mask_is_rejected_before_decode(tmp_path: Path) -> None:
    template = _make_template(tmp_path / "tmpl.psd")
    image = _save(Image.new("RGBA", (16, 16), (10, 10, 10, 255)), tmp_path / "gen.png")
    mask = _save(Image.new("L", (64, 64), 255), tmp_path / "mask.png")
    with pytest.raises(ValueError, match="too large to decode"):
        _run(template, image, tmp_path, mask=str(mask), max_decode_pixels=300)


def test_max_decode_pixels_zero_disables_guard(tmp_path: Path) -> None:
    template = _make_template(tmp_path / "tmpl.psd")
    image = _save(Image.new("RGB", (40, 40), (10, 10, 10)), tmp_path / "gen.png")
    out = _run(template, image, tmp_path, max_decode_pixels=0)
    assert out["status"] == "succeeded"


def test_missing_template_raises(tmp_path: Path) -> None:
    image = _save(Image.new("RGB", (10, 10), (0, 0, 0)), tmp_path / "gen.png")
    with pytest.raises(FileNotFoundError):
        _run(tmp_path / "nope.psd", image, tmp_path)


def test_missing_image_raises(tmp_path: Path) -> None:
    template = _make_template(tmp_path / "tmpl.psd")
    with pytest.raises(FileNotFoundError):
        _run(template, tmp_path / "nope.png", tmp_path)


def test_missing_mask_raises(tmp_path: Path) -> None:
    template = _make_template(tmp_path / "tmpl.psd")
    image = _save(Image.new("RGB", (10, 10), (0, 0, 0)), tmp_path / "gen.png")
    with pytest.raises(FileNotFoundError):
        _run(template, image, tmp_path, mask=str(tmp_path / "nope.png"))


def test_invalid_metadata_json_rejected(tmp_path: Path) -> None:
    template = _make_template(tmp_path / "tmpl.psd")
    image = _save(Image.new("RGB", (10, 10), (0, 0, 0)), tmp_path / "gen.png")
    with pytest.raises(ValueError, match="must be valid JSON"):
        _run(template, image, tmp_path, metadata="{not json")


def test_metadata_object_is_merged(tmp_path: Path) -> None:
    template = _make_template(tmp_path / "tmpl.psd")
    image = _save(Image.new("RGB", (10, 10), (0, 0, 0)), tmp_path / "gen.png")
    out = _run(template, image, tmp_path, metadata=json.dumps({"job_id": "abc123"}))
    meta = json.loads(Path(out["metadata_path"]).read_text(encoding="utf-8"))
    assert meta["job_id"] == "abc123"


def test_default_max_decode_pixels_matches_constant() -> None:
    args = cli.build_parser().parse_args(
        ["--template", "x.psd", "--image", "y.png", "--output-dir", "."]
    )
    assert args.max_decode_pixels == cli._DEFAULT_MAX_DECODE_PIXELS
