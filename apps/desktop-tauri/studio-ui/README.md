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

## Editor features

- **Node palette** (left rail): drag a node kind onto the canvas (drop position
  honoured) or click to add. Kinds are grouped (inputs / generate / outputs).
- **Node kinds**: `prompt`, `imageSource`, `psdTemplate`, `number`, `generate`,
  `preview`, `save` (export sink). `generate` forwards all non-reserved params
  to the broker task and accepts an optional `seed` input that overrides the
  param.
- **Param controls**: `text`, `textarea`, `number`, `select`, `slider`,
  `checkbox`, `path` — rendered by a shared `ParamField` used by both the
  Inspector and the node card.
- **Inline editing**: params marked `inline` in `nodeSpecs` are editable
  directly on the node card (prompt text, paths, number value, generate
  operation/steps, export filename); the rest stay in the Inspector. Card
  inputs carry `nodrag`/`nowheel` so editing never drags the node or pans the
  canvas. Edits flow through `NodeEditingContext` so memoized cards update
  their own params without putting callbacks in the serializable graph.
- **Save / Load / Clear**: serialize the graph to `workflow.json` and load it
  back (params are merged over the kind's current defaults). Delete removes the
  selected node/edge.
- **Lazy thumbnails**: preview nodes request `generate_thumbnail` only when they
  scroll into view (IntersectionObserver), so the graph data stays light (only
  the original path) and off-screen media is never decoded.
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
