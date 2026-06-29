import { describe, expect, it } from "vitest";
import { isLodActive, LOD_ZOOM_THRESHOLD } from "./lod";

describe("isLodActive", () => {
  it("is active (collapsed) below the threshold", () => {
    expect(isLodActive(LOD_ZOOM_THRESHOLD - 0.1)).toBe(true);
    expect(isLodActive(0.2)).toBe(true);
  });

  it("is inactive at or above the threshold", () => {
    expect(isLodActive(LOD_ZOOM_THRESHOLD)).toBe(false);
    expect(isLodActive(1)).toBe(false);
    expect(isLodActive(2)).toBe(false);
  });

  it("honours a custom threshold", () => {
    expect(isLodActive(0.8, 0.9)).toBe(true);
    expect(isLodActive(0.8, 0.5)).toBe(false);
  });
});
