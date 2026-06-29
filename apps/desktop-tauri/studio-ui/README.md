# H-Gripe Studio (node-graph editor)

A Vite + React + TypeScript sub-app implementing H-Gripe's own production
node-graph editor on top of [React Flow](https://reactflow.dev) (`@xyflow/react`).
This is the in-house alternative to the embedded ComfyUI — ComfyUI stays as the
**Advanced Canvas** and is **not touched** by this work.

> Status: **embedded in the desktop shell** as the **Node Editor** tab. The
> build output is written to `../dist/studio` (gitignored) and loaded as an
> iframe by the static shell. The shell and the embedded ComfyUI **Advanced
> Canvas** are otherwise unaffected. When embedded, the Tauri bridge reaches
> IPC via the parent window (see `bridge/tauri.ts`). A plain `cargo run` does
> not build this; run `npm run build` first (the Tauri CLI does it via the
> `before*` hooks in `tauri.conf.json`).

## Architecture (renderer-agnostic by design)

The durable assets are deliberately independent of the renderer, so React Flow
can be swapped later (e.g. for tldraw / a canvas renderer) without a data or
runtime migration:

```
src/
  graph/      renderer-agnostic data model + node specs (typed ports)
  runtime/    DAG runtime (topo sort, parallel levels, validation, executors)
  bridge/     thin Tauri bridge (run_task_json, generate_thumbnail) + mocks
  editor/     React Flow rendering layer (adapter <-> graph model)
```

- **graph/model.ts** – `WorkflowGraph` (`nodes` / `edges` / typed ports /
  `MediaRef`), serialization, and port-type compatibility.
- **graph/nodeSpecs.ts** – node catalogue. Each kind declares typed input/output
  ports and param controls.
- **runtime/dag.ts** – `topoLevels` (Kahn, grouped for parallel execution),
  `wouldCreateCycle`, `validateGraph` (type + cycle checks), and `runGraph`
  (threads outputs to inputs, runs independent branches concurrently, memoizes
  by signature). Runs headless — no UI dependency.
- **runtime/executors.ts** – maps node kinds to behaviour; `generate` composes
  an `ApiTask` and runs it through the existing broker via `run_task_json`.
- **editor/** – React Flow nodes are memoized, connections are validated by port
  type and acyclicity (`isValidConnection`), and `onlyRenderVisibleElements` is
  on. The adapter converts render state <-> `WorkflowGraph` (both directions:
  `toWorkflowGraph` / `fromWorkflowGraph` for save/load).

## Backend boundary

The editor is allowed to be frontend-heavy for interaction work: drag/drop,
selection, grouping, reroute nodes, helper lines, minimap, context menus,
viewport LOD, inline controls, undo/redo, and graph validation can all stay in
TypeScript because they do not touch real files, credentials, provider APIs, GPU
services, or long-running jobs.

Production execution must not remain frontend-only. Anything with real side
effects goes through Tauri/Rust:

- API execution uses the existing broker via `run_task_json`.
- PSD composition uses the desktop backend command `compose_psd`.
- Thumbnails use `generate_thumbnail`, with the original media path remaining
  the source of truth.
- File access uses native commands such as `pick_file`, `list_psd_outputs`, and
  runtime path/profile commands rather than browser-only state.
- Desktop workflow autosave uses `read_studio_autosave`,
  `write_studio_autosave`, and `clear_studio_autosave`; `localStorage` is only
  the browser-preview fallback.
- Explicit workflow save/open and the project folder browser use native
  commands: `pick_workflow_save_path`, `pick_workflow_open_path`,
  `pick_project_folder`, `read_studio_workflow`, `write_studio_workflow`,
  `list_studio_workflows`, and `read_studio_recents` / `write_studio_recents`
  (which persist the active folder + recent files next to the autosave).
- Credentials, provider profiles, output directories, history, cache indexes,
  local GPU service startup, and video export should live behind Rust commands.

The desktop Run / Run xN path uses the Rust-side `run_studio_graph` Tauri
command. It accepts the same renderer-agnostic `WorkflowGraph` JSON, executes
pure/value/control nodes, prunes untaken branches, and routes `generate` /
`psdExport` through the backend broker and PSD pipeline. The TypeScript
`runtime/runGraph` remains as the browser-preview fallback and unit-tested
reference implementation. The Rust runner emits node-level Tauri events on
`studio:graph-run` (`queued` / `running` / `succeeded` / `skipped` / `failed`),
filtered by `run_id` in the webview so repeated or batch runs do not cross-talk.
The toolbar's **Cancel** button calls `cancel_studio_run`; cancellation stops
before the next node starts and passes a cancellation token down into the Rust
broker/provider layer for `generate` nodes. `custom_http async_job` can call a
configured provider-native `cancel_url` / `cancel_url_path` / `urls.cancel`, and
`replicate run` calls the prediction cancel endpoint. This is third-party
provider/API remote job control, not an H-Gripe account/cloud system. Durable
workflow save/load beyond autosave now exists (explicit Save/Open + project
folder, above). Before the Node Editor becomes the primary production surface,
the backend runner still needs a media index/cache, richer logs, more complete
error details, more provider-native cancellation adapters, and FFmpeg-backed
video assembly/export.

## Editor features

- **Node palette** (left rail): drag a node kind onto the canvas (drop position
  honoured) or click to add. Kinds are grouped (inputs / generate / outputs).
- **Node kinds**: `prompt`, `batch`, `imageSource`, `psdTemplate`, `number`,
  `generate`, `preview`, `save` (export sink). `generate` forwards all
  non-reserved params to the broker task and accepts an optional `seed` input
  that overrides the param.
- **Batch fan-out**: a `batch` node holds a list of text items (one per line)
  and emits one (`item`, type `text`). A normal Run emits the first item; the
  toolbar's **Run ×N** runs the graph once per item, sweeping the batch node's
  `index` via `runGraph`'s `paramOverrides` (the graph itself is never mutated).
- **Param controls**: `text`, `textarea`, `number`, `select`, `slider`,
  `checkbox`, `path` — rendered by a shared `ParamField` used by both the
  Inspector and the node card.
- **Native file picker**: `path` controls show a **Browse…** button (inside
  Tauri) that opens the OS file-open dialog via the `pick_file` command
  (`tauri-plugin-dialog`), scoped by the spec's `pickerExtensions`. In a plain
  browser the button is hidden and the manual path input remains.
- **Inline editing**: params marked `inline` in `nodeSpecs` are editable
  directly on the node card (prompt text, paths, number value, generate
  operation/steps, export filename); the rest stay in the Inspector. Card
  inputs carry `nodrag`/`nowheel` so editing never drags the node or pans the
  canvas. Edits flow through `NodeEditingContext` so memoized cards update
  their own params without putting callbacks in the serializable graph.
- **Explicit Save / Save As / Open + project folder**: on desktop these use
  native dialogs and write/read named workflow files anywhere on disk
  (`pick_workflow_save_path`, `pick_workflow_open_path`, `write_studio_workflow`,
  `read_studio_workflow`). **Save** writes to the current file (falling back to
  **Save As…** when the workflow is untitled); the toolbar shows the current
  file name with a `*` when there are unsaved edits. The **Project** panel
  (left rail) picks a project folder (`pick_project_folder`), lists its workflow
  files newest-first (`list_studio_workflows`), and reopens recent files. The
  active folder + recent files persist between sessions
  (`read_studio_recents` / `write_studio_recents`). In a plain browser there is
  no filesystem, so Save/Save As download `workflow.json` and Open uses the file
  input. **New** starts an empty untitled workflow; Reset restores the sample;
  Clear empties the canvas and wipes the autosave. Delete removes the selected
  node/edge.
- **Workspace autosave**: independently of the explicit file, the graph is
  autosaved (debounced, structural fields only) — through the Rust backend on
  desktop, `localStorage` in browser preview — and restored on next open, so
  in-progress work survives a reload even before it is saved to a file. See
  `editor/persist.ts`.
- **Lazy thumbnails**: preview nodes request `generate_thumbnail` only when they
  scroll into view (IntersectionObserver), so the graph data stays light (only
  the original path) and off-screen media is never decoded.
- **PSD Studio integration**: the Inspector reuses the same backend the static
  PSD Studio tab uses — a generate node can adopt a provider **profile**
  (`get_profiles`) to fill provider / model / `credentials_ref` in one step, and
  any `path` param can be filled from the configured **output directory**'s
  `.psd` outputs (`get_runtime_info` + `list_psd_outputs`). `credentials_ref`
  flows to the broker task as a top-level field. See `editor/ProfilePicker.tsx`
  and `editor/OutputPicker.tsx`.
- **Validation**: `validateGraph` issues (type mismatch, cycle, dangling edge)
  are surfaced in the toolbar and block Run.

## Media / thumbnail discipline (why previews never blur)

Nodes display a **backend-generated thumbnail**; the original file path is the
source of truth for execution/export. The webview never downscales originals —
that is the real memory/quality killer.

Backend contract (consumed by `bridge/tauri.ts`, implemented by the Rust
`generate_thumbnail` command):

```
generate_thumbnail({ path, size, dpr })
  -> { data_url, cache_path, width, height, source_hash, mime }
```

The backend generates at `size * dpr` with Lanczos3 resampling, caches the
thumbnail on disk keyed by `source_hash + target_size`, and returns a `data:`
URL the webview can display cheaply. Fields are snake_case to match the Rust
serialization.

## Develop

```
npm install
npm run dev        # browser preview; backend calls are mocked when not in Tauri
npm run typecheck
npm test           # vitest (DAG runtime unit tests)
npm run build
```

### Note on `npm audit`

The remaining advisories are the well-known **esbuild dev-server** issue, pulled
in transitively by Vite 5/6. It only affects the local dev server and is **not**
part of the shipped bundle. Clearing it requires Vite 8 (a breaking bump), so it
is intentionally deferred; production builds are unaffected.
