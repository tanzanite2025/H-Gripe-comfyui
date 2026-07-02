// Node/graph editing actions for the studio editor: add/delete/disconnect/
// duplicate nodes, param edits (with undo coalescing), clipboard copy/paste,
// selection/focus helpers, tidy layout, and spawning bound edit nodes from a
// media source card. Owns the id sequence and clipboard; graph state itself
// stays in the caller (App), which passes the setters in.

import { useCallback, useRef, type Dispatch, type MutableRefObject, type SetStateAction } from "react";
import type { Edge, Node } from "@xyflow/react";

import { buildPaste, clipFromSelection, type Clip } from "./clipboard";
import { detachChildren, isGroupNode, makeGroupNode, orderNodes } from "./grouping";
import { layeredPositions } from "./layout";
import type { HgripeNodeData } from "./HgripeNode";
import { toWorkflowGraph } from "./adapter";
import { defaultParams } from "../graph/nodeSpecs";
import { topoLevels } from "../runtime/dag";

export function makeNode(id: string, kind: string, x: number, y: number, params?: Record<string, unknown>): Node {
  const data: HgripeNodeData = { kind, params: { ...defaultParams(kind), ...params }, status: "idle" };
  return { id, type: "hgripe", position: { x, y }, data };
}

export interface UseNodeEditingArgs {
  nodes: Node[];
  edges: Edge[];
  setNodes: Dispatch<SetStateAction<Node[]>>;
  setEdges: Dispatch<SetStateAction<Edge[]>>;
  setSelectedId: Dispatch<SetStateAction<string | null>>;
  takeSnapshot: () => void;
  setMessage: (msg: string) => void;
  fitView: (opts?: { nodes?: { id: string }[]; padding?: number; duration?: number; maxZoom?: number }) => Promise<boolean>;
  /** Suppress the next dirty flag for programmatic selection (from the file controller). */
  suppressNextDirty: () => void;
  /** Node id queued for a "run up to this node" once pending edits land in `nodes`. */
  pendingRunNode: MutableRefObject<string | null>;
  /** Open the mask editor for a freshly spawned bound edit node. */
  openMaskEditorFor: (nodeId: string) => void;
  /** Open the crop editor for a freshly spawned bound edit node. */
  openCropEditorFor: (nodeId: string) => void;
}

export function useNodeEditing({
  nodes,
  edges,
  setNodes,
  setEdges,
  setSelectedId,
  takeSnapshot,
  setMessage,
  fitView,
  suppressNextDirty,
  pendingRunNode,
  openMaskEditorFor,
  openCropEditorFor,
}: UseNodeEditingArgs) {
  const idSeq = useRef(0);
  const clipboard = useRef<Clip | null>(null);
  // Coalesce rapid edits to the same param (e.g. typing) into one undo step.
  const lastParamEdit = useRef<{ id: string; key: string; t: number } | null>(null);

  const newNodeId = useCallback((kind: string) => `${kind}-${Date.now()}-${idSeq.current++}`, []);

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
    [setNodes, takeSnapshot],
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

  const copySelection = useCallback(() => {
    const clip = clipFromSelection(nodes, edges);
    if (clip.nodes.length === 0) return;
    clipboard.current = clip;
    setMessage(`copied ${clip.nodes.length} node${clip.nodes.length > 1 ? "s" : ""}`);
  }, [nodes, edges, setMessage]);

  const pasteClipboard = useCallback(() => {
    const clip = clipboard.current;
    if (!clip || clip.nodes.length === 0) return;
    takeSnapshot();
    const pasted = buildPaste(clip, { x: 40, y: 40 }, newNodeId);
    setNodes((ns) => orderNodes(ns.map((n): Node => ({ ...n, selected: false })).concat(pasted.nodes)));
    setEdges((es) => es.map((e): Edge => ({ ...e, selected: false })).concat(pasted.edges));
    setSelectedId(pasted.nodes[0]?.id ?? null);
    setMessage(`pasted ${pasted.nodes.length} node${pasted.nodes.length > 1 ? "s" : ""}`);
  }, [setNodes, setEdges, setSelectedId, takeSnapshot, newNodeId, setMessage]);

  // Select/focus a node in the editor (e.g. from a run-log line). Programmatic,
  // so it must not flag the file dirty.
  const focusNode = useCallback(
    (nodeId: string) => {
      suppressNextDirty();
      setNodes((ns) => ns.map((n) => ({ ...n, selected: n.id === nodeId })));
      setSelectedId(nodeId);
    },
    [setNodes, setSelectedId, suppressNextDirty],
  );

  // Select a node and pan/zoom the viewport to center it (used by node search).
  const jumpToNode = useCallback(
    (id: string) => {
      focusNode(id);
      void fitView({ nodes: [{ id }], duration: 400, maxZoom: 1.5 });
    },
    [focusNode, fitView],
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
    [nodes, setNodes, setEdges, setSelectedId, takeSnapshot],
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
    [nodes, setNodes, setSelectedId, takeSnapshot, newNodeId],
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
  }, [nodes, edges, setNodes, takeSnapshot, fitView, setMessage]);

  // Select every node and edge (Ctrl/Cmd+A).
  const selectAll = useCallback(() => {
    setNodes((ns) => ns.map((n) => ({ ...n, selected: true })));
    setEdges((es) => es.map((ed) => ({ ...ed, selected: true })));
  }, [setNodes, setEdges]);

  // Spawn a bound edit node from a media source card: place it to the right,
  // wire a `binding` edge (source.image -> edit.image), and select it. The
  // source card is never mutated; the new node becomes the output the rest of
  // the workflow consumes. `opts` chooses the manual/auto entry (see
  // docs/cards/generic-media-card.md): `params` seeds the node, `openEditor`
  // (default true) opens its editor for manual edits, and `run` runs the
  // ancestor subgraph so a computed (auto) edit surfaces its result directly.
  const addBoundEdit = useCallback(
    (
      sourceId: string,
      editKind: string,
      opts?: { params?: Record<string, unknown>; openEditor?: boolean; run?: boolean },
    ) => {
      const source = nodes.find((n) => n.id === sourceId);
      if (!source) return;
      takeSnapshot();
      const editId = newNodeId(editKind);
      const pos = { x: source.position.x + 320, y: source.position.y };
      setNodes((ns) =>
        ns
          .map((n) => ({ ...n, selected: false }))
          .concat({ ...makeNode(editId, editKind, pos.x, pos.y, opts?.params), selected: true }),
      );
      setEdges((es) =>
        es.concat({
          id: `binding-${editId}`,
          source: sourceId,
          sourceHandle: "image",
          target: editId,
          targetHandle: "image",
          type: "binding",
        }),
      );
      setSelectedId(editId);
      if (opts?.openEditor !== false) {
        if (editKind === "subjectMask") openMaskEditorFor(editId);
        if (editKind === "crop") openCropEditorFor(editId);
      }
      // Defer the partial run to the effect that fires once the new node has
      // landed in `nodes` (setNodes is async), matching the editor-confirm path.
      if (opts?.run) pendingRunNode.current = editId;
    },
    [
      nodes,
      setNodes,
      setEdges,
      setSelectedId,
      takeSnapshot,
      newNodeId,
      openMaskEditorFor,
      openCropEditorFor,
      pendingRunNode,
    ],
  );

  return {
    idSeq,
    clipboard,
    newNodeId,
    patchNode,
    onParamChange,
    addNode,
    copySelection,
    pasteClipboard,
    focusNode,
    jumpToNode,
    deleteNode,
    disconnectNode,
    duplicateNode,
    tidyLayout,
    selectAll,
    addBoundEdit,
  };
}
