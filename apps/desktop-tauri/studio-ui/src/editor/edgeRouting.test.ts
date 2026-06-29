import { describe, expect, it } from "vitest";
import { avoidanceMidX, orthogonalPoints, pointsToPath, routedPath, type Rect } from "./edgeRouting";

const s = { x: 0, y: 0 };
const t = { x: 200, y: 0 };

describe("avoidanceMidX", () => {
  it("keeps the midpoint when nothing blocks the vertical segment", () => {
    expect(avoidanceMidX(s, t, [])).toBe(100);
    // An obstacle far from the column is ignored.
    const far: Rect = { x: 400, y: 0, width: 40, height: 40 };
    expect(avoidanceMidX(s, t, [far])).toBe(100);
  });

  it("shifts to the nearer side of a blocking obstacle", () => {
    // Obstacle straddling the midpoint column (x≈100), endpoints span y 0..0
    // but the rect covers y -20..20, so the column at x=100 cuts through it.
    const block: Rect = { x: 90, y: -20, width: 40, height: 40 };
    const pad = 12;
    // right edge = 130 + 12 = 142 (deviation 42); left edge = 90 - 12 = 78 (deviation 22) → left wins
    expect(avoidanceMidX({ x: 0, y: -20 }, { x: 200, y: 20 }, [block], pad)).toBe(78);
  });

  it("clears all blocking obstacles at once", () => {
    const blocks: Rect[] = [
      { x: 80, y: -10, width: 30, height: 40 },
      { x: 100, y: -10, width: 60, height: 40 },
    ];
    const pad = 12;
    const mid = avoidanceMidX({ x: 0, y: -10 }, { x: 260, y: 30 }, blocks, pad);
    // midX=130; right of all = max(110,160)+12 = 172 (dev 42); left of all =
    // min(80,100)-12 = 68 (dev 62) → right side is the smaller deviation
    expect(mid).toBe(172);
  });
});

describe("orthogonalPoints / path", () => {
  it("builds a 4-point staircase through midX", () => {
    expect(orthogonalPoints(s, { x: 200, y: 50 }, 100)).toEqual([
      { x: 0, y: 0 },
      { x: 100, y: 0 },
      { x: 100, y: 50 },
      { x: 200, y: 50 },
    ]);
  });

  it("serializes points to an SVG path", () => {
    expect(pointsToPath([{ x: 0, y: 0 }, { x: 10, y: 0 }])).toBe("M 0,0 L 10,0");
  });

  it("routedPath composes avoidance + path", () => {
    expect(routedPath(s, t, [])).toBe("M 0,0 L 100,0 L 100,0 L 200,0");
  });
});
