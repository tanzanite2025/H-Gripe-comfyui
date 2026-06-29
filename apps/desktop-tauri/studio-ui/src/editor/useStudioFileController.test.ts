// @vitest-environment jsdom
import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Edge, Node } from "@xyflow/react";

import { defaultParams } from "../graph/nodeSpecs";
import type { HgripeNodeData } from "./HgripeNode";
import {
  useStudioFileController,
  type StudioFileControllerOptions,
} from "./useStudioFileController";

// The persistence boundary (Tauri bridge) is mocked to the browser-preview
// shape (isTauri === false), so the desktop paths are inert and the hook falls
// back to localStorage. These tests pin the controller's own logic: the dirty
// flag, the discard guard, snapshot capture/restore/diff, and the sample/new
// resets. The snapshot store helpers and persistence have their own suites.
vi.mock("../bridge/tauri", () => ({
  isTauri: () => false,
  clearStudioAutosave: vi.fn(async () => {}),
  deleteStudioWorkflow: vi.fn(async () => {}),
  duplicateStudioWorkflow: vi.fn(async () => ""),
  listStudioWorkflows: vi.fn(async () => []),
  pickProjectFolder: vi.fn(async () => null),
  pickWorkflowOpenPath: vi.fn(async () => null),
  pickWorkflowSavePath: vi.fn(async () => null),
  readStudioAutosave: vi.fn(async () => null),
  readStudioRecents: vi.fn(async () => null),
  readStudioSnapshots: vi.fn(async () => null),
  readStudioWorkflow: vi.fn(async () => ""),
  renameStudioWorkflow: vi.fn(async () => ""),
  writeStudioAutosave: vi.fn(async () => {}),
  writeStudioRecents: vi.fn(async () => {}),
  writeStudioSnapshots: vi.fn(async () => {}),
  writeStudioWorkflow: vi.fn(async () => {}),
}));

function makeNode(id: string, kind: string, params?: Record<string, unknown>): Node {
  const data: HgripeNodeData = {
    kind,
    params: { ...defaultParams(kind), ...params },
    status: "idle",
  };
  return { id, type: "hgripe", position: { x: 0, y: 0 }, data };
}

function setup(nodes: Node[], edges: Edge[] = []) {
  const setNodes = vi.fn();
  const setEdges = vi.fn();
  const setSelectedId = vi.fn();
  const takeSnapshot = vi.fn();
  const setMessage = vi.fn();
  const sampleNodes = [makeNode("s1", "prompt")];
  const sampleEdges: Edge[] = [];
  const options: StudioFileControllerOptions = {
    nodes,
    edges,
    setNodes,
    setEdges,
    setSelectedId,
    takeSnapshot,
    setMessage,
    sampleNodes,
    sampleEdges,
    restoredOnMount: false,
  };
  return { options, setNodes, setEdges, setSelectedId, takeSnapshot, setMessage };
}

beforeEach(() => {
  localStorage.clear();
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("useStudioFileController", () => {
  it("flags the file dirty on a user edit but not on the initial mount", () => {
    const { options } = setup([makeNode("p1", "prompt")]);
    const { result, rerender } = renderHook((props) => useStudioFileController(props), {
      initialProps: options,
    });
    // The mount swap is treated as programmatic, so nothing is dirty yet.
    expect(result.current.fileDirty).toBe(false);

    rerender({ ...options, nodes: [makeNode("p1", "prompt"), makeNode("p2", "prompt")] });
    expect(result.current.fileDirty).toBe(true);
  });

  it("suppresses the next dirty-mark for a programmatic swap", () => {
    const { options } = setup([makeNode("p1", "prompt")]);
    const { result, rerender } = renderHook((props) => useStudioFileController(props), {
      initialProps: options,
    });

    act(() => result.current.suppressNextDirty());
    rerender({ ...options, nodes: [makeNode("p1", "prompt"), makeNode("p2", "prompt")] });
    expect(result.current.fileDirty).toBe(false);
  });

  it("captures a named snapshot when the prompt is answered", () => {
    vi.spyOn(window, "prompt").mockReturnValue("My snapshot");
    const { options, setMessage } = setup([makeNode("p1", "prompt")]);
    const { result } = renderHook(() => useStudioFileController(options));

    act(() => result.current.captureSnapshot());
    expect(result.current.snapshots).toHaveLength(1);
    expect(result.current.snapshots[0].name).toBe("My snapshot");
    expect(setMessage).toHaveBeenCalledWith("snapshot saved: My snapshot");
  });

  it("does not capture a snapshot when the prompt is cancelled", () => {
    vi.spyOn(window, "prompt").mockReturnValue(null);
    const { options } = setup([makeNode("p1", "prompt")]);
    const { result } = renderHook(() => useStudioFileController(options));

    act(() => result.current.captureSnapshot());
    expect(result.current.snapshots).toHaveLength(0);
  });

  it("restores a snapshot into the editor, swapping the graph", () => {
    vi.spyOn(window, "prompt").mockReturnValue("snap");
    const { options, setNodes, setEdges } = setup([makeNode("p1", "prompt")]);
    const { result } = renderHook(() => useStudioFileController(options));

    act(() => result.current.captureSnapshot());
    const id = result.current.snapshots[0].id;
    setNodes.mockClear();
    setEdges.mockClear();

    act(() => result.current.restoreSnapshot(id));
    expect(setNodes).toHaveBeenCalled();
    expect(setEdges).toHaveBeenCalled();
    expect(result.current.fileDirty).toBe(true);
  });

  it("guards snapshot restore behind the discard prompt when the file is dirty", () => {
    vi.spyOn(window, "prompt").mockReturnValue("snap");
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(false);
    const { options, setNodes } = setup([makeNode("p1", "prompt")]);
    const { result, rerender } = renderHook((props) => useStudioFileController(props), {
      initialProps: options,
    });

    act(() => result.current.captureSnapshot());
    const id = result.current.snapshots[0].id;
    // Make the file dirty so the restore must ask for confirmation.
    rerender({ ...options, nodes: [makeNode("p1", "prompt"), makeNode("p2", "prompt")] });
    expect(result.current.fileDirty).toBe(true);
    setNodes.mockClear();

    act(() => result.current.restoreSnapshot(id));
    expect(confirm).toHaveBeenCalled();
    expect(setNodes).not.toHaveBeenCalled();
  });

  it("computes a snapshot diff and clears it", () => {
    vi.spyOn(window, "prompt").mockReturnValue("snap");
    const { options } = setup([makeNode("p1", "prompt")]);
    const { result } = renderHook(() => useStudioFileController(options));

    act(() => result.current.captureSnapshot());
    const id = result.current.snapshots[0].id;

    act(() => result.current.diffSnapshot(id));
    expect(result.current.snapshotDiff).not.toBeNull();
    expect(result.current.snapshotDiff?.id).toBe(id);

    act(() => result.current.clearSnapshotDiff());
    expect(result.current.snapshotDiff).toBeNull();
  });

  it("deletes a snapshot only after confirmation", () => {
    vi.spyOn(window, "prompt").mockReturnValue("snap");
    const confirm = vi.spyOn(window, "confirm");
    const { options } = setup([makeNode("p1", "prompt")]);
    const { result } = renderHook(() => useStudioFileController(options));

    act(() => result.current.captureSnapshot());
    const id = result.current.snapshots[0].id;

    confirm.mockReturnValueOnce(false);
    act(() => result.current.deleteSnapshot(id));
    expect(result.current.snapshots).toHaveLength(1);

    confirm.mockReturnValueOnce(true);
    act(() => result.current.deleteSnapshot(id));
    expect(result.current.snapshots).toHaveLength(0);
  });

  it("auto-captures before a run only when enabled and the graph is non-empty", () => {
    const { options } = setup([makeNode("p1", "prompt")]);
    const { result } = renderHook(() => useStudioFileController(options));

    // When disabled, the pre-run hook is a no-op.
    act(() => result.current.setAutoSnapshot(false));
    act(() => result.current.autoSnapshotBeforeRun());
    expect(result.current.snapshots).toHaveLength(0);

    act(() => result.current.setAutoSnapshot(true));
    act(() => result.current.autoSnapshotBeforeRun());
    expect(result.current.snapshots).toHaveLength(1);
    expect(result.current.snapshots[0].name).toMatch(/^Auto · /);
  });

  it("does not auto-capture when the graph is empty", () => {
    const { options } = setup([]);
    const { result } = renderHook(() => useStudioFileController(options));

    expect(result.current.autoSnapshot).toBe(true);
    act(() => result.current.autoSnapshotBeforeRun());
    expect(result.current.snapshots).toHaveLength(0);
  });

  it("loads a browser-preview file, swapping the graph and clearing currentFile", async () => {
    const { options, setNodes, setMessage } = setup([makeNode("p1", "prompt")]);
    const { result } = renderHook(() => useStudioFileController(options));
    const graphJson = JSON.stringify({ version: 1, nodes: [], edges: [] });
    const file = { name: "wf.json", text: async () => graphJson } as unknown as File;

    await act(async () => {
      await result.current.load(file);
    });
    expect(setNodes).toHaveBeenCalled();
    expect(result.current.currentFile).toBeNull();
    expect(setMessage).toHaveBeenCalledWith("loaded wf.json");
  });

  it("resets to the sample graph via the discard-guarded reset", () => {
    const { options, setNodes, setEdges, setMessage } = setup([makeNode("p1", "prompt")]);
    const { result } = renderHook(() => useStudioFileController(options));
    setNodes.mockClear();
    setEdges.mockClear();

    act(() => result.current.resetSample());
    expect(setNodes).toHaveBeenCalledWith(options.sampleNodes);
    expect(setEdges).toHaveBeenCalledWith(options.sampleEdges);
    expect(setMessage).toHaveBeenCalledWith("reset to sample workflow");
  });

  it("creates a new untitled workflow", () => {
    const { options, setNodes, setMessage } = setup([makeNode("p1", "prompt")]);
    const { result } = renderHook(() => useStudioFileController(options));
    setNodes.mockClear();

    act(() => result.current.newWorkflow());
    expect(setNodes).toHaveBeenCalledWith([]);
    expect(result.current.currentFile).toBeNull();
    expect(setMessage).toHaveBeenCalledWith("new workflow");
  });
});
