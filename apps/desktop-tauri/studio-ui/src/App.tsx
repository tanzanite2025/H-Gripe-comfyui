import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ReactFlowProvider,
  useEdgesState,
  useNodesState,
  useReactFlow,
  type Edge,
  type Node,
  type OnNodesChange,
  type OnEdgesChange,
  type NodePositionChange,
} from "@xyflow/react";

import { FlowCanvas, type EdgeStyle } from "./editor/FlowCanvas";
import { Inspector } from "./editor/Inspector";
import { Palette } from "./editor/Palette";
import { ContextMenu, type MenuItem } from "./editor/ContextMenu";
import { NodeEditingContext } from "./editor/editingContext";
import { useHistory } from "./editor/useHistory";
import { buildPaste, clipFromSelection, type Clip } from "./editor/clipboard";
import {
  detachChildren,
  findContainingGroup,
  isGroupNode,
  makeGroupNode,
  orderNodes,
  reparentNode,
} from "./editor/grouping";
import { getHelperLines } from "./editor/helperLines";
import { layeredPositions } from "./editor/layout";
import type { HgripeNodeData } from "./editor/HgripeNode";
import { fromWorkflowGraph, toWorkflowGraph } from "./editor/adapter";
import { ProjectPanel, baseName } from "./editor/ProjectPanel";
import { RunLog } from "./editor/RunLogPanel";
import {
  appendLog,
  describeNodeStatus,
  formatLogText,
  levelForStatus,
  type LogLevel,
  type RunLogEntry,
} from "./editor/runlog";
import { SnapshotsPanel } from "./editor/SnapshotsPanel";
import {
  addSnapshot,
  loadSnapshots,
  newSnapshotId,
  removeSnapshot,
  renameSnapshot,
  saveSnapshots,
  type Snapshot,
} from "./editor/snapshots";
import { clearPersistedGraph, loadPersistedGraph, persistGraph } from "./editor/persist";
import { defaultParams } from "./graph/nodeSpecs";
import { deserializeGraph, serializeGraph, type WorkflowGraph } from "./graph/model";
import { runGraph, topoLevels, validateGraph, type NodeRunInfo, type NodeStatus } from "./runtime/dag";
import { batchItems, defaultExecutors } from "./runtime/executors";
import {
  cancelStudioRun,
  clearStudioAutosave,
  createStudioRunId,
  deleteStudioWorkflow,
  duplicateStudioWorkflow,
  isTauri,
  listStudioWorkflows,
  pickProjectFolder,
  pickWorkflowOpenPath,
  pickWorkflowSavePath,
  readStudioAutosave,
  readStudioRecents,
  readStudioWorkflow,
  renameStudioWorkflow,
  runStudioGraph,
  writeStudioAutosave,
  writeStudioRecents,
  writeStudioWorkflow,
  type StudioGraphRunEvent,
  type StudioGraphRunResult,
  type StudioWorkflowFile,
} from "./bridge/tauri";

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

function makeNode(id: string, kind: string, x: number, y: number, params?: Record<string, unknown>): Node {
  const data: HgripeNodeData = { kind, params: { ...defaultParams(kind), ...params }, status: "idle" };
  return { id, type: "hgripe", position: { x, y }, data };
}

// Minimal pre-wired workflow: Prompt -> Generate -> Preview.
const initialNodes: Node[] = [
  makeNode("prompt-1", "prompt", 40, 120, { text: "a watercolor fox" }),
  makeNode("generate-1", "generate", 360, 80),
  makeNode("preview-1", "preview", 700, 120),
];
const initialEdges: Edge[] = [
  { id: "e1", source: "prompt-1", sourceHandle: "text", target: "generate-1", targetHandle: "prompt" },
  { id: "e2", source: "generate-1", sourceHandle: "image", target: "preview-1", targetHandle: "image" },
];

function Studio() {
  // Restore the last autosaved workflow from this workspace; fall back to the
  // pre-wired sample graph on a fresh / unreadable workspace.
  const initial = useMemo(() => {
    const restored = loadPersistedGraph();
    if (restored && restored.nodes.length) return fromWorkflowGraph(restored);
    return { nodes: initialNodes, edges: initialEdges };
  }, []);
  const restoredOnMount = useRef(initial.nodes !== initialNodes);

  const [nodes, setNodes, onNodesChange] = useNodesState(initial.nodes);
  const [edges, setEdges, onEdgesChange] = useEdgesState(initial.edges);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [running, setRunning] = useState(false);
  const [currentRunId, setCurrentRunId] = useState<string | null>(null);
  const [runLog, setRunLog] = useState<RunLogEntry[]>([]);
  const [showLog, setShowLog] = useState(false);
  const [snapshots, setSnapshots] = useState<Snapshot[]>(() => loadSnapshots());
  const [showSnapshots, setShowSnapshots] = useState(false);
  const [snapToGrid, setSnapToGrid] = useState(false);
  const [helperLines, setHelperLines] = useState<{ horizontal?: number; vertical?: number }>({});
  const [edgeType, setEdgeType] = useState<EdgeStyle>("default");
  const [showMinimap, setShowMinimap] = useState(true);
  const [menu, setMenu] = useState<{ x: number; y: number; nodeId: string | null } | null>(null);
  const { fitView } = useReactFlow();
  const [saved, setSaved] = useState(restoredOnMount.current);
  const [message, setMessage] = useState<string>(
    isTauri()
      ? restoredOnMount.current
        ? "restored last workflow"
        : ""
      : "browser preview (backend mocked)",
  );
  const [desktopAutosaveReady, setDesktopAutosaveReady] = useState(!isTauri());

  // Explicit save/open + project folder. `currentFile` is the on-disk workflow
  // backing the editor (null = untitled); `fileDirty` tracks unsaved edits
  // against it (separate from the workspace autosave indicator).
  const isDesktop = isTauri();
  const [projectDir, setProjectDir] = useState<string | null>(null);
  const [workflowFiles, setWorkflowFiles] = useState<StudioWorkflowFile[]>([]);
  const [recentFiles, setRecentFiles] = useState<string[]>([]);
  const [currentFile, setCurrentFile] = useState<string | null>(null);
  const [fileDirty, setFileDirty] = useState(false);
  const [showProject, setShowProject] = useState(false);
  const [projectBusy, setProjectBusy] = useState(false);
  const [recentsReady, setRecentsReady] = useState(!isTauri());
  // Skips the next dirty-mark when the graph is swapped programmatically
  // (mount restore, open, new) rather than by a user edit.
  const skipDirty = useRef(true);

  const idSeq = useRef(0);
  const logSeq = useRef(0);
  const currentRunIdRef = useRef<string | null>(null);
  const fileInput = useRef<HTMLInputElement | null>(null);
  const clipboard = useRef<Clip | null>(null);
  // True while a node drag is in progress, so we snapshot only once per drag.
  const dragging = useRef(false);
  // Coalesce rapid edits to the same param (e.g. typing) into one undo step.
  const lastParamEdit = useRef<{ id: string; key: string; t: number } | null>(null);

  const newNodeId = useCallback((kind: string) => `${kind}-${Date.now()}-${idSeq.current++}`, []);

  const history = useHistory({ nodes, edges, setNodes, setEdges });
  const { takeSnapshot, undo, redo } = history;

  const selectedNode = useMemo(
    () => nodes.find((n) => n.id === selectedId) ?? null,
    [nodes, selectedId],
  );

  useEffect(() => {
    if (!isTauri()) return;
    let cancelled = false;
    void readStudioAutosave()
      .then((raw) => {
        if (cancelled || !raw) return;
        const graph = deserializeGraph(raw);
        const next = fromWorkflowGraph(graph);
        skipDirty.current = true;
        setNodes(next.nodes);
        setEdges(next.edges);
        setSelectedId(null);
        setSaved(true);
        setMessage("restored desktop workflow");
      })
      .catch((err) => {
        if (!cancelled) setMessage(`desktop autosave restore failed: ${String(err)}`);
      })
      .finally(() => {
        if (!cancelled) setDesktopAutosaveReady(true);
      });
    return () => {
      cancelled = true;
    };
  }, [setNodes, setEdges]);

  // Static validation surfaced in the toolbar (type mismatches, cycles, …).
  const issues = useMemo(
    () => validateGraph(toWorkflowGraph(nodes, edges)),
    [nodes, edges],
  );

  const patchNode = useCallback(
    (id: string, patch: Partial<HgripeNodeData>) => {
      setNodes((ns) =>
        ns.map((n) => (n.id === id ? { ...n, data: { ...(n.data as HgripeNodeData), ...patch } } : n)),
      );
    },
    [setNodes],
  );

  const onParamChange = useCallback(
    (id: string, key: string, value: unknown) => {
      const now = Date.now();
      const last = lastParamEdit.current;
      const coalesce = last && last.id === id && last.key === key && now - last.t < 600;
      if (!coalesce) takeSnapshot();
      lastParamEdit.current = { id, key, t: now };
      setNodes((ns) =>
        ns.map((n) => {
          if (n.id !== id) return n;
          const d = n.data as HgripeNodeData;
          return { ...n, data: { ...d, params: { ...d.params, [key]: value } } };
        }),
      );
    },
    [setNodes],
  );

  const addNode = useCallback(
    (kind: string, position?: { x: number; y: number }) => {
      takeSnapshot();
      const id = newNodeId(kind);
      // Click-to-add cascades nodes so they do not stack exactly.
      const pos = position ?? { x: 80 + (idSeq.current % 6) * 36, y: 80 + (idSeq.current % 6) * 36 };
      if (kind === "group") {
        // Group frames go to the front of the array (painted behind, parents
        // before children).
        setNodes((ns) => orderNodes([makeGroupNode(id, pos.x, pos.y), ...ns]));
        return;
      }
      setNodes((ns) => ns.concat(makeNode(id, kind, pos.x, pos.y)));
    },
    [setNodes, takeSnapshot, newNodeId],
  );

  // After a drag, (re)assign the node to whatever group frame now contains it,
  // or detach it when dropped outside every group. Groups themselves are never
  // reparented. The pre-drag snapshot (taken on drag start) covers the undo.
  const handleNodeDragStop = useCallback(
    (dragged: Node) => {
      if (isGroupNode(dragged)) return;
      setNodes((ns) => {
        const merged = ns.map((n) =>
          n.id === dragged.id
            ? { ...n, position: dragged.position, parentId: dragged.parentId, measured: dragged.measured ?? n.measured }
            : n,
        );
        const groupId = findContainingGroup(dragged.id, merged);
        return reparentNode(merged, dragged.id, groupId);
      });
    },
    [setNodes],
  );

  // Snapshot before structural changes that React Flow applies itself
  // (deletions, and the start of a drag), so they can be undone.
  const handleNodesChange = useCallback<OnNodesChange>(
    (changes) => {
      if (changes.some((c) => c.type === "remove")) {
        takeSnapshot();
        // When a group frame is deleted, free its members (back to absolute
        // coords) so they survive instead of becoming orphaned children.
        const removed = new Set(changes.filter((c) => c.type === "remove").map((c) => c.id));
        const removedGroups = new Set(
          nodes.filter((n) => removed.has(n.id) && isGroupNode(n)).map((n) => n.id),
        );
        if (removedGroups.size > 0) {
          setNodes((ns) => detachChildren(ns, removedGroups));
        }
      } else if (changes.some((c) => c.type === "position" && c.dragging) && !dragging.current) {
        dragging.current = true;
        takeSnapshot();
      }
      if (changes.some((c) => c.type === "position" && c.dragging === false)) {
        dragging.current = false;
      }
      // Alignment guides: while dragging a single node, snap its edges to other
      // nodes' edges and surface the guide lines. Grid snapping (if enabled) is
      // applied by React Flow separately and composes with this.
      let lines: { horizontal?: number; vertical?: number } = {};
      if (changes.length === 1 && changes[0].type === "position" && changes[0].dragging && changes[0].position) {
        const change = changes[0] as NodePositionChange;
        const helper = getHelperLines(change, nodes);
        if (helper.snapPosition.x !== undefined) change.position!.x = helper.snapPosition.x;
        if (helper.snapPosition.y !== undefined) change.position!.y = helper.snapPosition.y;
        lines = { horizontal: helper.horizontal, vertical: helper.vertical };
      }
      setHelperLines(lines);
      onNodesChange(changes);
    },
    [onNodesChange, takeSnapshot, nodes, setNodes],
  );

  const handleEdgesChange = useCallback<OnEdgesChange>(
    (changes) => {
      if (changes.some((c) => c.type === "remove")) takeSnapshot();
      onEdgesChange(changes);
    },
    [onEdgesChange, takeSnapshot],
  );

  const copySelection = useCallback(() => {
    const clip = clipFromSelection(nodes, edges);
    if (clip.nodes.length === 0) return;
    clipboard.current = clip;
    setMessage(`copied ${clip.nodes.length} node${clip.nodes.length > 1 ? "s" : ""}`);
  }, [nodes, edges]);

  const pasteClipboard = useCallback(() => {
    const clip = clipboard.current;
    if (!clip || clip.nodes.length === 0) return;
    takeSnapshot();
    const pasted = buildPaste(clip, { x: 40, y: 40 }, newNodeId);
    setNodes((ns) => orderNodes(ns.map((n): Node => ({ ...n, selected: false })).concat(pasted.nodes)));
    setEdges((es) => es.map((e): Edge => ({ ...e, selected: false })).concat(pasted.edges));
    setSelectedId(pasted.nodes[0]?.id ?? null);
    setMessage(`pasted ${pasted.nodes.length} node${pasted.nodes.length > 1 ? "s" : ""}`);
  }, [setNodes, setEdges, takeSnapshot, newNodeId]);

  const setStatus = useCallback(
    (id: string, status: NodeStatus) => patchNode(id, { status }),
    [patchNode],
  );

  // Append a line to the run log (capped, never mutating the previous array).
  const pushLog = useCallback((level: LogLevel, message: string, node?: string) => {
    setRunLog((log) => appendLog(log, { id: logSeq.current++, t: Date.now(), level, message, node }));
  }, []);

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

  // Select/focus a node in the editor (e.g. from a run-log line). Programmatic,
  // so it must not flag the file dirty.
  const focusNode = useCallback(
    (nodeId: string) => {
      skipDirty.current = true;
      setNodes((ns) => ns.map((n) => ({ ...n, selected: n.id === nodeId })));
      setSelectedId(nodeId);
    },
    [setNodes],
  );

  // Per-node run telemetry (duration / error) for node-level logs/progress.
  const recordRun = useCallback(
    (id: string, info: NodeRunInfo) => {
      patchNode(id, { durationMs: info.durationMs, error: info.error ?? null });
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
    if (!runId) return;
    setMessage("cancelling…");
    pushLog("warn", "✋ cancellation requested");
    void cancelStudioRun(runId).catch((err) => setMessage(`cancel failed: ${String(err)}`));
  }, [pushLog]);

  // Surface output paths into preview nodes. The thumbnail itself is fetched
  // lazily by the node when it scrolls into view (see HgripeNode).
  const applyPreviews = useCallback(
    (graph: ReturnType<typeof toWorkflowGraph>, result: { outputs: Map<string, Record<string, unknown>> }) => {
      const paths: string[] = [];
      for (const node of graph.nodes) {
        if (node.kind !== "preview") continue;
        const out = result.outputs.get(node.id);
        const imagePath = (out?.image as string | null) ?? null;
        patchNode(node.id, { imagePath });
        if (imagePath) paths.push(imagePath);
      }
      return paths;
    },
    [patchNode],
  );

  const run = useCallback(async () => {
    setRunning(true);
    setShowLog(true);
    const useRustBackend = isTauri();
    const backend = useRustBackend ? "Rust backend" : "browser preview";
    setMessage(useRustBackend ? "running Rust backend…" : "running browser preview…");
    clearRunInfo();
    pushLog("info", `▶ run started (${backend})`);
    try {
      const graph = toWorkflowGraph(nodes, edges);
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
        const result = await runGraph(graph, defaultExecutors, observer);
        applyPreviews(graph, result);
        setMessage("done (browser preview)");
      }
      pushLog("success", `✔ run finished (${backend})`);
    } catch (err) {
      const message = String(err);
      const cancelled = message.toLowerCase().includes("cancel");
      setMessage(cancelled ? "cancelled" : `error: ${message}`);
      pushLog(cancelled ? "warn" : "error", cancelled ? "run cancelled" : `run failed: ${message}`);
    } finally {
      setRunning(false);
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
  ]);

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
    setRunning(true);
    setShowLog(true);
    clearRunInfo();
    const useRustBackend = isTauri();
    const backend = useRustBackend ? "Rust backend" : "browser preview";
    const rustRunId = useRustBackend ? beginRustRun() : null;
    pushLog("info", `▶ batch started: ${batchCount} run(s) (${backend})`);
    try {
      const graph = toWorkflowGraph(nodes, edges);
      const collected: string[] = [];
      for (let i = 0; i < batchCount; i++) {
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
          const result = await runGraph(graph, defaultExecutors, observer, overrides);
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
      setMessage(cancelled ? "batch cancelled" : `batch error: ${message}`);
      pushLog(cancelled ? "warn" : "error", cancelled ? "batch cancelled" : `batch failed: ${message}`);
    } finally {
      if (rustRunId) endRustRun(rustRunId);
      setRunning(false);
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
  ]);

  // Switch the rendering style of all edges (and future ones).
  const changeEdgeType = useCallback(
    (t: EdgeStyle) => {
      setEdgeType(t);
      setEdges((es) => es.map((e) => ({ ...e, type: t })));
    },
    [setEdges],
  );

  // Delete a single node (freeing group members if it is a group frame) and
  // drop any edges touching it.
  const deleteNode = useCallback(
    (id: string) => {
      takeSnapshot();
      const isGroup = nodes.some((n) => n.id === id && isGroupNode(n));
      setNodes((ns) => {
        const remaining = ns.filter((n) => n.id !== id);
        return isGroup ? detachChildren(remaining, new Set([id])) : remaining;
      });
      setEdges((es) => es.filter((e) => e.source !== id && e.target !== id));
      setSelectedId((cur) => (cur === id ? null : cur));
    },
    [nodes, setNodes, setEdges, takeSnapshot],
  );

  // Remove every edge connected to a node, leaving the node in place.
  const disconnectNode = useCallback(
    (id: string) => {
      const touching = edges.some((e) => e.source === id || e.target === id);
      if (!touching) return;
      takeSnapshot();
      setEdges((es) => es.filter((e) => e.source !== id && e.target !== id));
    },
    [edges, setEdges, takeSnapshot],
  );

  // Duplicate a single node (fresh id, offset position, no edges).
  const duplicateNode = useCallback(
    (id: string) => {
      const node = nodes.find((n) => n.id === id);
      if (!node) return;
      takeSnapshot();
      const pasted = buildPaste({ nodes: [node], edges: [] }, { x: 40, y: 40 }, newNodeId);
      setNodes((ns) => orderNodes(ns.map((n): Node => ({ ...n, selected: false })).concat(pasted.nodes)));
      setSelectedId(pasted.nodes[0]?.id ?? null);
    },
    [nodes, setNodes, takeSnapshot, newNodeId],
  );

  const openNodeMenu = useCallback(
    (nodeId: string, at: { x: number; y: number }) => setMenu({ ...at, nodeId }),
    [],
  );
  const openPaneMenu = useCallback(
    (at: { x: number; y: number }) => setMenu({ ...at, nodeId: null }),
    [],
  );

  // Tidy layout: arrange nodes on a grid by DAG depth. Grouped nodes (and group
  // frames) keep their positions so containers are not torn apart.
  const tidyLayout = useCallback(() => {
    const graph = toWorkflowGraph(nodes, edges);
    let levels: string[][];
    try {
      levels = topoLevels(graph);
    } catch {
      setMessage("无法整理：图中存在环");
      return;
    }
    const movable = new Set(
      nodes.filter((n) => !isGroupNode(n) && !n.parentId).map((n) => n.id),
    );
    const positions = layeredPositions(levels.map((level) => level.filter((id) => movable.has(id))));
    if (positions.size === 0) {
      setMessage("没有可整理的节点");
      return;
    }
    takeSnapshot();
    setNodes((ns) => ns.map((n) => (positions.has(n.id) ? { ...n, position: positions.get(n.id)! } : n)));
    setMessage("已整理布局");
    // Re-center after React Flow applies the new positions.
    setTimeout(() => fitView({ padding: 0.2, duration: 300 }), 0);
  }, [nodes, edges, setNodes, takeSnapshot, fitView]);

  // Right-click menu items, depending on whether a node or empty pane was hit.
  const menuItems = useMemo<MenuItem[]>(() => {
    if (!menu) return [];
    if (menu.nodeId) {
      const id = menu.nodeId;
      const connected = edges.some((e) => e.source === id || e.target === id);
      return [
        { label: "复制", onClick: () => duplicateNode(id) },
        { label: "断开全部连线", onClick: () => disconnectNode(id), disabled: !connected },
        { label: "删除", onClick: () => deleteNode(id) },
      ];
    }
    return [
      { label: "整理布局", onClick: tidyLayout },
      { label: "适应视图", onClick: () => fitView({ padding: 0.2, duration: 300 }) },
      { label: "粘贴", onClick: pasteClipboard, disabled: !clipboard.current },
    ];
  }, [menu, edges, duplicateNode, disconnectNode, deleteNode, tidyLayout, fitView, pasteClipboard]);

  // Swap the editor graph without flagging it as an unsaved user edit.
  const loadGraphIntoEditor = useCallback(
    (graph: WorkflowGraph) => {
      skipDirty.current = true;
      const next = fromWorkflowGraph(graph);
      setNodes(next.nodes);
      setEdges(next.edges);
      setSelectedId(null);
    },
    [setNodes, setEdges],
  );

  // Browser-preview download: there is no native filesystem, so Save / Save As
  // fall back to a JSON download.
  const downloadWorkflow = useCallback(() => {
    const json = serializeGraph(toWorkflowGraph(nodes, edges));
    const blob = new Blob([json], { type: "application/json" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = currentFile ? baseName(currentFile) : "workflow.json";
    a.click();
    URL.revokeObjectURL(url);
  }, [nodes, edges, currentFile]);

  // Guard destructive actions that would replace the current graph. Returns
  // true when it is safe to proceed (no unsaved edits, or the user confirms).
  const confirmDiscard = useCallback(
    (action: string): boolean => {
      if (!fileDirty) return true;
      const label = currentFile ? baseName(currentFile) : "this workflow";
      return window.confirm(`Discard unsaved changes to ${label}? (${action})`);
    },
    [fileDirty, currentFile],
  );

  // Persist snapshots to localStorage whenever the list changes.
  useEffect(() => {
    saveSnapshots(snapshots);
  }, [snapshots]);

  // Capture the current graph as a named snapshot.
  const captureSnapshot = useCallback(() => {
    const suggested = `Snapshot ${new Date().toLocaleString()}`;
    const name = window.prompt("Snapshot name", suggested);
    if (name === null) return;
    const graph = toWorkflowGraph(nodes, edges);
    const snap: Snapshot = { id: newSnapshotId(), name: name.trim() || suggested, t: Date.now(), graph };
    setSnapshots((list) => addSnapshot(list, snap));
    setMessage(`snapshot saved: ${snap.name}`);
  }, [nodes, edges]);

  // Restore a snapshot into the editor (guarded by the unsaved-changes check).
  const restoreSnapshot = useCallback(
    (id: string) => {
      const snap = snapshots.find((s) => s.id === id);
      if (!snap) return;
      if (!confirmDiscard(`restore ${snap.name}`)) return;
      takeSnapshot();
      loadGraphIntoEditor(snap.graph);
      setFileDirty(true);
      setMessage(`restored snapshot: ${snap.name}`);
    },
    [snapshots, confirmDiscard, takeSnapshot, loadGraphIntoEditor],
  );

  const renameSnapshotById = useCallback((id: string) => {
    setSnapshots((list) => {
      const snap = list.find((s) => s.id === id);
      const name = window.prompt("Rename snapshot", snap?.name ?? "");
      if (name === null) return list;
      return renameSnapshot(list, id, name);
    });
  }, []);

  const deleteSnapshot = useCallback((id: string) => {
    setSnapshots((list) => {
      const snap = list.find((s) => s.id === id);
      if (snap && !window.confirm(`Delete snapshot "${snap.name}"?`)) return list;
      return removeSnapshot(list, id);
    });
  }, []);

  // Browser-preview upload (the hidden file input). Desktop uses native dialogs.
  const load = useCallback(
    async (file: File) => {
      if (!confirmDiscard(`load ${file.name}`)) return;
      try {
        takeSnapshot();
        const graph = deserializeGraph(await file.text());
        loadGraphIntoEditor(graph);
        setCurrentFile(null);
        setFileDirty(false);
        setMessage(`loaded ${file.name}`);
      } catch (err) {
        setMessage(`load failed: ${String(err)}`);
      }
    },
    [takeSnapshot, loadGraphIntoEditor, confirmDiscard],
  );

  const refreshProjectFiles = useCallback(async (dir: string) => {
    setProjectBusy(true);
    try {
      setWorkflowFiles(await listStudioWorkflows(dir));
    } catch (err) {
      setMessage(`folder scan failed: ${String(err)}`);
    } finally {
      setProjectBusy(false);
    }
  }, []);

  // Most-recent-first, de-duplicated, capped recent-files list.
  const rememberFile = useCallback((path: string) => {
    setRecentFiles((prev) => [path, ...prev.filter((p) => p !== path)].slice(0, 8));
  }, []);

  const openFromPath = useCallback(
    async (path: string) => {
      if (!confirmDiscard(`open ${baseName(path)}`)) return;
      try {
        takeSnapshot();
        const graph = deserializeGraph(await readStudioWorkflow(path));
        loadGraphIntoEditor(graph);
        setCurrentFile(path);
        setFileDirty(false);
        rememberFile(path);
        setMessage(`opened ${baseName(path)}`);
      } catch (err) {
        setMessage(`open failed: ${String(err)}`);
      }
    },
    [takeSnapshot, loadGraphIntoEditor, rememberFile, confirmDiscard],
  );

  const saveToPath = useCallback(
    async (path: string) => {
      try {
        await writeStudioWorkflow(path, toWorkflowGraph(nodes, edges));
        setCurrentFile(path);
        setFileDirty(false);
        rememberFile(path);
        if (projectDir && path.startsWith(projectDir)) void refreshProjectFiles(projectDir);
        setMessage(`saved ${baseName(path)}`);
      } catch (err) {
        setMessage(`save failed: ${String(err)}`);
      }
    },
    [nodes, edges, projectDir, rememberFile, refreshProjectFiles],
  );

  const handleSaveAs = useCallback(async () => {
    if (!isDesktop) {
      downloadWorkflow();
      return;
    }
    const defaultName = currentFile ? baseName(currentFile) : "workflow.json";
    const path = await pickWorkflowSavePath(defaultName, projectDir);
    if (path) await saveToPath(path);
  }, [isDesktop, currentFile, projectDir, saveToPath, downloadWorkflow]);

  const handleSave = useCallback(async () => {
    if (!isDesktop) {
      downloadWorkflow();
      return;
    }
    if (currentFile) await saveToPath(currentFile);
    else await handleSaveAs();
  }, [isDesktop, currentFile, saveToPath, handleSaveAs, downloadWorkflow]);

  const handleOpen = useCallback(async () => {
    if (!isDesktop) {
      fileInput.current?.click();
      return;
    }
    const path = await pickWorkflowOpenPath(projectDir);
    if (path) await openFromPath(path);
  }, [isDesktop, projectDir, openFromPath]);

  const handlePickFolder = useCallback(async () => {
    const dir = await pickProjectFolder(projectDir);
    if (dir) {
      setProjectDir(dir);
      void refreshProjectFiles(dir);
    }
  }, [projectDir, refreshProjectFiles]);

  // Create a new, empty workflow saved straight into the active project folder.
  const handleNewInFolder = useCallback(async () => {
    if (!projectDir) return;
    if (!confirmDiscard("New file")) return;
    const input = window.prompt("New workflow file name", "workflow.json");
    if (!input) return;
    const trimmed = input.trim();
    if (!trimmed) return;
    const fileName = /\.json$/i.test(trimmed) ? trimmed : `${trimmed}.json`;
    const sep = projectDir.includes("\\") ? "\\" : "/";
    const target = `${projectDir.replace(/[\\/]$/, "")}${sep}${fileName}`;
    try {
      await writeStudioWorkflow(target, toWorkflowGraph([], []));
      skipDirty.current = true;
      setNodes([]);
      setEdges([]);
      setSelectedId(null);
      setCurrentFile(target);
      setFileDirty(false);
      rememberFile(target);
      void refreshProjectFiles(projectDir);
      setMessage(`created ${fileName}`);
    } catch (err) {
      setMessage(`create failed: ${String(err)}`);
    }
  }, [projectDir, confirmDiscard, setNodes, setEdges, rememberFile, refreshProjectFiles]);

  const handleRenameFile = useCallback(
    async (path: string) => {
      const current = baseName(path);
      const input = window.prompt("Rename workflow", current);
      if (!input) return;
      const next = input.trim();
      if (!next || next === current) return;
      try {
        const newPath = await renameStudioWorkflow(path, next);
        setCurrentFile((cur) => (cur === path ? newPath : cur));
        setRecentFiles((prev) => prev.map((p) => (p === path ? newPath : p)));
        if (projectDir) void refreshProjectFiles(projectDir);
        setMessage(`renamed to ${baseName(newPath)}`);
      } catch (err) {
        setMessage(`rename failed: ${String(err)}`);
      }
    },
    [projectDir, refreshProjectFiles],
  );

  const handleDuplicateFile = useCallback(
    async (path: string) => {
      try {
        const newPath = await duplicateStudioWorkflow(path);
        if (projectDir) void refreshProjectFiles(projectDir);
        setMessage(`duplicated to ${baseName(newPath)}`);
      } catch (err) {
        setMessage(`duplicate failed: ${String(err)}`);
      }
    },
    [projectDir, refreshProjectFiles],
  );

  const handleDeleteFile = useCallback(
    async (path: string) => {
      if (!window.confirm(`Delete ${baseName(path)}? This cannot be undone.`)) return;
      try {
        await deleteStudioWorkflow(path);
        setCurrentFile((cur) => (cur === path ? null : cur));
        setRecentFiles((prev) => prev.filter((p) => p !== path));
        if (projectDir) void refreshProjectFiles(projectDir);
        setMessage(`deleted ${baseName(path)}`);
      } catch (err) {
        setMessage(`delete failed: ${String(err)}`);
      }
    },
    [projectDir, refreshProjectFiles],
  );

  const newWorkflow = useCallback(() => {
    if (!confirmDiscard("New")) return;
    takeSnapshot();
    skipDirty.current = true;
    setNodes([]);
    setEdges([]);
    setSelectedId(null);
    setCurrentFile(null);
    setFileDirty(false);
    setMessage("new workflow");
  }, [setNodes, setEdges, takeSnapshot, confirmDiscard]);

  const clear = useCallback(() => {
    if (!confirmDiscard("Clear")) return;
    takeSnapshot();
    skipDirty.current = true;
    setNodes([]);
    setEdges([]);
    setSelectedId(null);
    setCurrentFile(null);
    setFileDirty(false);
    clearPersistedGraph();
    if (isTauri()) void clearStudioAutosave().catch((err) => setMessage(`clear autosave failed: ${String(err)}`));
  }, [setNodes, setEdges, takeSnapshot, confirmDiscard]);

  const resetSample = useCallback(() => {
    if (!confirmDiscard("Reset")) return;
    takeSnapshot();
    skipDirty.current = true;
    setNodes(initialNodes);
    setEdges(initialEdges);
    setSelectedId(null);
    setCurrentFile(null);
    setFileDirty(false);
    setMessage("reset to sample workflow");
  }, [setNodes, setEdges, takeSnapshot, confirmDiscard]);

  // Restore the persisted project folder + recent files on desktop start.
  useEffect(() => {
    if (!isDesktop) return;
    let cancelled = false;
    void readStudioRecents()
      .then((recents) => {
        if (cancelled) return;
        setProjectDir(recents.project_dir ?? null);
        setCurrentFile(recents.current_file ?? null);
        setRecentFiles(recents.files ?? []);
        if (recents.project_dir) {
          setShowProject(true);
          void refreshProjectFiles(recents.project_dir);
        }
      })
      .catch(() => {
        /* no recents yet — start clean */
      })
      .finally(() => {
        if (!cancelled) setRecentsReady(true);
      });
    return () => {
      cancelled = true;
    };
  }, [isDesktop, refreshProjectFiles]);

  // Persist project folder + recent files whenever they change (after restore).
  useEffect(() => {
    if (!isDesktop || !recentsReady) return;
    void writeStudioRecents({
      project_dir: projectDir,
      current_file: currentFile,
      files: recentFiles,
    }).catch(() => {
      /* best-effort */
    });
  }, [isDesktop, recentsReady, projectDir, currentFile, recentFiles]);

  // Flag the current file dirty on user edits (programmatic swaps set skipDirty).
  useEffect(() => {
    if (skipDirty.current) {
      skipDirty.current = false;
      return;
    }
    setFileDirty(true);
  }, [nodes, edges]);

  // Warn before closing the tab/window while there are unsaved file edits.
  useEffect(() => {
    if (!fileDirty) return;
    const handler = (event: BeforeUnloadEvent) => {
      event.preventDefault();
      event.returnValue = "";
    };
    window.addEventListener("beforeunload", handler);
    return () => window.removeEventListener("beforeunload", handler);
  }, [fileDirty]);

  // Autosave to the workspace (debounced). Desktop builds persist through the
  // Rust backend; browser preview falls back to localStorage.
  useEffect(() => {
    if (isTauri() && !desktopAutosaveReady) return;
    setSaved(false);
    let cancelled = false;
    const t = setTimeout(() => {
      const graph = toWorkflowGraph(nodes, edges);
      if (isTauri()) {
        void writeStudioAutosave(graph)
          .then(() => {
            if (!cancelled) setSaved(true);
          })
          .catch((err) => {
            if (!cancelled) {
              setSaved(false);
              setMessage(`autosave failed: ${String(err)}`);
            }
          });
      } else {
        persistGraph(graph);
        setSaved(true);
      }
    }, 500);
    return () => {
      cancelled = true;
      clearTimeout(t);
    };
  }, [nodes, edges, desktopAutosaveReady]);

  // Keyboard: undo/redo (Ctrl+Z / Ctrl+Shift+Z / Ctrl+Y) and copy/paste
  // (Ctrl+C / Ctrl+V). Skipped while editing a form field so native text
  // editing keeps working there.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey)) return;
      const t = e.target as HTMLElement | null;
      const editable =
        !!t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT" || t.isContentEditable);
      if (editable) return;
      switch (e.key.toLowerCase()) {
        case "z":
          e.preventDefault();
          if (e.shiftKey) redo();
          else undo();
          break;
        case "y":
          e.preventDefault();
          redo();
          break;
        case "a":
          e.preventDefault();
          setNodes((ns) => ns.map((n) => ({ ...n, selected: true })));
          setEdges((es) => es.map((ed) => ({ ...ed, selected: true })));
          break;
        case "c":
          copySelection();
          break;
        case "v":
          e.preventDefault();
          pasteClipboard();
          break;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [undo, redo, copySelection, pasteClipboard, setNodes, setEdges]);

  // File shortcuts: Ctrl/Cmd+S save, +Shift+S save as, +O open, +N new. These
  // intentionally fire even while editing a field so a quick Ctrl+S always saves.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey) || e.altKey) return;
      switch (e.key.toLowerCase()) {
        case "s":
          e.preventDefault();
          if (e.shiftKey) void handleSaveAs();
          else void handleSave();
          break;
        case "o":
          if (e.shiftKey) return;
          e.preventDefault();
          void handleOpen();
          break;
        case "n":
          if (e.shiftKey) return;
          e.preventDefault();
          newWorkflow();
          break;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [handleSave, handleSaveAs, handleOpen, newWorkflow]);

  // Stable context value so memoized node cards can edit their own params.
  const editing = useMemo(() => ({ onParamChange }), [onParamChange]);

  return (
    <div className="app">
      <header className="toolbar">
        <strong>H-Gripe Studio</strong>
        <span className="muted">node-graph (React Flow)</span>
        <div className="spacer" />
        {issues.length > 0 && (
          <span className="issues" title={issues.map((i) => i.message).join("\n")}>
            ⚠ {issues.length} issue{issues.length > 1 ? "s" : ""}
          </span>
        )}
        {isDesktop && (
          <span className="muted current-file" title={currentFile ?? "untitled (not yet saved to a file)"}>
            {currentFile ? baseName(currentFile) : "untitled"}
            {fileDirty ? " *" : ""}
          </span>
        )}
        <span className="muted autosave" title="this workflow is autosaved to the workspace and restored on next open">
          {saved ? "● autosaved" : "○ saving…"}
        </span>
        <button onClick={undo} disabled={!history.canUndo} title="Undo (Ctrl+Z)">
          Undo
        </button>
        <button onClick={redo} disabled={!history.canRedo} title="Redo (Ctrl+Shift+Z)">
          Redo
        </button>
        {isDesktop && (
          <button
            onClick={() => setShowProject((s) => !s)}
            title="toggle the project folder browser"
          >
            {showProject ? "Hide Project" : "Project"}
          </button>
        )}
        <button
          onClick={() => setShowSnapshots((s) => !s)}
          title="toggle the snapshots panel (named versions of the workflow)"
        >
          {showSnapshots ? "Hide Snapshots" : "Snapshots"}
          {snapshots.length > 0 ? ` (${snapshots.length})` : ""}
        </button>
        {isDesktop && (
          <button onClick={newWorkflow} title="start a new, empty workflow (Ctrl/Cmd+N)">
            New
          </button>
        )}
        <button
          onClick={() => void handleOpen()}
          title={isDesktop ? "open a workflow file (Ctrl/Cmd+O)" : "load workflow.json (Ctrl/Cmd+O)"}
        >
          {isDesktop ? "Open…" : "Load"}
        </button>
        <button
          onClick={() => void handleSave()}
          title={
            isDesktop
              ? "save to the current file (Save As… if none) — Ctrl/Cmd+S"
              : "download workflow.json (Ctrl/Cmd+S)"
          }
        >
          Save
        </button>
        {isDesktop && (
          <button
            onClick={() => void handleSaveAs()}
            title="save to a new file via the native dialog (Ctrl/Cmd+Shift+S)"
          >
            Save As…
          </button>
        )}
        <button onClick={resetSample}>Reset</button>
        <button onClick={clear}>Clear</button>
        <label className="snap-toggle" title="snap node positions to a 16px grid while dragging">
          <input type="checkbox" checked={snapToGrid} onChange={(e) => setSnapToGrid(e.target.checked)} />
          Snap
        </label>
        <button onClick={tidyLayout} title="arrange nodes on a grid by DAG depth">
          Tidy
        </button>
        <label className="snap-toggle" title="edge rendering style">
          Edges
          <select value={edgeType} onChange={(e) => changeEdgeType(e.target.value as EdgeStyle)}>
            <option value="default">curved</option>
            <option value="smoothstep">orthogonal</option>
            <option value="smart">avoid</option>
          </select>
        </label>
        <label className="snap-toggle" title="toggle the minimap">
          <input type="checkbox" checked={showMinimap} onChange={(e) => setShowMinimap(e.target.checked)} />
          Map
        </label>
        <button
          onClick={() => setShowLog((s) => !s)}
          title="toggle the run log (per-node status, timing and errors)"
        >
          {showLog ? "Hide Log" : "Log"}
          {runLog.length > 0 ? ` (${runLog.length})` : ""}
        </button>
        <button className="primary" onClick={run} disabled={running || issues.length > 0}>
          {running ? "Running…" : "Run"}
        </button>
        {running && currentRunId && (
          <button onClick={cancelRun} title="request cancellation before the next node starts">
            Cancel
          </button>
        )}
        {batchNode && (
          <button
            onClick={runBatch}
            disabled={running || issues.length > 0 || batchCount === 0}
            title="run the graph once per batch item"
          >
            Run ×{batchCount}
          </button>
        )}
        <span className="muted">{message}</span>
        <input
          ref={fileInput}
          type="file"
          accept="application/json,.json"
          style={{ display: "none" }}
          onChange={(e) => {
            const f = e.target.files?.[0];
            if (f) void load(f);
            e.target.value = "";
          }}
        />
      </header>

      <NodeEditingContext.Provider value={editing}>
        <div className="workspace">
          {isDesktop && showProject && (
            <ProjectPanel
              projectDir={projectDir}
              files={workflowFiles}
              recentFiles={recentFiles}
              currentFile={currentFile}
              busy={projectBusy}
              onPickFolder={() => void handlePickFolder()}
              onRefresh={() => projectDir && void refreshProjectFiles(projectDir)}
              onOpenFile={(path) => void openFromPath(path)}
              onNew={newWorkflow}
              onNewInFolder={() => void handleNewInFolder()}
              onRenameFile={(path) => void handleRenameFile(path)}
              onDuplicateFile={(path) => void handleDuplicateFile(path)}
              onDeleteFile={(path) => void handleDeleteFile(path)}
            />
          )}
          {showSnapshots && (
            <SnapshotsPanel
              snapshots={snapshots}
              onCapture={captureSnapshot}
              onRestore={restoreSnapshot}
              onRename={renameSnapshotById}
              onDelete={deleteSnapshot}
              onClose={() => setShowSnapshots(false)}
            />
          )}
          <Palette onAdd={addNode} />
          <div className="canvas">
            <div className="canvas-flow">
              <FlowCanvas
                nodes={nodes}
                edges={edges}
                onNodesChange={handleNodesChange}
                onEdgesChange={handleEdgesChange}
                setEdges={setEdges}
                onSelect={setSelectedId}
                onAddNode={addNode}
                onBeforeConnect={takeSnapshot}
                onNodeDragStop={handleNodeDragStop}
                snapToGrid={snapToGrid}
                helperLines={helperLines}
                edgeType={edgeType}
                showMinimap={showMinimap}
                onNodeContextMenu={openNodeMenu}
                onPaneContextMenu={openPaneMenu}
              />
            </div>
            {showLog && (
              <RunLog
                entries={runLog}
                onClear={() => setRunLog([])}
                onClose={() => setShowLog(false)}
                onExport={exportLog}
                onSelectNode={focusNode}
              />
            )}
          </div>
          <Inspector node={selectedNode} onParamChange={onParamChange} />
        </div>
      </NodeEditingContext.Provider>
      {menu && (
        <ContextMenu x={menu.x} y={menu.y} items={menuItems} onClose={() => setMenu(null)} />
      )}
    </div>
  );
}

export default function App() {
  // Provider gives FlowCanvas access to screenToFlowPosition for drag-and-drop.
  return (
    <ReactFlowProvider>
      <Studio />
    </ReactFlowProvider>
  );
}
