# Subject Mask / Matte Editor card

Backend: **native Rust** (in-process), not the `python/bridge`. This is the first
card whose image processing lives in Rust (`image` + `imageproc`), so it also
establishes the reusable `studio_image` hardening util that later Rust cards
share. Phase 2 segmentation / matting models also run in Rust (`ort` / `candle`),
so the card never splits across a Python boundary.

The PSD-first subject-selection card. It answers *where the subject is, what to
keep, which edges go semi-transparent, and what needs manual fixing* — and hands
a clean matte to `Refine Mask Edge` → `PSD Export`. It is **not** an edge-quality
card: selection lives here, edge cleanup / feather / fringe removal stays in
`Refine Mask Edge`. The two are never merged.

This document is the card's frozen contract. Phase 1 (manual + magic-wand, no
model) is specified in full; later phases are listed as planned extensions
behind the same ports.

## Backend / executor lane

The existing `StudioExecutor::Local` is defined structurally as *"always a
`python/bridge` CLI; must not touch the network"* (`exec.rs`). A native-Rust,
in-process card does **not** fit that lane — routing it through `Local` would
break the invariant that `Local` == python-bridge.

**Decision:** add a new executor class for in-process Rust compute and route this
card through it:

```
StudioExecutor::Graph    pure in-process graph node, no heavy work
StudioExecutor::Local    always a python/bridge CLI, no network   (existing 7 cards)
StudioExecutor::Compute  in-process Rust image/model work, no network   (NEW — subjectMask)
StudioExecutor::Api      provider call through the broker
StudioExecutor::Hybrid   graph + api
```

`Compute` is given **no broker / network handle**, exactly like `Local`, so the
security gate (`studio_executor_for_kind` → class handler) stays structural: a
`Compute` card can never make a provider call. Phase 2 local models (`ort` /
`candle`) run in-process and stay on `Compute`; only a *remote* segmentation API
would move the relevant mode to `Api` / `Hybrid`.

`studio_executor_for_kind("subjectMask")` returns `Compute`; `nodeSpecs.ts` gets
`executor: "compute"`.

## `studio_image` (new reusable util)

The Python cards share hardened loaders (`_load_rgba` / `_load_mask`). Rust cards
need the same guarantees, so this card introduces `studio_image` (a new module
under `src-tauri/src/studio/`) that every later Rust card reuses:

| Fn | Guarantee |
| --- | --- |
| `load_rgba(path, max_decode_pixels)` | Reject a decoded size over the budget **before** allocation (decompression-bomb guard); decode to 8-bit RGBA. |
| `load_mask(path, max_decode_pixels)` | Same guard; decode to 8-bit `L` (high-bit mattes tone-scaled, not clipped). |
| colour-space normalise | CMYK → sRGB (ICC when embedded), 16-bit / float → 8-bit, palette / grayscale → RGBA; record `source_mode`. |
| `apply_exif_orientation` | Normalise only a real, non-identity orientation; record `exif_transposed`. |

The `image` crate decodes most of this; CMYK-ICC and EXIF are added here so the
behaviour matches the Python loaders the other cards already use.

## Responsibility split (node / preview modal / mask-edit modal)

Two separate modals, deliberately not one:

| Layer | Owns | Notes |
| --- | --- | --- |
| **Node body** | thumbnail, `Auto Detect` / `Edit Mask` / `Apply`, lightweight **click-to-select** | Heavy canvas stays out of the body (it would fight the graph's LOD rendering + lazy-thumbnail media discipline). The node holds only the **result** (`mask` / `cutout` / `edit_paths`). |
| **Preview modal** (shared) | read-only review of the current image / mask / result at a stage | A **generic, reusable** component, not Subject-Mask-specific: it is a *review gate* you can drop after **any** stage to eyeball the output and decide whether to proceed. It exposes an `Edit` button that opens the mask-edit modal. |
| **Mask-Edit modal** (on-demand) | full canvas: brush / eraser / wand / feather, undo/redo, overlay + transparency preview | A separate heavier editor, opened **from** the preview's `Edit` button. The edit tool set is driven by a registry (below). It reads/writes the node's result and closes back to the preview. |

Keeping the preview generic (so it can sit at every stage) and isolating the
heavy pen/brush work in a separate on-demand editor is the key structural choice:
the review surface stays universal and cheap, the editor stays specialised.

### Click-to-select (node) — one interaction, two backends

| Phase | Click = | Lane |
| --- | --- | --- |
| 1 | magic-wand flood fill: select a contiguous region by colour similarity (`wand_tolerance`) — native Rust | `Compute` |
| 2 | a model point-prompt (SAM 2): click points → model computes the mask — `ort` in-process | `Compute` |

**Phase 2 point-prompt is wired (PR-4b).** The `Point (SAM 2)` tool records each
click into `edit_paths.points` (image-space `[x, y]`); when an `auto_*` mode runs
with points present, the backend routes to the SAM 2 segmenter ("segment what you
clicked"). No points ⇒ the prompt-free salient cascade (BiRefNet → U²-Netp →
`builtin-cpu`). The UI ships the same "click → region" interaction for both lanes.

The UI ships "click → get a region" once; the backend is hot-swapped Phase 1 → 2
without a frontend rewrite.

### Mask-Edit tool registry (Phase 1)

| Tool | Status | Phase 1 behaviour |
| --- | --- | --- |
| `brush` (add) / `eraser` (subtract) | ready | Paint mask in / out. |
| `point` (SAM 2 prompt) | ready | Record a positive point prompt into `edit_paths.points`; routes `auto_*` modes to the SAM 2 segmenter. |
| `wand` (click-select) | ready | Flood fill a contiguous region by colour similarity. |
| `rect` / `ellipse` | ready | Marquee add/subtract. |
| `invert` | ready | Invert the whole mask. |
| `fill_holes` | ready | Close interior holes. |
| `smooth` | ready | Morphological open/close. |
| `grow` / `shrink` | ready | Dilate / erode by N px. |
| `feather` | ready | Gaussian-feather the mask edge. |
| `pen` (bezier path) | **planned** | Phase 3 — path rasterised + boolean-combined with the mask. |
| `lasso` | **planned** | Phase 3. |
| `matting` (continuous alpha) | ready | Cascade 3/4 — a **paint** tool: stroke the trimap *unknown band* over hair / fur / glass; the backend resolves it into continuous alpha via a trimap (ViTMatte, else a builtin **guided-filter** matte). Recorded as `matte_strokes`. |

`planned` tools render greyed ("coming soon"); this is what lets Phase 1 ship with
the morphology/brush set while pen/lasso stay stubbed.

## Inputs (ports)

| Port | Type | Required | Notes |
| --- | --- | --- | --- |
| `image` | image path | yes | The image to cut the subject from. |
| `reference` | image path | no | Reference / target subject. |
| `visual_context` | object | no | `VisualContext` from `PSD Context Analyze`. |
| `placeholder_mask` | image path | no | PSD placeholder region; can constrain the subject extent. |
| `prompt` | text | no | e.g. `perfume bottle`, `main product`, `person` (used by Phase 2 models). |
| `previous_mask` | image path | no | Continue editing a prior mask. |
| `edit_paths` | object | no | Prior brush/path edits to re-apply (see schema). |

## Parameters (Phase 1)

| Param | Type | Default | Range / values | Notes |
| --- | --- | --- | --- | --- |
| `mode` | enum | `hybrid` | `auto_subject` \| `auto_product` \| `auto_person` \| `auto_transparent_object` \| `manual_brush` \| `manual_pen` \| `hybrid` | Phase 1 implements `manual_*` + `hybrid` (manual layer over an empty / `previous_mask` base). The `auto_*` model modes are Phase 2: with `edit_paths.points` they run SAM 2, otherwise the salient cascade. |
| `wand_tolerance` | int | `24` | `0..255` | Colour distance for the magic-wand flood fill. |
| `feather_px` | float | `0.0` | `>= 0` | Edge feather applied last. |
| `grow_px` | int | `0` | any | Positive dilates, negative erodes. |
| `fill_holes` | bool | `false` | | Close interior holes before feather. |
| `alpha_matting` | bool | `false` | | Resolve the binary edge into continuous alpha via a trimap (hair / glass). Runs **ViTMatte** when its weight resolves, else a deterministic `builtin-cpu-matte` guided-filter matte. Applied after morphology, before `feather_px`. Also runs automatically when `edit_paths.matte_strokes` is non-empty. |
| `matting_band_px` | int | `12` | `>= 0` | Width of the *auto* trimap *unknown* band the matter resolves; hand-painted `matte_strokes` add to it. Used whenever matting runs. |
| `output_dir` | path | run output dir | | Triplet written here. |
| `output_name` | basename | `<image>_mask` | plain basename | Rejected if it contains `..` or a path separator (`studio_reject_unsafe_basename`). |
| `max_decode_pixels` | int | `96_000_000` | `>= 0` (0 disables) | Rejects an **input** image / mask larger than this before decoding (decompression-bomb guard, via `studio_image`, aligned with the other cards). |

## Outputs

| Port | Type | Notes |
| --- | --- | --- |
| `mask` | grayscale PNG | The matte (`L`); binary or feathered. |
| `alpha_image` | RGBA PNG | The full image with the mask as alpha. |
| `cutout_image` | RGBA PNG | Subject cropped to its bbox; feeds `Refine Mask Edge`. |
| `matte_report` | object | Operations + provenance (see schema). |
| `edit_paths` | object | Pen/lasso/brush record for re-editing (see schema). |

### `SubjectMaskResult` (Rust struct, serde-serialised)

```json
{
  "mask_path": "",
  "alpha_image_path": "",
  "cutout_image_path": "",
  "edit_paths_path": "",
  "matte_report": {
    "mode": "hybrid",
    "provider": "rust-native",
    "source_mode": "RGB",
    "exif_transposed": false,
    "max_decode_pixels": 96000000,
    "image_size": [1200, 1600],
    "mask_coverage": 0.41,
    "detected_subjects": [
      { "label": "product", "confidence": 0.92, "bbox": [120, 80, 900, 1300] }
    ],
    "operations": [
      { "type": "wand", "tolerance": 24 },
      { "type": "brush_subtract", "radius": 18 },
      { "type": "fill_holes" },
      { "type": "alpha_matting", "provider": "vitmatte", "band_px": 12, "painted_strokes": 2 },
      { "type": "feather", "px": 2.5 }
    ],
    "triplet": { "mask": true, "alpha_image": true, "cutout_image": true },
    "processing_time_ms": 0
  }
}
```

`matte_report` follows the same enriched-report convention as the other cards:
`source_mode`, `exif_transposed`, `max_decode_pixels`, `image_size`,
`processing_time_ms`, and a `triplet` completeness flag. `provider` is
`rust-native` for the manual / hybrid lanes; for an `auto_*` base matte it is
the segmenter that ran, in priority order: `birefnet` (high-quality model),
`u2netp` (lightweight bundled model), or `builtin-cpu` when no model weight is
resolvable. When the request carries `edit_paths.points`, an `auto_*` mode
instead runs the interactive `sam2` point-prompt segmenter (the salient cascade
is the no-points path).

### `EditPaths`

```json
{
  "version": 1,
  "paths": [
    {
      "id": "path_1", "mode": "add", "tool": "pen", "closed": true,
      "points": [ { "x": 100, "y": 120, "in": [90, 110], "out": [110, 130] } ]
    }
  ],
  "brush_strokes": [
    { "id": "stroke_1", "mode": "subtract", "radius": 18, "points": [[100, 120], [105, 124]] }
  ],
  "matte_strokes": [
    { "id": "matte_1", "radius": 16, "points": [[300, 200], [312, 214]] }
  ],
  "operations": [ { "type": "feather", "amount": 2 } ],
  "points": [[420, 360], [690, 540]]
}
```

In Phase 1 `paths` (pen / lasso) are **stored but not rasterised** — the field is
versioned so a workflow saved now stays loadable once Phase 3 adds rasterisation.
`brush_strokes` and the morphology `operations` are applied. `matte_strokes` are
trimap *unknown-band* strokes painted by the **Matting** tool: the backend
stamps them as the unknown level on top of the auto `matting_band_px` ring, and
their presence runs matting even when the `alpha_matting` flag is off. `points`
are positive SAM 2 point prompts (image-space `[x, y]`) consumed by the `auto_*`
model lane (Phase 2); they are ignored by the manual lanes.

## Colour space & bit depth

The image is normalised to 8-bit RGBA via `studio_image::load_rgba` (and
`source_mode` recorded):

| Source mode | Handling |
| --- | --- |
| RGB / RGBA / L / LA | Used directly. |
| palette | Expanded to RGB(A); transparency treated as alpha. |
| CMYK | Converted to sRGB via the embedded ICC profile when present, else naive. |
| 16-bit / float | Tone-scaled to 8-bit before conversion. |

A `placeholder_mask` / `previous_mask` is read as 8-bit `L` via
`studio_image::load_mask` (high-bit mattes tone-scaled, not clipped) and used to
constrain / seed the subject.

## Boundary behaviour

| Condition | Behaviour |
| --- | --- |
| Missing / blank `image` | Card handler errors fast: `Subject Mask needs a connected image input`. |
| Missing file on disk | `<which> not found: <path>`. |
| Input larger than `max_decode_pixels` | `input image too large to decode safely: <path> WxH ...` (before allocation). |
| Empty selection after edits | Emits a fully-transparent mask + `mask_coverage: 0.0`; never panics. |
| EXIF-rotated image | Orientation normalised; `exif_transposed: true`. |
| `auto_*` mode (BiRefNet weight resolvable) | The BiRefNet ONNX model produces the base matte in-process (`provider: birefnet`) — highest quality, preferred when present. A `previous_mask`, if connected, still takes precedence as continuation. |
| `auto_*` mode (only U²-Netp weight resolvable) | The lightweight bundled U²-Netp model produces the base matte (`provider: u2netp`). |
| `auto_*` mode (no model weight resolvable) | Falls back to a deterministic built-in CPU segmenter (border-background distance + largest connected component, point-prompt aware); `provider: builtin-cpu`. |
| Unsafe `output_name` | Rejected via `studio_reject_unsafe_basename`. |

## Determinism

Phase 1 is deterministic: identical `image` + params + `edit_paths` produce the
same triplet. Phase 2 model modes may not be, and will report their `provider` /
model id in `matte_report`.

## Phases

1. **Manual usable** (this contract): native-Rust magic-wand + brush/eraser +
   morphology + feather, triplet output, `edit_paths` stored. Ships into the real
   PSD chain.
2. **Auto subject** — SAM / RMBG / BiRefNet in-process via `ort` / `candle` on the
   `Compute` lane; the node's click-to-select becomes a model point-prompt. A
   *remote* segmentation API instead moves that mode to `Api` / `Hybrid`.
   - *Landed:* a `SubjectSegmenter` trait routes the four `auto_*` modes through
     the `Compute` lane, behind a shared `ModelSpec`-driven `ort` backend so
     multiple models share one load + inference path. Backends are tried in
     priority order — when the request carries point prompts, **SAM 2**
     (`provider: sam2`, interactive); otherwise **BiRefNet**
     (`provider: birefnet`, high quality) → **U²-Netp** (`provider: u2netp`,
     lightweight default) → deterministic **`builtin-cpu`** fallback when no
     weight resolves — so the modes always work. `matte_report` carries
     `provider` and `detected_subjects` (`label` / `bbox` / `coverage`).
   - *Interactive (SAM 2):* a two-stage **SAM 2 tiny** backend (encoder +
     prompt decoder, Apache-2.0, ~154 MB combined) implements the same trait
     (`provider: sam2`). `segmenter_for_mode(mode, points)` is point-aware: a
     non-empty `edit_paths.points` routes to SAM 2, otherwise the salient
     cascade runs. The frontend `Point (SAM 2)` tool records those clicks into
     `edit_paths.points` (PR-4b), so the node's click-to-select drives the model.
   - *Weight sourcing:* no `.onnx` is committed to git. **u2netp** (Apache-2.0,
     ~4.6 MB) is the *bundled default* — fetched at package time
     (`scripts/fetch-subject-model.*`) and shipped via `tauri.conf.json`
     `bundle.resources` under `<install>/resources/models/`. **BiRefNet lite**
     (MIT, ~224 MB), **SAM 2 tiny** (Apache-2.0) and **ViTMatte small**
     (Apache-2.0, ~104 MB) are the *downloadable big tier* — not bundled by
     default; `scripts/fetch-birefnet.*` / `scripts/fetch-sam2.*` /
     `scripts/fetch-vitmatte.*` place them in the same dir to ship or test with.
     `HGRIPE_SUBJECT_MODEL` / `HGRIPE_BIREFNET_MODEL` / `HGRIPE_SAM2_ENCODER` /
     `HGRIPE_SAM2_DECODER` / `HGRIPE_VITMATTE_MODEL` env vars override the paths
     for dev / tests.
   - *Pending:* a portrait-matting net for `auto_person` can slot into
     `segmenter_for_mode` behind the same trait.
3. **Pen paths** — bezier rasterise, path add/subtract/intersect, re-editable.
4. **Alpha matting** — continuous alpha (hair / glass / translucency), trimap,
   tighter `Refine Mask Edge` hand-off.
   - *Landed (cascade 3, backend):* the `alpha_matting` param derives a trimap
     from the binary matte (`trimap_from_mask`: erode → FG core, dilate → BG
     exterior, the `matting_band_px` ring between → unknown) and resolves it
     through an `AlphaMatter`. **ViTMatte small** (`provider: vitmatte`,
     Apache-2.0, ~104 MB, single 4-channel `pixel_values` = RGB + trimap)
     runs in-process via `ort` when its weight resolves; otherwise a
     deterministic `builtin-cpu-matte` **guided filter** (He et al., image-guided)
     resolves the unknown band along real edges, so the toggle always works
     without the weight. The op is recorded in `matte_report.operations` and
     the soft matte hands off to `Refine Mask Edge`. The real ViTMatte path is
     covered by a weight-gated test (`vitmatte_inference_when_weight_present`)
     that skips when no blob resolves; the opt-in `tauri (vitmatte e2e)` CI job
     (`workflow_dispatch`) fetches the weight and runs it. See
     `resources/models/README.md` → *Verify ViTMatte end-to-end*.
   - *Landed (cascade 4, UI):* a dedicated `matting` paint tool in the Mask-Edit
     modal records `matte_strokes` (per-region trimap-unknown painting); the
     backend stamps them onto the trimap before matting (`parse_matte_strokes`).
   - *Pending:* a trimap-aware hair refine path.

## Backend boundary

```
React UI         -> node preview + shared Preview modal + on-demand Mask-Edit modal (brush/pen/undo/redo)
Rust / Tauri     -> studio_image (decode guard + colour-space), morphology / wand / feather,
                    Phase 2 model inference (ort/candle), file IO, path validation
Refine Mask Edge -> receives mask / cutout, owns edge fusion
```

## Tests

- `src-tauri/src/studio/subject_mask.rs` — *(added with the Phase 1
  implementation)* `#[cfg(test)]` unit tests: rejects missing/blank image, param
  defaults, wand flood-fill, brush apply, morphology + feather, empty-selection
  fallback, report/triplet shape.
- `src-tauri/src/studio/studio_image.rs` — decode-guard rejection, colour-space
  normalisation, EXIF orientation.
- `src-tauri/src/studio/subject_matte.rs` — trimap derivation (FG / unknown /
  BG levels, zero-band pass-through), ViTMatte pre/post-processing
  (4-channel pack, `[-1, 1]` RGB + rescaled trimap, clamp + resize-back), the
  deterministic `builtin-cpu-matte` guided-filter matte (including an
  image-edge-following assertion), and a weight-gated ViTMatte inference smoke
  test (skipped when the blob is absent).
- `exec.rs` — `subjectMask` maps to `Compute`; the `Compute` handler rejects
  foreign kinds (mirrors the existing `class_handlers_reject_foreign_kinds`).
- studio-ui — the shared Preview modal as a stage gate, the Mask-Edit tool
  registry (`ready` vs `planned`, incl. the `point` SAM 2 tool), the edit-state
  model (`maskEdit` brush / op / **point** record + undo/redo), and
  click-to-select (E2E).
