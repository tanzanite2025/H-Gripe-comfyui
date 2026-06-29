// Run log model + formatting helpers.
//
// Pure, renderer-agnostic: the editor feeds node/run events in and renders the
// resulting entries. Kept separate from the React component so the trimming and
// formatting logic is unit-testable without a DOM.

import type { NodeStatus } from "../runtime/dag";

export type LogLevel = "info" | "success" | "error" | "warn";

export interface RunLogEntry {
  /** Monotonic id, used as a stable React key. */
  id: number;
  /** Epoch ms when the entry was recorded. */
  t: number;
  level: LogLevel;
  /** Node id when this is a node-level event; omitted for run-level lines. */
  node?: string;
  message: string;
}

/** Default cap on retained entries so a long session cannot grow unbounded. */
export const RUN_LOG_CAP = 300;

/** Append an entry, keeping at most `cap` newest entries (oldest trimmed). */
export function appendLog(
  log: RunLogEntry[],
  entry: RunLogEntry,
  cap = RUN_LOG_CAP,
): RunLogEntry[] {
  const next = log.length >= cap ? log.slice(log.length - cap + 1) : log.slice();
  next.push(entry);
  return next;
}

/** Map a node status to the log level it should be rendered at. */
export function levelForStatus(status: NodeStatus): LogLevel {
  switch (status) {
    case "succeeded":
    case "cached":
      return "success";
    case "failed":
      return "error";
    case "skipped":
    case "cancelled":
      return "warn";
    default:
      return "info";
  }
}

/** Human-readable one-liner describing a node status event. */
export function describeNodeStatus(
  status: NodeStatus,
  opts: { durationMs?: number | null; error?: string | null } = {},
): string {
  switch (status) {
    case "queued":
      return "queued";
    case "running":
      return "running…";
    case "succeeded":
      return opts.durationMs != null ? `done in ${Math.round(opts.durationMs)} ms` : "done";
    case "cached":
      return "cached (unchanged)";
    case "failed":
      return `failed: ${opts.error ?? "unknown error"}`;
    case "skipped":
      return "skipped (branch not taken)";
    case "cancelled":
      return "cancelled";
    default:
      return status;
  }
}

/** `HH:MM:SS` local-time stamp for an entry. */
export function formatTime(t: number): string {
  const d = new Date(t);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}
