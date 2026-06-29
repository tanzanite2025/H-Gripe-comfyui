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

import { FlowCanvas } from "./editor/FlowCanvas";
import { Inspector } from "./editor/Inspector";
import { Palette } from "./editor/Palette";
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
import { clearPersistedGraph, loadPersistedGraph, persistGraph } from "./editor/persist";
import { defaultParams } from "./graph/nodeSpecs";
import { deserializeGraph, serializeGraph } from "./graph/model";
import { runGraph, topoLevels, validateGraph, type NodeRunInfo, type NodeStatus } from "./runtime/dag";
import { batchItems, defaultExecutors } from "./runtime/executors";
import { isTauri } from "./bridge/tauri";

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
  const [snapToGrid, setSnapToGrid] = useState(false);
  const [helperLines, setHelperLines] = useState<{ horizontal?: number; vertical?: number }>({});
  const [edgeType, setEdgeType] = useState<"default" | "smoothstep">("default");
  const [showMinimap, setShowMinimap] = useState(true);
  const { fitView } = useReactFlow();
  const [saved, setSaved] = useState(restoredOnMount.current);
  const [message, setMessage] = useState<string>(
    isTauri()
      ? restoredOnMount.current
        ? "restored last workflow"
        : ""
      : "browser preview (backend mocked)",
  );
  const idSeq = useRef(0);
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

  // Per-node run telemetry (duration / error) for node-level logs/progress.
  const recordRun = useCallback(
    (id: string, info: NodeRunInfo) =>
      patchNode(id, { durationMs: info.durationMs ?? null, error: info.error ?? null }),
    [patchNode],
  );
  // Clear the previous run's duration/error before a fresh run.
  const clearRunInfo = useCallback(
    () => setNodes((ns) => ns.map((n) => ({ ...n, data: { ...n.data, durationMs: undefined, error: undefined } }))),
    [setNodes],
  );
  const observer = useMemo(() => ({ onStatus: setStatus, onNodeRun: recordRun }), [setStatus, recordRun]);

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
    setMessage("running…");
    clearRunInfo();
    try {
      const graph = toWorkflowGraph(nodes, edges);
      const result = await runGraph(graph, defaultExecutors, observer);
      applyPreviews(graph, result);
      setMessage("done");
    } catch (err) {
      setMessage(`error: ${String(err)}`);
    } finally {
      setRunning(false);
    }
  }, [nodes, edges, observer, clearRunInfo, applyPreviews]);

  // Batch fan-out: run the graph once per item of the (first) batch node,
  // sweeping its `index` via runGraph's paramOverrides. Sequential so node
  // statuses and previews update visibly per item.
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
    clearRunInfo();
    try {
      const graph = toWorkflowGraph(nodes, edges);
      const collected: string[] = [];
      for (let i = 0; i < batchCount; i++) {
        setMessage(`batch ${i + 1}/${batchCount}…`);
        const overrides = new Map([[batchNode.id, { index: i }]]);
        const result = await runGraph(graph, defaultExecutors, observer, overrides);
        collected.push(...applyPreviews(graph, result));
      }
      setMessage(`batch done: ${batchCount} run(s), ${collected.length} output(s)`);
    } catch (err) {
      setMessage(`batch error: ${String(err)}`);
    } finally {
      setRunning(false);
    }
  }, [batchNode, batchCount, nodes, edges, observer, clearRunInfo, applyPreviews]);

  // Switch the rendering style of all edges (and future ones).
  const changeEdgeType = useCallback(
    (t: "default" | "smoothstep") => {
      setEdgeType(t);
      setEdges((es) => es.map((e) => ({ ...e, type: t })));
    },
    [setEdges],
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

  const save = useCallback(() => {
    const json = serializeGraph(toWorkflowGraph(nodes, edges));
    const blob = new Blob([json], { type: "application/json" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = "workflow.json";
    a.click();
    URL.revokeObjectURL(url);
  }, [nodes, edges]);

  const load = useCallback(
    async (file: File) => {
      try {
        takeSnapshot();
        const graph = deserializeGraph(await file.text());
        const next = fromWorkflowGraph(graph);
        setNodes(next.nodes);
        setEdges(next.edges);
        setSelectedId(null);
        setMessage(`loaded ${file.name}`);
      } catch (err) {
        setMessage(`load failed: ${String(err)}`);
      }
    },
    [setNodes, setEdges, takeSnapshot],
  );

  const clear = useCallback(() => {
    takeSnapshot();
    setNodes([]);
    setEdges([]);
    setSelectedId(null);
    clearPersistedGraph();
  }, [setNodes, setEdges, takeSnapshot]);

  const resetSample = useCallback(() => {
    takeSnapshot();
    setNodes(initialNodes);
    setEdges(initialEdges);
    setSelectedId(null);
    setMessage("reset to sample workflow");
  }, [setNodes, setEdges, takeSnapshot]);

  // Autosave to the workspace (debounced). Only graph-structural fields are
  // serialized (kind/params/position/edges), so transient run statuses and
  // fetched thumbnails never hit storage.
  useEffect(() => {
    setSaved(false);
    const t = setTimeout(() => {
      persistGraph(toWorkflowGraph(nodes, edges));
      setSaved(true);
    }, 500);
    return () => clearTimeout(t);
  }, [nodes, edges]);

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
        <span className="muted autosave" title="this workflow is autosaved to the workspace and restored on next open">
          {saved ? "● autosaved" : "○ saving…"}
        </span>
        <button onClick={undo} disabled={!history.canUndo} title="Undo (Ctrl+Z)">
          Undo
        </button>
        <button onClick={redo} disabled={!history.canRedo} title="Redo (Ctrl+Shift+Z)">
          Redo
        </button>
        <button onClick={save}>Save</button>
        <button onClick={() => fileInput.current?.click()}>Load</button>
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
          <select
            value={edgeType}
            onChange={(e) => changeEdgeType(e.target.value as "default" | "smoothstep")}
          >
            <option value="default">curved</option>
            <option value="smoothstep">orthogonal</option>
          </select>
        </label>
        <label className="snap-toggle" title="toggle the minimap">
          <input type="checkbox" checked={showMinimap} onChange={(e) => setShowMinimap(e.target.checked)} />
          Map
        </label>
        <button className="primary" onClick={run} disabled={running || issues.length > 0}>
          {running ? "Running…" : "Run"}
        </button>
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
          <Palette onAdd={addNode} />
          <div className="canvas">
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
            />
          </div>
          <Inspector node={selectedNode} onParamChange={onParamChange} />
        </div>
      </NodeEditingContext.Provider>
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
