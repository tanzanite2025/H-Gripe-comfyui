import { describe, expect, it } from "vitest";
import type { Node } from "@xyflow/react";

import { searchNodes } from "./nodesearch";

const node = (id: string, kind: string, type = "hgripe"): Node => ({
  id,
  type,
  position: { x: 0, y: 0 },
  data: { kind, params: {} },
});

describe("searchNodes", () => {
  const nodes: Node[] = [
    node("n1", "prompt"),
    node("n2", "generate"),
    node("g1", "prompt", "group"),
  ];

  it("returns nothing for a blank query", () => {
    expect(searchNodes(nodes, "  ")).toEqual([]);
  });

  it("matches by kind, case-insensitively", () => {
    expect(searchNodes(nodes, "GENERATE").map((m) => m.id)).toEqual(["n2"]);
  });

  it("matches by node id", () => {
    expect(searchNodes(nodes, "n1").map((m) => m.id)).toEqual(["n1"]);
  });

  it("skips group frames", () => {
    expect(searchNodes(nodes, "prompt").map((m) => m.id)).toEqual(["n1"]);
  });

  it("respects the result cap", () => {
    const many = Array.from({ length: 30 }, (_, i) => node(`p${i}`, "prompt"));
    expect(searchNodes(many, "prompt", 5)).toHaveLength(5);
  });
});
