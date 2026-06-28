import { deserializeGraph, serializeGraph, type WorkflowGraph } from "../graph/model";

// Autosave key. Bump the suffix if the persisted shape ever changes
// incompatibly (deserializeGraph already tolerates missing params via the
// adapter's default-merge, so most additions are forward-compatible).
const STORAGE_KEY = "hgripe.studio.workflow.v1";

/** Restore the last autosaved workflow, or null if none / unreadable. */
export function loadPersistedGraph(): WorkflowGraph | null {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return null;
    return deserializeGraph(raw);
  } catch {
    // Corrupt payload or storage unavailable — fall back to the sample graph.
    return null;
  }
}

/** Persist the current workflow to the workspace (best-effort). */
export function persistGraph(graph: WorkflowGraph): void {
  try {
    localStorage.setItem(STORAGE_KEY, serializeGraph(graph));
  } catch {
    // Quota exceeded / storage disabled — autosave is best-effort.
  }
}

export function clearPersistedGraph(): void {
  try {
    localStorage.removeItem(STORAGE_KEY);
  } catch {
    /* ignore */
  }
}
