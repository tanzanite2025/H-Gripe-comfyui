"""Wide-gamut ingress shared by the bridge CLIs (colour pipeline P5).

The Rust manual chain writes its wide-gamut products as 16-bit **ProPhoto
RGB** PNG/TIFF with the profile embedded (``docs/design/colour-pipeline.md``).
Pillow opens those as 8-bit and, without help, the pixel maths downstream
would read the ProPhoto numbers *as if sRGB* — a large, silent colour shift
(mid-grey alone lands 18 codes off). This module is the Python half of the
egress contract: detect a ProPhoto-tagged RGB source and colour-manage it
into sRGB via the embedded profile, mirroring the Rust side's
``WorkingImage::to_srgb_rgba8``.

Only ProPhoto/ROMM tags are converted. Anything else — untagged, sRGB-tagged,
or foreign wide profiles — passes through byte-identical, preserving every
CLI's existing behaviour (the Rust loader is equally conservative: it only
rebuilds a ProPhoto surface for the profile its own outputs embed). CMYK is
untouched here; the CLIs already run its dedicated profile transform.
"""

from __future__ import annotations

import io
from typing import Any

_RGB_MODES = {"RGB", "RGBA"}


def _is_prophoto(profile: Any) -> bool:
    """Whether an ``ImageCmsProfile`` describes ProPhoto RGB (ROMM)."""
    from PIL import ImageCms

    try:
        desc = ImageCms.getProfileDescription(profile) or ""
    except Exception:  # noqa: BLE001 - a malformed profile is "not ProPhoto"
        return False
    lowered = desc.lower()
    return "prophoto" in lowered or "romm" in lowered


def managed_to_srgb(img: Any) -> tuple[Any, bool]:
    """Colour-manage a ProPhoto-tagged RGB/RGBA image into sRGB.

    Returns ``(image, converted)``. When ``converted`` is true the returned
    image holds sRGB pixels and the stale ``icc_profile`` has been dropped
    from ``info`` (the profile no longer describes the samples, and must not
    be re-embedded on any output). On any error, or for any source that is
    not a ProPhoto-tagged RGB/RGBA, the input is returned untouched so the
    established per-CLI behaviour stays byte-identical.
    """
    if img.mode not in _RGB_MODES:
        return img, False
    icc = img.info.get("icc_profile")
    if not icc:
        return img, False
    try:
        from PIL import ImageCms

        src = ImageCms.ImageCmsProfile(io.BytesIO(icc))
        if not _is_prophoto(src):
            return img, False
        dst = ImageCms.createProfile("sRGB")
        out = ImageCms.profileToProfile(img, src, dst, outputMode=img.mode)
        if out is None:
            return img, False
        out.info.pop("icc_profile", None)
        return out, True
    except Exception:  # noqa: BLE001 - never let colour management abort a card
        return img, False
