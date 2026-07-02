"""Unit tests for the Refine Mask Edge CLI (``edge_refine_cli.py``).

These exercise the node's contract and the v1 hardening: morphology / guided /
feather / decontaminate / background-blend behaviour, explicit-mask precedence,
the no-edge report note, input-decode guard, colour-space handling and preset
parsing. They run on the vendored ``Pillow`` + ``numpy`` only (no GPU),
matching the Phase 1 backend.
"""

from __future__ import annotations

import sys
from pathlib import Path

import numpy as np
import pytest

# The CLI lives one directory up (``python/bridge``); make it importable.
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import edge_refine_cli as cli  # noqa: E402

PIL = pytest.importorskip("PIL")
from PIL import Image  # noqa: E402


def _run(image_path: Path, output_dir: Path, **kwargs: object) -> dict:
    """Build args from defaults + overrides, run ``refine`` and return JSON."""
    argv = ["--image", str(image_path), "--output-dir", str(output_dir)]
    for key, value in kwargs.items():
        flag = "--" + key.replace("_", "-")
        if isinstance(value, bool):
            if value:
                argv.append(flag)
            continue
        argv.extend([flag, str(value)])
    args = cli.build_parser().parse_args(argv)
    return cli.refine(args)


def _disc_alpha(size: int, radius: float) -> np.ndarray:
    """A centred soft disc alpha (0..1) so there is a real transitional band."""
    yy, xx = np.mgrid[0:size, 0:size]
    cx = cy = (size - 1) / 2.0
    dist = np.sqrt((xx - cx) ** 2 + (yy - cy) ** 2)
    alpha = np.clip((radius - dist) / 2.0 + 0.5, 0.0, 1.0)
    return alpha.astype(np.float32)


def _subject_rgba(path: Path, size: int = 48, radius: float = 16.0, color=(40, 160, 60)) -> Path:
    alpha = _disc_alpha(size, radius)
    arr = np.zeros((size, size, 4), dtype=np.uint8)
    arr[..., :3] = color
    arr[..., 3] = np.rint(alpha * 255.0).astype(np.uint8)
    Image.fromarray(arr, "RGBA").save(path)
    return path


def _opaque_rgb(path: Path, size: int = 32, color=(120, 120, 120)) -> Path:
    Image.new("RGB", (size, size), tuple(color)).save(path)
    return path


def _coverage_of(mask_path: str) -> float:
    arr = np.asarray(Image.open(mask_path).convert("L"), dtype=np.float32) / 255.0
    return float(arr.mean())


def _band_px(mask_path: str) -> int:
    arr = np.asarray(Image.open(mask_path).convert("L"), dtype=np.float32) / 255.0
    band = np.minimum(arr, 1.0 - arr) * 2.0
    return int((band > 0.05).sum())


def test_default_preset_and_output_naming(tmp_path: Path) -> None:
    img = _subject_rgba(tmp_path / "hero.png")
    out = _run(img, tmp_path)
    assert out["edge_report"]["preset"] == "natural"
    assert out["edge_report"]["source_mask"] == "alpha"
    assert Path(out["refined_image"]).name == "hero_refined.png"
    assert Path(out["refined_mask"]).name == "hero_refined_mask.png"
    assert Path(out["refined_image"]).is_file()
    assert Path(out["refined_mask"]).is_file()


def test_erosion_reduces_coverage(tmp_path: Path) -> None:
    img = _subject_rgba(tmp_path / "s.png")
    out = _run(
        img,
        tmp_path,
        preset="custom",
        erode_px=3,
        dilate_px=0,
        feather_px=0,
        guided_radius=0,
    )
    rep = out["edge_report"]
    assert rep["coverage_after"] < rep["coverage_before"]


def test_feather_widens_the_band(tmp_path: Path) -> None:
    img = _subject_rgba(tmp_path / "s.png")
    narrow = _run(
        img, tmp_path, preset="custom", feather_px=1, guided_radius=0, output_name="narrow"
    )
    wide = _run(
        img, tmp_path, preset="custom", feather_px=10, guided_radius=0, output_name="wide"
    )
    assert _band_px(wide["refined_mask"]) > _band_px(narrow["refined_mask"])


def test_guided_filter_snaps_to_luminance_edge(tmp_path: Path) -> None:
    # A subject whose alpha disc is larger than its colour disc: the guided
    # filter should pull the matte back toward the actual colour edge.
    size = 48
    alpha = _disc_alpha(size, 20.0)
    arr = np.zeros((size, size, 4), dtype=np.uint8)
    color = _disc_alpha(size, 12.0)
    arr[..., 0] = np.rint(color * 220.0).astype(np.uint8)
    arr[..., 1] = np.rint(color * 220.0).astype(np.uint8)
    arr[..., 2] = np.rint(color * 220.0).astype(np.uint8)
    arr[..., 3] = np.rint(alpha * 255.0).astype(np.uint8)
    img = tmp_path / "g.png"
    Image.fromarray(arr, "RGBA").save(img)
    with_guide = _run(
        img, tmp_path, preset="custom", feather_px=0, guided_radius=8, output_name="g_on"
    )
    no_guide = _run(
        img, tmp_path, preset="custom", feather_px=0, guided_radius=0, output_name="g_off"
    )
    # Snapping to the smaller luminance disc cannot increase coverage.
    assert with_guide["edge_report"]["coverage_after"] <= no_guide["edge_report"][
        "coverage_after"
    ] + 1e-4


def test_decontaminate_pulls_subject_colour_into_band(tmp_path: Path) -> None:
    # White fringe around a saturated subject: decontamination should pull the
    # subject's own colour into the band, reducing the white halo.
    size = 48
    alpha = _disc_alpha(size, 16.0)
    arr = np.zeros((size, size, 4), dtype=np.uint8)
    arr[..., :3] = (230, 230, 230)  # near-white everywhere (fringe colour)
    inner = _disc_alpha(size, 10.0) > 0.5
    arr[inner, 0] = 200
    arr[inner, 1] = 30
    arr[inner, 2] = 30  # red subject core
    arr[..., 3] = np.rint(alpha * 255.0).astype(np.uint8)
    img = tmp_path / "fringe.png"
    Image.fromarray(arr, "RGBA").save(img)

    on = _run(
        img, tmp_path, preset="custom", feather_px=2, guided_radius=0,
        edge_decontaminate=True, output_name="dc_on",
    )
    off = _run(
        img, tmp_path, preset="custom", feather_px=2, guided_radius=0,
        edge_decontaminate=False, output_name="dc_off",
    )
    on_rgb = np.asarray(Image.open(on["refined_image"]).convert("RGB"), dtype=np.float32)
    off_rgb = np.asarray(Image.open(off["refined_image"]).convert("RGB"), dtype=np.float32)
    # Decontamination should make the band redder / less white on average.
    assert on_rgb[..., 0].mean() - on_rgb[..., 2].mean() > (
        off_rgb[..., 0].mean() - off_rgb[..., 2].mean()
    )


def test_background_blend_pulls_band_toward_target(tmp_path: Path) -> None:
    img = _subject_rgba(tmp_path / "s.png", color=(230, 230, 230))
    bg = _opaque_rgb(tmp_path / "bg.png", color=(10, 20, 200))
    out = _run(
        img, tmp_path, preset="custom", feather_px=3, guided_radius=0,
        background=str(bg), background_blend_strength=0.9,
    )
    assert out["edge_report"]["background_applied"] is True
    rgb = np.asarray(Image.open(out["refined_image"]).convert("RGB"), dtype=np.float32)
    # The blend should introduce some of the blue target into the frame.
    assert rgb[..., 2].max() > 60.0


def test_explicit_mask_takes_precedence_over_alpha(tmp_path: Path) -> None:
    img = _subject_rgba(tmp_path / "s.png", radius=8.0)
    mask_arr = np.rint(_disc_alpha(48, 20.0) * 255.0).astype(np.uint8)
    mask = tmp_path / "m.png"
    Image.fromarray(mask_arr, "L").save(mask)
    out = _run(img, tmp_path, mask=str(mask), preset="custom", feather_px=0, guided_radius=0)
    assert out["edge_report"]["source_mask"] == "explicit"
    # Coverage should track the larger explicit disc, not the small alpha disc.
    assert out["edge_report"]["coverage_before"] > 0.1


def test_no_edge_note_when_fully_opaque(tmp_path: Path) -> None:
    img = _opaque_rgb(tmp_path / "flat.png")
    out = _run(img, tmp_path, preset="custom", feather_px=0, guided_radius=0)
    rep = out["edge_report"]
    assert rep["edge_band_px"] == 0
    assert "note" in rep
    assert "no transitional edge" in rep["note"]


def test_band_present_has_no_note(tmp_path: Path) -> None:
    img = _subject_rgba(tmp_path / "s.png")
    out = _run(img, tmp_path)
    assert out["edge_report"]["edge_band_px"] > 0
    assert "note" not in out["edge_report"]


def test_preset_parsing(tmp_path: Path) -> None:
    img = _subject_rgba(tmp_path / "s.png")
    for preset, feather in (("clean", 2.0), ("natural", 6.0), ("soft", 12.0)):
        out = _run(img, tmp_path, preset=preset, output_name=preset)
        assert out["edge_report"]["preset"] == preset
        assert out["edge_report"]["feather_px"] == feather


def test_oversized_input_refused_before_decode(tmp_path: Path) -> None:
    img = _subject_rgba(tmp_path / "s.png", size=48)
    with pytest.raises(ValueError, match="too large to decode"):
        _run(img, tmp_path, max_decode_pixels=16)


def test_cmyk_source_mode_recorded(tmp_path: Path) -> None:
    img = tmp_path / "c.tif"
    Image.new("CMYK", (32, 32), (0, 0, 0, 0)).save(img)
    out = _run(img, tmp_path)
    assert out["edge_report"]["source_mode"] == "CMYK"


def test_invalid_preset_rejected(tmp_path: Path) -> None:
    img = _subject_rgba(tmp_path / "s.png")
    with pytest.raises(ValueError, match="unknown preset"):
        _run(img, tmp_path, preset="ultra")


def test_missing_image_raises(tmp_path: Path) -> None:
    with pytest.raises(FileNotFoundError):
        _run(tmp_path / "nope.png", tmp_path)


def test_missing_background_raises(tmp_path: Path) -> None:
    img = _subject_rgba(tmp_path / "s.png")
    with pytest.raises(FileNotFoundError, match="background image not found"):
        _run(img, tmp_path, background=str(tmp_path / "ghost.png"))


def test_trimap_protects_unknown_band_from_erosion(tmp_path: Path) -> None:
    # The soft transitional ring of a matte is genuine continuous alpha (think
    # hair), not fringe. A trimap marking that ring as `unknown` must protect it
    # from the erode clean-up that would otherwise bite it away.
    size = 48
    alpha = _disc_alpha(size, 16.0)
    img = _subject_rgba(tmp_path / "hair.png", size=size, radius=16.0)

    # FG (255) solid core, BG (0) empty exterior, the soft ring between unknown.
    tri = np.full((size, size), 128, dtype=np.uint8)
    tri[alpha > 0.9] = 255
    tri[alpha < 0.1] = 0
    trimap_path = tmp_path / "trimap.png"
    Image.fromarray(tri, "L").save(trimap_path)

    common = dict(preset="custom", erode_px=3, dilate_px=0, feather_px=0, guided_radius=0)
    with_tri = _run(img, tmp_path, trimap=str(trimap_path), output_name="prot", **common)
    without = _run(img, tmp_path, output_name="raw", **common)

    assert with_tri["edge_report"]["trimap_applied"] is True
    assert with_tri["edge_report"]["protected_band_px"] > 0
    assert without["edge_report"]["trimap_applied"] is False

    unknown = tri == 128
    prot = np.asarray(Image.open(with_tri["refined_mask"]).convert("L"), np.float32) / 255.0
    raw = np.asarray(Image.open(without["refined_mask"]).convert("L"), np.float32) / 255.0
    # In the unknown band the protected matte tracks the original soft alpha,
    # while the unprotected one is eroded away from it.
    assert np.abs(prot[unknown] - alpha[unknown]).mean() < np.abs(raw[unknown] - alpha[unknown]).mean()


def test_trimap_absent_leaves_report_flag_false(tmp_path: Path) -> None:
    img = _subject_rgba(tmp_path / "s.png")
    out = _run(img, tmp_path)
    assert out["edge_report"]["trimap_applied"] is False
    assert out["edge_report"]["protected_band_px"] == 0


# --------------------------------------------------------------------------- #
# engine seam (learned matter dispatch + telemetry)
# --------------------------------------------------------------------------- #
def _trimap_for(alpha: np.ndarray, path: Path) -> Path:
    tri = np.full(alpha.shape, 128, dtype=np.uint8)
    tri[alpha > 0.9] = 255
    tri[alpha < 0.1] = 0
    Image.fromarray(tri, "L").save(path)
    return path


def test_engine_defaults_to_cpu(tmp_path: Path) -> None:
    img = _subject_rgba(tmp_path / "s.png")
    report = _run(img, tmp_path)["edge_report"]
    assert report["engine"] == "cpu"
    assert report["engine_requested"] == "cpu"
    assert report["engine_fallback_reason"] is None
    assert report["backend_model"] is None


def test_learned_engine_falls_back_to_cpu_when_unavailable(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # No onnxruntime / weight on this box: the heuristic still runs and the
    # report explains why the learned matter was not used.
    monkeypatch.delenv("HGRIPE_MATTING_MODEL", raising=False)
    monkeypatch.setenv("HGRIPE_MODEL_CACHE", "/definitely/not/here")
    size = 48
    alpha = _disc_alpha(size, 16.0)
    img = _subject_rgba(tmp_path / "s.png", size=size, radius=16.0)
    trimap = _trimap_for(alpha, tmp_path / "trimap.png")
    report = _run(img, tmp_path, trimap=str(trimap), engine="onnx_matting")["edge_report"]
    assert report["engine"] == "cpu"
    assert report["engine_requested"] == "onnx_matting"
    assert report["engine_fallback_reason"]
    assert report["backend_model"] is None
    assert report["trimap_applied"] is True


def test_unknown_engine_falls_back_with_reason(tmp_path: Path) -> None:
    size = 48
    alpha = _disc_alpha(size, 16.0)
    img = _subject_rgba(tmp_path / "s.png", size=size, radius=16.0)
    trimap = _trimap_for(alpha, tmp_path / "trimap.png")
    report = _run(img, tmp_path, trimap=str(trimap), engine="bogus")["edge_report"]
    assert report["engine"] == "cpu"
    assert "unknown engine" in report["engine_fallback_reason"]


def test_learned_engine_skipped_without_trimap(tmp_path: Path) -> None:
    img = _subject_rgba(tmp_path / "s.png")
    report = _run(img, tmp_path, engine="onnx_matting")["edge_report"]
    assert report["engine"] == "cpu"
    assert "no trimap" in report["engine_fallback_reason"]


def _vitmatte_stack_ready() -> bool:
    """Whether the opt-in learned matter can genuinely run here.

    True only when ``onnxruntime`` imports *and* the trained weight is present,
    i.e. on the manual-dispatch real-inference CI lane (or a dev box that ran
    ``scripts/fetch-vitmatte``); the gated e2e test skips everywhere else.
    """
    try:
        import onnxruntime  # noqa: F401
    except Exception:
        return False
    from matting_backends.vitmatte_onnx import OnnxMattingBackend

    return OnnxMattingBackend().weight_path().is_file()


requires_vitmatte = pytest.mark.skipif(
    not _vitmatte_stack_ready(),
    reason="onnxruntime / matting weight not present (opt-in real-inference gate)",
)


@requires_vitmatte
def test_onnx_matting_real_inference_when_weight_present(tmp_path: Path) -> None:
    # The real trained ViTMatte weight, end-to-end through the CLI: no fakes,
    # no fallback. Runs only on the opt-in CI lane (or a dev box) with
    # onnxruntime + the fetched weight (HGRIPE_MATTING_MODEL); the engine must
    # actually bind, solve the unknown band and report itself truthfully.
    size = 48
    alpha = _disc_alpha(size, 16.0)
    img = _subject_rgba(tmp_path / "s.png", size=size, radius=16.0)
    trimap = _trimap_for(alpha, tmp_path / "trimap.png")
    out = _run(img, tmp_path, trimap=str(trimap), engine="onnx_matting")
    report = out["edge_report"]
    assert report["engine"] == "onnx_matting"
    assert report["engine_requested"] == "onnx_matting"
    assert report["engine_fallback_reason"] is None
    assert report["backend_model"]
    assert report["device"] in ("cpu", "cuda")
    assert report["trimap_applied"] is True
    refined = np.asarray(
        Image.open(out["refined_mask"]).convert("L"), dtype=np.float32
    ) / 255.0
    # A real matte: definite fg stays essentially opaque, definite bg stays
    # essentially transparent, and the unknown band got a continuous solve.
    assert refined[alpha > 0.95].mean() > 0.5
    assert refined[alpha < 0.05].mean() < 0.5


def test_probe_engines_flag_emits_capability_json(tmp_path: Path, capsys) -> None:
    rc = cli.main(["--image", "ignored", "--probe-engines"])
    assert rc == 0
    import json

    payload = json.loads(capsys.readouterr().out)
    assert payload["engines"]["cpu"]["available"] is True
    assert "onnx_matting" in payload["engines"]
    assert "model_cache_dir" in payload
