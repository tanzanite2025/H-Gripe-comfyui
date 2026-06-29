// Task + Studio-graph execution commands.

import { tauriInvoke, tauriListen } from "./core";
import type { UnlistenFn } from "./core";

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

export interface StudioGraphNodeRun {
  node_id: string;
  kind: string;
  status: string;
  duration_ms?: number | null;
  error?: string | null;
}

export interface StudioGraphRunResult {
  version: number;
  outputs: Record<string, Record<string, unknown>>;
  statuses: Record<string, string>;
  node_runs: StudioGraphNodeRun[];
}

export interface StudioGraphRunEvent {
  run_id: string;
  node_id?: string | null;
  kind?: string | null;
  status: string;
  duration_ms?: number | null;
  error?: string | null;
  message?: string | null;
}

const STUDIO_GRAPH_RUN_EVENT = "studio:graph-run";

export function createStudioRunId(): string {
  const random =
    typeof crypto !== "undefined" && "randomUUID" in crypto
      ? crypto.randomUUID()
      : Math.random().toString(36).slice(2);
  return `studio-${Date.now()}-${random}`;
}

/** Run a renderer-agnostic Studio WorkflowGraph through the Rust backend. */
export async function runStudioGraph(
  graph: unknown,
  onEvent?: (event: StudioGraphRunEvent) => void,
  runId = createStudioRunId(),
): Promise<StudioGraphRunResult> {
  const invoke = tauriInvoke();
  if (!invoke) {
    const version =
      typeof graph === "object" &&
      graph !== null &&
      "version" in graph &&
      typeof (graph as { version?: unknown }).version === "number"
        ? (graph as { version: number }).version
        : 1;
    return { version, outputs: {}, statuses: {}, node_runs: [] };
  }

  let unlisten: UnlistenFn | null = null;
  const listen = tauriListen();
  if (listen && onEvent) {
    try {
      unlisten = await listen<StudioGraphRunEvent>(STUDIO_GRAPH_RUN_EVENT, (event) => {
        if (event.payload?.run_id === runId) onEvent(event.payload);
      });
    } catch {
      unlisten = null;
    }
  }

  try {
    return (await invoke("run_studio_graph", {
      graphJson: JSON.stringify(graph),
      runId,
    })) as StudioGraphRunResult;
  } finally {
    unlisten?.();
  }
}

/** Request cancellation for an in-flight Studio graph run. */
export async function cancelStudioRun(runId: string): Promise<void> {
  const invoke = tauriInvoke();
  if (!invoke) return;
  await invoke("cancel_studio_run", { runId });
}
