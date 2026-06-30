# Auto-subject model weights

The Phase 2 auto-subject modes (`auto_subject` / `auto_product` / `auto_person`
/ `auto_transparent_object`) run a salient-object / dichotomous-segmentation
model in-process via ONNX Runtime on the `Compute` lane. Backends are tried in
priority order; the first whose weight resolves is used, otherwise the card
falls back to the deterministic `builtin-cpu` segmenter so the modes always
work.

| Priority | Model | `provider` | License | Size | Tier |
| --- | --- | --- | --- | --- | --- |
| 1 | BiRefNet (lite) | `birefnet` | MIT | ~224 MB | downloadable big tier |
| 2 | U²-Netp | `u2netp` | Apache-2.0 | ~4.6 MB | bundled default |
| — | builtin CPU | `builtin-cpu` | — | — | always-on fallback |

## Why the weights are not committed

The `.onnx` blobs are **not stored in git** (binary-free repo). They are fetched
by the scripts below into this directory; `bundle.resources` in
`tauri.conf.json` then ships whatever is present under
`<install>/resources/models/`.

- **u2netp** is the small *bundled default* — fetched at package time so it
  ships in the release for out-of-the-box auto modes.
- **birefnet_lite** is the *downloadable big tier* — not bundled by default
  (~224 MB). Place it here to bundle it for a release, or point
  `HGRIPE_BIREFNET_MODEL` at a local copy for dev; when present it is preferred
  over u2netp for higher-quality background removal.

## Models

### U²-Netp (bundled default)
- **License:** Apache-2.0 (https://github.com/xuebinqin/U-2-Net)
- **Input:** RGB `1x3x320x320`, max-channel scaled + ImageNet-normalised
- **Output:** `1x1x320x320` saliency map in roughly `[0, 1]`
- **sha256:** `309c8469258dda742793dce0ebea8e6dd393174f89934733ecc8b14c76f4ddd8`

### BiRefNet lite (downloadable big tier)
- **License:** MIT (https://github.com/ZhengPeng7/BiRefNet)
- **Input:** RGB `1x3x1024x1024`, `1/255` rescaled + ImageNet-normalised
- **Output:** `1x1x1024x1024` map (min-max normalised + thresholded)
- **sha256:** `5600024376f572a557870a5eb0afb1e5961636bef4e1e22132025467d0f03333`

## Manual fetch (dev)

```sh
# from the repo root
bash scripts/fetch-subject-model.sh   # u2netp  (or .ps1)
bash scripts/fetch-birefnet.sh        # birefnet (or .ps1)
```

Or point the segmenter at any local weight without bundling:

```sh
export HGRIPE_SUBJECT_MODEL=/path/to/u2netp.onnx
export HGRIPE_BIREFNET_MODEL=/path/to/birefnet_lite.onnx
```
