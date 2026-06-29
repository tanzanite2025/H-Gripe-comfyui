// @vitest-environment jsdom
import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Edge, Node } from "@xyflow/react";

import { defaultParams } from "../graph/nodeSpecs";
import type { HgripeNodeData } from "./HgripeNode";
import type { NodeRunInfo, RunObserver } from "../runtime/dag";
import {
  useStudioRunController,
  type StudioRunControllerOptions,
} from "./useStudioRunController";

// Run execution is exercised through its mocked boundary (runGraph); the
// controller's own orchestration -- logging, history recording, failure
// highlighting, batch fan-out -- is what these tests pin down. The DAG runtime
// itself has its own suite (dag.test.ts).
const { runGraphMock } = vi.hoisted(() => ({ runGraphMock: vi.fn() }));

vi.mock("../runtime/dag", () => ({
  runGraph: runGraphMock,
}));

// Force the browser-preview path (isTauri === false); the Rust bridge calls
// are never reached, so they are inert stubs.
vi.mock("../bridge/tauri", () => ({
  isTauri: () => false,
  createStudioRunId: () => "run-id",
  cancelStudioRun: vi.fn(async () => {}),
  inspectPsd: vi.fn(async () => null),
  runStudioGraph: vi.fn(),
  readStudioRunHistory: vi.fn(async () => null),
  writeStudioRunHistory: vi.fn(async () => {}),
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
  const patchNode = vi.fn();
  const focusNode = vi.fn();
  const setMessage = vi.fn();
  const autoSnapshotBeforeRun = vi.fn();
  const options: StudioRunControllerOptions = {
    nodes,
    edges,
    setNodes,
    patchNode,
    focusNode,
    setMessage,
    autoSnapshotBeforeRun,
    projectStoreDir: null,
  };
  return { options, setNodes, patchNode, focusNode, setMessage, autoSnapshotBeforeRun };
}

beforeEach(() => {
  localStorage.clear();
  runGraphMock.mockReset();
  runGraphMock.mockResolvedValue({ outputs: new Map() });
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("useStudioRunController", () => {
  it("runs the browser-preview path and records a succeeded run", async () => {
    const { options, focusNode, autoSnapshotBeforeRun } = setup([makeNode("p1", "prompt")]);
    const { result } = renderHook(() => useStudioRunController(options));

    await act(async () => {
      await result.current.run();
    });

    expect(autoSnapshotBeforeRun).toHaveBeenCalledOnce();
    expect(runGraphMock).toHaveBeenCalledOnce();
    expect(result.current.running).toBe(false);
    expect(result.current.showLog).toBe(true);

    const messages = result.current.runLog.map((e) => e.message);
    expect(messages.some((m) => m.includes("run started"))).toBe(true);
    expect(messages.some((m) => m.includes("run finished"))).toBe(true);

    expect(result.current.runHistory).toHaveLength(1);
    expect(result.current.runHistory[0]).toMatchObject({
      kind: "run",
      outcome: "succeeded",
      backend: "browser preview",
    });
    expect(focusNode).not.toHaveBeenCalled();
  });

  it("ignores a re-entrant run while one is already in flight", async () => {
    let release!: () => void;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    runGraphMock.mockImplementation(async () => {
      await gate;
      return { outputs: new Map() };
    });
    const { options } = setup([makeNode("p1", "prompt")]);
    const { result } = renderHook(() => useStudioRunController(options));

    await act(async () => {
      // Kick off the first run (stays pending on `gate`) then try a second.
      const first = result.current.run();
      const second = result.current.run();
      release();
      await Promise.all([first, second]);
    });

    expect(runGraphMock).toHaveBeenCalledOnce();
    expect(result.current.runHistory).toHaveLength(1);
  });

  it("records per-node telemetry and highlights the first failed node", async () => {
    runGraphMock.mockImplementation(async (_graph: unknown, _registry: unknown, observer?: RunObserver) => {
      observer?.onNodeRun?.("p1", { status: "failed", durationMs: 5, error: "boom" } as NodeRunInfo);
      return { outputs: new Map() };
    });
    const { options, patchNode, focusNode } = setup([makeNode("p1", "prompt")]);
    const { result } = renderHook(() => useStudioRunController(options));

    await act(async () => {
      await result.current.run();
    });

    expect(patchNode).toHaveBeenCalledWith("p1", { durationMs: 5, error: "boom" });
    expect(focusNode).toHaveBeenCalledWith("p1");

    const record = result.current.runHistory[0];
    expect(record.failedNodes).toBe(1);
    // A nominal success is promoted to failed when a node reported failure.
    expect(record.outcome).toBe("failed");
    expect(
      result.current.runLog.some((e) => e.level === "error" && e.message.includes("1 node(s) failed")),
    ).toBe(true);
  });

  it("marks the run failed when execution throws", async () => {
    runGraphMock.mockRejectedValue(new Error("kaboom"));
    const { options, setMessage } = setup([makeNode("p1", "prompt")]);
    const { result } = renderHook(() => useStudioRunController(options));

    await act(async () => {
      await result.current.run();
    });

    expect(result.current.running).toBe(false);
    expect(result.current.runHistory[0].outcome).toBe("failed");
    expect(setMessage).toHaveBeenCalledWith(expect.stringContaining("error:"));
  });

  it("marks the run cancelled when execution throws a cancellation", async () => {
    runGraphMock.mockRejectedValue(new Error("Run was cancelled"));
    const { options, setMessage } = setup([makeNode("p1", "prompt")]);
    const { result } = renderHook(() => useStudioRunController(options));

    await act(async () => {
      await result.current.run();
    });

    expect(result.current.runHistory[0].outcome).toBe("cancelled");
    expect(setMessage).toHaveBeenCalledWith("cancelled");
  });

  it("fans out a batch run once per item", async () => {
    const { options } = setup([makeNode("b1", "batch", { items: "a\nb\nc" })]);
    const { result } = renderHook(() => useStudioRunController(options));

    expect(result.current.hasBatch).toBe(true);
    expect(result.current.batchCount).toBe(3);

    await act(async () => {
      await result.current.runBatch();
    });

    expect(runGraphMock).toHaveBeenCalledTimes(3);
    expect(result.current.runHistory[0]).toMatchObject({ kind: "batch", outcome: "succeeded" });
    expect(result.current.runLog.some((e) => e.message.includes("batch started: 3"))).toBe(true);
  });

  it("runBatch is a no-op without a batch node", async () => {
    const { options, setMessage } = setup([makeNode("p1", "prompt")]);
    const { result } = renderHook(() => useStudioRunController(options));

    expect(result.current.hasBatch).toBe(false);
    expect(result.current.batchCount).toBe(0);

    await act(async () => {
      await result.current.runBatch();
    });

    expect(setMessage).toHaveBeenCalledWith("batch: no items");
    expect(runGraphMock).not.toHaveBeenCalled();
    expect(result.current.runHistory).toHaveLength(0);
  });

  it("clears the log and (with confirmation) the history", async () => {
    const { options } = setup([makeNode("p1", "prompt")]);
    const { result } = renderHook(() => useStudioRunController(options));

    await act(async () => {
      await result.current.run();
    });
    expect(result.current.runLog.length).toBeGreaterThan(0);
    expect(result.current.runHistory).toHaveLength(1);

    act(() => result.current.clearLog());
    expect(result.current.runLog).toHaveLength(0);

    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(false);
    act(() => result.current.clearHistory());
    expect(result.current.runHistory).toHaveLength(1);

    confirmSpy.mockReturnValue(true);
    act(() => result.current.clearHistory());
    expect(result.current.runHistory).toHaveLength(0);
  });

  it("exports the run log as a text download", async () => {
    if (!("createObjectURL" in URL)) {
      Object.assign(URL, { createObjectURL: () => "", revokeObjectURL: () => {} });
    }
    const createSpy = vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:run-log");
    const revokeSpy = vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => {});
    const clickSpy = vi
      .spyOn(HTMLAnchorElement.prototype, "click")
      .mockImplementation(() => {});

    const { options } = setup([makeNode("p1", "prompt")]);
    const { result } = renderHook(() => useStudioRunController(options));

    await act(async () => {
      await result.current.run();
    });
    act(() => result.current.exportLog());

    expect(createSpy).toHaveBeenCalledOnce();
    expect(createSpy.mock.calls[0][0]).toBeInstanceOf(Blob);
    expect(clickSpy).toHaveBeenCalledOnce();
    expect(revokeSpy).toHaveBeenCalledWith("blob:run-log");
  });
});
