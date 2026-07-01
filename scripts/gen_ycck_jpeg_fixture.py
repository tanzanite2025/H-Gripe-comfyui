#!/usr/bin/env python3
"""Regenerate the Adobe YCCK JPEG test fixture used by the Rust decode tests.

The fixture is a 32x32 YCCK JPEG (Adobe APP14 transform 2) written by
``imagecodecs`` (Pillow cannot emit YCCK -- it only writes transform 0 for
CMYK). YCCK is YCbCr-encoded CMY plus a K plane, and Adobe stores the ink
*inverted* (0 = full ink); ``imagecodecs.jpeg8_encode`` takes the stored/inverted
CMYK, so we feed it ``255 - device`` to land on the intended device inks.

``studio/cmyk_decode.rs`` pins zune's output colourspace to YCCK to get the raw
Y/Cb/Cr/K planes (keeping the embedded ICC), reconstructs CMYK the way libjpeg's
``ycck_cmyk_convert`` does, then undoes the Adobe inversion so the samples match
TIFF Separated (0 = no ink) before the CMYK->sRGB transform.

Run from the repo root (needs ``pip install imagecodecs numpy``):

    python scripts/gen_ycck_jpeg_fixture.py

Device inks (0 = no ink) per tile, and the sRGB Pillow's naive
``Image.open(fixture).convert("RGB")`` produces at each tile centre -- both
frozen into the Rust tests as the cross-language contract:

    top-left  (0,0,0,0)       -> (255, 255, 255)
    top-right (255,0,0,0)     -> (1, 255, 255)
    bot-left  (0,0,0,255)     -> (0, 0, 0)
    bot-right (128,64,32,16)  -> (119, 180, 210)
"""

import io
from pathlib import Path

import imagecodecs
import numpy as np
from PIL import Image

FIXTURE = Path("apps/desktop-tauri/src-tauri/tests/fixtures/cmyk_ycck_app14.jpg")

# Device inks (0 = no ink), row-major 2x2 grid: TL, TR, BL, BR.
TILES = [
    (0, 0, 0, 0),        # top-left: no ink -> white
    (255, 0, 0, 0),      # top-right: full cyan
    (0, 0, 0, 255),      # bottom-left: full black
    (128, 64, 32, 16),   # bottom-right: mixed
]


def build_device_array(size: int) -> np.ndarray:
    half = size // 2
    arr = np.zeros((size, size, 4), np.uint8)
    for y in range(size):
        for x in range(size):
            tile = (0 if y < half else 2) + (0 if x < half else 1)
            arr[y, x] = TILES[tile]
    return arr


def main() -> None:
    size = 32
    arr = build_device_array(size)

    # imagecodecs stores the array in Adobe's inverted convention, so feed
    # 255 - device. level=100 + no chroma subsampling ('4:4:4') keeps the flat
    # tiles near-lossless so the cross-decoder (libjpeg vs zune) drift stays
    # within the tolerance the Rust tests use.
    enc = imagecodecs.jpeg8_encode(
        255 - arr,
        colorspace="cmyk",
        outcolorspace="ycck",
        level=100,
        subsampling="4:4:4",
    )

    FIXTURE.parent.mkdir(parents=True, exist_ok=True)
    FIXTURE.write_bytes(enc)
    transform = enc[enc.find(b"Adobe") + 11]
    print(f"wrote {FIXTURE} ({FIXTURE.stat().st_size} bytes, APP14 transform {transform})")

    # Echo the tile-centre references so the Rust test constants can be verified.
    im = Image.open(io.BytesIO(enc))
    cmyk = np.array(im)
    rgb = np.array(im.convert("RGB"))
    half = size // 2
    for i, dev in enumerate(TILES):
        cx = (half // 2) + (half if i % 2 else 0)
        cy = (half // 2) + (half if i >= 2 else 0)
        print(
            f"  tile centre ({cx},{cy}) device={dev} "
            f"PIL_CMYK={tuple(int(v) for v in cmyk[cy, cx])} "
            f"PIL_RGB={tuple(int(v) for v in rgb[cy, cx])}"
        )


if __name__ == "__main__":
    main()
