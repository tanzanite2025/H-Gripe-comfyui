// Named workflow snapshots, persisted to browser localStorage.
//
// Lightweight, renderer-agnostic version history: the user captures the current
// graph under a name and can restore it later. This mirrors the autosave
// fallback in persist.ts (localStorage), so it works identically in the desktop
// build and in `vite dev`. The mutation helpers are pure for unit testing; the
// load/save wrappers are best-effort and swallow storage errors.

import type { WorkflowGraph } from "../graph/model";

export interface Snapshot {
  id: string;
  name: string;
  /** Epoch ms when the snapshot was captured. */
  t: number;
  graph: WorkflowGraph;
}

// Bump the suffix if the persisted shape changes incompatibly.
const STORAGE_KEY = "hgripe.studio.snapshots.v1";
const AUTO_KEY = "hgripe.studio.autosnapshot.v1";
/** Cap retained snapshots so localStorage cannot grow without bound. */
export const SNAPSHOT_CAP = 50;

/** Generate a reasonably unique snapshot id. */
export function newSnapshotId(): string {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) return crypto.randomUUID();
  return `snap-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

/**
 * Insert a new snapshot at the front (newest first), trimming to the cap.
 * Pure: returns a new array and does not mutate the input.
 */
export function addSnapshot(
  list: Snapshot[],
  snapshot: Snapshot,
  cap = SNAPSHOT_CAP,
): Snapshot[] {
  return [snapshot, ...list].slice(0, cap);
}

/** Remove a snapshot by id (pure). */
export function removeSnapshot(list: Snapshot[], id: string): Snapshot[] {
  return list.filter((s) => s.id !== id);
}

/** Rename a snapshot by id, trimming whitespace; ignores blank names (pure). */
export function renameSnapshot(list: Snapshot[], id: string, name: string): Snapshot[] {
  const trimmed = name.trim();
  if (!trimmed) return list;
  return list.map((s) => (s.id === id ? { ...s, name: trimmed } : s));
}

function isSnapshot(value: unknown): value is Snapshot {
  if (typeof value !== "object" || value === null) return false;
  const s = value as Record<string, unknown>;
  return (
    typeof s.id === "string" &&
    typeof s.name === "string" &&
    typeof s.t === "number" &&
    typeof s.graph === "object" &&
    s.graph !== null
  );
}

/**
 * Parse a serialized snapshot array (from localStorage or a project file),
 * keeping only well-formed entries. Returns [] on any parse error.
 */
export function parseSnapshots(raw: string): Snapshot[] {
  try {
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter(isSnapshot);
  } catch {
    return [];
  }
}

/** Restore the persisted snapshot list (newest first), or [] if none/unreadable. */
export function loadSnapshots(): Snapshot[] {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return [];
    return parseSnapshots(raw);
  } catch {
    return [];
  }
}

/** Persist the snapshot list to localStorage (best-effort). */
export function saveSnapshots(list: Snapshot[]): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(list));
  } catch {
    // Quota exceeded / storage disabled — snapshots are best-effort.
  }
}

/** Read the "auto-snapshot before run" preference (defaults to true). */
export function loadAutoSnapshotPref(): boolean {
  try {
    return localStorage.getItem(AUTO_KEY) !== "0";
  } catch {
    return true;
  }
}

/** Persist the "auto-snapshot before run" preference (best-effort). */
export function saveAutoSnapshotPref(on: boolean): void {
  try {
    localStorage.setItem(AUTO_KEY, on ? "1" : "0");
  } catch {
    /* best-effort */
  }
}
