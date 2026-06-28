import { useCallback, useMemo, useState } from "react";
import { useEdgesState, useNodesState, type Edge, type Node } from "@xyflow/react";

import { FlowCanvas } from "./editor/FlowCanvas";
import { Inspector } from "./editor/Inspector";
import type { HgripeNodeData } from "./editor/HgripeNode";
import { toWorkflowGraph } from "./editor/adapter";
import { defaultParams } from "./graph/nodeSpecs";
import { runGraph, type NodeStatus } from "./runtime/dag";
import { defaultExecutors } from "./runtime/executors";
import { generateThumbnail, isTauri } from "./bridge/tauri";

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

export default function App() {
  const [nodes, setNodes, onNodesChange] = useNodesState(initialNodes);
  const [edges, setEdges, onEdgesChange] = useEdgesState(initialEdges);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [running, setRunning] = useState(false);
  const [message, setMessage] = useState<string>(isTauri() ? "" : "browser preview (backend mocked)");

  const selectedNode = useMemo(
    () => nodes.find((n) => n.id === selectedId) ?? null,
    [nodes, selectedId],
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

      // Surface outputs into preview nodes + generate thumbnails for display.
      for (const node of graph.nodes) {
        if (node.kind !== "preview") continue;
        const out = result.outputs.get(node.id);
        const imagePath = (out?.image as string | null) ?? null;
        if (imagePath) {
          const thumb = await generateThumbnail({ path: imagePath, size: 256 });
          patchNode(node.id, { imagePath, thumbnail: thumb.data_url || null });
        } else {
          patchNode(node.id, { imagePath: null, thumbnail: null });
        }
      }
      setMessage("done");
    } catch (err) {
      setMessage(`error: ${String(err)}`);
    } finally {
      setRunning(false);
    }
  }, [nodes, edges, setStatus, patchNode]);

  return (
    <div className="app">
      <header className="toolbar">
        <strong>H-Gripe Studio</strong>
        <span className="muted">node-graph (React Flow)</span>
        <div className="spacer" />
        <button className="primary" onClick={run} disabled={running}>
          {running ? "Running…" : "Run"}
        </button>
        <span className="muted">{message}</span>
      </header>

      <div className="workspace">
        <div className="canvas">
          <FlowCanvas
            nodes={nodes}
            edges={edges}
            onNodesChange={onNodesChange}
            onEdgesChange={onEdgesChange}
            setEdges={setEdges}
            onSelect={setSelectedId}
          />
        </div>
        <Inspector node={selectedNode} onParamChange={onParamChange} />
      </div>
    </div>
  );
}
