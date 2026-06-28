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
