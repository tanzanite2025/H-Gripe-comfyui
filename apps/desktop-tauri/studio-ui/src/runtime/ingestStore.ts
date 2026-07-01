// Process-wide sink for backend ingestion progress (`prime_ingest` →
// `ingest://progress`). A single Tauri listener fans events out to media cards
// keyed by source path, and the latest state per path is cached so a card that
// mounts *after* its events arrived still gets them replayed on subscribe.
//
// This is what lets a large-image drop feel instant without the React main
// thread doing any work: the backend probes the header and decodes the
// thumbnail off-thread, then pushes the dimensions and the ready `data:` URL
// here for the card to render. Cards keep their own probe/lazy-thumbnail path
// as a fallback (manual path entry, project load, or a missed event).

import { listenIngestProgress, type IngestProgress, type UnlistenFn } from "../bridge/tauri";

/** Latest known ingestion state for a single source path. */
export interface IngestState {
  dims?: { w: number; h: number };
  /** `data:` URL of the ready thumbnail, once decoded. */
  thumb?: string;
  /** Set when the backend reported a decode/probe failure for this path. */
  failed?: boolean;
}

type Listener = (state: IngestState) => void;

const cache = new Map<string, IngestState>();
const listeners = new Map<string, Set<Listener>>();

let started = false;
let unlisten: UnlistenFn | null = null;

function applyEvent(prev: IngestState, ev: IngestProgress): IngestState {
  const next: IngestState = { ...prev };
  if (ev.phase === "dims") {
    if (ev.width && ev.height) next.dims = { w: ev.width, h: ev.height };
  } else if (ev.phase === "thumb") {
    if (ev.data_url) next.thumb = ev.data_url;
    // The thumb event also carries dims; keep any header dims we already have.
    if (!next.dims && ev.width && ev.height) next.dims = { w: ev.width, h: ev.height };
  } else if (ev.phase === "error") {
    next.failed = true;
  }
  return next;
}

/** Fold one event into the cache and notify that path's listeners. */
function dispatch(ev: IngestProgress): void {
  const next = applyEvent(cache.get(ev.path) ?? {}, ev);
  cache.set(ev.path, next);
  listeners.get(ev.path)?.forEach((fn) => fn(next));
}

/**
 * Start the shared `ingest://progress` listener once. Safe to call repeatedly;
 * only the first call registers. Idempotent and a no-op outside Tauri.
 */
export function startIngestListener(): void {
  if (started) return;
  started = true;
  void listenIngestProgress(dispatch).then((fn) => {
    unlisten = fn;
  });
}

/**
 * Subscribe to ingestion updates for `path`. The current cached state (if any)
 * is replayed synchronously so late-mounting cards are not stuck waiting for
 * the next event. Returns an unsubscribe function.
 */
export function subscribeIngest(path: string, fn: Listener): () => void {
  startIngestListener();
  let set = listeners.get(path);
  if (!set) {
    set = new Set();
    listeners.set(path, set);
  }
  set.add(fn);
  const cached = cache.get(path);
  if (cached) fn(cached);
  return () => {
    const s = listeners.get(path);
    if (!s) return;
    s.delete(fn);
    if (s.size === 0) listeners.delete(path);
  };
}

/** Test-only: feed an event through the same path the live listener uses. */
export function __dispatchIngestForTests(ev: IngestProgress): void {
  dispatch(ev);
}

/** Test-only: drop all cached state, listeners, and the started flag. */
export function __resetIngestStoreForTests(): void {
  cache.clear();
  listeners.clear();
  unlisten?.();
  unlisten = null;
  started = false;
}
