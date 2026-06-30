import { useCallback, useMemo, useRef, useState, type Dispatch, type SetStateAction } from "react";
import type { Edge, Node } from "@xyflow/react";

import type { HgripeNodeData } from "./HgripeNode";
import { toWorkflowGraph } from "./adapter";
import { psdExportTargets, psdTemplatePaths, validatePsdChain } from "./psdcheck";
import {
  appendLog,
  describeNodeStatus,
  formatLogText,
  levelForStatus,
  type LogLevel,
  type RunLogEntry,
} from "./runlog";
import {
  addRunRecord,
  loadRunHistory,
  newRunRecordId,
  parseRunHistory,
  saveRunHistory,
  type RunKind,
  type RunOutcome,
  type RunRecord,
} from "./runhistory";
import { useProjectScopedStore } from "./useProjectScopedStore";
import type { WorkflowGraph } from "../graph/model";
import { ancestorSubgraph, runGraph, type NodeRunInfo, type NodeStatus } from "../runtime/dag";
import { batchItems, defaultExecutors } from "../runtime/executors";
import {
  cancelStudioRun,
  createStudioRunId,
  inspectPsd,
  isTauri,
  readStudioRunHistory,
  runStudioGraph,
  writeStudioRunHistory,
  type StudioGraphRunEvent,
  type StudioGraphRunResult,
} from "../bridge/tauri";

const NODE_STATUSES = new Set<NodeStatus>([
  "idle",
  "queued",
  "running",
  "succeeded",
  "failed",
  "cancelled",
  "cached",
  "skipped",
]);

function toNodeStatus(status: string): NodeStatus {
  return NODE_STATUSES.has(status as NodeStatus) ? (status as NodeStatus) : "failed";
}

function studioOutputsToMap(
  result: StudioGraphRunResult,
): Map<string, Record<string, unknown>> {
  return new Map(Object.entries(result.outputs));
}

function graphWithParamOverrides(
  graph: WorkflowGraph,
  nodeId: string,
  params: Record<string, unknown>,
): WorkflowGraph {
  return {
    ...graph,
    nodes: graph.nodes.map((node) =>
      node.id === nodeId ? { ...node, params: { ...node.params, ...params } } : node,
    ),
  };
}

export interface StudioRunControllerOptions {
  /** Live editor graph (used to build the workflow to run). */
  nodes: Node[];
  edges: Edge[];
  /** React Flow node setter, for applying statuses/outputs back onto cards. */
  setNodes: Dispatch<SetStateAction<Node[]>>;
  /** Patch a single node's data (status, duration, preview paths, …). */
  patchNode: (id: string, patch: Partial<HgripeNodeData>) => void;
  /** Select/focus a node in the editor (used to surface the first failure). */
  focusNode: (nodeId: string) => void;
  /** Surface a status-bar message. */
  setMessage: (message: string) => void;
  /** Auto-capture a snapshot before a run (when enabled). */
  autoSnapshotBeforeRun: () => void;
  /** Sink folder for project-scoped run history (null → localStorage). */
  projectStoreDir: string | null;
}

export interface StudioRunController {
  /** True while a run/batch is in flight. */
  running: boolean;
  /** Active Rust run id (desktop), or null. */
  currentRunId: string | null;
  /** Whether the in-flight run can be cancelled (true for either backend). */
  canCancel: boolean;
  /** Append-only run log (capped). */
  runLog: RunLogEntry[];
  showLog: boolean;
  setShowLog: Dispatch<SetStateAction<boolean>>;
  clearLog: () => void;
  /** Download the run log as a plain-text file. */
  exportLog: () => void;
  /** Persisted run history (project-scoped). */
  runHistory: RunRecord[];
  showHistory: boolean;
  setShowHistory: Dispatch<SetStateAction<boolean>>;
  /** Clear all run history (prompts when non-empty). */
  clearHistory: () => void;
  /** Run the current graph once. */
  run: () => Promise<void>;
  /** Run only `nodeId` and its transitive inputs, then surface its result. */
  runUpToNode: (nodeId: string) => Promise<void>;
  /** Run the graph once per item of the (first) batch node. */
  runBatch: () => Promise<void>;
  /** Request cancellation of the active run (Rust backend or browser preview). */
  cancelRun: () => void;
  /** Whether the graph contains a batch node. */
  hasBatch: boolean;
  /** Number of items the batch node fans out to. */
  batchCount: number;
}

// Owns the studio run lifecycle along with its run log and run history:
// executes the graph (Rust backend on desktop, browser-preview executors
// otherwise), streams per-node status into the log, finalizes each run into a
// persisted history record, and exposes the log/history view toggles. The
// editor (graph mutation, file/project state) stays in the caller and is
// reached through the supplied callbacks.
export function useStudioRunController({
  nodes,
  edges,
  setNodes,
  patchNode,
  focusNode,
  setMessage,
  autoSnapshotBeforeRun,
  projectStoreDir,
}: StudioRunControllerOptions): StudioRunController {
  const [running, setRunning] = useState(false);
  const [currentRunId, setCurrentRunId] = useState<string | null>(null);
  const [runLog, setRunLog] = useState<RunLogEntry[]>([]);
  const [showLog, setShowLog] = useState(false);
  const [runHistory, setRunHistory] = useState<RunRecord[]>(() => loadRunHistory());
  const [showHistory, setShowHistory] = useState(false);

  // True from the moment a run/batch starts until it settles. Guards against
  // re-entrancy (e.g. the keyboard shortcut firing while a run is in flight),
  // which would otherwise let two runs clobber each other's refs and history.
  const inFlight = useRef(false);
  // Cooperative cancel token for the active browser-preview run (no server-side
  // cancel exists for that backend); null when no browser run is in flight.
  const browserCancel = useRef<{ cancelled: boolean } | null>(null);
  // While a run is in flight this collects that run's log entries so they can
  // be saved as a RunRecord when it ends; null when no run is active.
  const runEntriesRef = useRef<RunLogEntry[] | null>(null);
  const logSeq = useRef(0);
  // Node ids that reported "failed" during the in-flight run, in first-seen order.
  const runFailures = useRef<string[]>([]);
  const currentRunIdRef = useRef<string | null>(null);

  const setStatus = useCallback(
    (id: string, status: NodeStatus) => patchNode(id, { status }),
    [patchNode],
  );

  // Append a line to the run log (capped, never mutating the previous array).
  const pushLog = useCallback((level: LogLevel, message: string, node?: string) => {
    const entry: RunLogEntry = { id: logSeq.current++, t: Date.now(), level, message, node };
    setRunLog((log) => appendLog(log, entry));
    if (runEntriesRef.current) runEntriesRef.current.push(entry);
  }, []);

  // Finalize the in-flight run into a persisted history record. Promotes a
  // nominal "succeeded" to "failed" when any node reported a failure.
  const recordRunHistory = useCallback(
    (kind: RunKind, startedAt: number, outcome: RunOutcome, backend: string) => {
      const entries = runEntriesRef.current ?? [];
      runEntriesRef.current = null;
      const failedNodes = runFailures.current.length;
      const finalOutcome: RunOutcome =
        outcome === "succeeded" && failedNodes > 0 ? "failed" : outcome;
      const record: RunRecord = {
        id: newRunRecordId(),
        kind,
        startedAt,
        endedAt: Date.now(),
        outcome: finalOutcome,
        backend,
        failedNodes,
        entries,
      };
      setRunHistory((h) => addRunRecord(h, record));
    },
    [],
  );

  // Download the run log as a plain-text file (browser + desktop webview).
  const exportLog = useCallback(() => {
    const blob = new Blob([formatLogText(runLog)], { type: "text/plain" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = "run-log.txt";
    a.click();
    URL.revokeObjectURL(url);
  }, [runLog]);

  // After a run settles, surface any failed nodes: select/focus the first one
  // and summarise them in the log. Returns the number of failed nodes.
  const highlightFailures = useCallback(() => {
    const failed = runFailures.current;
    if (failed.length === 0) return 0;
    focusNode(failed[0]);
    pushLog("error", `⚠ ${failed.length} node(s) failed: ${failed.join(", ")}`);
    return failed.length;
  }, [focusNode, pushLog]);

  // Per-node run telemetry (duration / error) for node-level logs/progress.
  const recordRun = useCallback(
    (id: string, info: NodeRunInfo) => {
      patchNode(id, { durationMs: info.durationMs, error: info.error ?? null });
      if (info.status === "failed" && !runFailures.current.includes(id)) {
        runFailures.current.push(id);
      }
      pushLog(
        levelForStatus(info.status),
        describeNodeStatus(info.status, { durationMs: info.durationMs, error: info.error }),
        id,
      );
    },
    [patchNode, pushLog],
  );

  // Clear the previous run's duration/error before a fresh run.
  const clearRunInfo = useCallback(
    () => setNodes((ns) => ns.map((n) => ({ ...n, data: { ...n.data, durationMs: undefined, error: undefined } }))),
    [setNodes],
  );
  const observer = useMemo(() => ({ onStatus: setStatus, onNodeRun: recordRun }), [setStatus, recordRun]);

  const applyStudioRunResult = useCallback(
    (result: StudioGraphRunResult) => {
      const statuses = new Map(
        Object.entries(result.statuses).map(([id, status]) => [id, toNodeStatus(status)]),
      );
      const runs = new Map(result.node_runs.map((run) => [run.node_id, run]));
      setNodes((ns) =>
        ns.map((n) => {
          const d = n.data as HgripeNodeData;
          const runInfo = runs.get(n.id);
          return {
            ...n,
            data: {
              ...d,
              status: statuses.get(n.id) ?? d.status,
              durationMs: runInfo ? runInfo.duration_ms ?? undefined : d.durationMs,
              error: runInfo ? runInfo.error ?? null : d.error,
            },
          };
        }),
      );
    },
    [setNodes],
  );

  const applyStudioRunEvent = useCallback(
    (event: StudioGraphRunEvent) => {
      if (!event.node_id) {
        if (event.message) pushLog("info", event.message);
        return;
      }
      const status = toNodeStatus(event.status);
      if (status === "failed" && !runFailures.current.includes(event.node_id)) {
        runFailures.current.push(event.node_id);
      }
      pushLog(
        levelForStatus(status),
        describeNodeStatus(status, { durationMs: event.duration_ms, error: event.error }),
        event.node_id,
      );
      setNodes((ns) =>
        ns.map((n) => {
          if (n.id !== event.node_id) return n;
          const d = n.data as HgripeNodeData;
          const isFreshStatus = status === "queued" || status === "running";
          return {
            ...n,
            data: {
              ...d,
              status,
              durationMs: event.duration_ms ?? (isFreshStatus ? undefined : d.durationMs),
              error: event.error ?? (isFreshStatus ? undefined : d.error),
            },
          };
        }),
      );
    },
    [setNodes, pushLog],
  );

  const beginRustRun = useCallback(() => {
    const runId = createStudioRunId();
    currentRunIdRef.current = runId;
    setCurrentRunId(runId);
    return runId;
  }, []);

  const endRustRun = useCallback((runId: string) => {
    if (currentRunIdRef.current !== runId) return;
    currentRunIdRef.current = null;
    setCurrentRunId(null);
  }, []);

  const cancelRun = useCallback(() => {
    const runId = currentRunIdRef.current;
    if (runId) {
      setMessage("cancelling…");
      pushLog("warn", "✋ cancellation requested");
      void cancelStudioRun(runId).catch((err) => setMessage(`cancel failed: ${String(err)}`));
      return;
    }
    // Browser-preview runs have no backend to call: flip the cooperative token
    // so runGraph aborts before its next node.
    if (browserCancel.current) {
      browserCancel.current.cancelled = true;
      setMessage("cancelling…");
      pushLog("warn", "✋ cancellation requested");
    }
  }, [pushLog, setMessage]);

  // Surface output paths into preview nodes. The thumbnail itself is fetched
  // lazily by the node when it scrolls into view (see HgripeNode).
  const applyPreviews = useCallback(
    (graph: ReturnType<typeof toWorkflowGraph>, result: { outputs: Map<string, Record<string, unknown>> }) => {
      const paths: string[] = [];
      const str = (v: unknown): string | null => (typeof v === "string" && v ? v : null);
      for (const node of graph.nodes) {
        const out = result.outputs.get(node.id);
        if (node.kind === "preview") {
          const imagePath = str(out?.image);
          patchNode(node.id, { imagePath });
          if (imagePath) paths.push(imagePath);
        } else if (node.kind === "psdExport") {
          // Surface the export triplet onto the card. Browser executors return
          // camelCase; the Rust backend returns the raw snake_case fields.
          const psdPath = str(out?.psdPath) ?? str(out?.psd_path);
          const psdPreviewPath = str(out?.previewPath) ?? str(out?.preview_path);
          const psdMetadataPath = str(out?.metadataPath) ?? str(out?.metadata_path);
          patchNode(node.id, {
            psdPath,
            psdPreviewPath,
            psdMetadataPath,
            placeholderKind: str(out?.placeholderKind) ?? str(out?.placeholder_kind),
            smartObjectMode: str(out?.smartObjectMode) ?? str(out?.smart_object_mode),
          });
          if (psdPreviewPath) paths.push(psdPreviewPath);
        }
      }
      return paths;
    },
    [patchNode],
  );

  // Surface PSD-chain problems (missing template path / unconnected inputs) in
  // the run log before executing, so users do not have to wait for a mid-run
  // failure to find them.
  const warnPsdChain = useCallback(
    async (graph: WorkflowGraph) => {
      for (const w of validatePsdChain(graph)) pushLog("warn", `⚠ ${w.node}: ${w.message}`);
      // Beyond the syntactic checks above, confirm against the real files on
      // disk. This needs the Python/psd-tools backend, so it is desktop-only;
      // browser preview keeps just the path-shape check.
      if (!isTauri()) return;
      for (const tpl of psdTemplatePaths(graph)) {
        try {
          const info = await inspectPsd(tpl.path);
          if (info && !info.exists) {
            pushLog("warn", `⚠ ${tpl.node}: PSD Template: file not found on disk (${tpl.path})`);
          }
        } catch (err) {
          pushLog("warn", `⚠ ${tpl.node}: PSD Template: could not inspect (${String(err)})`);
        }
      }
      for (const tgt of psdExportTargets(graph)) {
        if (!tgt.placeholder || !tgt.templatePath) continue;
        try {
          const info = await inspectPsd(tgt.templatePath, [tgt.placeholder]);
          if (info && info.exists && info.missing.includes(tgt.placeholder)) {
            const available = info.layers
              .map((l) => l.name)
              .filter(Boolean)
              .slice(0, 12)
              .join(", ");
            pushLog(
              "warn",
              `⚠ ${tgt.node}: PSD Export: placeholder layer "${tgt.placeholder}" not found in PSD${available ? ` (available: ${available})` : ""}`,
            );
          }
        } catch (err) {
          pushLog("warn", `⚠ ${tgt.node}: PSD Export: could not inspect template (${String(err)})`);
        }
      }
    },
    [pushLog],
  );

  const run = useCallback(async () => {
    if (inFlight.current) return;
    inFlight.current = true;
    setRunning(true);
    setShowLog(true);
    runFailures.current = [];
    autoSnapshotBeforeRun();
    const useRustBackend = isTauri();
    const backend = useRustBackend ? "Rust backend" : "browser preview";
    setMessage(useRustBackend ? "running Rust backend…" : "running browser preview…");
    clearRunInfo();
    const startedAt = Date.now();
    runEntriesRef.current = [];
    let outcome: RunOutcome = "succeeded";
    pushLog("info", `▶ run started (${backend})`);
    try {
      const graph = toWorkflowGraph(nodes, edges);
      await warnPsdChain(graph);
      if (useRustBackend) {
        const runId = beginRustRun();
        try {
          const result = await runStudioGraph(graph, applyStudioRunEvent, runId);
          applyStudioRunResult(result);
          applyPreviews(graph, { outputs: studioOutputsToMap(result) });
          setMessage("done (Rust backend)");
        } finally {
          endRustRun(runId);
        }
      } else {
        const token = { cancelled: false };
        browserCancel.current = token;
        try {
          const result = await runGraph(
            graph,
            defaultExecutors,
            observer,
            undefined,
            () => token.cancelled,
          );
          applyPreviews(graph, result);
          setMessage("done (browser preview)");
        } finally {
          browserCancel.current = null;
        }
      }
      pushLog("success", `✔ run finished (${backend})`);
    } catch (err) {
      const message = String(err);
      const cancelled = message.toLowerCase().includes("cancel");
      outcome = cancelled ? "cancelled" : "failed";
      setMessage(cancelled ? "cancelled" : `error: ${message}`);
      pushLog(cancelled ? "warn" : "error", cancelled ? "run cancelled" : `run failed: ${message}`);
    } finally {
      setRunning(false);
      inFlight.current = false;
      browserCancel.current = null;
      highlightFailures();
      recordRunHistory("run", startedAt, outcome, backend);
    }
  }, [
    nodes,
    edges,
    observer,
    clearRunInfo,
    applyPreviews,
    applyStudioRunResult,
    applyStudioRunEvent,
    beginRustRun,
    endRustRun,
    pushLog,
    autoSnapshotBeforeRun,
    highlightFailures,
    warnPsdChain,
    recordRunHistory,
    setMessage,
  ]);

  // Run only the target node + its transitive inputs (ancestor subgraph), so
  // confirming an edit surfaces that node's result without executing unrelated
  // downstream branches. Reuses the same executor + preview machinery as run().
  const runUpToNode = useCallback(
    async (nodeId: string) => {
      if (inFlight.current) return;
      inFlight.current = true;
      setRunning(true);
      setShowLog(true);
      runFailures.current = [];
      autoSnapshotBeforeRun();
      const useRustBackend = isTauri();
      const backend = useRustBackend ? "Rust backend" : "browser preview";
      setMessage(useRustBackend ? "running to node (Rust backend)…" : "running to node (browser preview)…");
      clearRunInfo();
      const startedAt = Date.now();
      runEntriesRef.current = [];
      let outcome: RunOutcome = "succeeded";
      pushLog("info", `▶ run up to ${nodeId} started (${backend})`);
      try {
        const full = toWorkflowGraph(nodes, edges);
        const graph = ancestorSubgraph(full, nodeId);
        await warnPsdChain(graph);
        if (useRustBackend) {
          const runId = beginRustRun();
          try {
            const result = await runStudioGraph(graph, applyStudioRunEvent, runId);
            applyStudioRunResult(result);
            applyPreviews(graph, { outputs: studioOutputsToMap(result) });
            setMessage("done (Rust backend)");
          } finally {
            endRustRun(runId);
          }
        } else {
          const token = { cancelled: false };
          browserCancel.current = token;
          try {
            const result = await runGraph(
              graph,
              defaultExecutors,
              observer,
              undefined,
              () => token.cancelled,
            );
            applyPreviews(graph, result);
            setMessage("done (browser preview)");
          } finally {
            browserCancel.current = null;
          }
        }
        pushLog("success", `✔ run up to ${nodeId} finished (${backend})`);
      } catch (err) {
        const message = String(err);
        const cancelled = message.toLowerCase().includes("cancel");
        outcome = cancelled ? "cancelled" : "failed";
        setMessage(cancelled ? "cancelled" : `error: ${message}`);
        pushLog(cancelled ? "warn" : "error", cancelled ? "run cancelled" : `run failed: ${message}`);
      } finally {
        setRunning(false);
        inFlight.current = false;
        browserCancel.current = null;
        highlightFailures();
        recordRunHistory("run", startedAt, outcome, backend);
      }
    },
    [
      nodes,
      edges,
      observer,
      clearRunInfo,
      applyPreviews,
      applyStudioRunResult,
      applyStudioRunEvent,
      beginRustRun,
      endRustRun,
      pushLog,
      autoSnapshotBeforeRun,
      highlightFailures,
      warnPsdChain,
      recordRunHistory,
      setMessage,
    ],
  );

  // Batch fan-out: run the graph once per item of the (first) batch node,
  // sweeping its `index`. In Tauri, the graph is copied with an index override
  // and sent to Rust; in browser preview, runGraph uses paramOverrides.
  const batchNode = useMemo(
    () => nodes.find((n) => (n.data as HgripeNodeData).kind === "batch") ?? null,
    [nodes],
  );
  const batchCount = useMemo(
    () => (batchNode ? batchItems((batchNode.data as HgripeNodeData).params.items).length : 0),
    [batchNode],
  );

  const runBatch = useCallback(async () => {
    if (!batchNode || batchCount === 0) {
      setMessage("batch: no items");
      return;
    }
    if (inFlight.current) return;
    inFlight.current = true;
    setRunning(true);
    setShowLog(true);
    runFailures.current = [];
    autoSnapshotBeforeRun();
    clearRunInfo();
    const useRustBackend = isTauri();
    const backend = useRustBackend ? "Rust backend" : "browser preview";
    const rustRunId = useRustBackend ? beginRustRun() : null;
    const startedAt = Date.now();
    runEntriesRef.current = [];
    let outcome: RunOutcome = "succeeded";
    pushLog("info", `▶ batch started: ${batchCount} run(s) (${backend})`);
    const browserToken = useRustBackend ? null : { cancelled: false };
    if (browserToken) browserCancel.current = browserToken;
    try {
      const graph = toWorkflowGraph(nodes, edges);
      await warnPsdChain(graph);
      const collected: string[] = [];
      for (let i = 0; i < batchCount; i++) {
        if (browserToken?.cancelled) throw new Error("batch cancelled");
        setMessage(
          `batch ${i + 1}/${batchCount}${useRustBackend ? " (Rust backend)" : " (browser preview)"}…`,
        );
        pushLog("info", `— batch item ${i + 1}/${batchCount}`);
        if (useRustBackend) {
          const graphForRun = graphWithParamOverrides(graph, batchNode.id, { index: i });
          const result = await runStudioGraph(graphForRun, applyStudioRunEvent, rustRunId ?? undefined);
          applyStudioRunResult(result);
          collected.push(...applyPreviews(graphForRun, { outputs: studioOutputsToMap(result) }));
        } else {
          const overrides = new Map([[batchNode.id, { index: i }]]);
          const result = await runGraph(
            graph,
            defaultExecutors,
            observer,
            overrides,
            () => browserToken?.cancelled ?? false,
          );
          collected.push(...applyPreviews(graph, result));
        }
      }
      setMessage(
        `batch done: ${batchCount} run(s), ${collected.length} output(s)${
          useRustBackend ? " via Rust backend" : ""
        }`,
      );
      pushLog("success", `✔ batch finished: ${batchCount} run(s), ${collected.length} output(s)`);
    } catch (err) {
      const message = String(err);
      const cancelled = message.toLowerCase().includes("cancel");
      outcome = cancelled ? "cancelled" : "failed";
      setMessage(cancelled ? "batch cancelled" : `batch error: ${message}`);
      pushLog(cancelled ? "warn" : "error", cancelled ? "batch cancelled" : `batch failed: ${message}`);
    } finally {
      if (rustRunId) endRustRun(rustRunId);
      setRunning(false);
      inFlight.current = false;
      browserCancel.current = null;
      highlightFailures();
      recordRunHistory("batch", startedAt, outcome, backend);
    }
  }, [
    batchNode,
    batchCount,
    nodes,
    edges,
    observer,
    clearRunInfo,
    applyPreviews,
    applyStudioRunResult,
    applyStudioRunEvent,
    beginRustRun,
    endRustRun,
    pushLog,
    autoSnapshotBeforeRun,
    highlightFailures,
    warnPsdChain,
    recordRunHistory,
    setMessage,
  ]);

  // Run history is a project-scoped store: persisted into the selected project
  // folder on desktop (so it travels with the project), else to localStorage.
  // The shared hook owns the load/persist effects.
  useProjectScopedStore({
    dir: projectStoreDir,
    state: runHistory,
    setState: setRunHistory,
    parse: parseRunHistory,
    read: readStudioRunHistory,
    write: writeStudioRunHistory,
    saveLocal: saveRunHistory,
    label: "run history",
    onError: setMessage,
  });

  const clearLog = useCallback(() => setRunLog([]), []);

  const clearHistory = useCallback(() => {
    setRunHistory((h) => (h.length === 0 || window.confirm("Clear all run history?") ? [] : h));
  }, []);

  return {
    running,
    currentRunId,
    canCancel: running,
    runLog,
    showLog,
    setShowLog,
    clearLog,
    exportLog,
    runHistory,
    showHistory,
    setShowHistory,
    clearHistory,
    run,
    runUpToNode,
    runBatch,
    cancelRun,
    hasBatch: !!batchNode,
    batchCount,
  };
}
