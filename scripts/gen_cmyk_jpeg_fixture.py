#!/usr/bin/env python3
"""Regenerate the Adobe CMYK JPEG test fixture used by the Rust decode tests.

The fixture is a 32x32 CMYK JPEG written by Pillow as a 2x2 grid of flat ink
tiles. Pillow writes an APP14 Adobe marker (transform 0) and stores the ink
*inverted* (0 = full ink); ``studio/cmyk_decode.rs`` undoes that inversion so
the samples match TIFF Separated (0 = no ink) before the CMYK->sRGB transform.

Run from the repo root:

    python scripts/gen_cmyk_jpeg_fixture.py

Device inks (0 = no ink) per tile, and the sRGB Pillow's naive
``Image.convert("RGB")`` produces at each tile centre -- both frozen into the
Rust tests as the cross-language contract:

    top-left  (0,0,0,0)       -> (255, 255, 255)
    top-right (255,0,0,0)     -> (0, 255, 255)
    bot-left  (0,0,0,255)     -> (0, 0, 0)
    bot-right (128,64,32,16)  -> (119, 179, 209)
"""

from pathlib import Path

from PIL import Image

FIXTURE = Path("apps/desktop-tauri/src-tauri/tests/fixtures/cmyk_adobe_app14.jpg")

TILES = [
    (0, 0, 0, 0),        # top-left: no ink -> white
    (255, 0, 0, 0),      # top-right: full cyan
    (0, 0, 0, 255),      # bottom-left: full black
    (128, 64, 32, 16),   # bottom-right: mixed
]


def main() -> None:
    size = 32
    half = size // 2
    img = Image.new("CMYK", (size, size))
    px = img.load()
    for y in range(size):
        for x in range(size):
            tile = (0 if y < half else 2) + (0 if x < half else 1)
            px[x, y] = TILES[tile]

    FIXTURE.parent.mkdir(parents=True, exist_ok=True)
    # quality=100 + no chroma subsampling keeps the flat tiles near-lossless so
    # the cross-decoder (PIL/libjpeg vs zune) IDCT drift stays within tolerance.
    img.save(FIXTURE, format="JPEG", quality=100, subsampling=0)
    print(f"wrote {FIXTURE} ({FIXTURE.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
