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
import { Toolbar } from "./editor/Toolbar";
import { RunLog } from "./editor/RunLogPanel";
import { SnapshotsPanel, type SnapshotDiffView } from "./editor/SnapshotsPanel";
import { RunHistoryPanel } from "./editor/RunHistoryPanel";
import { diffGraphs } from "./editor/snapshotdiff";
import { useProjectScopedStore } from "./editor/useProjectScopedStore";
import { useKeyboardShortcuts } from "./editor/useKeyboardShortcuts";
import { useStudioRunController } from "./editor/useStudioRunController";
import { LangContext, loadLang, saveLang, type Lang } from "./i18n";
import {
  addSnapshot,
  loadAutoSnapshotPref,
  loadSnapshots,
  newSnapshotId,
  parseSnapshots,
  removeSnapshot,
  renameSnapshot,
  saveAutoSnapshotPref,
  saveSnapshots,
  type Snapshot,
} from "./editor/snapshots";
import { clearPersistedGraph, loadPersistedGraph, persistGraph } from "./editor/persist";
import { defaultParams } from "./graph/nodeSpecs";
import { deserializeGraph, serializeGraph, type WorkflowGraph } from "./graph/model";
import { topoLevels, validateGraph } from "./runtime/dag";
import {
  clearStudioAutosave,
  deleteStudioWorkflow,
  duplicateStudioWorkflow,
  isTauri,
  listStudioWorkflows,
  pickProjectFolder,
  pickWorkflowOpenPath,
  pickWorkflowSavePath,
  readStudioAutosave,
  readStudioRecents,
  readStudioSnapshots,
  readStudioWorkflow,
  renameStudioWorkflow,
  writeStudioAutosave,
  writeStudioRecents,
  writeStudioSnapshots,
  writeStudioWorkflow,
  type StudioWorkflowFile,
} from "./bridge/tauri";

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
  const [snapshots, setSnapshots] = useState<Snapshot[]>(() => loadSnapshots());
  const [showSnapshots, setShowSnapshots] = useState(false);
  const [snapshotDiff, setSnapshotDiff] = useState<SnapshotDiffView | null>(null);
  const [autoSnapshot, setAutoSnapshot] = useState<boolean>(() => loadAutoSnapshotPref());
  const [snapToGrid, setSnapToGrid] = useState(false);
  const [lang, setLang] = useState<Lang>(() => loadLang());
  const toggleLang = useCallback(() => {
    setLang((prev) => {
      const next: Lang = prev === "en" ? "zh" : "en";
      saveLang(next);
      return next;
    });
  }, []);
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

  // Select a node and pan/zoom the viewport to center it (used by node search).
  const jumpToNode = useCallback(
    (id: string) => {
      focusNode(id);
      void fitView({ nodes: [{ id }], duration: 400, maxZoom: 1.5 });
    },
    [focusNode, fitView],
  );

  // Capture the current graph under a given name (no prompt).
  const captureNamed = useCallback(
    (name: string) => {
      const graph = toWorkflowGraph(nodes, edges);
      const snap: Snapshot = { id: newSnapshotId(), name, t: Date.now(), graph };
      setSnapshots((list) => addSnapshot(list, snap));
      return snap;
    },
    [nodes, edges],
  );

  // Capture the current graph as a named snapshot (prompts for a name).
  const captureSnapshot = useCallback(() => {
    const suggested = `Snapshot ${new Date().toLocaleString()}`;
    const name = window.prompt("Snapshot name", suggested);
    if (name === null) return;
    const snap = captureNamed(name.trim() || suggested);
    setMessage(`snapshot saved: ${snap.name}`);
  }, [captureNamed]);

  // Auto-capture before a run (when enabled and the graph is non-empty).
  const autoSnapshotBeforeRun = useCallback(() => {
    if (!autoSnapshot || nodes.length === 0) return;
    captureNamed(`Auto · ${new Date().toLocaleTimeString()}`);
  }, [autoSnapshot, nodes.length, captureNamed]);

  // Snapshots and run history are both project-scoped stores: persisted into
  // the selected project folder on desktop (so they travel with the project),
  // else to localStorage.
  const projectStoreDir = isDesktop ? projectDir : null;

  // The run lifecycle, run log, and run history live in their own controller.
  // The editor reaches it through these callbacks and consumes the returned
  // view state (panel toggles, counts, run actions).
  const {
    running,
    canCancel,
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
    runBatch,
    cancelRun,
    hasBatch,
    batchCount,
  } = useStudioRunController({
    nodes,
    edges,
    setNodes,
    patchNode,
    focusNode,
    setMessage,
    autoSnapshotBeforeRun,
    projectStoreDir,
  });

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

  // Snapshots are a project-scoped store: persisted into the selected project
  // folder on desktop (so they travel with the project), else to localStorage.
  // The shared hook owns the load/persist effects. (Run history follows the
  // same pattern, owned by useStudioRunController.)
  useProjectScopedStore({
    dir: projectStoreDir,
    state: snapshots,
    setState: setSnapshots,
    parse: parseSnapshots,
    read: readStudioSnapshots,
    write: writeStudioSnapshots,
    saveLocal: saveSnapshots,
    label: "snapshots",
    onError: setMessage,
  });

  // Persist the auto-snapshot preference.
  useEffect(() => {
    saveAutoSnapshotPref(autoSnapshot);
  }, [autoSnapshot]);

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

  // Compare a snapshot against the live graph (no mutation — read-only diff).
  const diffSnapshot = useCallback(
    (id: string) => {
      const snap = snapshots.find((s) => s.id === id);
      if (!snap) return;
      const current = toWorkflowGraph(nodes, edges);
      setSnapshotDiff({ id: snap.id, name: snap.name, diff: diffGraphs(snap.graph, current) });
    },
    [snapshots, nodes, edges],
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

  // Select every node and edge (Ctrl/Cmd+A).
  const selectAll = useCallback(() => {
    setNodes((ns) => ns.map((n) => ({ ...n, selected: true })));
    setEdges((es) => es.map((ed) => ({ ...ed, selected: true })));
  }, [setNodes, setEdges]);

  // Global keyboard shortcuts (edit + file/run); see the hook for behavior.
  useKeyboardShortcuts({
    undo,
    redo,
    selectAll,
    copySelection,
    pasteClipboard,
    save: () => void handleSave(),
    saveAs: () => void handleSaveAs(),
    open: () => void handleOpen(),
    newWorkflow,
    run: () => void run(),
    canRun: !running && issues.length === 0,
  });

  // Stable context value so memoized node cards can edit their own params.
  const editing = useMemo(() => ({ onParamChange }), [onParamChange]);

  return (
    <LangContext.Provider value={lang}>
    <div className="app">
      <Toolbar
        issues={issues}
        isDesktop={isDesktop}
        currentFile={currentFile}
        fileDirty={fileDirty}
        saved={saved}
        message={message}
        canUndo={history.canUndo}
        canRedo={history.canRedo}
        onUndo={undo}
        onRedo={redo}
        onToggleLang={toggleLang}
        showProject={showProject}
        setShowProject={setShowProject}
        showSnapshots={showSnapshots}
        setShowSnapshots={setShowSnapshots}
        showLog={showLog}
        setShowLog={setShowLog}
        showHistory={showHistory}
        setShowHistory={setShowHistory}
        snapshotCount={snapshots.length}
        logCount={runLog.length}
        historyCount={runHistory.length}
        nodes={nodes}
        onJumpToNode={jumpToNode}
        onNew={newWorkflow}
        onOpen={() => void handleOpen()}
        onSave={() => void handleSave()}
        onSaveAs={() => void handleSaveAs()}
        onReset={resetSample}
        onClear={clear}
        fileInputRef={fileInput}
        onFilePicked={(f) => void load(f)}
        snapToGrid={snapToGrid}
        setSnapToGrid={setSnapToGrid}
        onTidyLayout={tidyLayout}
        edgeType={edgeType}
        onChangeEdgeType={changeEdgeType}
        showMinimap={showMinimap}
        setShowMinimap={setShowMinimap}
        running={running}
        canCancel={canCancel}
        onRun={run}
        onCancelRun={cancelRun}
        hasBatch={hasBatch}
        batchCount={batchCount}
        onRunBatch={runBatch}
      />

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
              autoSnapshot={autoSnapshot}
              onToggleAutoSnapshot={setAutoSnapshot}
              onCapture={captureSnapshot}
              onRestore={restoreSnapshot}
              onRename={renameSnapshotById}
              onDelete={deleteSnapshot}
              onDiff={diffSnapshot}
              diff={snapshotDiff}
              onClearDiff={() => setSnapshotDiff(null)}
              onClose={() => setShowSnapshots(false)}
            />
          )}
          {showHistory && (
            <RunHistoryPanel
              history={runHistory}
              onClear={clearHistory}
              onClose={() => setShowHistory(false)}
              onSelectNode={focusNode}
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
                onClear={clearLog}
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
    </LangContext.Provider>
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
