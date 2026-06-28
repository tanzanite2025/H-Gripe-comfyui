import { describe, expect, it } from "vitest";
import { matches } from "./Palette";

const spec = {
  kind: "psdExport",
  title: "PSD Export",
  description: "Write the generated image into a PSD template's placeholder and export final.psd.",
};

describe("palette search matcher", () => {
  it("matches everything for an empty query", () => {
    expect(matches(spec, "")).toBe(true);
    expect(matches(spec, "   ")).toBe(true);
  });

  it("matches on title, kind, or description, case-insensitively", () => {
    expect(matches(spec, "psd")).toBe(true);
    expect(matches(spec, "EXPORT")).toBe(true);
    expect(matches(spec, "psdexport")).toBe(true);
    expect(matches(spec, "template")).toBe(true);
  });

  it("requires every whitespace-separated term to match (AND)", () => {
    expect(matches(spec, "psd export")).toBe(true);
    expect(matches(spec, "psd missing")).toBe(false);
  });

  it("returns false when nothing matches", () => {
    expect(matches(spec, "reroute")).toBe(false);
  });
});
