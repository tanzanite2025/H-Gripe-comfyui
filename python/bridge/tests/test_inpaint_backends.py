"""Unit tests for the Detail Repaint local inpaint seam (``inpaint_backends``).

These run with neither ``torch`` / ``diffusers`` nor an inpaint weight present
(as on CI and most dev boxes): the Stable Diffusion backend must report itself
*unavailable* rather than crash, and asking it to run anyway must raise
``InpaintUnavailable`` so the orchestrator falls back to the always-available
remote ``image.edit`` provider path. ``provider`` is never a registered local
backend -- it is the default and the fallback.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path
from typing import Any

import pytest

# ``inpaint_backends`` lives one directory up (``python/bridge``).
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import inpaint_backends as ib  # noqa: E402
from inpaint_backends import (  # noqa: E402
    InpaintUnavailable,
    known_engines,
    probe,
    resolve,
)
from inpaint_backends.flux_fill import FluxFillBackend  # noqa: E402
from inpaint_backends.sd_inpaint import StableDiffusionInpaintBackend  # noqa: E402
from inpaint_backends.sdxl_inpaint import StableDiffusionXLInpaintBackend  # noqa: E402

PIL = pytest.importorskip("PIL")
from PIL import Image  # noqa: E402


def test_resolve_provider_and_blank_return_none() -> None:
    # ``provider`` is the remote path the orchestrator owns, not a local
    # backend; the caller runs it directly.
    assert resolve("provider") is None
    assert resolve("") is None
    assert resolve(None) is None
    assert resolve("PROVIDER") is None  # case-insensitive


def test_resolve_unknown_engine_returns_none() -> None:
    # A stale / bogus engine name must not raise -- the caller records the
    # reason and emits an empty repaint set.
    assert resolve("does_not_exist") is None


@pytest.mark.parametrize("engine", ["sd_inpaint", "sdxl_inpaint", "flux_fill"])
def test_resolve_known_backend(engine: str) -> None:
    backend = resolve(engine)
    assert backend is not None
    assert backend.id == engine


def test_known_engines_lists_provider_first() -> None:
    engines = known_engines()
    assert engines[0] == "provider"
    assert "sd_inpaint" in engines
    assert "sdxl_inpaint" in engines
    assert "flux_fill" in engines


def test_probe_always_reports_provider_available() -> None:
    report = probe()
    assert report["engines"]["provider"]["available"] is True
    assert "sd_inpaint" in report["engines"]
    # The remote provider is not a local accelerator; the local engine is.
    assert report["engines"]["provider"]["accelerated"] is False
    for engine in ("sd_inpaint", "sdxl_inpaint", "flux_fill"):
        assert report["engines"][engine]["accelerated"] is True
    # Cached-weight inventory: the remote provider loads none; the local engine
    # names its (directory) weight.
    assert "weight" not in report["engines"]["provider"]
    weight = report["engines"]["sd_inpaint"]["weight"]
    assert weight["path"].endswith("sd-inpaint")
    assert isinstance(weight["present"], bool)
    assert "model_cache_dir" in report


_BACKENDS = [
    (StableDiffusionInpaintBackend, "HGRIPE_INPAINT_MODEL", "sd-inpaint"),
    (StableDiffusionXLInpaintBackend, "HGRIPE_SDXL_INPAINT_MODEL", "sdxl-inpaint"),
    (FluxFillBackend, "HGRIPE_FLUX_FILL_MODEL", "flux-fill"),
]


@pytest.mark.parametrize(("backend_cls", "env_var", "weight_dir"), _BACKENDS)
def test_backend_unavailable_without_weight(
    monkeypatch: pytest.MonkeyPatch, backend_cls: type, env_var: str, weight_dir: str
) -> None:
    # No weight on disk -> unavailable with a helpful reason, never a crash.
    monkeypatch.delenv(env_var, raising=False)
    monkeypatch.setenv("HGRIPE_MODEL_CACHE", "/definitely/not/here")
    backend = backend_cls()
    ok, reason = backend.available()
    assert ok is False
    assert reason  # non-empty explanation
    assert backend.weight_path().name == weight_dir


@pytest.mark.parametrize(("backend_cls", "env_var", "weight_dir"), _BACKENDS)
def test_backend_inpaint_raises_when_unavailable(
    monkeypatch: pytest.MonkeyPatch, backend_cls: type, env_var: str, weight_dir: str
) -> None:
    monkeypatch.delenv(env_var, raising=False)
    monkeypatch.setenv("HGRIPE_MODEL_CACHE", "/definitely/not/here")
    backend = backend_cls()
    crop = Image.new("RGB", (16, 16))
    mask = Image.new("L", (16, 16), 255)
    with pytest.raises(InpaintUnavailable):
        backend.inpaint(crop, mask, "restore")


@pytest.mark.parametrize(("backend_cls", "env_var", "weight_dir"), _BACKENDS)
def test_weight_path_prefers_env_override(
    monkeypatch: pytest.MonkeyPatch, backend_cls: type, env_var: str, weight_dir: str
) -> None:
    monkeypatch.setenv(env_var, "/models/my-inpaint")
    assert backend_cls().weight_path() == Path("/models/my-inpaint")


def test_controlnet_weight_path_prefers_env_override(monkeypatch: pytest.MonkeyPatch) -> None:
    from inpaint_backends.sd_inpaint import controlnet_weight_path

    monkeypatch.setenv("HGRIPE_CONTROLNET_MODEL", "/models/my-controlnet")
    assert controlnet_weight_path() == Path("/models/my-controlnet")
    monkeypatch.delenv("HGRIPE_CONTROLNET_MODEL", raising=False)
    monkeypatch.setenv("HGRIPE_MODEL_CACHE", "/cache")
    assert controlnet_weight_path().name == "controlnet-canny"


def test_sd_inpaint_controlnet_requires_weight(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    # With the SD weight present but the ControlNet weight missing, a canny
    # request must raise (degrading to the provider) rather than silently
    # dropping the conditioning. Deps are stubbed as importable via the SD
    # weight dir trick only if torch/diffusers exist; otherwise available()
    # already fails on deps -- both paths raise InpaintUnavailable.
    weight = tmp_path / "sd-inpaint"
    weight.mkdir()
    monkeypatch.setenv("HGRIPE_INPAINT_MODEL", str(weight))
    monkeypatch.delenv("HGRIPE_CONTROLNET_MODEL", raising=False)
    monkeypatch.setenv("HGRIPE_MODEL_CACHE", "/definitely/not/here")
    backend = StableDiffusionInpaintBackend()
    crop = Image.new("RGB", (16, 16))
    mask = Image.new("L", (16, 16), 255)
    with pytest.raises(InpaintUnavailable):
        backend.inpaint(crop, mask, "restore", controlnet="canny")


def test_sd_inpaint_rejects_unknown_controlnet(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    weight = tmp_path / "sd-inpaint"
    weight.mkdir()
    monkeypatch.setenv("HGRIPE_INPAINT_MODEL", str(weight))
    backend = StableDiffusionInpaintBackend()
    crop = Image.new("RGB", (16, 16))
    mask = Image.new("L", (16, 16), 255)
    with pytest.raises(InpaintUnavailable):
        backend.inpaint(crop, mask, "restore", controlnet="depth")


@pytest.mark.parametrize("backend_cls", [StableDiffusionXLInpaintBackend, FluxFillBackend])
def test_other_backends_reject_controlnet(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, backend_cls: type
) -> None:
    # SDXL / Flux Fill have no ControlNet path today: an explicit request must
    # raise (truthful provider fallback) instead of being silently ignored.
    weight = tmp_path / "weight"
    weight.mkdir()
    env_var = {
        StableDiffusionXLInpaintBackend: "HGRIPE_SDXL_INPAINT_MODEL",
        FluxFillBackend: "HGRIPE_FLUX_FILL_MODEL",
    }[backend_cls]
    monkeypatch.setenv(env_var, str(weight))
    backend = backend_cls()
    crop = Image.new("RGB", (16, 16))
    mask = Image.new("L", (16, 16), 255)
    with pytest.raises(InpaintUnavailable, match="controlnet not supported"):
        backend.inpaint(crop, mask, "restore", controlnet="canny")


def test_canny_condition_is_rgb_edge_map() -> None:
    np = pytest.importorskip("numpy")
    from inpaint_backends.sd_inpaint import canny_condition

    # A half-black / half-white image has one strong vertical edge.
    img = Image.new("RGB", (32, 32), (0, 0, 0))
    for y in range(32):
        for x in range(16, 32):
            img.putpixel((x, y), (255, 255, 255))
    cond = canny_condition(img)
    assert cond.mode == "RGB"
    assert cond.size == img.size
    arr = np.asarray(cond.convert("L"))
    # Edges only near the boundary column; flat areas stay black.
    assert arr[:, 14:18].max() == 255
    assert arr[:, :8].max() == 0
    assert arr[:, 24:].max() == 0


# ---- gated real diffusers inference (opt-in CI lane) ------------------------


def _diffusers_stack_importable() -> bool:
    try:
        import diffusers  # noqa: F401
        import torch  # noqa: F401
        import transformers  # noqa: F401
    except Exception:
        return False
    return True


requires_diffusers = pytest.mark.skipif(
    not _diffusers_stack_importable(),
    reason="torch / diffusers / transformers not importable (opt-in inpaint gate)",
)


def _run_repaint_e2e(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    *,
    engine: str,
    env_var: str,
    save_snapshot: Any,
) -> dict[str, Any]:
    """Drive the CLI ``repaint`` subcommand for real over a tiny snapshot.

    The real diffusers path, end-to-end: ``from_pretrained`` on a
    diffusers-format snapshot, the denoise loop, the VAE decode, and truthful
    device/precision telemetry. Runs only where torch + diffusers +
    transformers are installed (the opt-in lane).
    """
    import detail_repaint_cli as cli

    snapshot = tmp_path / engine.replace("_", "-")
    save_snapshot(snapshot)
    monkeypatch.setenv(env_var, str(snapshot))

    crop_path = tmp_path / "crop.png"
    Image.new("RGB", (64, 64), (150, 90, 40)).save(crop_path)
    manifest = {
        "regions": [
            {
                "index": 0,
                "type": "garbled_text",
                "crop_path": str(crop_path),
                "inner_box": [16, 16, 32, 32],
            }
        ]
    }

    args = cli.build_parser().parse_args(
        [
            "repaint",
            "--manifest",
            json.dumps(manifest),
            "--engine",
            engine,
            "--prompt",
            "fix text",
            "--steps",
            "2",
            "--seed",
            "1",
            "--output-dir",
            str(tmp_path / "out"),
        ]
    )
    result = cli.repaint(args)

    assert result["engine"] == engine
    assert result["engine_fallback_reason"] is None
    assert result["backend_model"] == engine.replace("_", "-")
    assert result["device"] in ("cpu", "cuda")
    assert result["precision"] in ("fp32", "fp16")
    repainted = result["repainted"]
    assert len(repainted) == 1
    out = Image.open(repainted[0]["path"])
    assert out.size == (64, 64)
    assert out.mode == "RGBA"
    return result


@requires_diffusers
def test_sd_inpaint_real_inference_with_tiny_snapshot(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from tiny_diffusers import save_tiny_sd_inpaint

    _run_repaint_e2e(
        tmp_path,
        monkeypatch,
        engine="sd_inpaint",
        env_var="HGRIPE_INPAINT_MODEL",
        save_snapshot=save_tiny_sd_inpaint,
    )


@requires_diffusers
def test_sdxl_inpaint_real_inference_with_tiny_snapshot(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from tiny_diffusers import save_tiny_sdxl_inpaint

    _run_repaint_e2e(
        tmp_path,
        monkeypatch,
        engine="sdxl_inpaint",
        env_var="HGRIPE_SDXL_INPAINT_MODEL",
        save_snapshot=save_tiny_sdxl_inpaint,
    )


@requires_diffusers
def test_flux_fill_real_inference_with_tiny_snapshot(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from tiny_diffusers import save_tiny_flux_fill

    _run_repaint_e2e(
        tmp_path,
        monkeypatch,
        engine="flux_fill",
        env_var="HGRIPE_FLUX_FILL_MODEL",
        save_snapshot=save_tiny_flux_fill,
    )


def test_probe_survives_a_broken_backend(monkeypatch: pytest.MonkeyPatch) -> None:
    # A backend whose available() explodes must be reported unavailable, not
    # crash the whole capability probe.
    class Boom:
        id = "boom"

        def available(self) -> tuple[bool, str]:
            raise RuntimeError("kaboom")

    monkeypatch.setattr(ib, "_registry", lambda: {"boom": Boom()})
    report = probe()
    assert report["engines"]["boom"]["available"] is False
    assert "kaboom" in report["engines"]["boom"]["reason"]
