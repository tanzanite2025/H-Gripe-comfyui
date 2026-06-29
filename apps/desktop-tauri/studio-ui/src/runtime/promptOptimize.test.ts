import { describe, expect, it } from "vitest";
import { optimizePromptLocally } from "./promptOptimize";

describe("optimizePromptLocally", () => {
  it("collapses whitespace and trims comma segments", () => {
    expect(optimizePromptLocally("  a fox ,   running  \n , river ")).toBe(
      "a fox, running, river",
    );
  });

  it("dedupes segments case-insensitively, keeping the first occurrence", () => {
    expect(optimizePromptLocally("Fox, fox, FOX, river")).toBe("Fox, river");
  });

  it("appends preset booster tags (deduped against existing content)", () => {
    expect(optimizePromptLocally("a cat", "photographic")).toBe(
      "a cat, photorealistic, high detail, sharp focus, natural lighting, 8k",
    );
    // An already-present booster is not duplicated.
    expect(optimizePromptLocally("a cat, masterpiece", "detailed")).toBe(
      "a cat, masterpiece, highly detailed, intricate, ultra quality",
    );
  });

  it("cleanup preset only normalises (no boosters)", () => {
    expect(optimizePromptLocally("a, b", "cleanup")).toBe("a, b");
  });

  it("returns an empty string for empty/whitespace input without adding tags", () => {
    expect(optimizePromptLocally("   ", "photographic")).toBe("");
    expect(optimizePromptLocally("")).toBe("");
  });
});
