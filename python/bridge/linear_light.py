"""sRGB transfer-curve (TRC) helpers for linear-light pixel maths.

The working space stays gamma-encoded (``docs/design/colour-pipeline.md``,
open decision 2): only operations whose maths assume light-linear values —
today, the enhance resample — decode to linear here, work in ``float32``, and
re-encode. Averaging gamma-encoded values under-weights bright pixels (a
black/white edge resamples to sRGB 128 instead of the photometrically
correct 188), which shows up as dark fringing on high-contrast edges.

The Rust engine mirrors these exact curves in ``studio/color/linear.rs``;
the goldens in both test suites pin the two engines to the same values.
"""

from __future__ import annotations

from typing import Any

_DECODE_LUT = None


def _decode_lut() -> Any:
    """The 256-entry sRGB-decode LUT (IEC 61966-2-1), built lazily."""
    global _DECODE_LUT
    if _DECODE_LUT is None:
        import numpy as np

        c = np.arange(256, dtype=np.float32) / 255.0
        _DECODE_LUT = np.where(
            c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4
        ).astype(np.float32)
    return _DECODE_LUT


def srgb_to_linear(arr_u8: Any) -> Any:
    """Decode an 8-bit sRGB array to linear light ``float32`` in ``0..1``."""
    return _decode_lut()[arr_u8]


def linear_to_srgb(arr_f32: Any) -> Any:
    """Encode linear light back to 8-bit sRGB (clamping to ``0..1``)."""
    import numpy as np

    l = np.clip(arr_f32, 0.0, 1.0)
    c = np.where(l <= 0.0031308, 12.92 * l, 1.055 * l ** (1.0 / 2.4) - 0.055)
    return (c * 255.0 + 0.5).astype(np.uint8)


def resize_rgb_linear(img: Any, out_w: int, out_h: int, resample: int) -> Any:
    """Resize an 8-bit ``RGB`` image with the filtering done in linear light.

    Each channel is decoded to a ``float32`` linear plane, resized as a PIL
    ``F`` image with the given ``resample`` filter, and re-encoded, so the
    filter averages photometric light instead of gamma codes.
    """
    import numpy as np
    from PIL import Image

    arr = np.asarray(img, dtype=np.uint8)
    linear = srgb_to_linear(arr)
    planes = []
    for ch in range(3):
        plane = Image.fromarray(linear[:, :, ch], mode="F")
        planes.append(np.asarray(plane.resize((out_w, out_h), resample)))
    out = linear_to_srgb(np.stack(planes, axis=-1))
    return Image.fromarray(out, mode="RGB")
