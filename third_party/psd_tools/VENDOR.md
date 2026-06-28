# Vendored `psd_tools`

This directory contains a vendored copy of [`psd-tools`](https://github.com/psd-tools/psd-tools),
checked into the repository so the H-Gripe PSD nodes depend on a copy we control
rather than on whatever version PyPI resolves at install time.

| | |
| --- | --- |
| Upstream | https://github.com/psd-tools/psd-tools |
| Version  | 1.17.2 |
| License  | MIT (see `LICENSE`) |

## Why vendored

The PSD production nodes (`custom_nodes/hgripe_psd_nodes.py`) read and write
multi-layer PSDs and need features we extend ourselves (e.g. smart-object
content replacement). Vendoring decouples us from upstream release churn and
lets us patch/extend the library directly. Upgrades are deliberate: replace the
contents of this directory with a newer release and re-run the PSD example.

## Local modifications

Changes we make on top of the upstream 1.17.2 source are listed here so they can
be re-applied when upgrading:

- **Smart-object content replacement** (`api/smart_object.py`, `api/layers.py`):
  - `SmartObject.replace_contents(data, filetype=None)` — swaps the embedded
    bytes of a `kind == 'data'` smart object in place (UUID/transform/warp kept).
  - `SmartObjectLayer.replace_with_image(image, compression=RLE)` — encodes a PIL
    image as PNG into the embedded source *and* refreshes the layer's cached
    raster (preserving the SO tagged blocks), then calls `PSDImage._update_record`
    so the new pixels serialize on `save`. Used by `H-Gripe PSD Compose`'s
    `smart_object_mode="replace_content"`.

## Notes

- The compiled Cython RLE codec (`compression/_rle*`) is intentionally **not**
  vendored. `compression/__init__.py` already falls back to the pure-Python
  `compression/rle.py` when the extension is missing, so the vendored copy is
  cross-platform with no build step (slower RLE only).
- Runtime dependencies (`attrs`, `typing-extensions`, `Pillow`, `numpy`) are
  declared in the repository `requirements.txt`. The optional `composite` extras
  (`aggdraw`, `scipy`, `scikit-image`) are not required for the PSD nodes.
