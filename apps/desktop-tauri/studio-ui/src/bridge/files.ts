// Generic file helpers: native file picker + backend thumbnail generation.

import { tauriInvoke, tauriListen, type UnlistenFn } from "./core";

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

// Fields are snake_case to match the Rust `VideoProbeResult` serialization.
export interface VideoProbeResult {
  width: number;
  height: number;
  /** Clip length in seconds; `null` when the container reports none. */
  duration_sec: number | null;
  /** Frame rate; `null` when unknown. */
  fps: number | null;
  codec: string | null;
  /** On-disk PNG of the poster frame (render it via `generateThumbnail`). */
  poster_path: string;
}

/**
 * Probe a video for the generic video card: read its metadata and decode a
 * poster frame to a cached PNG. Rust has no video decoder, so this shells out
 * to the bundled Python (PyAV). Outside Tauri there is no backend, so this
 * returns a mock with an empty poster path (browser preview shows a placeholder).
 */
export async function videoProbe(path: string, timestamp = 0): Promise<VideoProbeResult> {
  const invoke = tauriInvoke();
  if (!invoke) {
    return { width: 0, height: 0, duration_sec: null, fps: null, codec: null, poster_path: "" };
  }
  return (await invoke("video_probe", { path, timestamp })) as VideoProbeResult;
}

// Fields are snake_case to match the Rust `ImageDims` serialization.
export interface ImageDims {
  width: number;
  height: number;
}

/**
 * Read an image's pixel dimensions from its header only (no full decode). This
 * is the fast first phase of media-card ingestion: the info row can show `W×H`
 * near-instantly while the heavier {@link generateThumbnail} decode runs
 * separately. Outside Tauri there is no backend, so this returns `null` and the
 * caller falls back to the dimensions the thumbnail reports.
 */
export async function probeImageDims(path: string): Promise<ImageDims | null> {
  const invoke = tauriInvoke();
  if (!invoke) return null;
  try {
    return (await invoke("probe_image_dims", { path })) as ImageDims;
  } catch {
    return null;
  }
}

// Fields are snake_case to match the Rust `IngestEvent` serialization.
export interface IngestProgress {
  /** Absolute source path this update is about. */
  path: string;
  /** `"dims"` (header W×H known), `"thumb"` (thumbnail ready), or `"error"`. */
  phase: "dims" | "thumb" | "error";
  width?: number;
  height?: number;
  /** `data:` URL of the ready thumbnail (only on the `"thumb"` phase). */
  data_url?: string;
  cache_path?: string;
  source_hash?: string;
  mime?: string;
  /** Failure message (only on the `"error"` phase). */
  error?: string;
}

/**
 * Warm the backend ingestion pipeline for freshly dropped image `paths`. This
 * fires and returns immediately: the backend probes header dimensions and
 * generates thumbnails off the UI thread, pushing {@link IngestProgress}
 * updates over {@link listenIngestProgress}. Cards render `W×H` from the pushed
 * `dims` and swap in the thumbnail from the pushed `thumb`, so ingestion never
 * touches the React main thread. No-op outside Tauri (browser preview).
 */
export async function primeIngest(paths: string[], size = 256, dpr?: number): Promise<void> {
  const invoke = tauriInvoke();
  if (!invoke || paths.length === 0) return;
  try {
    await invoke("prime_ingest", {
      paths,
      size,
      dpr: dpr ?? window.devicePixelRatio ?? 1,
    });
  } catch {
    /* best-effort warmup; cards still lazy-load on their own */
  }
}

/**
 * Subscribe to backend ingestion progress ({@link primeIngest}). Returns `null`
 * outside Tauri.
 */
export async function listenIngestProgress(
  cb: (event: IngestProgress) => void,
): Promise<UnlistenFn | null> {
  const listen = tauriListen();
  if (!listen) return null;
  return listen<IngestProgress>("ingest://progress", (event) => {
    if (event.payload?.path) cb(event.payload);
  });
}

// Fields are snake_case to match the Rust `ResourceRef` serialization.
export interface ResourceRef {
  /** Stable `res-…` handle for this media file (hash of its canonical path). */
  id: string;
  /** Canonical absolute path the id resolves to. */
  path: string;
  width?: number;
  height?: number;
}

/**
 * Register a media `path` with the backend resource registry and get back a
 * lightweight {@link ResourceRef}. Cards hold the returned `id` (not the pixels)
 * and pass it to {@link resourceThumbnail} / {@link resourceInfo}, keeping heavy
 * data in Rust. The id is stable across sessions (a hash of the canonical path),
 * so re-registering on project load yields the same handle. Returns `null`
 * outside Tauri (browser preview), where callers fall back to path-based calls.
 */
export async function registerResource(path: string): Promise<ResourceRef | null> {
  const invoke = tauriInvoke();
  if (!invoke || !path) return null;
  try {
    return (await invoke("register_resource", { path })) as ResourceRef;
  } catch {
    return null;
  }
}

/**
 * Resolve a registered {@link ResourceRef} by id. Returns `null` outside Tauri
 * or when the id was never registered this session.
 */
export async function resourceInfo(id: string): Promise<ResourceRef | null> {
  const invoke = tauriInvoke();
  if (!invoke || !id) return null;
  try {
    return (await invoke("resource_info", { id })) as ResourceRef;
  } catch {
    return null;
  }
}

/**
 * Generate (or fetch from cache) a thumbnail for a registered resource id. The
 * backend resolves the id to its path and shares the same caches as
 * {@link generateThumbnail}. Returns `null` outside Tauri or for an unknown id.
 */
export async function resourceThumbnail(
  id: string,
  size = 256,
  dpr?: number,
): Promise<ThumbnailResult | null> {
  const invoke = tauriInvoke();
  if (!invoke || !id) return null;
  try {
    return (await invoke("resource_thumbnail", {
      id,
      size,
      dpr: dpr ?? window.devicePixelRatio ?? 1,
    })) as ThumbnailResult;
  } catch {
    return null;
  }
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

export interface FileDropEvent {
  /** Absolute paths of the dropped files. */
  paths: string[];
  /** Drop point in physical (device) pixels relative to the webview. */
  position: { x: number; y: number };
}

/**
 * Subscribe to OS files dropped onto the webview. This is the only way to get
 * absolute filesystem paths for a drag-and-drop (the DOM `drop` event yields a
 * sandboxed `File` with no real path), so canvas file ingestion goes through
 * here on desktop. Returns `null` outside Tauri (browser preview has no native
 * drag-drop paths). Tauri emits physical-pixel coordinates; callers divide by
 * `devicePixelRatio` before mapping to flow space.
 */
export async function listenFileDrop(
  cb: (event: FileDropEvent) => void,
): Promise<UnlistenFn | null> {
  const listen = tauriListen();
  if (!listen) return null;
  return listen<{ paths?: string[]; position?: { x: number; y: number } }>(
    "tauri://drag-drop",
    (event) => {
      const payload = event.payload;
      if (!payload?.paths || payload.paths.length === 0) return;
      cb({ paths: payload.paths, position: payload.position ?? { x: 0, y: 0 } });
    },
  );
}
