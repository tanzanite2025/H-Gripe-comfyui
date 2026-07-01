import { describe, expect, it } from "vitest";
import {
  applyOp,
  buildProxyMask,
  createProxyMask,
  dilate,
  erode,
  feather,
  fillHoles,
  invert,
  isPreviewableOp,
  PREVIEWABLE_OP_IDS,
  smooth,
  stampDisc,
  type ProxyMask,
} from "./maskMorphology";
import { emptyEditPaths, type EditPaths } from "../types/production";

/** Count of set (>=128) pixels — a proxy for mask "area". */
function area(mask: ProxyMask): number {
  let n = 0;
  for (const v of mask.data) if (v >= 128) n++;
  return n;
}

function filledSquare(size: number, inset: number): ProxyMask {
  const mask = createProxyMask(size, size);
  for (let y = inset; y < size - inset; y++) {
    for (let x = inset; x < size - inset; x++) mask.data[y * size + x] = 255;
  }
  return mask;
}

describe("maskMorphology preview primitives", () => {
  it("stampDisc fills a clamped circular region", () => {
    const mask = createProxyMask(20, 20);
    stampDisc(mask, 10, 10, 5, 255);
    expect(mask.data[10 * 20 + 10]).toBe(255); // centre set
    expect(mask.data[10 * 20 + 19]).toBe(0); // far corner untouched
    expect(area(mask)).toBeGreaterThan(0);
  });

  it("dilate grows and erode shrinks the mask area", () => {
    const base = filledSquare(40, 12); // 16x16 block
    const grown = dilate(base, 3);
    const eroded = erode(base, 3);
    expect(area(grown)).toBeGreaterThan(area(base));
    expect(area(eroded)).toBeLessThan(area(base));
  });

  it("dilate/erode with radius 0 are identity", () => {
    const base = filledSquare(20, 6);
    expect(area(dilate(base, 0))).toBe(area(base));
    expect(area(erode(base, 0))).toBe(area(base));
  });

  it("feather produces soft (intermediate) alpha at the edge", () => {
    const base = filledSquare(40, 12);
    const soft = feather(base, 3);
    const hasSoftEdge = Array.from(soft.data).some((v) => v > 0 && v < 255);
    expect(hasSoftEdge).toBe(true);
  });

  it("invert flips every pixel", () => {
    const base = filledSquare(10, 3);
    const inv = invert(base);
    for (let i = 0; i < base.data.length; i++) expect(inv.data[i]).toBe(255 - base.data[i]);
  });

  it("smooth removes an isolated speckle (morphological open)", () => {
    const mask = createProxyMask(40, 40);
    stampDisc(mask, 20, 20, 8, 255); // main blob
    mask.data[2 * 40 + 2] = 255; // 1px speckle in the corner
    const cleaned = smooth(mask, 2);
    expect(cleaned.data[2 * 40 + 2]).toBe(0);
    expect(cleaned.data[20 * 40 + 20]).toBe(255); // blob survives
  });

  it("fillHoles closes an enclosed interior hole but not the exterior", () => {
    const mask = filledSquare(21, 4); // solid block
    const cx = 10;
    mask.data[cx * 21 + cx] = 0; // punch a 1px hole in the centre
    const filled = fillHoles(mask);
    expect(filled.data[cx * 21 + cx]).toBe(255); // hole filled
    expect(filled.data[0]).toBe(0); // exterior background stays background
  });

  it("applyOp dispatches by op type and no-ops for wand", () => {
    const base = filledSquare(30, 10);
    expect(area(applyOp(base, "grow", 2))).toBeGreaterThan(area(base));
    expect(area(applyOp(base, "shrink", 2))).toBeLessThan(area(base));
    expect(area(applyOp(base, "wand", 4))).toBe(area(base)); // pixels needed → identity
  });

  it("exposes the amount-taking morphology ops as previewable", () => {
    expect([...PREVIEWABLE_OP_IDS]).toEqual(["grow", "shrink", "feather", "smooth"]);
    expect(isPreviewableOp("grow")).toBe(true);
    expect(isPreviewableOp("invert")).toBe(false);
    expect(isPreviewableOp("wand")).toBe(false);
  });
});

describe("buildProxyMask", () => {
  it("rasterises a brush stroke into a downscaled proxy", () => {
    const edits: EditPaths = {
      ...emptyEditPaths(),
      brush_strokes: [{ id: "s1", mode: "add", radius: 40, points: [[480, 320]] }],
    };
    const { mask, scale } = buildProxyMask(edits, { w: 960, h: 640 }, { proxyWidth: 320 });
    expect(mask.w).toBe(320);
    expect(mask.h).toBe(213); // 640 * (320/960) rounded
    expect(scale).toBeCloseTo(1 / 3, 5);
    expect(area(mask)).toBeGreaterThan(0);
  });

  it("applies queued morphology operations in order on top of strokes", () => {
    const stroke = { id: "s1", mode: "add", radius: 40, points: [[480, 320]] as [number, number][] };
    const baseEdits: EditPaths = { ...emptyEditPaths(), brush_strokes: [stroke] };
    const grownEdits: EditPaths = {
      ...emptyEditPaths(),
      brush_strokes: [stroke],
      operations: [{ type: "grow", amount: 12 }],
    };
    const base = buildProxyMask(baseEdits, { w: 960, h: 640 });
    const grown = buildProxyMask(grownEdits, { w: 960, h: 640 });
    expect(area(grown.mask)).toBeGreaterThan(area(base.mask));
  });

  it("skips wand ops (no source pixels on the proxy)", () => {
    const edits: EditPaths = {
      ...emptyEditPaths(),
      brush_strokes: [{ id: "s1", mode: "add", radius: 40, points: [[480, 320]] }],
      operations: [{ type: "wand", amount: 30, region: [10, 10] }],
    };
    const withWand = buildProxyMask(edits, { w: 960, h: 640 });
    const withoutWand = buildProxyMask(
      { ...edits, operations: [] },
      { w: 960, h: 640 },
    );
    expect(area(withWand.mask)).toBe(area(withoutWand.mask));
  });
});
