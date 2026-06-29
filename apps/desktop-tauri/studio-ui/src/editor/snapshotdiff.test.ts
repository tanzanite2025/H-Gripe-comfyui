import { describe, expect, it } from "vitest";

import { GRAPH_VERSION, type WorkflowGraph } from "../graph/model";
import { diffGraphs, isEmptyDiff } from "./snapshotdiff";

const node = (id: string, kind: string, params: Record<string, unknown> = {}) => ({
  id,
  kind,
  position: { x: 0, y: 0 },
  params,
});
const edge = (id: string, source: string, sourcePort: string, target: string, targetPort: string) => ({
  id,
  source,
  sourcePort,
  target,
  targetPort,
});
const graph = (nodes: WorkflowGraph["nodes"], edges: WorkflowGraph["edges"] = []): WorkflowGraph => ({
  version: GRAPH_VERSION,
  nodes,
  edges,
});

describe("diffGraphs", () => {
  it("reports no changes for identical graphs", () => {
    const g = graph([node("a", "prompt", { text: "hi" })], []);
    const d = diffGraphs(g, g);
    expect(isEmptyDiff(d)).toBe(true);
  });

  it("detects added and removed nodes", () => {
    const base = graph([node("a", "prompt")]);
    const curr = graph([node("b", "generate")]);
    const d = diffGraphs(base, curr);
    expect(d.addedNodes).toEqual([{ id: "b", kind: "generate" }]);
    expect(d.removedNodes).toEqual([{ id: "a", kind: "prompt" }]);
    expect(isEmptyDiff(d)).toBe(false);
  });

  it("detects changed params and kind", () => {
    const base = graph([node("a", "prompt", { text: "x", steps: 1 })]);
    const curr = graph([node("a", "generate", { text: "y", steps: 1 })]);
    const d = diffGraphs(base, curr);
    expect(d.changedNodes).toEqual([{ id: "a", kind: "generate", params: ["text"], kindChanged: true }]);
  });

  it("ignores edge id changes, matching on endpoints", () => {
    const base = graph([node("a", "prompt"), node("b", "generate")], [
      edge("e1", "a", "text", "b", "prompt"),
    ]);
    const curr = graph([node("a", "prompt"), node("b", "generate")], [
      edge("different-id", "a", "text", "b", "prompt"),
    ]);
    expect(isEmptyDiff(diffGraphs(base, curr))).toBe(true);
  });

  it("detects added and removed edges", () => {
    const base = graph([node("a", "prompt"), node("b", "generate")], []);
    const curr = graph([node("a", "prompt"), node("b", "generate")], [
      edge("e1", "a", "text", "b", "prompt"),
    ]);
    const d = diffGraphs(base, curr);
    expect(d.addedEdges).toEqual(["a:text → b:prompt"]);
    expect(d.removedEdges).toEqual([]);
  });
});
