// Thin bridge to the Tauri backend. When running outside Tauri (e.g. `vite dev`
// in a plain browser) it falls back to mocks so the editor stays usable for UI
// development without the desktop backend.

type Invoke = (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;

interface TauriWindow {
  __TAURI__?: { core?: { invoke?: Invoke } };
}

// When this app runs embedded as an iframe inside the desktop shell, the Tauri
// IPC may live on the parent/top window rather than this frame. Both are
// same-origin (tauri://localhost), so reaching across is allowed. We try this
// frame first, then parent, then top.
function tauriInvoke(): Invoke | null {
  // No DOM (e.g. unit tests run in a node environment) → always use mocks.
  if (typeof window === "undefined") return null;
  const candidates: (Window | null)[] = [window];
  try {
    if (window.parent && window.parent !== window) candidates.push(window.parent);
    if (window.top && window.top !== window) candidates.push(window.top);
  } catch {
    // Cross-origin access can throw; ignore and use what we have.
  }
  for (const frame of candidates) {
    const invoke = (frame as unknown as TauriWindow | null)?.__TAURI__?.core?.invoke;
    if (invoke) return invoke;
  }
  return null;
}

export const isTauri = (): boolean => tauriInvoke() !== null;

export interface ApiResultLike {
  id: string;
  status: string;
  output_files?: { path: string }[];
  output_json?: unknown;
  error?: { message: string } | null;
}

/** Run an ApiTask JSON payload through the broker (`run_task_json`). */
export async function runTaskJson(task: unknown): Promise<ApiResultLike> {
  const invoke = tauriInvoke();
  if (!invoke) {
    // Mock for browser dev: echo back a fake succeeded result.
    return {
      id: "mock",
      status: "succeeded",
      output_json: { mocked: true, task },
      output_files: [],
    };
  }
  return (await invoke("run_task_json", { taskJson: JSON.stringify(task) })) as ApiResultLike;
}

export interface StudioGraphNodeRun {
  node_id: string;
  kind: string;
  status: string;
  duration_ms?: number | null;
  error?: string | null;
}

export interface StudioGraphRunResult {
  version: number;
  outputs: Record<string, Record<string, unknown>>;
  statuses: Record<string, string>;
  node_runs: StudioGraphNodeRun[];
}

/** Run a renderer-agnostic Studio WorkflowGraph through the Rust backend. */
export async function runStudioGraph(graph: unknown): Promise<StudioGraphRunResult> {
  const invoke = tauriInvoke();
  if (!invoke) {
    const version =
      typeof graph === "object" &&
      graph !== null &&
      "version" in graph &&
      typeof (graph as { version?: unknown }).version === "number"
        ? (graph as { version: number }).version
        : 1;
    return { version, outputs: {}, statuses: {}, node_runs: [] };
  }
  return (await invoke("run_studio_graph", {
    graphJson: JSON.stringify(graph),
  })) as StudioGraphRunResult;
}

export interface ThumbnailRequest {
  path: string;
  /** Display size in CSS px; backend should generate at size * dpr. */
  size: number;
  dpr?: number;
}

// Fields are snake_case to match the Rust `ThumbnailResult` serialization.
export interface ThumbnailResult {
  /** `data:` URL ready for an `<img src>`. */
  data_url: string;
  /** On-disk cached thumbnail path. */
  cache_path: string;
  width: number;
  height: number;
  source_hash: string;
  mime: string;
}

/**
 * Ask the backend to generate (or fetch from cache) a crisp thumbnail.
 * Never downscale the original in the webview — that is the actual perf/quality
 * killer. The original path stays the source of truth for export.
 */
export async function generateThumbnail(req: ThumbnailRequest): Promise<ThumbnailResult> {
  const invoke = tauriInvoke();
  if (!invoke) {
    return { data_url: "", cache_path: req.path, width: req.size, height: req.size, source_hash: "mock", mime: "image/*" };
  }
  return (await invoke("generate_thumbnail", {
    path: req.path,
    size: req.size,
    dpr: req.dpr ?? window.devicePixelRatio ?? 1,
  })) as ThumbnailResult;
}

export interface PickFileOptions {
  title?: string;
  /** Display name for the extension filter (e.g. "Images"). */
  filterName?: string;
  /** Bare extensions without the leading dot (e.g. ["png", "jpg"]). */
  extensions?: string[];
}

/**
 * Open the OS-native file-open dialog (`pick_file`) and resolve to the chosen
 * path, or `null` if the user cancelled. Outside Tauri there is no native
 * dialog, so this returns `null` (callers keep the manual path input).
 */
export async function pickFile(opts: PickFileOptions = {}): Promise<string | null> {
  const invoke = tauriInvoke();
  if (!invoke) return null;
  const path = await invoke("pick_file", {
    title: opts.title ?? null,
    filterName: opts.filterName ?? null,
    extensions: opts.extensions ?? null,
  });
  return (path as string | null) ?? null;
}

// --- PSD Studio integration -------------------------------------------------
// Reuses the same backend commands the static PSD Studio tab uses, so the node
// editor shares provider profiles and the output directory rather than
// re-implementing them.

// Fields are snake_case to match the Rust `ProviderProfileSummary`.
export interface ProviderProfile {
  profile_ref: string;
  provider?: string | null;
  model?: string | null;
  credentials_ref?: string | null;
  params_count?: number;
}

/** List H-Gripe provider profiles (`get_profiles`). */
export async function listProfiles(): Promise<ProviderProfile[]> {
  const invoke = tauriInvoke();
  if (!invoke) {
    return [
      { profile_ref: "mock-openai", provider: "openai", model: "gpt-image-1", credentials_ref: "openai-key" },
      { profile_ref: "mock-local", provider: "comfyui", model: "sdxl", credentials_ref: null },
    ];
  }
  return (await invoke("get_profiles")) as ProviderProfile[];
}

/** Resolve the configured output directory (`get_runtime_info().output_dir`). */
export async function getOutputDir(): Promise<string> {
  const invoke = tauriInvoke();
  if (!invoke) return "/mock/outputs";
  const info = (await invoke("get_runtime_info")) as { output_dir?: { path?: string } };
  return info.output_dir?.path ?? "";
}

// Fields are snake_case to match the Rust `PsdOutputFile`.
export interface PsdOutput {
  name: string;
  psd_path: string;
  preview_path?: string | null;
  metadata_path?: string | null;
  smart_object?: boolean;
}

/** List `.psd` outputs in a directory (`list_psd_outputs`). */
export async function listPsdOutputs(dir: string): Promise<PsdOutput[]> {
  const invoke = tauriInvoke();
  if (!invoke) {
    return [
      { name: "fox-poster", psd_path: "/mock/outputs/fox-poster.psd", preview_path: "/mock/outputs/fox-poster_preview.png", smart_object: true },
      { name: "banner", psd_path: "/mock/outputs/banner.psd", preview_path: null, smart_object: false },
    ];
  }
  return (await invoke("list_psd_outputs", { dir })) as PsdOutput[];
}

// --- PSD compose / export ---------------------------------------------------
// Wraps the Rust `compose_psd` command, which shells out to the torch-free
// `compose_psd_cli.py` helper to write the generated image into a PSD
// template's placeholder (true smart-object content replacement when possible)
// and export `<filename>.psd` + `_preview.png` + `_metadata.json`.

export interface ComposePsdRequest {
  /** Path to the `.psd` template. */
  template: string;
  /** Path to the generated image to place into the placeholder. */
  image: string;
  /** Directory the exported files are written to. */
  outputDir: string;
  /** Base name for the exported triplet (default `final`). */
  filename?: string;
  /** JSON: `{"name": "<layer>"}` or `{left,top,width,height}`. */
  placeholder?: string;
  fitMode?: "contain" | "cover" | "stretch";
  zOrder?: "above_background" | "placeholder" | "top";
  smartObjectMode?: "disable" | "replace_content";
  hidePlaceholder?: "enable" | "disable";
  /** JSON object merged into the exported metadata. */
  metadata?: string;
  savePreview?: boolean;
}

// Fields are snake_case to match the Rust `ComposePsdResult` serialization.
export interface ComposePsdResult {
  status: string;
  psd_path: string;
  /** Empty string when preview generation was disabled. */
  preview_path: string;
  metadata_path: string;
  placeholder_kind: string | null;
  smart_object_mode: string;
}

/**
 * Compose + export a PSD via the backend (`compose_psd`). Outside Tauri there is
 * no Python/psd-tools pipeline, so this returns a mocked succeeded result so the
 * editor stays runnable in browser dev.
 */
export async function composePsd(req: ComposePsdRequest): Promise<ComposePsdResult> {
  const invoke = tauriInvoke();
  if (!invoke) {
    const base = `${req.outputDir}/${req.filename ?? "final"}`;
    return {
      status: "succeeded",
      psd_path: `${base}.psd`,
      preview_path: req.savePreview === false ? "" : `${base}_preview.png`,
      metadata_path: `${base}_metadata.json`,
      placeholder_kind: req.smartObjectMode === "replace_content" ? "smartobject" : "pixel",
      smart_object_mode: req.smartObjectMode ?? "disable",
    };
  }
  return (await invoke("compose_psd", {
    template: req.template,
    image: req.image,
    outputDir: req.outputDir,
    filename: req.filename ?? null,
    placeholder: req.placeholder ?? null,
    fitMode: req.fitMode ?? null,
    zOrder: req.zOrder ?? null,
    smartObjectMode: req.smartObjectMode ?? null,
    hidePlaceholder: req.hidePlaceholder ?? null,
    metadata: req.metadata ?? null,
    savePreview: req.savePreview ?? null,
  })) as ComposePsdResult;
}
