# Generic Media cards (Image / Video) + bound edit-result nodes

The canvas-first ingestion + editing surface. Dropping a **file onto the canvas**
creates a **generic media card** — an **Image** card or a **Video** card, chosen
by the file type (the two are deliberately **never** the same card). Editing an
image (mask, crop, colour, …) never mutates the source card: it spawns a
**bound result node** wired from the source, so the produced image flows on to
the rest of the workflow.

This document is the frozen contract for the ingestion + binding model and the
auto/manual split. It does not redefine the individual edit backends (mask =
`subjectMask`, etc.); it defines how a media card *hands off* to them and how
their result becomes a downstream node.

## Why a bound result node (not in-place mutation)

A node graph is already non-destructive by construction: a processing node
*reads* an input image and *emits* a new one. Overwriting the source card would
throw that away and leave nothing for downstream steps to consume.

**Decision (confirmed):**

- The **source media card stays a pure input** — its `path` / thumbnail never
  change as a result of an edit.
- Each edit is a **separate processing node** (`crop`, `subjectMask`, colour, …)
  connected from the source by an edge.
- That edit node **renders + displays its own result** on its card and exposes
  it on an **output port** for the next step.
- Editing is therefore **chainable**: a result node can itself be edited, adding
  another bound node downstream, growing the workflow naturally.

The "generic image card + a row of edit buttons" is thus a **convenience layer**
over the existing graph model: clicking an edit button *auto-creates the right
edit node, auto-wires the binding edge, and opens that node's editor*. There is
no new destructive "apply that rewrites the source".

## Binding edge

The source→edit link is a normal data edge **plus** a distinct visual treatment
so a binding reads differently from an ordinary workflow connection.

| Aspect | Value |
| --- | --- |
| Edge `type` | `binding` (new React Flow edge type) |
| Render | a **short, straight** connector (not the default bezier) — the two cards sit close, the line is the literal "this result is bound to that source" tie |
| Data | identical to a normal `image`-typed edge (source `image` out → edit `image` in); it still participates in the DAG / cycle checks |
| Auto-layout | the edit node is placed immediately to the right of the source so the binding line stays short |

`binding` is purely presentational + a layout hint; the executor / `toWorkflowGraph`
treats it as a regular edge so nothing downstream needs to special-case it.

## Drop → card routing

`FlowCanvas` currently only handles **palette** drags (`DND_NODE_KIND`, an
in-app node-kind string) in `onDrop`. OS **file** drops need a separate path,
and crucially must yield an **absolute filesystem path** (the Rust / Python
backends all work on disk paths):

| Environment | Mechanism | Gives path? |
| --- | --- | --- |
| Desktop (Tauri) | the webview drag-drop event (`onDragDropEvent` / `tauri://file-drop`) | **yes** — absolute paths |
| Browser preview | DOM `DataTransfer.files` | no real disk path (browser-sandboxed) |

**Decision:** ingestion uses the **Tauri** drag-drop event on desktop (the real
target); browser-preview drop is best-effort / disabled for files. The dropped
path's extension routes it:

| Extension (case-insensitive) | Card |
| --- | --- |
| `png` `jpg` `jpeg` `webp` `gif` `bmp` `tif` `tiff` | **Image** card (`imageSource`, extended) |
| `mp4` `mov` `mkv` `webm` `avi` `m4v` | **Video** card (`videoSource`, new) |
| anything else | rejected with a status-bar note |

The node is created at the drop position (`screenToFlowPosition`), with `path`
pre-filled.

## Image card (`imageSource`, extended)

The existing `imageSource` node grows from "a path input" into the generic image
card, mirroring the `subjectMask` card body pattern (thumbnail + action row):

| Element | Notes |
| --- | --- |
| Thumbnail | `LazyThumb(path)` — already lazy + backend-thumbnailed |
| Info row | `width × height` (free — `generateThumbnail` already returns dims), file basename; format / DPI / size are a later enrich |
| Action row | icon buttons that each **spawn a bound edit node** + open its editor: `Mask`, `Crop`, … (`planned` ones render greyed) |

The action buttons call a new `editing.addBoundEdit(sourceId, editKind)` context
method (see below); they do **not** mutate `imageSource`.

## Video card (`videoSource`, new) — separate track

Video is **not** an image and gets its own card, editor set, and backend. It is
scoped separately because it needs real video decoding the repo does not yet
have:

- `MediaViewer` today explicitly supports **images only** (non-image ⇒ "open
  externally"); there is no `<video>` / frame extraction.
- A video card needs a backend **poster-frame / thumbnail extraction** (ffmpeg
  class) for the card thumbnail, and its edits (trim, frame-crop, …) are a
  distinct op set.

Phase 1 of this contract ships the **image** path end-to-end; the video card is
specified here as the bound-node sibling and implemented on its own track.

## Auto (computed) vs Manual — how the split is decided

Every editable operation is one of two kinds, decided by a single rule:

> **Can the result be derived from the input alone by an algorithm / model?**

| → | Kind | UI | Lane |
| --- | --- | --- | --- |
| **Yes** | **Auto (computed)** | a button + engine/param selectors; running it produces the result | backend compute (ML / native) |
| **No — needs human spatial intent** | **Manual** | tools in the editor (brush, wand, crop box, rotate angle, region pick) | recorded as image-space **ops**, rasterised by the backend |

Examples: subject segmentation, edge refine, light/colour match, enhance, defect
detect, *crop-to-subject* are **auto**; precise brush touch-ups, magic-wand
補点, hand-drawn crop box / aspect, manual rotate angle are **manual**.

Many features have **both** forms (crop = auto crop-to-subject **and** manual
box; mask = auto segment **and** manual brush). The clean design — already
embodied by the `subjectMask` card's `Auto` + `Edit Mask` buttons — is:

> **Auto produces a base result; manual edits refine on top**, both folded into
> the **same op stack** on the node and resolved by the **same render pipeline**.

So auto/manual is a split **inside** an edit node (an `engine`/auto param set +
a manual `edit_paths`/ops set); the **binding** is the relationship **between**
nodes. The two concerns are orthogonal.

## "Confirm → result appears" (run-up-to-node)

Clicking **Confirm** in an editor must show the produced result immediately,
without forcing a full-graph run. The run controller today only runs the **whole
graph** (`run`) or a **batch** (`runBatch`) — there is no single-node / partial
run.

**Decision (confirmed):** add a **run-up-to-node** execution path:

1. Editor commits its params/ops onto the (bound) edit node.
2. The controller builds the **ancestor subgraph** of that node (the node + all
   its transitive inputs) and runs only that.
3. The edit node's output paths are surfaced onto its card (it renders its
   result thumbnail, like `preview` / `psdExport` already do).

This reuses the existing executor + `applyPreviews` machinery; it only needs a
subgraph selector ("node + ancestors") and a `run(targetNodeId)` entry point.

## NodeEditing context additions

```ts
interface NodeEditing {
  onParamChange(nodeId, key, value): void;        // existing
  openPreview?(nodeId): void;                       // existing
  openMaskEdit?(nodeId): void;                      // existing
  // NEW: create a bound edit node from a source media card + open its editor.
  addBoundEdit?(sourceId: string, editKind: string): void;
  // NEW: run only the target node + its ancestors, then surface the result.
  runUpToNode?(nodeId: string): void;
}
```

`addBoundEdit` owns: spawn `editKind` node → connect a `binding` edge from
`sourceId.image` → position it to the right → open the matching editor.

## Phases

1. **Image ingestion + binding (this contract, first):**
   - Tauri file-drop → `imageSource` at drop position (absolute path).
   - `imageSource` card body: thumbnail + `w×h` info + action row.
   - `binding` edge type (short, straight) + `addBoundEdit`.
   - `runUpToNode` partial execution so Confirm shows a result.
2. **Crop edit node:** auto crop-to-subject (compute) **+** manual box/aspect
   (ops), wired through the result pipeline; first non-mask edit to validate the
   unified auto/manual + binding model end-to-end.
3. **Video card:** `videoSource` + backend poster-frame extraction + its own
   (trim / frame-crop) editors, on a separate track.
4. **Editor unification (optional, "all-in-one"):** fold crop / rotate / colour
   tools into one editor entry per the same auto-base + manual-refine op model.
