// Run log model + formatting helpers.
//
// Pure, renderer-agnostic: the editor feeds node/run events in and renders the
// resulting entries. Kept separate from the React component so the trimming and
// formatting logic is unit-testable without a DOM.

import type { StudioRunErrorDetail } from "../bridge/tauri";
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

/**
 * Compact `key=value` context suffix for a structured error detail, e.g.
 * `provider=replicate op=run code=poll_timeout request=req-42 retryable`.
 * Returns "" when the detail carries nothing beyond the flat message.
 */
export function formatErrorDetail(detail: StudioRunErrorDetail | null | undefined): string {
  if (!detail) return "";
  const parts: string[] = [];
  if (detail.provider) parts.push(`provider=${detail.provider}`);
  if (detail.operation) parts.push(`op=${detail.operation}`);
  if (detail.code) parts.push(`code=${detail.code}`);
  if (detail.provider_request_id) parts.push(`request=${detail.provider_request_id}`);
  if (detail.task_id) parts.push(`task=${detail.task_id}`);
  if (detail.retryable) parts.push("retryable");
  return parts.join(" ");
}

/** Human-readable one-liner describing a node status event. */
export function describeNodeStatus(
  status: NodeStatus,
  opts: {
    durationMs?: number | null;
    error?: string | null;
    detail?: StudioRunErrorDetail | null;
  } = {},
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
    case "failed": {
      const context = formatErrorDetail(opts.detail);
      const base = `failed: ${opts.error ?? "unknown error"}`;
      return context ? `${base} [${context}]` : base;
    }
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

/** Render the log as plain text, one entry per line, for export/clipboard. */
export function formatLogText(entries: RunLogEntry[]): string {
  return entries
    .map((e) => `${formatTime(e.t)} [${e.level}]${e.node ? ` ${e.node}` : ""} ${e.message}`)
    .join("\n");
}
