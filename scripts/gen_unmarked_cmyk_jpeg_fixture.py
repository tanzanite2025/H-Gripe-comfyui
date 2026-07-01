#!/usr/bin/env python3
"""Regenerate the *unmarked* CMYK JPEG test fixture used by the Rust decode tests.

An "unmarked" CMYK JPEG is a 4-component (CMYK) JPEG that carries **no** Adobe
APP14 marker, so nothing in the stream declares the ink direction. libjpeg /
Pillow always writes the APP14 marker for CMYK, so we derive the fixture from the
committed Adobe CMYK fixture (`cmyk_adobe_app14.jpg`) by stripping its APP14
"Adobe" segment. The stored ink bytes are untouched -- only the marker is removed.

Pillow decodes such a file identically to the Adobe one: it inverts the stored
ink to the device direction (0 = no ink) *unconditionally*, regardless of the
marker's presence. `studio/cmyk_decode.rs` matches that -- an unmarked CMYK JPEG
is decoded exactly like Adobe CMYK (transform 0). Verified with Pillow 12.3:

    top-left  (0,0,0,0)       -> (255, 255, 255)
    top-right (255,0,0,0)     -> (0, 255, 255)
    bot-left  (0,0,0,255)     -> (0, 0, 0)
    bot-right (128,64,32,16)  -> (119, 179, 209)

Run from the repo root:

    python scripts/gen_unmarked_cmyk_jpeg_fixture.py
"""

from pathlib import Path

FIXTURES = Path("apps/desktop-tauri/src-tauri/tests/fixtures")
SRC = FIXTURES / "cmyk_adobe_app14.jpg"
DST = FIXTURES / "cmyk_unmarked.jpg"


def strip_app14_adobe(data: bytes) -> bytes:
    """Return `data` with any APP14 (0xFFEE) 'Adobe' marker segment removed."""
    if not data.startswith(b"\xff\xd8"):
        raise ValueError("source is not a JPEG (missing SOI)")
    out = bytearray(data[:2])  # SOI
    i, n = 2, len(data)
    while i + 1 < n:
        if data[i] != 0xFF:
            out.append(data[i])
            i += 1
            continue
        marker = data[i + 1]
        if marker == 0xFF:  # fill byte
            out.append(0xFF)
            i += 1
            continue
        # Standalone markers with no length payload.
        if marker == 0xD8 or marker == 0x01 or 0xD0 <= marker <= 0xD7:
            out += data[i : i + 2]
            i += 2
            continue
        if marker == 0xD9:  # EOI
            out += data[i : i + 2]
            i += 2
            continue
        if marker == 0xDA:  # SOS: entropy data follows, copy the rest verbatim
            out += data[i:]
            return bytes(out)
        seg_len = int.from_bytes(data[i + 2 : i + 4], "big")
        seg = data[i : i + 2 + seg_len]
        payload = data[i + 4 : i + 2 + seg_len]
        if not (marker == 0xEE and payload.startswith(b"Adobe")):
            out += seg  # keep every segment except the Adobe APP14 one
        i += 2 + seg_len
    return bytes(out)


def main() -> None:
    data = SRC.read_bytes()
    stripped = strip_app14_adobe(data)
    if len(stripped) >= len(data):
        raise SystemExit("no APP14 Adobe marker was found to strip")
    DST.write_bytes(stripped)
    print(f"wrote {DST} ({DST.stat().st_size} bytes, dropped {len(data) - len(stripped)})")


if __name__ == "__main__":
    main()
