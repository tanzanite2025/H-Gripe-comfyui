// Generic file helpers: native file picker + backend thumbnail generation.

import { tauriInvoke } from "./core";

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
