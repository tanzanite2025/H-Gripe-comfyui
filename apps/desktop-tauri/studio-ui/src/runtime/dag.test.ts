import { describe, expect, it } from "vitest";
import { GRAPH_VERSION, type WorkflowGraph } from "../graph/model";
import {
  ancestorSubgraph,
  runGraph,
  topoLevels,
  validateGraph,
  wouldCreateCycle,
  type ExecutorRegistry,
} from "./dag";
import { defaultExecutors } from "./executors";

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

  it("emits per-node run telemetry (duration on success, error on failure)", async () => {
    const events = new Map<string, { status: string; durationMs?: number; error?: string }>();
    const registry: ExecutorRegistry = {
      prompt: async () => ({ text: "x" }),
      generate: async () => {
        throw new Error("boom");
      },
      preview: async (ctx) => ({ image: ctx.inputs.image ?? null }),
    };
    await expect(
      runGraph(chain, registry, { onNodeRun: (id, info) => events.set(id, info) }),
    ).rejects.toThrow("boom");

    expect(events.get("prompt-1")?.status).toBe("succeeded");
    expect(typeof events.get("prompt-1")?.durationMs).toBe("number");
    expect(events.get("generate-1")).toMatchObject({ status: "failed", error: "boom" });
    // preview never ran (its level was reached after the failure threw).
    expect(events.has("preview-1")).toBe(false);
  });

  it("aborts cooperatively when shouldCancel becomes true", async () => {
    const ran: string[] = [];
    const registry: ExecutorRegistry = {
      prompt: async (ctx) => {
        ran.push(ctx.nodeId);
        return { text: "x" };
      },
      generate: async (ctx) => {
        ran.push(ctx.nodeId);
        return { image: "y" };
      },
      preview: async (ctx) => {
        ran.push(ctx.nodeId);
        return { image: ctx.inputs.image ?? null };
      },
    };
    // Cancel after the first node so later levels never execute.
    let cancelled = false;
    const statuses = new Map<string, string>();
    await expect(
      runGraph(
        chain,
        registry,
        {
          onStatus: (id, s) => statuses.set(id, s),
          onNodeRun: (id) => {
            if (id === "prompt-1") cancelled = true;
          },
        },
        new Map(),
        () => cancelled,
      ),
    ).rejects.toThrow(/cancel/i);

    expect(ran).toEqual(["prompt-1"]);
    expect(statuses.get("generate-1")).toBe("cancelled");
  });
});

describe("conditional branch execution", () => {
  // prompt -> if -> { true: prev-true, false: prev-false }
  const branched = (cond: "true" | "false") =>
    graph({
      nodes: [
        { id: "p", kind: "prompt", position: { x: 0, y: 0 }, params: { text: "hi" } },
        { id: "if", kind: "if", position: { x: 0, y: 0 }, params: { cond } },
        { id: "t", kind: "preview", position: { x: 0, y: 0 }, params: {} },
        { id: "f", kind: "preview", position: { x: 0, y: 0 }, params: {} },
      ],
      edges: [
        { id: "e0", source: "p", sourcePort: "text", target: "if", targetPort: "value" },
        { id: "e1", source: "if", sourcePort: "true", target: "t", targetPort: "image" },
        { id: "e2", source: "if", sourcePort: "false", target: "f", targetPort: "image" },
      ],
    });

  // Tracks which nodes actually executed.
  const tracker = (ran: string[]): ExecutorRegistry => ({
    prompt: async (c) => ({ text: String(c.params.text ?? "") }),
    preview: async (c) => {
      ran.push(c.nodeId);
      return { image: c.inputs.image ?? null };
    },
    if: async (c) => {
      const active = String(c.params.cond ?? "true") === "true";
      return active ? { true: c.inputs.value } : { false: c.inputs.value };
    },
  });

  it("runs only the taken branch and skips the other", async () => {
    const ran: string[] = [];
    const { statuses, outputs } = await runGraph(branched("true"), tracker(ran));
    expect(ran).toEqual(["t"]);
    expect(statuses.get("t")).toBe("succeeded");
    expect(statuses.get("f")).toBe("skipped");
    expect(outputs.has("f")).toBe(false);
  });

  it("prunes the other branch when the condition flips", async () => {
    const ran: string[] = [];
    const { statuses } = await runGraph(branched("false"), tracker(ran));
    expect(ran).toEqual(["f"]);
    expect(statuses.get("t")).toBe("skipped");
    expect(statuses.get("f")).toBe("succeeded");
  });

  it("propagates skipping transitively down a pruned branch", async () => {
    const g = graph({
      nodes: [
        { id: "p", kind: "prompt", position: { x: 0, y: 0 }, params: { text: "hi" } },
        { id: "if", kind: "if", position: { x: 0, y: 0 }, params: { cond: "true" } },
        { id: "r", kind: "reroute", position: { x: 0, y: 0 }, params: {} },
        { id: "f", kind: "preview", position: { x: 0, y: 0 }, params: {} },
      ],
      edges: [
        { id: "e0", source: "p", sourcePort: "text", target: "if", targetPort: "value" },
        { id: "e1", source: "if", sourcePort: "false", target: "r", targetPort: "in" },
        { id: "e2", source: "r", sourcePort: "out", target: "f", targetPort: "image" },
      ],
    });
    const ran: string[] = [];
    const reg: ExecutorRegistry = {
      ...tracker(ran),
      reroute: async (c) => {
        ran.push(c.nodeId);
        return { out: c.inputs.in ?? null };
      },
    };
    const { statuses } = await runGraph(g, reg);
    expect(ran).toEqual([]);
    expect(statuses.get("r")).toBe("skipped");
    expect(statuses.get("f")).toBe("skipped");
  });

  it("drives the If condition from a Compare result (real condition chain)", async () => {
    // numA, numB -> compare(>) -> if.cond ; value -> if.value ; -> {true,false}
    const g = graph({
      nodes: [
        { id: "a", kind: "number", position: { x: 0, y: 0 }, params: { value: 5 } },
        { id: "b", kind: "number", position: { x: 0, y: 0 }, params: { value: 3 } },
        { id: "v", kind: "prompt", position: { x: 0, y: 0 }, params: { text: "go" } },
        { id: "cmp", kind: "compare", position: { x: 0, y: 0 }, params: { op: ">" } },
        { id: "if", kind: "if", position: { x: 0, y: 0 }, params: { cond: "false" } },
        { id: "t", kind: "preview", position: { x: 0, y: 0 }, params: {} },
        { id: "f", kind: "preview", position: { x: 0, y: 0 }, params: {} },
      ],
      edges: [
        { id: "e0", source: "a", sourcePort: "value", target: "cmp", targetPort: "a" },
        { id: "e1", source: "b", sourcePort: "value", target: "cmp", targetPort: "b" },
        { id: "e2", source: "cmp", sourcePort: "result", target: "if", targetPort: "cond" },
        { id: "e3", source: "v", sourcePort: "text", target: "if", targetPort: "value" },
        { id: "e4", source: "if", sourcePort: "true", target: "t", targetPort: "image" },
        { id: "e5", source: "if", sourcePort: "false", target: "f", targetPort: "image" },
      ],
    });
    expect(validateGraph(g)).toEqual([]);
    // 5 > 3 is true, so the wired cond (1) wins over the param fallback ("false").
    const { statuses } = await runGraph(g, defaultExecutors);
    expect(statuses.get("t")).toBe("succeeded");
    expect(statuses.get("f")).toBe("skipped");
  });

  it("keeps a merge node alive when at least one incoming branch survives", async () => {
    // if -> { true: merge, false: merge }: only one port fires, but the merge
    // has one live incoming edge, so it must still run.
    const g = graph({
      nodes: [
        { id: "p", kind: "prompt", position: { x: 0, y: 0 }, params: { text: "hi" } },
        { id: "if", kind: "if", position: { x: 0, y: 0 }, params: { cond: "true" } },
        { id: "m", kind: "preview", position: { x: 0, y: 0 }, params: {} },
      ],
      edges: [
        { id: "e0", source: "p", sourcePort: "text", target: "if", targetPort: "value" },
        { id: "e1", source: "if", sourcePort: "true", target: "m", targetPort: "image" },
        { id: "e2", source: "if", sourcePort: "false", target: "m", targetPort: "image" },
      ],
    });
    const ran: string[] = [];
    const { statuses, outputs } = await runGraph(g, tracker(ran));
    expect(ran).toEqual(["m"]);
    expect(statuses.get("m")).toBe("succeeded");
    expect(outputs.get("m")).toEqual({ image: "hi" });
  });
});

describe("ancestorSubgraph", () => {
  it("keeps only the target and its transitive inputs", () => {
    const sub = ancestorSubgraph(chain, "generate-1");
    expect(sub.nodes.map((n) => n.id).sort()).toEqual(["generate-1", "prompt-1"]);
    expect(sub.edges.map((e) => e.id)).toEqual(["e1"]);
  });

  it("drops sibling branches that do not feed the target", () => {
    const g = graph({
      nodes: [
        { id: "a", kind: "prompt", position: { x: 0, y: 0 }, params: {} },
        { id: "b", kind: "prompt", position: { x: 0, y: 0 }, params: {} },
        { id: "gen", kind: "generate", position: { x: 0, y: 0 }, params: {} },
        { id: "other", kind: "preview", position: { x: 0, y: 0 }, params: {} },
      ],
      edges: [
        { id: "e1", source: "a", sourcePort: "text", target: "gen", targetPort: "prompt" },
        { id: "e2", source: "gen", sourcePort: "image", target: "other", targetPort: "image" },
      ],
    });
    const sub = ancestorSubgraph(g, "gen");
    expect(sub.nodes.map((n) => n.id).sort()).toEqual(["a", "gen"]);
    expect(sub.edges.map((e) => e.id)).toEqual(["e1"]);
  });

  it("returns the graph unchanged when the target is absent", () => {
    expect(ancestorSubgraph(chain, "missing")).toBe(chain);
  });
});
