"""Unit tests for the Match Light & Colour CLI (``color_match_cli.py``).

These exercise the node's contract and the v1 hardening: background-alpha
weighting, input-decode guard, colour-space / high-bit handling, the protection
toggles and the match report. They run on the vendored ``Pillow`` + ``numpy``
only (no GPU), matching the Phase 1 backend.
"""

from __future__ import annotations

import sys
from pathlib import Path

import numpy as np
import pytest

# The CLI lives one directory up (``python/bridge``); make it importable.
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import color_match_cli as cli  # noqa: E402

PIL = pytest.importorskip("PIL")
from PIL import Image  # noqa: E402


def _run(image_path: Path, **kwargs: object) -> dict:
    """Build args from defaults + overrides, run ``match`` and return JSON."""
    argv = ["--image", str(image_path)]
    for key, value in kwargs.items():
        flag = "--" + key.replace("_", "-")
        if isinstance(value, bool):
            if value:
                argv.append(flag)
            continue
        argv.extend([flag, str(value)])
    args = cli.build_parser().parse_args(argv)
    return cli.match(args)


def _solid_rgba(path: Path, size, color, alpha=255) -> Path:
    arr = np.zeros((size[1], size[0], 4), dtype=np.uint8)
    arr[..., :3] = color
    arr[..., 3] = alpha
    Image.fromarray(arr, "RGBA").save(path)
    return path


def _solid_rgb(path: Path, size, color) -> Path:
    Image.new("RGB", size, tuple(color)).save(path)
    return path


def _load_rgb(path: str) -> np.ndarray:
    return np.asarray(Image.open(path).convert("RGB"), dtype=np.float32)


def _mean_rgb(path: str) -> np.ndarray:
    return _load_rgb(path).reshape(-1, 3).mean(axis=0)


# --------------------------------------------------------------------------- #
# contract / pass-through
# --------------------------------------------------------------------------- #
def test_prompt_only_passes_pixels_through(tmp_path: Path) -> None:
    subj = _solid_rgb(tmp_path / "s.png", (16, 16), (40, 90, 180))
    bg = _solid_rgb(tmp_path / "b.png", (16, 16), (200, 120, 40))
    out = _run(subj, background=bg, mode="prompt_only", output_dir=tmp_path)

    assert out["match_report"]["applied"] is False
    assert out["prompt_suffix"]
    np.testing.assert_allclose(_mean_rgb(out["matched_image"]), (40, 90, 180), atol=1.0)


def test_no_background_passes_through_with_note(tmp_path: Path) -> None:
    subj = _solid_rgb(tmp_path / "s.png", (16, 16), (40, 90, 180))
    out = _run(subj, mode="color_transfer", output_dir=tmp_path)

    report = out["match_report"]
    assert report["applied"] is False
    assert "no background" in report.get("note", "")
    np.testing.assert_allclose(_mean_rgb(out["matched_image"]), (40, 90, 180), atol=1.0)


# --------------------------------------------------------------------------- #
# colour transfer
# --------------------------------------------------------------------------- #
def test_color_transfer_moves_mean_toward_background(tmp_path: Path) -> None:
    subj = _solid_rgb(tmp_path / "s.png", (16, 16), (40, 90, 180))  # cool
    bg = _solid_rgb(tmp_path / "b.png", (16, 16), (200, 120, 40))  # warm
    out = _run(subj, background=bg, mode="color_transfer", strength=1.0, output_dir=tmp_path)

    report = out["match_report"]
    assert report["applied"] is True
    before = np.array(report["before"]["mean_color"], dtype=np.float32)
    after = np.array(report["after"]["mean_color"], dtype=np.float32)
    bg_mean = np.array([200, 120, 40], dtype=np.float32)
    assert np.linalg.norm(after - bg_mean) < np.linalg.norm(before - bg_mean)


def test_hybrid_reports_transfer_stats(tmp_path: Path) -> None:
    subj = _solid_rgb(tmp_path / "s.png", (16, 16), (40, 90, 180))
    bg = _solid_rgb(tmp_path / "b.png", (16, 16), (200, 120, 40))
    out = _run(subj, background=bg, mode="hybrid", strength=0.8, output_dir=tmp_path)

    report = out["match_report"]
    assert report["applied"] is True
    assert "src_mean_lab" in report and "dst_mean_lab" in report


def test_protect_saturation_keeps_subject_chroma(tmp_path: Path) -> None:
    # Comparable-luminance colours so the in-gamut a/b stays representable
    # after the L-only match (an extreme L shift could push chroma out of gamut).
    subj_rgb = (170, 100, 90)
    subj = _solid_rgb(tmp_path / "s.png", (16, 16), subj_rgb)  # warm
    bg = _solid_rgb(tmp_path / "b.png", (16, 16), (100, 120, 160))  # cool
    out = _run(
        subj,
        background=bg,
        mode="color_transfer",
        strength=1.0,
        protect_saturation=True,
        output_dir=tmp_path,
    )

    in_lab = cli._rgb_to_lab(np.full((16, 16, 3), subj_rgb, dtype=np.uint8))
    out_lab = cli._rgb_to_lab(np.asarray(Image.open(out["matched_image"]).convert("RGB")))
    # a/b (chroma) must be preserved; only L (luminance) may move.
    np.testing.assert_allclose(out_lab[..., 1:].mean(axis=(0, 1)),
                               in_lab[..., 1:].mean(axis=(0, 1)), atol=6.0)


def test_protect_brand_color_flag_recorded(tmp_path: Path) -> None:
    subj = _solid_rgb(tmp_path / "s.png", (16, 16), (200, 30, 30))
    bg = _solid_rgb(tmp_path / "b.png", (16, 16), (60, 120, 60))
    out = _run(subj, background=bg, mode="color_transfer", protect_brand_color=True,
               output_dir=tmp_path)
    assert out["match_report"]["protect_brand_color"] is True


# --------------------------------------------------------------------------- #
# background-alpha weighting (the v1 correctness fix)
# --------------------------------------------------------------------------- #
def test_transparent_background_pixels_excluded_from_target(tmp_path: Path) -> None:
    subj = _solid_rgb(tmp_path / "s.png", (16, 16), (40, 90, 180))

    # Fully-opaque warm background.
    bg_opaque = _solid_rgba(tmp_path / "bo.png", (16, 16), (200, 120, 40), alpha=255)

    # Same warm colour on the left half, a wildly different (blue) *transparent*
    # right half that must not influence the target statistics.
    arr = np.zeros((16, 16, 4), dtype=np.uint8)
    arr[:, :8, :3] = (200, 120, 40)
    arr[:, :8, 3] = 255
    arr[:, 8:, :3] = (0, 0, 255)
    arr[:, 8:, 3] = 0
    bg_mixed = tmp_path / "bm.png"
    Image.fromarray(arr, "RGBA").save(bg_mixed)

    out_opaque = _run(subj, background=bg_opaque, mode="color_transfer", strength=1.0,
                      output_dir=tmp_path, output_name="oo")
    out_mixed = _run(subj, background=bg_mixed, mode="color_transfer", strength=1.0,
                     output_dir=tmp_path, output_name="om")

    # The transparent blue half is ignored: both runs target the same warm stats.
    np.testing.assert_allclose(out_opaque["match_report"]["dst_mean_lab"],
                               out_mixed["match_report"]["dst_mean_lab"], atol=1.5)
    np.testing.assert_allclose(_mean_rgb(out_opaque["matched_image"]),
                               _mean_rgb(out_mixed["matched_image"]), atol=3.0)


def test_subject_transparent_region_left_unchanged(tmp_path: Path) -> None:
    # Left half opaque cool subject, right half fully transparent.
    arr = np.zeros((16, 16, 4), dtype=np.uint8)
    arr[:, :8, :3] = (40, 90, 180)
    arr[:, :8, 3] = 255
    arr[:, 8:, :3] = (10, 20, 30)
    arr[:, 8:, 3] = 0
    subj = tmp_path / "s.png"
    Image.fromarray(arr, "RGBA").save(subj)
    bg = _solid_rgb(tmp_path / "b.png", (16, 16), (200, 120, 40))

    out = _run(subj, background=bg, mode="color_transfer", strength=1.0, output_dir=tmp_path)
    rgba = np.asarray(Image.open(out["matched_image"]).convert("RGBA"))
    # Transparent region keeps its original RGB (weight 0 there; the only drift
    # is the lossless-ish Lab round-trip, within 2 levels).
    np.testing.assert_allclose(rgba[:, 8:, :3].reshape(-1, 3)[0], (10, 20, 30), atol=2)


# --------------------------------------------------------------------------- #
# input hardening
# --------------------------------------------------------------------------- #
def test_oversized_input_refused_before_decode(tmp_path: Path) -> None:
    subj = _solid_rgb(tmp_path / "s.png", (64, 64), (40, 90, 180))
    with pytest.raises(ValueError, match="too large"):
        _run(subj, max_decode_pixels=100, output_dir=tmp_path)


def test_cmyk_subject_reports_source_mode(tmp_path: Path) -> None:
    cmyk = tmp_path / "s.tif"
    Image.new("CMYK", (16, 16), (10, 20, 30, 5)).save(cmyk)
    bg = _solid_rgb(tmp_path / "b.png", (16, 16), (200, 120, 40))
    out = _run(cmyk, background=bg, mode="color_transfer", output_dir=tmp_path)
    assert out["match_report"]["source_mode"] == "CMYK"


def test_highbit_subject_normalised(tmp_path: Path) -> None:
    path = tmp_path / "s.tiff"
    Image.new("I", (16, 16), 40000).save(path)
    bg = _solid_rgb(tmp_path / "b.png", (16, 16), (200, 120, 40))
    out = _run(path, background=bg, mode="color_transfer", output_dir=tmp_path)
    assert out["match_report"]["source_mode"] in {"I", "I;16"}


# --------------------------------------------------------------------------- #
# error handling / contract details
# --------------------------------------------------------------------------- #
def test_unknown_mode_rejected(tmp_path: Path) -> None:
    subj = _solid_rgb(tmp_path / "s.png", (16, 16), (40, 90, 180))
    with pytest.raises(ValueError, match="unknown mode"):
        _run(subj, mode="bogus", output_dir=tmp_path)


def test_missing_subject_raises(tmp_path: Path) -> None:
    with pytest.raises(FileNotFoundError):
        _run(tmp_path / "nope.png", output_dir=tmp_path)


def test_invalid_context_json_ignored(tmp_path: Path) -> None:
    subj = _solid_rgb(tmp_path / "s.png", (16, 16), (40, 90, 180))
    bg = _solid_rgb(tmp_path / "b.png", (16, 16), (200, 120, 40))
    out = _run(subj, background=bg, mode="color_transfer", context="{not json",
               output_dir=tmp_path)
    assert out["match_report"]["applied"] is True


def test_default_output_name(tmp_path: Path) -> None:
    subj = _solid_rgb(tmp_path / "subject.png", (16, 16), (40, 90, 180))
    out = _run(subj, output_dir=tmp_path)
    assert Path(out["matched_image"]).name == "subject_matched.png"


# --------------------------------------------------------------------------- #
# engine seam (learned matcher dispatch + telemetry)
# --------------------------------------------------------------------------- #
def test_engine_defaults_to_cpu(tmp_path: Path) -> None:
    subj = _solid_rgb(tmp_path / "s.png", (16, 16), (40, 90, 180))
    bg = _solid_rgb(tmp_path / "b.png", (16, 16), (200, 120, 40))
    report = _run(subj, background=bg, mode="color_transfer", output_dir=tmp_path)["match_report"]
    assert report["engine"] == "cpu"
    assert report["engine_requested"] == "cpu"
    assert report["engine_fallback_reason"] is None
    assert report["backend_model"] is None


def test_learned_engine_falls_back_to_cpu_when_unavailable(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # No onnxruntime / weight on this box: the heuristic still runs and the
    # report explains why the learned engine was not used.
    monkeypatch.delenv("HGRIPE_COLOR_MODEL", raising=False)
    monkeypatch.setenv("HGRIPE_MODEL_CACHE", "/definitely/not/here")
    subj = _solid_rgb(tmp_path / "s.png", (16, 16), (40, 90, 180))
    bg = _solid_rgb(tmp_path / "b.png", (16, 16), (200, 120, 40))
    report = _run(
        subj, background=bg, mode="color_transfer", engine="onnx_harmonize", output_dir=tmp_path
    )["match_report"]
    assert report["engine"] == "cpu"
    assert report["engine_requested"] == "onnx_harmonize"
    assert report["engine_fallback_reason"]
    assert report["backend_model"] is None
    assert report["applied"] is True


def test_unknown_engine_falls_back_with_reason(tmp_path: Path) -> None:
    subj = _solid_rgb(tmp_path / "s.png", (16, 16), (40, 90, 180))
    bg = _solid_rgb(tmp_path / "b.png", (16, 16), (200, 120, 40))
    report = _run(
        subj, background=bg, mode="color_transfer", engine="bogus", output_dir=tmp_path
    )["match_report"]
    assert report["engine"] == "cpu"
    assert "unknown engine" in report["engine_fallback_reason"]


def test_learned_engine_skipped_when_no_background(tmp_path: Path) -> None:
    subj = _solid_rgb(tmp_path / "s.png", (16, 16), (40, 90, 180))
    report = _run(subj, mode="color_transfer", engine="onnx_harmonize", output_dir=tmp_path)[
        "match_report"
    ]
    assert report["engine"] == "cpu"
    assert report["engine_fallback_reason"] == "no background reference"


def test_learned_engine_skipped_for_prompt_only(tmp_path: Path) -> None:
    subj = _solid_rgb(tmp_path / "s.png", (16, 16), (40, 90, 180))
    bg = _solid_rgb(tmp_path / "b.png", (16, 16), (200, 120, 40))
    report = _run(
        subj, background=bg, mode="prompt_only", engine="onnx_harmonize", output_dir=tmp_path
    )["match_report"]
    assert report["engine"] == "cpu"
    assert report["engine_fallback_reason"] == "mode does not change pixels"


def test_probe_engines_flag_emits_capability_json(tmp_path: Path, capsys) -> None:
    rc = cli.main(["--image", "ignored", "--probe-engines"])
    assert rc == 0
    import json

    payload = json.loads(capsys.readouterr().out)
    assert payload["engines"]["cpu"]["available"] is True
    assert "onnx_harmonize" in payload["engines"]
    assert "model_cache_dir" in payload
