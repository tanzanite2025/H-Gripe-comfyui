"""Unit tests for the Detail Repaint local inpaint seam (``inpaint_backends``).

Most of these run with neither ``torch`` nor ``diffusers`` nor a model weight
present (as on CI and most dev boxes): the SD inpaint backend must report itself
*unavailable* rather than crash, and asking the ``detail_repaint_cli.py inpaint``
subcommand to run it anyway must degrade to the provider / passthrough path with
the reason recorded -- never a hard failure.

The end-to-end dispatch is exercised with a synthetic in-process backend (a plain
fill), so no real diffusion weights are needed. A real-inference path behind
``torch`` + ``diffusers`` is opt-in like the ViTMatte / Real-ESRGAN e2e gates and
is not run here.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import numpy as np
import pytest

# ``inpaint_backends`` / the CLI live one directory up (``python/bridge``).
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import detail_repaint_cli as cli  # noqa: E402
import inpaint_backends as ib  # noqa: E402
from inpaint_backends import (  # noqa: E402
    InpaintUnavailable,
    known_engines,
    probe,
    resolve,
)
from inpaint_backends.sd_inpaint import StableDiffusionInpaintBackend  # noqa: E402

PIL = pytest.importorskip("PIL")
from PIL import Image  # noqa: E402


def test_resolve_provider_and_blank_return_none() -> None:
    # The provider path is not a registered backend; the orchestrator runs it.
    assert resolve("provider") is None
    assert resolve("") is None
    assert resolve(None) is None
    assert resolve("PROVIDER") is None  # case-insensitive


def test_resolve_unknown_engine_returns_none() -> None:
    # A stale / bogus engine name must not raise -- the caller records the reason
    # and keeps the provider / passthrough path.
    assert resolve("does_not_exist") is None


def test_resolve_known_backend() -> None:
    backend = resolve("sd_inpaint")
    assert backend is not None
    assert backend.id == "sd_inpaint"


def test_known_engines_lists_provider_first() -> None:
    engines = known_engines()
    assert engines[0] == "provider"
    assert "sd_inpaint" in engines


def test_probe_always_reports_provider_available() -> None:
    report = probe()
    assert report["engines"]["provider"]["available"] is True
    assert "sd_inpaint" in report["engines"]
    assert "model_cache_dir" in report


def test_sd_inpaint_unavailable_without_weight(monkeypatch: pytest.MonkeyPatch) -> None:
    # No weight on disk -> unavailable with a helpful reason, never a crash. (If
    # torch/diffusers are absent the dep message comes first -- both are fine.)
    monkeypatch.delenv("HGRIPE_INPAINT_MODEL", raising=False)
    monkeypatch.setenv("HGRIPE_MODEL_CACHE", "/definitely/not/here")
    backend = StableDiffusionInpaintBackend()
    ok, reason = backend.available()
    assert ok is False
    assert reason  # non-empty explanation


def test_sd_inpaint_inpaint_raises_when_unavailable(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.delenv("HGRIPE_INPAINT_MODEL", raising=False)
    monkeypatch.setenv("HGRIPE_MODEL_CACHE", "/definitely/not/here")
    backend = StableDiffusionInpaintBackend()
    rgb = np.zeros((16, 16, 3), dtype=np.uint8)
    mask = np.zeros((16, 16), dtype=np.uint8)
    with pytest.raises(InpaintUnavailable):
        backend.inpaint(rgb, mask, "restore")


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


# --- CLI dispatch: provider default + graceful fallback -------------------


def _manifest(crop_path: str) -> str:
    return json.dumps(
        {
            "regions": [
                {
                    "index": 0,
                    "type": "malformed_hands",
                    "crop_path": crop_path,
                    "inner_box": [4, 4, 12, 12],
                    "size": [16, 16],
                }
            ]
        }
    )


def test_inpaint_provider_default_is_passthrough(tmp_path: Path) -> None:
    args = cli.build_parser().parse_args(
        ["inpaint", "--manifest", _manifest("x.png"), "--output-dir", str(tmp_path)]
    )
    result = cli.inpaint(args)
    assert result["engine"] == "provider"
    assert result["engine_requested"] == "provider"
    assert result["repainted"] == []
    assert result["engine_fallback_reason"] == "provider engine (no local backend)"
    assert result["requested_count"] == 1


def test_inpaint_unknown_engine_falls_back(tmp_path: Path) -> None:
    args = cli.build_parser().parse_args(
        ["inpaint", "--engine", "bogus", "--manifest", _manifest("x.png"),
         "--output-dir", str(tmp_path)]
    )
    result = cli.inpaint(args)
    assert result["engine"] == "provider"
    assert result["engine_requested"] == "bogus"
    assert "unknown engine" in result["engine_fallback_reason"]


def test_inpaint_unavailable_engine_falls_back(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.delenv("HGRIPE_INPAINT_MODEL", raising=False)
    monkeypatch.setenv("HGRIPE_MODEL_CACHE", str(tmp_path / "empty"))
    args = cli.build_parser().parse_args(
        ["inpaint", "--engine", "sd_inpaint", "--manifest", _manifest("x.png"),
         "--output-dir", str(tmp_path)]
    )
    result = cli.inpaint(args)
    assert result["engine"] == "provider"
    assert result["engine_requested"] == "sd_inpaint"
    assert result["engine_fallback_reason"]  # records why it degraded
    assert result["repainted"] == []


def test_inpaint_dispatch_with_synthetic_backend(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # A fake in-process backend exercises the full dispatch + crop I/O without
    # any diffusion weights: it must paint only the masked core, write a crop
    # PNG, and emit the {index, path} shape composite reads.
    seen: dict[str, object] = {}

    class FakeBackend:
        id = "fake_inpaint"

        def available(self) -> tuple[bool, str]:
            return True, "ready"

        def backend_model(self) -> str:
            return "fake.ckpt"

        def inpaint(self, rgb, mask, prompt, **options):  # noqa: ANN001
            seen["prompt"] = prompt
            seen["options"] = options
            out = rgb.copy()
            out[mask > 0] = (255, 0, 0)
            return out

    monkeypatch.setattr(ib, "_registry", lambda: {"fake_inpaint": FakeBackend()})

    crop = tmp_path / "crop.png"
    Image.fromarray(np.zeros((16, 16, 3), dtype=np.uint8), "RGB").save(crop)

    args = cli.build_parser().parse_args(
        [
            "inpaint",
            "--engine",
            "fake_inpaint",
            "--manifest",
            _manifest(str(crop)),
            "--repaint-prompt-base",
            "make it clean",
            "--seed",
            "7",
            "--output-dir",
            str(tmp_path),
        ]
    )
    result = cli.inpaint(args)

    assert result["engine"] == "fake_inpaint"
    assert result["backend_model"] == "fake.ckpt"
    assert result["repainted_count"] == 1
    entry = result["repainted"][0]
    assert entry["index"] == 0
    out = np.asarray(Image.open(entry["path"]).convert("RGB"))
    # The masked core is repainted red; the surrounding context stays black.
    assert tuple(out[8, 8]) == (255, 0, 0)
    assert tuple(out[0, 0]) == (0, 0, 0)
    # Prompt + seed flowed through to the backend.
    assert seen["prompt"] == "make it clean (issue: malformed_hands)"
    assert seen["options"]["seed"] == 7


def test_inpaint_result_feeds_composite(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # The inpaint output plugs straight into the composite step (same contract
    # as the provider path): the repainted core lands back in the candidate.
    class FakeBackend:
        id = "fake_inpaint"

        def available(self) -> tuple[bool, str]:
            return True, "ready"

        def backend_model(self) -> str:
            return "fake.ckpt"

        def inpaint(self, rgb, mask, prompt, **options):  # noqa: ANN001
            out = rgb.copy()
            out[mask > 0] = (0, 255, 0)
            return out

    monkeypatch.setattr(ib, "_registry", lambda: {"fake_inpaint": FakeBackend()})

    candidate = tmp_path / "candidate.png"
    Image.fromarray(np.zeros((40, 40, 3), dtype=np.uint8), "RGB").convert("RGBA").save(
        candidate
    )

    prep_args = cli.build_parser().parse_args(
        [
            "prepare",
            "--image",
            str(candidate),
            "--quality-report",
            json.dumps(
                {
                    "issues": [
                        {
                            "type": "malformed_hands",
                            "confidence": 0.9,
                            "bbox": [10, 10, 30, 30],
                            "suggested_action": "detail_redraw",
                        }
                    ]
                }
            ),
            "--output-dir",
            str(tmp_path),
        ]
    )
    manifest = cli.prepare(prep_args)
    assert manifest["regions"]

    inpaint_args = cli.build_parser().parse_args(
        ["inpaint", "--engine", "fake_inpaint", "--manifest", json.dumps(manifest),
         "--output-dir", str(tmp_path)]
    )
    inpainted = cli.inpaint(inpaint_args)
    assert inpainted["repainted_count"] == 1

    comp_args = cli.build_parser().parse_args(
        [
            "composite",
            "--image",
            str(candidate),
            "--manifest",
            json.dumps(manifest),
            "--repainted",
            json.dumps(inpainted["repainted"]),
            "--output-dir",
            str(tmp_path),
        ]
    )
    composed = cli.composite(comp_args)
    assert composed["repaint_report"]["status"] == "repainted"
    fixed = np.asarray(Image.open(composed["fixed_image"]).convert("RGB"))
    # The centre of the issue core is green; a corner well outside is untouched.
    assert fixed[20, 20][1] > 200
    assert tuple(fixed[0, 0]) == (0, 0, 0)


def test_probe_engines_cli_mode(capsys: pytest.CaptureFixture[str]) -> None:
    rc = cli.main(["--probe-engines"])
    assert rc == 0
    payload = json.loads(capsys.readouterr().out)
    assert payload["engines"]["provider"]["available"] is True
    assert "sd_inpaint" in payload["engines"]
