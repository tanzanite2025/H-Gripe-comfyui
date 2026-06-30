# Subject Mask / Matte Editor card

Executor: **local** in Phase 1 (always `python/bridge/subject_mask_cli.py`, never
networks). Phase 2 segmentation providers move to **api** / **hybrid** behind the
same contract — the card kind and its I/O do not change.

The PSD-first subject-selection card. It answers *where the subject is, what to
keep, which edges go semi-transparent, and what needs manual fixing* — and hands
a clean matte to `Refine Mask Edge` → `PSD Export`. It is **not** an edge-quality
card: selection lives here, edge cleanup / feather / fringe removal stays in
`Refine Mask Edge`. The two are never merged.

This document is the card's frozen contract. Phase 1 (manual + magic-wand, no
model) is specified in full; later phases are listed as planned extensions
behind the same ports.

## Responsibility split (node / shared modal / tool registry)

The card is a **lightweight node + a shared preview-editor modal**, deliberately
split so the node body never carries a heavy canvas (it would fight the graph's
LOD rendering + lazy-thumbnail "media discipline"):

| Layer | Owns | Notes |
| --- | --- | --- |
| **Node body** | thumbnail preview, `Auto Detect` / `Edit Mask` / `Apply`, and a lightweight **click-to-select** | Clicking the preview selects one region (Phase 1: magic-wand flood fill; Phase 2: a model point-prompt). The node holds only the **result** (`mask` / `cutout` / `edit_paths`), not the editor. |
| **Shared modal** (`Edit Mask`) | full canvas: brush / eraser / wand / feather, undo/redo, overlay + transparency preview | A **reusable** preview-editor component, not Subject-Mask-specific: `Refine Mask Edge` / `Detail Repaint` can open the same modal with a different tool set. It reads/writes the node's result and closes back to the node. |
| **Tool registry** | the list of edit tools and each tool's status | Each tool is registered `ready` (rendered, usable) or `planned` (greyed, "coming soon"). This is what lets Phase 1 ship some tools while stubbing the rest — see the table below. |

### Tool registry (Phase 1)

| Tool | Status | Phase 1 behaviour |
| --- | --- | --- |
| `brush` (add) | ready | Paint mask in. |
| `eraser` (subtract) | ready | Paint mask out. |
| `wand` (click-select) | ready | Flood fill a contiguous region by colour similarity (`tolerance`). |
| `rect` / `ellipse` | ready | Marquee add/subtract. |
| `invert` | ready | Invert the whole mask. |
| `fill_holes` | ready | Close interior holes. |
| `smooth` | ready | Morphological open/close. |
| `grow` / `shrink` | ready | Dilate / erode by N px. |
| `feather` | ready | Gaussian-feather the mask edge. |
| `pen` (bezier path) | **planned** | Phase 3 — path rasterised + boolean-combined with the mask. |
| `lasso` | **planned** | Phase 3. |
| `matting` (continuous alpha) | **planned** | Phase 4 — hair / glass / translucency, trimap. |

## Inputs (ports)

| Port | Type | Required | Notes |
| --- | --- | --- | --- |
| `image` | image path | yes | The image to cut the subject from. |
| `reference` | image path | no | Reference / target subject. |
| `visual_context` | object | no | `VisualContext` from `PSD Context Analyze`. |
| `placeholder_mask` | image path | no | PSD placeholder region; can constrain the subject extent. |
| `prompt` | text | no | e.g. `perfume bottle`, `main product`, `person` (used by Phase 2 providers). |
| `previous_mask` | image path | no | Continue editing a prior mask. |
| `edit_paths` | object | no | Prior brush/path edits to re-apply (see schema). |

## Parameters (Phase 1)

| Param | Type | Default | Range / values | Notes |
| --- | --- | --- | --- | --- |
| `mode` | enum | `hybrid` | `auto_subject` \| `auto_product` \| `auto_person` \| `auto_transparent_object` \| `manual_brush` \| `manual_pen` \| `hybrid` | Phase 1 implements `manual_*` + `hybrid` (manual layer over an empty/`previous_mask` base); the `auto_*` providers are Phase 2. |
| `wand_tolerance` | int | `24` | `0..255` | Colour distance for the magic-wand flood fill. |
| `feather_px` | float | `0.0` | `>= 0` | Edge feather applied last. |
| `grow_px` | int | `0` | any | Positive dilates, negative erodes. |
| `fill_holes` | bool | `false` | | Close interior holes before feather. |
| `output_dir` | path | run output dir | | Triplet written here. |
| `output_name` | basename | `<image>_mask` | plain basename | Rejected if it contains `..` or a path separator (`studio_reject_unsafe_basename`). |
| `max_decode_pixels` | int | `96_000_000` | `>= 0` (0 disables) | Rejects an **input** image / mask larger than this before decoding (decompression-bomb guard, aligned with the other PSD CLIs). |

## Outputs

| Port | Type | Notes |
| --- | --- | --- |
| `mask` | grayscale PNG | The matte (`L`); binary or feathered. |
| `alpha_image` | RGBA PNG | The full image with the mask as alpha. |
| `cutout_image` | RGBA PNG | Subject cropped to its bbox; feeds `Refine Mask Edge`. |
| `matte_report` | object | Operations + provenance (see schema). |
| `edit_paths` | object | Pen/lasso/brush record for re-editing (see schema). |

### `SubjectMaskResult`

```json
{
  "mask_path": "",
  "alpha_image_path": "",
  "cutout_image_path": "",
  "edit_paths_path": "",
  "matte_report": {
    "mode": "hybrid",
    "provider": "local",
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
`processing_time_ms`, and a `triplet` completeness flag.

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

The image is normalised to an 8-bit RGBA working space (and `source_mode`
recorded) using the same loaders as `compose_psd` / `image_enhance`:

| Source mode | Handling |
| --- | --- |
| `RGB` / `RGBA` / `L` / `LA` | Used directly. |
| `P` (palette) | Expanded to RGB(A); `info` transparency treated as alpha. |
| `CMYK` | Converted to sRGB via the embedded ICC profile when present, else naive. |
| `I` / `I;16*` / `F` (high bit) | Tone-scaled to 8-bit via numpy before conversion. |

A `placeholder_mask` / `previous_mask` is read as 8-bit `L` (high-bit mattes
tone-scaled, not clipped) and used to constrain / seed the subject.

## Boundary behaviour

| Condition | Behaviour |
| --- | --- |
| Missing / blank `image` | Rust handler errors before shelling out. |
| Missing file on disk | `FileNotFoundError: <which> not found: <path>`. |
| Input larger than `max_decode_pixels` | `ValueError: input image too large to decode safely: <path> WxH ...` (before decode). |
| Empty selection after edits | Emits a fully-transparent mask + `mask_coverage: 0.0`; never crashes. |
| EXIF-rotated image | Orientation normalised; `exif_transposed: true`. |
| `auto_*` mode in Phase 1 | Falls back to an empty/`previous_mask` base for manual editing (no provider yet). |
| Unsafe `output_name` | Rejected server-side. |

## Determinism

Phase 1 is deterministic: identical `image` + params + `edit_paths` produce the
same triplet. Phase 2 segmentation providers may not be, and will report their
`provider` / model in `matte_report`.

## Phases

1. **Manual usable** (this contract): magic-wand + brush/eraser + morphology +
   feather, triplet output, `edit_paths` stored. Ships into the real PSD chain.
2. **Auto subject** — SAM / RMBG / BiRefNet / rembg / remote API behind
   `executor: api|hybrid`; the node's click-to-select becomes a model point-prompt.
3. **Pen paths** — bezier rasterise, path add/subtract/intersect, re-editable.
4. **Alpha matting** — continuous alpha (hair / glass / translucency), trimap,
   tighter `Refine Mask Edge` hand-off.

## Backend boundary

```
React UI            -> node preview + shared mask-editor modal (brush/pen/undo/redo)
Rust / Tauri        -> file IO, scheduling, cache, history, path validation, provider calls
Python bridge       -> segmentation / matting models, heavy image processing
Refine Mask Edge    -> receives mask / cutout, owns edge fusion
```

## Tests

- `python/bridge/tests/test_subject_mask_cli.py` — *(added with the Phase 1
  implementation)* triplet/report shape, wand flood-fill, brush apply, morphology
  + feather, colour-space handling, the decode guard, empty-selection fallback.
- studio-ui — the shared modal's tool registry (`ready` vs `planned`) and
  click-to-select interaction (E2E).
