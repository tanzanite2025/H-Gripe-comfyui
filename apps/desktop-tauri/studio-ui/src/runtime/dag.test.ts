import { describe, expect, it } from "vitest";
import { GRAPH_VERSION, type WorkflowGraph } from "../graph/model";
import { runGraph, topoLevels, validateGraph, wouldCreateCycle, type ExecutorRegistry } from "./dag";

function graph(partial: Pick<WorkflowGraph, "nodes" | "edges">): WorkflowGraph {
  return { version: GRAPH_VERSION, ...partial };
}

const chain = graph({
  nodes: [
    { id: "prompt-1", kind: "prompt", position: { x: 0, y: 0 }, params: { text: "hi" } },
    { id: "generate-1", kind: "generate", position: { x: 0, y: 0 }, params: { provider: "mock", operation: "echo" } },
    { id: "preview-1", kind: "preview", position: { x: 0, y: 0 }, params: {} },
  ],
  edges: [
    { id: "e1", source: "prompt-1", sourcePort: "text", target: "generate-1", targetPort: "prompt" },
    { id: "e2", source: "generate-1", sourcePort: "image", target: "preview-1", targetPort: "image" },
  ],
});

describe("topoLevels", () => {
  it("orders a linear chain into single-node levels", () => {
    expect(topoLevels(chain)).toEqual([["prompt-1"], ["generate-1"], ["preview-1"]]);
  });

  it("groups independent nodes into the same level", () => {
    const g = graph({
      nodes: [
        { id: "a", kind: "prompt", position: { x: 0, y: 0 }, params: {} },
        { id: "b", kind: "prompt", position: { x: 0, y: 0 }, params: {} },
        { id: "gen", kind: "generate", position: { x: 0, y: 0 }, params: {} },
      ],
      edges: [
        { id: "e1", source: "a", sourcePort: "text", target: "gen", targetPort: "prompt" },
        { id: "e2", source: "b", sourcePort: "text", target: "gen", targetPort: "prompt" },
      ],
    });
    const levels = topoLevels(g);
    expect(new Set(levels[0])).toEqual(new Set(["a", "b"]));
    expect(levels[1]).toEqual(["gen"]);
  });

  it("throws on a cycle", () => {
    const g = graph({
      nodes: [
        { id: "a", kind: "generate", position: { x: 0, y: 0 }, params: {} },
        { id: "b", kind: "generate", position: { x: 0, y: 0 }, params: {} },
      ],
      edges: [
        { id: "e1", source: "a", sourcePort: "image", target: "b", targetPort: "reference" },
        { id: "e2", source: "b", sourcePort: "image", target: "a", targetPort: "reference" },
      ],
    });
    expect(() => topoLevels(g)).toThrow(/cycle/);
  });
});

describe("wouldCreateCycle", () => {
  it("detects a back-edge", () => {
    expect(wouldCreateCycle(chain, "preview-1", "prompt-1")).toBe(true);
  });
  it("allows a forward edge", () => {
    expect(wouldCreateCycle(chain, "prompt-1", "preview-1")).toBe(false);
  });
});

describe("validateGraph", () => {
  it("passes a well-typed chain", () => {
    expect(validateGraph(chain)).toEqual([]);
  });

  it("flags a type mismatch", () => {
    const g = graph({
      nodes: chain.nodes,
      edges: [
        // image -> prompt(text) is incompatible
        { id: "bad", source: "generate-1", sourcePort: "image", target: "generate-1", targetPort: "prompt" },
      ],
    });
    const issues = validateGraph(g);
    expect(issues.some((i) => i.code === "type_mismatch")).toBe(true);
  });
});

describe("runGraph", () => {
  it("threads outputs through the chain and reports statuses", async () => {
    const seen: string[] = [];
    const registry: ExecutorRegistry = {
      prompt: async (ctx) => {
        seen.push(ctx.nodeId);
        return { text: String(ctx.params.text ?? "") };
      },
      generate: async (ctx) => {
        seen.push(ctx.nodeId);
        return { image: `img:${String(ctx.inputs.prompt ?? "")}` };
      },
      preview: async (ctx) => {
        seen.push(ctx.nodeId);
        return { image: ctx.inputs.image ?? null };
      },
    };
    const { outputs, statuses } = await runGraph(chain, registry);
    expect(seen).toEqual(["prompt-1", "generate-1", "preview-1"]);
    expect(outputs.get("preview-1")).toEqual({ image: "img:hi" });
    expect(statuses.get("preview-1")).toBe("succeeded");
  });
});
