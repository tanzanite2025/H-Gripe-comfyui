"""Unit tests for the Detail Repaint CLI (``detail_repaint_cli.py``).

These exercise the two-stage contract (prepare → composite) and the v1
hardening: issue selection / confidence / region cap, the inpaint-mask polarity,
the feathered paste-back, alpha isolation (RGB-only blend, original alpha
preserved), box-filter patch downsampling, the input-decode guard, colour-space
handling and EXIF orientation reporting. They run on the vendored
``Pillow`` + ``numpy`` only (no GPU), matching the Phase 1 backend.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import numpy as np
import pytest

# The CLI lives one directory up (``python/bridge``); make it importable.
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import detail_repaint_cli as cli  # noqa: E402

PIL = pytest.importorskip("PIL")
from PIL import Image  # noqa: E402


def _run_prepare(image_path: Path, output_dir: Path, **kwargs: object) -> dict:
    argv = ["prepare", "--image", str(image_path), "--output-dir", str(output_dir)]
    for key, value in kwargs.items():
        flag = "--" + key.replace("_", "-")
        if isinstance(value, bool):
            if value:
                argv.append(flag)
            continue
        argv.extend([flag, str(value)])
    args = cli.build_parser().parse_args(argv)
    return cli.prepare(args)


def _run_composite(image_path: Path, output_dir: Path, **kwargs: object) -> dict:
    argv = ["composite", "--image", str(image_path), "--output-dir", str(output_dir)]
    for key, value in kwargs.items():
        flag = "--" + key.replace("_", "-")
        if isinstance(value, bool):
            if value:
                argv.append(flag)
            continue
        argv.extend([flag, str(value)])
    args = cli.build_parser().parse_args(argv)
    return cli.composite(args)


def _rgba(path: Path, size: int = 64, color=(80, 120, 200), alpha: int = 255) -> Path:
    arr = np.zeros((size, size, 4), dtype=np.uint8)
    arr[..., :3] = color
    arr[..., 3] = alpha
    Image.fromarray(arr, "RGBA").save(path)
    return path


def _report(*issues: dict) -> str:
    return json.dumps({"status": "warning", "issues": list(issues)})


def _issue(bbox, confidence=0.9, action="detail_redraw", itype="face_blur") -> dict:
    return {"type": itype, "confidence": confidence, "bbox": bbox, "suggested_action": action}


# --- prepare ---------------------------------------------------------------

def test_prepare_selects_repaintable_issue_and_writes_assets(tmp_path: Path) -> None:
    img = _rgba(tmp_path / "hero.png")
    out = _run_prepare(
        img, tmp_path, quality_report=_report(_issue([10, 10, 30, 30]))
    )
    assert out["selected_count"] == 1
    region = out["regions"][0]
    assert Path(region["crop_path"]).is_file()
    assert Path(region["mask_path"]).is_file()
    assert out["mask_edit_is_transparent"] is True
    assert out["source_mode"] == "RGBA"
    assert out["exif_transposed"] is False


def test_prepare_skips_non_repaintable_action(tmp_path: Path) -> None:
    img = _rgba(tmp_path / "s.png")
    out = _run_prepare(
        img, tmp_path,
        quality_report=_report(_issue([5, 5, 20, 20], action="image_enhance")),
    )
    assert out["selected_count"] == 0
    assert out["skipped"][0]["reason"] == "action_not_repaintable"


def test_prepare_respects_min_confidence(tmp_path: Path) -> None:
    img = _rgba(tmp_path / "s.png")
    out = _run_prepare(
        img, tmp_path,
        quality_report=_report(_issue([5, 5, 20, 20], confidence=0.2)),
        min_confidence=0.5,
    )
    assert out["selected_count"] == 0
    assert out["skipped"][0]["reason"] == "below_min_confidence"


def test_prepare_caps_regions_by_confidence(tmp_path: Path) -> None:
    img = _rgba(tmp_path / "s.png")
    issues = [
        _issue([0, 0, 10, 10], confidence=0.5),
        _issue([12, 12, 22, 22], confidence=0.9),
        _issue([24, 24, 34, 34], confidence=0.7),
    ]
    out = _run_prepare(img, tmp_path, quality_report=_report(*issues), max_regions=1)
    assert out["selected_count"] == 1
    # The highest-confidence issue wins the single slot.
    assert out["regions"][0]["confidence"] == 0.9
    assert any(s["reason"] == "over_max_regions" for s in out["skipped"])


def test_prepare_mask_polarity_default_transparent(tmp_path: Path) -> None:
    img = _rgba(tmp_path / "s.png")
    out = _run_prepare(img, tmp_path, quality_report=_report(_issue([10, 10, 30, 30])))
    region = out["regions"][0]
    mask = np.asarray(Image.open(region["mask_path"]).convert("RGBA"), dtype=np.uint8)
    ix1, iy1, ix2, iy2 = region["inner_box"]
    # Edit area (issue core) is punched transparent; padding stays opaque.
    assert mask[iy1:iy2, ix1:ix2, 3].max() == 0
    assert mask[0, 0, 3] == 255


def test_prepare_invert_mask_flips_polarity(tmp_path: Path) -> None:
    img = _rgba(tmp_path / "s.png")
    out = _run_prepare(
        img, tmp_path, quality_report=_report(_issue([10, 10, 30, 30])), invert_mask=True
    )
    assert out["mask_edit_is_transparent"] is False
    region = out["regions"][0]
    mask = np.asarray(Image.open(region["mask_path"]).convert("RGBA"), dtype=np.uint8)
    ix1, iy1, ix2, iy2 = region["inner_box"]
    assert mask[iy1:iy2, ix1:ix2, 3].min() == 255
    assert mask[0, 0, 3] == 0


def test_prepare_padding_grows_crop(tmp_path: Path) -> None:
    img = _rgba(tmp_path / "s.png")
    out = _run_prepare(
        img, tmp_path, quality_report=_report(_issue([20, 20, 30, 30])), padding=8
    )
    region = out["regions"][0]
    cx1, cy1, cx2, cy2 = region["crop_box"]
    assert cx1 == 12 and cy1 == 12  # grown by padding, clamped to image
    assert region["size"] == [cx2 - cx1, cy2 - cy1]


def test_prepare_oversized_input_refused(tmp_path: Path) -> None:
    img = _rgba(tmp_path / "s.png")
    with pytest.raises(ValueError, match="too large to decode"):
        _run_prepare(
            img, tmp_path, quality_report=_report(_issue([5, 5, 20, 20])),
            max_decode_pixels=16,
        )


def test_prepare_cmyk_source_mode_recorded(tmp_path: Path) -> None:
    img = tmp_path / "c.tif"
    Image.new("CMYK", (64, 64), (0, 0, 0, 0)).save(img)
    out = _run_prepare(img, tmp_path, quality_report=_report(_issue([5, 5, 20, 20])))
    assert out["source_mode"] == "CMYK"


def test_prepare_missing_image_raises(tmp_path: Path) -> None:
    with pytest.raises(FileNotFoundError):
        _run_prepare(tmp_path / "nope.png", tmp_path, quality_report=_report())


def test_prepare_invalid_report_json_raises(tmp_path: Path) -> None:
    img = _rgba(tmp_path / "s.png")
    with pytest.raises(ValueError, match="invalid quality_report JSON"):
        _run_prepare(img, tmp_path, quality_report="{not json")


# --- composite -------------------------------------------------------------

def _prepare_then_patch(tmp_path: Path, patch_color, bbox=(16, 16, 48, 48)):
    """Run prepare, then write a flat repainted patch for the region."""
    img = _rgba(tmp_path / "base.png", size=64, color=(80, 120, 200))
    prep = _run_prepare(
        img, tmp_path, quality_report=_report(_issue(list(bbox))), padding=8
    )
    region = prep["regions"][0]
    cw, ch = region["size"]
    patch = np.zeros((ch, cw, 4), dtype=np.uint8)
    patch[..., :3] = patch_color
    patch[..., 3] = 255
    patch_path = tmp_path / "patch.png"
    Image.fromarray(patch, "RGBA").save(patch_path)
    manifest = json.dumps({"regions": prep["regions"]})
    repainted = json.dumps([{"index": region["index"], "path": str(patch_path)}])
    return img, manifest, repainted


def test_composite_paints_core_toward_patch(tmp_path: Path) -> None:
    img, manifest, repainted = _prepare_then_patch(tmp_path, (240, 30, 30))
    out = _run_composite(img, tmp_path, manifest=manifest, repainted=repainted)
    assert out["repaint_report"]["status"] == "repainted"
    assert out["repaint_report"]["repainted_count"] == 1
    res = np.asarray(Image.open(out["fixed_image"]).convert("RGBA"), dtype=np.float32)
    # The image centre should have shifted toward the red patch.
    assert res[32, 32, 0] > 150.0


def test_composite_preserves_original_alpha(tmp_path: Path) -> None:
    # Alpha isolation: a cut-out base (centre transparent) must keep its matte
    # even where an opaque patch is blended into the RGB.
    base = np.zeros((64, 64, 4), dtype=np.uint8)
    base[..., :3] = (80, 120, 200)
    base[..., 3] = 255
    base[26:38, 26:38, 3] = 0  # transparent hole inside the issue core
    img = tmp_path / "cutout.png"
    Image.fromarray(base, "RGBA").save(img)

    prep = _run_prepare(img, tmp_path, quality_report=_report(_issue([16, 16, 48, 48])), padding=8)
    region = prep["regions"][0]
    cw, ch = region["size"]
    patch = np.zeros((ch, cw, 4), dtype=np.uint8)
    patch[..., :3] = (240, 30, 30)
    patch[..., 3] = 255
    patch_path = tmp_path / "patch.png"
    Image.fromarray(patch, "RGBA").save(patch_path)
    out = _run_composite(
        img, tmp_path,
        manifest=json.dumps({"regions": prep["regions"]}),
        repainted=json.dumps([{"index": region["index"], "path": str(patch_path)}]),
    )
    res = np.asarray(Image.open(out["fixed_image"]).convert("RGBA"), dtype=np.uint8)
    # The transparent hole is preserved despite the opaque patch.
    assert res[32, 32, 3] == 0
    # ...while RGB still moved toward the patch.
    assert res[32, 32, 0] > 150


def test_composite_no_repaint_when_patch_missing(tmp_path: Path) -> None:
    img = _rgba(tmp_path / "base.png")
    prep = _run_prepare(img, tmp_path, quality_report=_report(_issue([16, 16, 48, 48])))
    region = prep["regions"][0]
    out = _run_composite(
        img, tmp_path,
        manifest=json.dumps({"regions": prep["regions"]}),
        repainted=json.dumps([{"index": region["index"], "path": ""}]),
    )
    assert out["repaint_report"]["status"] == "unchanged"
    assert out["repaint_report"]["regions"][0]["status"] == "no_repaint"


def test_composite_downsamples_with_box_filter(tmp_path: Path, monkeypatch) -> None:
    # A patch larger than the crop must be shrunk with the BOX filter, not
    # LANCZOS (avoids ringing on downsample).
    img = _rgba(tmp_path / "base.png", size=64)
    prep = _run_prepare(img, tmp_path, quality_report=_report(_issue([16, 16, 48, 48])), padding=8)
    region = prep["regions"][0]
    cw, ch = region["size"]
    big = np.zeros((ch * 2, cw * 2, 4), dtype=np.uint8)
    big[..., :3] = (10, 220, 10)
    big[..., 3] = 255
    patch_path = tmp_path / "big.png"
    Image.fromarray(big, "RGBA").save(patch_path)

    used = {}
    real_resize = Image.Image.resize

    def spy_resize(self, size, resample=None, *a, **k):
        used["resample"] = resample
        return real_resize(self, size, resample, *a, **k)

    monkeypatch.setattr(Image.Image, "resize", spy_resize)
    _run_composite(
        img, tmp_path,
        manifest=json.dumps({"regions": prep["regions"]}),
        repainted=json.dumps([{"index": region["index"], "path": str(patch_path)}]),
    )
    assert used["resample"] == Image.BOX


def test_composite_reports_hardening_fields(tmp_path: Path) -> None:
    img, manifest, repainted = _prepare_then_patch(tmp_path, (240, 30, 30))
    out = _run_composite(img, tmp_path, manifest=manifest, repainted=repainted)
    rep = out["repaint_report"]
    assert rep["source_mode"] == "RGBA"
    assert rep["exif_transposed"] is False
    assert rep["max_decode_pixels"] == cli._DEFAULT_MAX_DECODE_PIXELS


def test_composite_missing_image_raises(tmp_path: Path) -> None:
    with pytest.raises(FileNotFoundError):
        _run_composite(tmp_path / "nope.png", tmp_path, manifest="{}", repainted="[]")


def test_composite_oversized_input_refused(tmp_path: Path) -> None:
    img = _rgba(tmp_path / "base.png")
    with pytest.raises(ValueError, match="too large to decode"):
        _run_composite(img, tmp_path, manifest="{}", repainted="[]", max_decode_pixels=16)


# --- repaint (opt-in local inpaint seam) -----------------------------------

class _FakeBackend:
    """A stand-in local inpaint engine: paints the crop a flat colour."""

    id = "fake_inpaint"

    def __init__(self, color=(10, 220, 10)) -> None:
        self._color = color
        self.calls: list[dict] = []

    def weight_path(self) -> Path:
        return Path("fake-inpaint")

    def available(self) -> tuple[bool, str]:
        return True, "ready"

    def inpaint(self, crop, mask, prompt, **kwargs):  # noqa: ANN001
        self.calls.append({"size": crop.size, "prompt": prompt, **kwargs})
        # Mimic a CPU-only box: report the cpu device + the precision an
        # explicit fp16 degrades to there, exactly like the real torch backend.
        import sr_backends

        precision = sr_backends.resolve_precision(kwargs.get("precision"), "cpu")
        return Image.new("RGB", crop.size, self._color), "cpu", precision


def _prepared_manifest(tmp_path: Path) -> dict:
    img = _rgba(tmp_path / "hero.png", size=64)
    prep = _run_prepare(
        img, tmp_path, quality_report=_report(_issue([16, 16, 48, 48])), padding=8
    )
    return prep


def _run_repaint(tmp_path: Path, manifest: dict, **kwargs: object) -> dict:
    argv = ["repaint", "--manifest", json.dumps({"regions": manifest["regions"]}),
            "--output-dir", str(tmp_path)]
    for key, value in kwargs.items():
        flag = "--" + key.replace("_", "-")
        argv.extend([flag, str(value)])
    args = cli.build_parser().parse_args(argv)
    return cli.repaint(args)


def test_repaint_provider_engine_emits_no_local_repaint(tmp_path: Path) -> None:
    prep = _prepared_manifest(tmp_path)
    out = _run_repaint(tmp_path, prep, engine="provider")
    assert out["engine"] == "provider"
    assert out["repainted"] == []
    assert "provider" in out["engine_fallback_reason"]


def test_repaint_unknown_engine_falls_back(tmp_path: Path) -> None:
    prep = _prepared_manifest(tmp_path)
    out = _run_repaint(tmp_path, prep, engine="does_not_exist")
    assert out["engine"] == "provider"
    assert out["repainted"] == []
    assert "unknown engine" in out["engine_fallback_reason"]


def test_repaint_runs_available_backend(tmp_path: Path, monkeypatch) -> None:
    import inpaint_backends

    backend = _FakeBackend()
    monkeypatch.setattr(inpaint_backends, "resolve", lambda engine: backend)
    prep = _prepared_manifest(tmp_path)
    out = _run_repaint(
        tmp_path, prep, engine="fake_inpaint", prompt="restore the face", precision="fp16"
    )

    assert out["engine"] == "fake_inpaint"
    assert out["engine_fallback_reason"] is None
    assert out["backend_model"] == "fake-inpaint"
    assert out["repainted_count"] == 1
    region = prep["regions"][0]
    entry = out["repainted"][0]
    assert entry["index"] == region["index"]
    assert Path(entry["path"]).is_file()
    # The repainted crop is the backend's output, same size as the source crop.
    assert Image.open(entry["path"]).size == tuple(region["size"])
    assert backend.calls[0]["prompt"] == "restore the face"
    # precision threads to the backend; on this (faked) CPU-only box an explicit
    # fp16 request is reported as the fp32 it actually ran, never lying.
    assert backend.calls[0]["precision"] == "fp16"
    assert out["precision_requested"] == "fp16"
    assert out["precision"] == "fp32"
    assert out["device"] == "cpu"


def test_repaint_per_type_prompt_map_overrides(tmp_path: Path, monkeypatch) -> None:
    import inpaint_backends

    backend = _FakeBackend()
    monkeypatch.setattr(inpaint_backends, "resolve", lambda engine: backend)
    prep = _prepared_manifest(tmp_path)
    issue_type = prep["regions"][0]["type"]
    out = _run_repaint(
        tmp_path, prep, engine="fake_inpaint", prompt="generic",
        prompt_map=json.dumps({issue_type: "per-type prompt"}),
    )
    assert out["repainted_count"] == 1
    assert backend.calls[0]["prompt"] == "per-type prompt"


def test_repaint_unavailable_backend_falls_back(tmp_path: Path, monkeypatch) -> None:
    import inpaint_backends

    class _Unavailable(_FakeBackend):
        def available(self) -> tuple[bool, str]:
            return False, "missing optional dependency: torch"

    monkeypatch.setattr(inpaint_backends, "resolve", lambda engine: _Unavailable())
    prep = _prepared_manifest(tmp_path)
    out = _run_repaint(tmp_path, prep, engine="fake_inpaint")
    assert out["engine"] == "provider"
    assert out["repainted"] == []
    assert "torch" in out["engine_fallback_reason"]


def test_repaint_output_feeds_composite(tmp_path: Path, monkeypatch) -> None:
    # The repaint manifest plugs straight into composite as its `repainted` list.
    import inpaint_backends

    backend = _FakeBackend(color=(240, 30, 30))
    monkeypatch.setattr(inpaint_backends, "resolve", lambda engine: backend)
    img = _rgba(tmp_path / "hero.png", size=64, color=(80, 120, 200))
    prep = _run_prepare(
        img, tmp_path, quality_report=_report(_issue([16, 16, 48, 48])), padding=8
    )
    rep = _run_repaint(tmp_path, prep, engine="fake_inpaint", prompt="x")
    out = _run_composite(
        img, tmp_path,
        manifest=json.dumps({"regions": prep["regions"]}),
        repainted=json.dumps(rep["repainted"]),
    )
    assert out["repaint_report"]["repainted_count"] == 1
    res = np.asarray(Image.open(out["fixed_image"]).convert("RGBA"), dtype=np.float32)
    assert res[32, 32, 0] > 150.0


def test_probe_engines_reports_provider(tmp_path: Path, capsys) -> None:
    rc = cli.main(["--probe-engines"])
    assert rc == 0
    payload = json.loads(capsys.readouterr().out)
    assert payload["engines"]["provider"]["available"] is True
    assert "sd_inpaint" in payload["engines"]
    assert "model_cache_dir" in payload
