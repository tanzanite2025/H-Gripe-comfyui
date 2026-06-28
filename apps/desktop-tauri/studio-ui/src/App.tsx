import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ReactFlowProvider,
  useEdgesState,
  useNodesState,
  type Edge,
  type Node,
} from "@xyflow/react";

import { FlowCanvas } from "./editor/FlowCanvas";
import { Inspector } from "./editor/Inspector";
import { Palette } from "./editor/Palette";
import { NodeEditingContext } from "./editor/editingContext";
import type { HgripeNodeData } from "./editor/HgripeNode";
import { fromWorkflowGraph, toWorkflowGraph } from "./editor/adapter";
import { clearPersistedGraph, loadPersistedGraph, persistGraph } from "./editor/persist";
import { defaultParams } from "./graph/nodeSpecs";
import { deserializeGraph, serializeGraph } from "./graph/model";
import { runGraph, validateGraph, type NodeStatus } from "./runtime/dag";
import { defaultExecutors } from "./runtime/executors";
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
      const id = `${kind}-${Date.now()}-${idSeq.current++}`;
      // Click-to-add cascades nodes so they do not stack exactly.
      const pos = position ?? { x: 80 + (idSeq.current % 6) * 36, y: 80 + (idSeq.current % 6) * 36 };
      setNodes((ns) => ns.concat(makeNode(id, kind, pos.x, pos.y)));
    },
    [setNodes],
  );

  const setStatus = useCallback(
    (id: string, status: NodeStatus) => patchNode(id, { status }),
    [patchNode],
  );

  const run = useCallback(async () => {
    setRunning(true);
    setMessage("running…");
    try {
      const graph = toWorkflowGraph(nodes, edges);
      const result = await runGraph(graph, defaultExecutors, { onStatus: setStatus });

      // Surface output paths into preview nodes. The thumbnail itself is fetched
      // lazily by the node when it scrolls into view (see HgripeNode).
      for (const node of graph.nodes) {
        if (node.kind !== "preview") continue;
        const out = result.outputs.get(node.id);
        const imagePath = (out?.image as string | null) ?? null;
        patchNode(node.id, { imagePath });
      }
      setMessage("done");
    } catch (err) {
      setMessage(`error: ${String(err)}`);
    } finally {
      setRunning(false);
    }
  }, [nodes, edges, setStatus, patchNode]);

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
    [setNodes, setEdges],
  );

  const clear = useCallback(() => {
    setNodes([]);
    setEdges([]);
    setSelectedId(null);
    clearPersistedGraph();
  }, [setNodes, setEdges]);

  const resetSample = useCallback(() => {
    setNodes(initialNodes);
    setEdges(initialEdges);
    setSelectedId(null);
    setMessage("reset to sample workflow");
  }, [setNodes, setEdges]);

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
        <button onClick={save}>Save</button>
        <button onClick={() => fileInput.current?.click()}>Load</button>
        <button onClick={resetSample}>Reset</button>
        <button onClick={clear}>Clear</button>
        <button className="primary" onClick={run} disabled={running || issues.length > 0}>
          {running ? "Running…" : "Run"}
        </button>
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
              onNodesChange={onNodesChange}
              onEdgesChange={onEdgesChange}
              setEdges={setEdges}
              onSelect={setSelectedId}
              onAddNode={addNode}
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
