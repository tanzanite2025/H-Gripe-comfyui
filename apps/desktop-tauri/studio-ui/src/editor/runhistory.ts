// Run history model + persistence helpers.
//
// A RunRecord captures one Run/Batch execution -- when it started/ended, its
// outcome, and the log lines it produced -- so past runs can be reviewed after
// a refresh or on another machine, instead of the run log living only in memory
// (see runlog.ts). Mutation/parse helpers are pure for unit testing; the
// localStorage wrappers are best-effort and swallow storage errors. On desktop
// the records are persisted into the project folder (see App.tsx), mirroring
// the snapshot store.

import type { RunLogEntry } from "./runlog";

export type RunKind = "run" | "batch";
export type RunOutcome = "succeeded" | "failed" | "cancelled";

export interface RunRecord {
  id: string;
  kind: RunKind;
  /** Epoch ms when the run started. */
  startedAt: number;
  /** Epoch ms when the run ended. */
  endedAt: number;
  outcome: RunOutcome;
  /** "Rust backend" | "browser preview". */
  backend: string;
  /** Count of nodes that reported a failed status during the run. */
  failedNodes: number;
  /** The log lines produced during this run, in order. */
  entries: RunLogEntry[];
}

// Bump the suffix if the persisted shape changes incompatibly.
const STORAGE_KEY = "hgripe.studio.runhistory.v1";
/** Cap retained records so storage cannot grow without bound. */
export const RUN_HISTORY_CAP = 50;

/** Generate a reasonably unique run id. */
export function newRunRecordId(): string {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) return crypto.randomUUID();
  return `run-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

/**
 * Insert a record at the front (newest first), trimming to the cap.
 * Pure: returns a new array and does not mutate the input.
 */
export function addRunRecord(
  list: RunRecord[],
  record: RunRecord,
  cap = RUN_HISTORY_CAP,
): RunRecord[] {
  return [record, ...list].slice(0, cap);
}

/** Wall-clock duration of a run in ms (never negative). */
export function runDurationMs(record: RunRecord): number {
  return Math.max(0, record.endedAt - record.startedAt);
}

/** Compact one-line summary of a run for the history list. */
export function summarizeRun(record: RunRecord): string {
  const secs = (runDurationMs(record) / 1000).toFixed(1);
  const fail = record.failedNodes > 0 ? `, ${record.failedNodes} failed` : "";
  return `${record.kind} · ${record.outcome} · ${secs}s${fail}`;
}

function isRunLogEntry(value: unknown): value is RunLogEntry {
  if (typeof value !== "object" || value === null) return false;
  const e = value as Record<string, unknown>;
  return typeof e.id === "number" && typeof e.t === "number" && typeof e.message === "string";
}

function isRunRecord(value: unknown): value is RunRecord {
  if (typeof value !== "object" || value === null) return false;
  const r = value as Record<string, unknown>;
  return (
    typeof r.id === "string" &&
    (r.kind === "run" || r.kind === "batch") &&
    typeof r.startedAt === "number" &&
    typeof r.endedAt === "number" &&
    (r.outcome === "succeeded" || r.outcome === "failed" || r.outcome === "cancelled") &&
    Array.isArray(r.entries) &&
    r.entries.every(isRunLogEntry)
  );
}

/**
 * Parse a serialized run-history array (from localStorage or a project file),
 * keeping only well-formed records. Returns [] on any parse error.
 */
export function parseRunHistory(raw: string): RunRecord[] {
  try {
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter(isRunRecord);
  } catch {
    return [];
  }
}

/** Restore the persisted run history (newest first), or [] if none/unreadable. */
export function loadRunHistory(): RunRecord[] {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    return raw ? parseRunHistory(raw) : [];
  } catch {
    return [];
  }
}

/** Persist the run history to localStorage (best-effort). */
export function saveRunHistory(list: RunRecord[]): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(list));
  } catch {
    // Quota exceeded / storage disabled -- history is best-effort.
  }
}
