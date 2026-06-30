# Auto-subject model weights

The Phase 2 auto-subject modes (`auto_subject` / `auto_product` / `auto_person`
/ `auto_transparent_object`) run a salient-object / dichotomous-segmentation
model in-process via ONNX Runtime on the `Compute` lane. Backends are tried in
priority order; the first whose weight resolves is used, otherwise the card
falls back to the deterministic `builtin-cpu` segmenter so the modes always
work.

When the request carries **point prompts** (the node's click-to-select), the
interactive **SAM 2** backend is preferred instead — it segments *what the user
clicked* rather than the most salient subject — falling through to the salient
cascade below when its weights are absent.

Segmentation answers *which pixels are the subject* (a hard, binary matte). When
the node's **Alpha matting** toggle is on, a separate **ViTMatte** backend then
resolves the binary edge into continuous alpha (hair / fur / glass) via a
trimap; absent its weight a deterministic `builtin-cpu-matte` guided filter
(image-guided, He et al.) resolves the band along real edges so the toggle
always works.

| Priority | Model | `provider` | License | Size | Tier |
| --- | --- | --- | --- | --- | --- |
| prompt | SAM 2 (tiny) | `sam2` | Apache-2.0 | ~154 MB | downloadable big tier (point-prompted) |
| `auto_person` 1 | U²-Net human-seg | `u2net_human_seg` | Apache-2.0 | ~168 MB | downloadable big tier (person-only) |
| 1 | BiRefNet (lite) | `birefnet` | MIT | ~224 MB | downloadable big tier |
| 2 | U²-Netp | `u2netp` | Apache-2.0 | ~4.6 MB | bundled default |
| — | builtin CPU | `builtin-cpu` | — | — | always-on fallback |

The `auto_person` mode leads its priority list with the human-segmentation net
(`u2net_human_seg`) so a person matte tracks people rather than generic
saliency, then falls through to BiRefNet → U²-Netp → builtin-cpu like the other
modes. Every other auto mode uses the generic priority unchanged.

### Alpha matting (continuous alpha, opt-in)

| Backend | `provider` | License | Size | Tier |
| --- | --- | --- | --- | --- |
| ViTMatte (small) | `vitmatte` | Apache-2.0 | ~104 MB | downloadable big tier |
| builtin guided-filter matte | `builtin-cpu-matte` | — | — | always-on fallback |

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
- **u2net_human_seg** is the `auto_person` *downloadable big tier* — not bundled
  by default (~168 MB). Place it here to bundle it for a release, or point
  `HGRIPE_PERSON_MODEL` at a local copy for dev; the `auto_person` mode prefers
  it (same U²-Net architecture / preprocessing as u2netp) and only that mode
  uses it.
- **sam2_tiny.encoder / sam2_tiny.decoder** are the interactive *downloadable
  big tier* (~154 MB combined) — not bundled by default. Place both here to
  bundle for a release, or point `HGRIPE_SAM2_ENCODER` / `HGRIPE_SAM2_DECODER`
  at local copies for dev; used only when the request carries point prompts.
- **vitmatte** is the continuous-alpha *downloadable big tier* (~104 MB) — not
  bundled by default. Place it here to bundle for a release, or point
  `HGRIPE_VITMATTE_MODEL` at a local copy for dev; used only when the node's
  **Alpha matting** toggle is on.

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

### U²-Net human-seg (downloadable big tier, `auto_person`)
- **License:** Apache-2.0 (https://github.com/xuebinqin/U-2-Net, rembg export)
- **Input:** RGB `1x3x320x320`, max-channel scaled + ImageNet-normalised (same
  preprocessing as U²-Netp)
- **Output:** `1x1x320x320` saliency map in roughly `[0, 1]`
- **sha256:** `01eb6a29a5c4d8edb30b56adad9bb3a2a0535338e480724a213e0acfd2d1c73c`

### ViTMatte small (downloadable big tier, continuous alpha)
- **License:** Apache-2.0 (https://huggingface.co/Xenova/vitmatte-small-distinctions-646)
- **Input:** a single `pixel_values` tensor `1x4xHxW` — RGB `1/255` rescaled +
  `0.5`/`0.5` normalised (`[-1, 1]`) with the trimap rescaled `1/255` as the
  4th channel. Run at a fixed `1024x1024` (multiple of 32) and the alpha resized
  back.
- **Output:** `alphas` `1x1xHxW` continuous alpha in `[0, 1]`.
- **sha256:** `a1cf48234c369faa3ea1711981d961fe1ec71f51e593f9d6553aa5a0e7d557e3`

### SAM 2 tiny (downloadable big tier, point-prompted)
- **License:** Apache-2.0 (https://huggingface.co/vietanhdev/segment-anything-2-onnx-models)
- **Two stages:** an image encoder run once + a light mask decoder.
- **Encoder** `sam2_tiny.encoder.onnx` — input RGB `1x3x1024x1024` (`1/255`
  rescaled + ImageNet-normalised); outputs `image_embed` `1x256x64x64` plus two
  high-resolution feature maps.
  - **sha256:** `4cc015ee18520e93f8c7ddfeaca7436039daaaaf19721b4b96a8810a805e82f7`
- **Decoder** `sam2_tiny.decoder.onnx` — inputs the embeddings + `point_coords`
  / `point_labels` (image space scaled into 1024) + a zeroed `mask_input`;
  outputs candidate `masks` + `iou_predictions`. The highest-IoU mask is kept,
  thresholded at logit `0`, and resized to the original image.
  - **sha256:** `f5a4bd656c143899fb7f52d64ed81e6f6aeb37d477a0b6da50146ac7cf2187bf`

## Manual fetch (dev)

```sh
# from the repo root
bash scripts/fetch-subject-model.sh   # u2netp  (or .ps1)
bash scripts/fetch-birefnet.sh        # birefnet (or .ps1)
bash scripts/fetch-person-model.sh    # u2net_human_seg / auto_person (or .ps1)
bash scripts/fetch-sam2.sh            # sam2 encoder + decoder (or .ps1)
bash scripts/fetch-vitmatte.sh        # vitmatte continuous-alpha (or .ps1)
```

Or point the segmenter at any local weight without bundling:

```sh
export HGRIPE_SUBJECT_MODEL=/path/to/u2netp.onnx
export HGRIPE_BIREFNET_MODEL=/path/to/birefnet_lite.onnx
export HGRIPE_PERSON_MODEL=/path/to/u2net_human_seg.onnx
export HGRIPE_SAM2_ENCODER=/path/to/sam2_tiny.encoder.onnx
export HGRIPE_SAM2_DECODER=/path/to/sam2_tiny.decoder.onnx
export HGRIPE_VITMATTE_MODEL=/path/to/vitmatte.onnx
export HGRIPE_WATCHDOG_MODEL=/path/to/watchdog_defect.onnx
```

### Detail Watchdog ML detector (opt-in, no weight shipped)

The Detail Watchdog node always runs its CPU rule layer; the `onnx_defect`
`engine` is an **opt-in** learned detector that graduates the `hands` / `text` /
`logo` watch targets (recorded as `skipped` by the rule layer) into real
findings. No trained weight is published yet, so the engine probes as
*unavailable* and the node falls back to the rule-only report until a blob is
provided.

- **Weight:** `watchdog_defect.onnx`, resolved from `HGRIPE_WATCHDOG_MODEL`
  (explicit path) or `<HGRIPE_MODEL_CACHE>/watchdog_defect.onnx` (else this
  bundled dir). Not committed and not bundled in the installer.
- **Contract:** input `[1,3,H,W]` float32 RGB `0..1` (letterboxed, aspect
  preserved); outputs `boxes` `[N,4]` `xyxy` / `scores` `[N]` / `labels` `[N]`
  (matched by name, else positional). An optional sidecar
  `watchdog_defect.onnx.labels.json` maps class ids to target names, e.g.
  `{"0": "hands", "1": "text", "2": "logo"}`.
- **Probe:** `python python/bridge/detail_watchdog_cli.py --probe-engines`
  reports which engines are usable right now (for the UI to grey out).

## Verify ViTMatte end-to-end

The matting backends are weight-resolution-driven, so the real ViTMatte path
only runs once its blob is present. The Rust test
`subject_matte::tests::vitmatte_inference_when_weight_present` runs the actual
`ort` inference (definite-FG core stays opaque, definite-BG corner transparent)
and **skips** when no weight resolves — so the default CI matrix never exercises
it. To run it:

```sh
bash scripts/fetch-vitmatte.sh           # into resources/models/vitmatte.onnx
cd apps/desktop-tauri/src-tauri
cargo test vitmatte_inference_when_weight_present -- --nocapture
```

In CI, trigger the opt-in **`tauri (vitmatte e2e)`** job (the CI workflow's
`workflow_dispatch`): it fetches the weight and runs exactly this test, keeping
the ~104 MB download off every PR run while still giving a verifiable path.
