"""Linear-light resample goldens, mirroring the Rust engine's tests.

The Rust side (`studio/color/linear.rs`, `image_enhance_cpu.rs`) pins the
same values: a lossless TRC round-trip, the 188 photometric-midpoint golden,
and flat-colour byte-stability through a resize.
"""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

PIL = pytest.importorskip("PIL")
np = pytest.importorskip("numpy")
from PIL import Image  # noqa: E402

import image_enhance_cli  # noqa: E402
import linear_light  # noqa: E402


def test_trc_round_trips_all_256_codes():
    codes = np.arange(256, dtype=np.uint8)
    assert (linear_light.linear_to_srgb(linear_light.srgb_to_linear(codes)) == codes).all()


def test_linear_midpoint_encodes_to_188():
    assert int(linear_light.linear_to_srgb(np.float32(0.5))) == 188


def test_resample_averages_in_linear_light():
    # A 2x2 black/white checker downscaled to 1x1: gamma-space averaging gives
    # ~128; the photometric (linear-light) average encodes to 188.
    img = Image.new("RGB", (2, 2))
    img.putpixel((0, 0), (255, 255, 255))
    img.putpixel((1, 1), (255, 255, 255))
    out = image_enhance_cli._resample(img, 1, 1, downscaling=True)
    got = out.getpixel((0, 0))
    assert all(abs(c - 188) <= 1 for c in got), got


def test_resample_keeps_flat_colours_exact():
    img = Image.new("RGB", (3, 3), (120, 60, 30))
    out = image_enhance_cli._resample(img, 6, 6, downscaling=False)
    assert np.asarray(out).reshape(-1, 3).tolist() == [[120, 60, 30]] * 36


def test_alpha_resamples_on_the_gamma_free_track():
    # Alpha is coverage (already linear): a 2x2 checker matte box-downsamples
    # to the arithmetic mean, not the sRGB-encoded one.
    alpha = Image.new("L", (2, 2))
    alpha.putpixel((0, 0), 255)
    alpha.putpixel((1, 1), 255)
    out = image_enhance_cli._resample(alpha, 1, 1, downscaling=True)
    assert abs(out.getpixel((0, 0)) - 128) <= 1
