# Design: Local/API card split + PSD chain hardening

Status: proposal (no code changes in this PR)
Scope: defines (1) a first-class `executor` (local vs API) dimension for node
cards + the broker routing rules that enforce it, and (2) a per-chain
"do it to the extreme" checklist for the eight PSD processing chains.

This is the structural prerequisite the team agreed to land **before** deepening
the Phase 1 algorithms: once routing is explicit, each local chain can be
hardened independently without touching dispatch, and future API/local-model
management has clean seams to hook into.

---

## 1. Where we are today

### 1.1 Dispatch flow (current)

```
studio-ui  run_studio_graph(graph_json)
   -> Rust  studio/exec.rs   (topological run)
        -> match node.kind { ... }                       // exec.rs:769
             - local PSD nodes  -> psd.rs wrappers -> python/bridge/*_cli.py
             - API nodes        -> broker().execute_with_context(ApiTask) -> provider
```

- The big `match node.kind.as_str()` in `apps/desktop-tauri/src-tauri/src/studio/exec.rs:769`
  is the single dispatch point.
- **Local** handlers shell out to a vendored Python CLI through the wrappers in
  `apps/desktop-tauri/src-tauri/src/psd.rs` (each resolves `python/bridge/<x>_cli.py`
  under the project root via `resolve_project_dir`).
- **API** handlers build an `ApiTask` and call `broker().execute_with_context(...)`
  (`exec.rs:45`), which routes to a provider in `crates/hgripe-api/src/providers/`
  (`openai_compatible`, `replicate`, `custom_http`, `mock`).

### 1.2 The local/API distinction is currently *implicit*

There is no single field that says "this card runs locally" vs "this card calls a
provider". It is inferred three different ways:

| Mechanism | Example | Where |
| --- | --- | --- |
| Hard-coded by `node.kind` | `matchLightColor` is always local; `generate` is always API | `exec.rs:858-880` |
| A per-node `mode: off\|local\|api` param | `promptOptimize` | `nodeSpecs.ts:96`, `exec.rs:676-691` |
| A `provider` param that may be empty/`mock` | `detailRepaint` passes through when no edit-capable provider | `nodeSpecs.ts:858`, `exec.rs:484-548` |

This works but has costs the team already felt:
- you cannot group/filter the palette by "local vs cloud";
- you cannot enforce "a local card must never hit the network" (or vice-versa);
- API management (profiles/keys/model lists) and local-model management
  (weights path/device/precision) have nowhere consistent to attach.

### 1.3 Node kind -> executor -> backend (today)

| Node kind | Executor (today) | Backend |
| --- | --- | --- |
| `prompt`, `batch`, `imageSource`, `psdTemplate`, `number`, `reroute`, `compare`, `logic`, `if`, `switch`, `preview`, `save`, `group` | n/a (pure graph) | in-process Rust |
| `psdContextAnalyze` | local | `python/bridge/analyze_psd_cli.py` |
| `matchLightColor` | local | `color_match_cli.py` |
| `refineMaskEdge` | local | `edge_refine_cli.py` |
| `imageEnhance` | local | `image_enhance_cli.py` |
| `detailWatchdog` | local | `detail_watchdog_cli.py` |
| `psdExport` | local | `compose_psd_cli.py` / `inspect_psd_cli.py` |
| `promptOptimize` | hybrid (`off`/`local`/`api`) | rule-based / provider `text.generate` |
| `detailRepaint` | API (graceful pass-through) | broker `image.edit` + `detail_repaint_cli.py` (prepare/composite) |
| `generate` | API | broker `image.generate` |

---

## 2. Proposal A — first-class `executor` dimension

### 2.1 Schema (`studio-ui`)

Add an `executor` discriminator to `NodeSpec` (`graph/nodeSpecs.ts`):

```ts
export type Executor = "graph" | "local" | "api" | "hybrid";

export interface NodeSpec {
  kind: string;
  title: string;
  description: string;
  category: "input" | "generate" | "control" | "output" | "utility";
  executor: Executor;            // NEW — the routing/grouping discriminator
  inputs: PortSpec[];
  outputs: PortSpec[];
  params: ParamSpec[];
}
```

- `graph`  — pure in-process nodes (prompt/number/if/switch/...). No backend call.
- `local`  — always a `python/bridge` CLI; must not touch the network.
- `api`    — always a provider call (needs a profile + credentials_ref).
- `hybrid` — user picks per-node via the existing `mode` param (only
  `promptOptimize` today). A hybrid card carries `mode: "off" | "local" | "api"`
  and the inspector keeps using `visibleWhen` to reveal the relevant fields.

This is additive: every existing node gets one new field; no port/param changes.

For `local` cards we standardize an optional **engine** param so future local
models slot in without new plumbing:

```ts
// present on local (and the local branch of hybrid) image cards
{ key: "engine", label: "Engine", control: "select",
  options: ["cpu"], defaultValue: "cpu",
  hint: "local backend; more engines (e.g. supir, ccsr) added per chain" }
```

For `api` cards we standardize the existing provider trio
(`provider` / `profile` / `model`) and keep `credentials_ref` out of the graph
(resolved by the broker), exactly as today.

### 2.2 Palette grouping (`studio-ui`)

`paletteGroups()` (`nodeSpecs.ts:1007`) currently groups by `category`. Add a
secondary split by `executor` so the palette can show **Local** vs **API**
sections (or a filter toggle). Pure-`graph` nodes stay under their category
unchanged. This is the visible half of the user's request ("本地和 API 的卡片分开").

### 2.3 Broker routing rules (Rust)

Make `exec.rs` consult the executor instead of relying only on `kind`:

```rust
// pseudocode, exec.rs run_studio_node
match spec.executor {
    Executor::Graph  => run_pure_graph_node(node, inputs),
    Executor::Local  => run_local_chain(node, inputs),          // python/bridge CLI only
    Executor::Api    => run_api_node(node, inputs, ...).await,  // broker provider only
    Executor::Hybrid => match node.params["mode"] {
        "off"   => passthrough,
        "local" => run_local_chain(...),
        "api"   => run_api_node(...).await,
    },
}
```

Enforcement (the point of the split):
- a `Local` node **must not** be able to construct an `ApiTask` / reach a
  provider — `run_local_chain` has no `broker()` access path;
- an `Api` node with an empty/`mock` provider keeps today's graceful
  pass-through behavior (`detailRepaint`), but that decision now lives behind the
  `Api` branch, not scattered in the handler;
- unknown `kind` still errors (`exec.rs:881`).

The dispatch can keep the existing per-`kind` handlers underneath; `executor`
just becomes the outer guard rail + the thing the UI and managers key off.

### 2.4 Migration / compatibility

- Saved graphs only store `kind` + `params`; adding `executor` to the *spec*
  (not the saved node) means **no migration of saved files** is required.
- `promptOptimize` already has `mode`; mark it `executor: "hybrid"` and the
  behavior is unchanged.
- `detailRepaint` becomes `executor: "api"`; pass-through stays.
- Everything else maps per the table in §1.3.

### 2.5 Why this unlocks future management

- **API management**: an `api` card's `{provider, profile, model}` is the only
  surface to validate against the profile registry (`crates/hgripe-api/src/profiles.rs`)
  and credentials (`credentials.rs`). A future "API manager" UI enumerates `api`
  cards and their profiles in one place.
- **Local model management**: a `local` card's `engine` (+ future
  `weights_path`/`device`/`precision`) is the only surface a future "local model
  manager" needs to touch. CPU is the only engine today; SupIR/CCSR/matting are
  added per chain (see §3) by extending the `engine` enum + the CLI, with no
  dispatch changes.

---

## 3. Proposal B — per-chain "do it to the extreme" checklist

Hardening order (shallow deps first; earlier chains feed later ones):

```
inspect/analyze_psd  ->  color_match  ->  edge_refine  ->  image_enhance
   ->  detail_watchdog  ->  detail_repaint  ->  compose_psd
```

Each chain is "done" only when **all** of the following hold. A chain is not
declared complete until its row is green in CI.

### 3.0 Shared definition of done (every chain)

- **I/O contract frozen**: documented JSON in/out; the PSD triplet
  (`.psd` + `.png` preview + `.json` metadata) field set is fixed and versioned.
- **Boundary/failure handling**: empty/missing mask, huge images, non-8-bit,
  CMYK/grayscale, alpha vs no-alpha, zero-area regions, and unsafe `output_name`
  (already centralized via `reject_unsafe_output_name`) all return a clear error
  or a defined no-op — never a panic or silent wrong output.
- **Determinism**: same input + params => same output (seedable where randomness
  exists), so regression fixtures are stable.
- **Regression fixtures**: a tiny sample asset + expected-output assertion wired
  into `cargo test` (Rust wrapper) and/or `vitest` (UI param/preview), so
  "改一条断一条" is caught.
- **Params documented**: every `ParamSpec` has a `hint`, sane defaults, and
  ranges; inspector hides irrelevant controls via `visibleWhen`.
- **Perf budget**: a rough wall-clock budget on the sample asset recorded in the
  chain doc so regressions are visible.

### 3.1 inspect / analyze_psd  (`psdContextAnalyze`)
- Confirm `analyze_psd_cli.py` + `inspect_psd_cli.py` agree on layer model and
  `VisualContext` fields (lighting, bounds, masks).
- Fixtures: flat PSD, grouped/nested layers, smart-object layer, hidden layers,
  no-layer fallback.
- This is the input-quality floor for every downstream chain — harden first.

### 3.2 color_match  (`matchLightColor` -> `color_match_cli.py`)
- Freeze the snake_case JSON contract (`psd.rs:490`).
- Cases: source/target size mismatch, missing reference, extreme white balance,
  clipped histograms.
- Add `engine` param placeholder (`cpu` now) for a future learned matcher.

### 3.3 edge_refine  (`refineMaskEdge` -> `edge_refine_cli.py`)
- Cases: 1-px masks, fully-empty/fully-full masks, feather radius vs image size,
  anti-aliased vs hard edges.
- Define matting hand-off point (future `engine: matting`).

### 3.4 image_enhance / super-res  (`imageEnhance` -> `image_enhance_cli.py`)
- Today: CPU resize/sharpen/denoise (Lanczos3). Phase 2 target: SupIR/CCSR/
  Real-ESRGAN (see `docs/phase2-algorithm-roadmap.md`).
- Make scale factor, sharpen, denoise explicit + bounded; cap max output pixels.
- `engine` enum is the seam: `cpu` now; adding `supir`/`ccsr` later changes only
  the CLI + enum, not dispatch.

### 3.5 detail_watchdog  (`detailWatchdog` -> `detail_watchdog_cli.py`)
- Today: CPU rule-based blur/halo detection. Freeze the issue/`suggested_action`
  schema that `detail_repaint` consumes.
- Fixtures with known blur/halo/noise so thresholds are regression-tested.
- `engine` seam for a future ML/VLM detector.

### 3.6 detail_repaint  (`detailRepaint` -> broker `image.edit` + `detail_repaint_cli.py`)
- This is the one chain that is **`executor: "api"`** (provider `image.edit`),
  with CPU prepare/composite (`prepare_repaint_regions`/`composite_repaint`).
- Cases: no edit-capable provider (pass-through, already handled `exec.rs:484`),
  per-region provider failure (leave region unrepainted), seam feathering,
  region padding vs image bounds, cancellation mid-run.
- Future: a `local` engine (on-device inpaint) would make this `hybrid`.

### 3.7 compose_psd  (`psdExport` -> `compose_psd_cli.py`)
- The final assembler — depends on all upstream outputs being contract-clean.
- Cases: smart-object replacement, layer ordering, missing layers, filename
  sanitization (`studio_reject_unsafe_basename`), triplet completeness.

---

## 4. Suggested PR sequence (small, independent)

1. **Schema + grouping (UI only)**: add `executor` to `NodeSpec`, tag all kinds,
   split the palette. No backend behavior change. (lands the structure)
2. **Broker guard rail (Rust)**: route on `executor` in `exec.rs`, enforce
   "local never hits network / api never silently runs local". Pure refactor +
   tests; behavior identical.
3. **Per-chain hardening**: one PR per chain in the §3 order, each adding its
   fixtures + boundary handling + `engine` placeholder.

Only after #1–#2 are merged do the chain PRs (#3) avoid having to re-touch
routing while strengthening algorithms.

---

## 5. Explicitly out of scope here
- No Phase 2 algorithm implementations (SupIR/CCSR/ML watchdog/local inpaint).
- No saved-graph migration (executor lives on the spec, not the saved node).
- No credentials/profile UI changes; only the seams they will attach to.
