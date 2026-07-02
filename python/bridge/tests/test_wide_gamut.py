"""Cross-engine parity for the wide-gamut sRGB ingress (colour pipeline P5).

The fixture ``fixtures/prophoto16_gradient.png`` was written by the Rust
manual chain itself (``write_working_png`` on a ProPhoto ``WorkingImage``):
a 16-bit ProPhoto PNG with the moxcms-encoded ROMM profile embedded, whose
eight pixels are exactly the inputs of the Rust egress golden test
(``working_image.rs``). The parity test below pins Pillow/lcms ingress to the
same sRGB goldens moxcms produces, so the two engines cannot silently
disagree about what a manual product's pixels mean.
"""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import wide_gamut  # noqa: E402

PIL = pytest.importorskip("PIL")
np = pytest.importorskip("numpy")
from PIL import Image  # noqa: E402

FIXTURE = Path(__file__).resolve().parent / "fixtures" / "prophoto16_gradient.png"

# The Rust egress goldens (moxcms ProPhoto16 -> sRGB8, working_image.rs), in
# fixture pixel order. lcms and moxcms round independently; ±4 absorbs that
# while still catching a naive (unmanaged) read, which is 18+ codes off on
# mid-grey alone.
RUST_GOLDENS = [
    [0, 0, 0],
    [255, 255, 255],
    [146, 146, 146],
    [255, 0, 0],
    [0, 255, 0],
    [0, 0, 255],
    [236, 119, 30],
    [0, 186, 247],
]
TOLERANCE = 4


def test_prophoto_fixture_matches_rust_egress_goldens():
    img = Image.open(FIXTURE)
    img.load()
    out, converted = wide_gamut.managed_to_srgb(img)
    assert converted
    assert "icc_profile" not in out.info
    px = np.asarray(out.convert("RGB"), dtype=np.int64)[0]
    for got, want in zip(px.tolist(), RUST_GOLDENS):
        for g, w in zip(got, want):
            assert abs(g - w) <= TOLERANCE, f"got {got}, want {want}"


def test_naive_read_of_fixture_would_be_wrong():
    """Documents the bug the ingress fixes: unmanaged reads shift colour."""
    img = Image.open(FIXTURE)
    img.load()
    naive = np.asarray(img.convert("RGB"), dtype=np.int64)[0]
    grey = naive[2].tolist()
    assert grey != [146, 146, 146]  # ProPhoto numbers read as sRGB


def test_untagged_image_passes_through_untouched():
    img = Image.new("RGB", (2, 2), (10, 20, 30))
    out, converted = wide_gamut.managed_to_srgb(img)
    assert not converted
    assert out is img


def test_srgb_tagged_image_passes_through_untouched():
    from PIL import ImageCms

    icc = ImageCms.ImageCmsProfile(ImageCms.createProfile("sRGB")).tobytes()
    img = Image.new("RGBA", (2, 2), (10, 20, 30, 255))
    img.info["icc_profile"] = icc
    out, converted = wide_gamut.managed_to_srgb(img)
    assert not converted
    assert out is img


def test_edge_refine_loader_colour_manages_the_fixture():
    """The CLI loaders run the ingress: mid-grey lands on the managed value."""
    import edge_refine_cli as cli

    rgb, alpha, source_mode, _ = cli._load_rgb_alpha(str(FIXTURE))
    assert source_mode in ("RGB", "RGBA")
    assert abs(int(rgb[0, 2, 0]) - 146) <= TOLERANCE
    assert float(alpha[0, 0]) == 1.0
