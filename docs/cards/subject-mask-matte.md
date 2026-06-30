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
| 2 | a model point-prompt (SAM-style): click points → model computes the mask — `ort` / `candle` in-process | `Compute` |

The UI ships "click → get a region" once; the backend is hot-swapped Phase 1 → 2
without a frontend rewrite.

### Mask-Edit tool registry (Phase 1)

| Tool | Status | Phase 1 behaviour |
| --- | --- | --- |
| `brush` (add) / `eraser` (subtract) | ready | Paint mask in / out. |
| `wand` (click-select) | ready | Flood fill a contiguous region by colour similarity. |
| `rect` / `ellipse` | ready | Marquee add/subtract. |
| `invert` | ready | Invert the whole mask. |
| `fill_holes` | ready | Close interior holes. |
| `smooth` | ready | Morphological open/close. |
| `grow` / `shrink` | ready | Dilate / erode by N px. |
| `feather` | ready | Gaussian-feather the mask edge. |
| `pen` (bezier path) | **planned** | Phase 3 — path rasterised + boolean-combined with the mask. |
| `lasso` | **planned** | Phase 3. |
| `matting` (continuous alpha) | **planned** | Phase 4 — hair / glass / translucency, trimap. |

`planned` tools render greyed ("coming soon"); this is what lets Phase 1 ship with
the morphology/brush set while pen/lasso/matting stay stubbed.

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
| `mode` | enum | `hybrid` | `auto_subject` \| `auto_product` \| `auto_person` \| `auto_transparent_object` \| `manual_brush` \| `manual_pen` \| `hybrid` | Phase 1 implements `manual_*` + `hybrid` (manual layer over an empty / `previous_mask` base); the `auto_*` model modes are Phase 2. |
| `wand_tolerance` | int | `24` | `0..255` | Colour distance for the magic-wand flood fill. |
| `feather_px` | float | `0.0` | `>= 0` | Edge feather applied last. |
| `grow_px` | int | `0` | any | Positive dilates, negative erodes. |
| `fill_holes` | bool | `false` | | Close interior holes before feather. |
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
`rust-native` in Phase 1 and the model id (e.g. `birefnet`) in Phase 2.

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
  ]
}
```

In Phase 1 `paths` (pen / lasso) are **stored but not rasterised** — the field is
versioned so a workflow saved now stays loadable once Phase 3 adds rasterisation.
`brush_strokes` and the morphology ops are applied.

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
| `auto_*` mode in Phase 1 | Falls back to an empty / `previous_mask` base for manual editing (no model yet). |
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
3. **Pen paths** — bezier rasterise, path add/subtract/intersect, re-editable.
4. **Alpha matting** — continuous alpha (hair / glass / translucency), trimap,
   tighter `Refine Mask Edge` hand-off.

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
- `exec.rs` — `subjectMask` maps to `Compute`; the `Compute` handler rejects
  foreign kinds (mirrors the existing `class_handlers_reject_foreign_kinds`).
- studio-ui — the shared Preview modal as a stage gate, the Mask-Edit tool
  registry (`ready` vs `planned`), and click-to-select (E2E).
