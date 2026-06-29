import { describe, expect, it } from "vitest";
import { layeredPositions } from "./layout";

describe("layeredPositions", () => {
  it("places a linear chain in left-to-right columns at the same row", () => {
    const pos = layeredPositions([["a"], ["b"], ["c"]]);
    expect(pos.get("a")).toEqual({ x: 40, y: 40 });
    expect(pos.get("b")).toEqual({ x: 300, y: 40 });
    expect(pos.get("c")).toEqual({ x: 560, y: 40 });
  });

  it("stacks nodes within a level and centers shorter columns", () => {
    const pos = layeredPositions([["a"], ["b", "c"]]);
    // tallest column has 2 nodes; the single-node column is offset down by half.
    expect(pos.get("a")).toEqual({ x: 40, y: 40 + 70 });
    expect(pos.get("b")).toEqual({ x: 300, y: 40 });
    expect(pos.get("c")).toEqual({ x: 300, y: 180 });
  });

  it("honours custom gaps / origin", () => {
    const pos = layeredPositions([["a"], ["b"]], { xGap: 100, yGap: 50, xStart: 0, yStart: 0 });
    expect(pos.get("a")).toEqual({ x: 0, y: 0 });
    expect(pos.get("b")).toEqual({ x: 100, y: 0 });
  });

  it("returns an empty map for no levels", () => {
    expect(layeredPositions([]).size).toBe(0);
    expect(layeredPositions([[]]).size).toBe(0);
  });
});
