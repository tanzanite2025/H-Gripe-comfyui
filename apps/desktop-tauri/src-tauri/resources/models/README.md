# Auto-subject model weights

The Phase 2 auto-subject modes (`auto_subject` / `auto_product` / `auto_person`
/ `auto_transparent_object`) run a U²-Netp salient-object model in-process via
ONNX Runtime on the `Compute` lane. When no weight is present the card falls
back to the deterministic `builtin-cpu` segmenter, so the modes always work.

## Why the weight is not committed

`u2netp.onnx` (~4.6 MB) is **fetched at package time**, not stored in git, to
keep the repository binary-free. Release packaging runs
`scripts/fetch-subject-model.(ps1|sh)`, which downloads the weight into this
directory; `bundle.resources` in `tauri.conf.json` then ships it under
`<install>/resources/models/u2netp.onnx`.

## Model

- **Name:** U²-Netp (small U²-Net variant)
- **License:** Apache-2.0 (https://github.com/xuebinqin/U-2-Net)
- **Input:** RGB `1x3x320x320`, ImageNet-normalised
- **Output:** `1x1x320x320` saliency map in roughly `[0, 1]`
- **sha256:** `309c8469258dda742793dce0ebea8e6dd393174f89934733ecc8b14c76f4ddd8`

## Manual fetch (dev)

```sh
# from apps/desktop-tauri/src-tauri
bash ../../../scripts/fetch-subject-model.sh        # or
pwsh ../../../scripts/fetch-subject-model.ps1
```

Or point the segmenter at any local weight without bundling:

```sh
export HGRIPE_SUBJECT_MODEL=/path/to/u2netp.onnx
```
