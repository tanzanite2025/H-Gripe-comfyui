import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { GRAPH_VERSION, type WorkflowGraph } from "../graph/model";
import { clearPersistedGraph, loadPersistedGraph, persistGraph } from "./persist";

const graph: WorkflowGraph = {
  version: GRAPH_VERSION,
  nodes: [{ id: "n1", kind: "prompt", position: { x: 1, y: 2 }, params: { text: "hi" } }],
  edges: [],
};

function fakeStorage(): Storage {
  const m = new Map<string, string>();
  return {
    get length() {
      return m.size;
    },
    clear: () => m.clear(),
    getItem: (k: string) => (m.has(k) ? m.get(k)! : null),
    key: (i: number) => Array.from(m.keys())[i] ?? null,
    removeItem: (k: string) => void m.delete(k),
    setItem: (k: string, v: string) => void m.set(k, v),
  };
}

beforeEach(() => {
  (globalThis as unknown as { localStorage: Storage }).localStorage = fakeStorage();
});
afterEach(() => {
  delete (globalThis as unknown as { localStorage?: Storage }).localStorage;
});

describe("workspace autosave", () => {
  it("returns null when nothing is stored", () => {
    expect(loadPersistedGraph()).toBeNull();
  });

  it("round-trips a graph through storage", () => {
    persistGraph(graph);
    expect(loadPersistedGraph()).toEqual(graph);
  });

  it("clears the stored graph", () => {
    persistGraph(graph);
    clearPersistedGraph();
    expect(loadPersistedGraph()).toBeNull();
  });

  it("returns null (does not throw) on a corrupt payload", () => {
    localStorage.setItem("hgripe.studio.workflow.v1", "{not json");
    expect(loadPersistedGraph()).toBeNull();
  });
});
