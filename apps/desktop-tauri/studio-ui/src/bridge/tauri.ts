// Thin bridge to the Tauri backend. When running outside Tauri (e.g. `vite dev`
// in a plain browser) it falls back to mocks so the editor stays usable for UI
// development without the desktop backend.

interface TauriWindow {
  __TAURI__?: { core: { invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown> } };
}

function tauriInvoke(): ((cmd: string, args?: Record<string, unknown>) => Promise<unknown>) | null {
  const w = window as unknown as TauriWindow;
  return w.__TAURI__?.core?.invoke ?? null;
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

export interface ThumbnailResult {
  thumbnailPath: string;
  width: number;
  height: number;
  hash: string;
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
    return { thumbnailPath: req.path, width: req.size, height: req.size, hash: "mock", mime: "image/*" };
  }
  return (await invoke("generate_thumbnail", {
    path: req.path,
    size: req.size,
    dpr: req.dpr ?? window.devicePixelRatio ?? 1,
  })) as ThumbnailResult;
}
