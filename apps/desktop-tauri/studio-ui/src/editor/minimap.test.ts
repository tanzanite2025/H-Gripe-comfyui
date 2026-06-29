import { describe, expect, it } from "vitest";
import { miniMapColor } from "./minimap";

describe("miniMapColor", () => {
  it("uses the run-status color when a status is present", () => {
    expect(miniMapColor("failed", "input")).toBe("#ff5d5d");
    expect(miniMapColor("running", "output")).toBe("#ffcc00");
    expect(miniMapColor("succeeded", "control")).toBe("#38d39f");
    expect(miniMapColor("cached", "control")).toBe("#38d39f");
    expect(miniMapColor("skipped", "input")).toBe("#555a66");
  });

  it("falls back to the category color when idle / unset", () => {
    expect(miniMapColor("idle", "control")).toBe("#ffa657");
    expect(miniMapColor(undefined, "input")).toBe("#6aa3ff");
    expect(miniMapColor(undefined, "generate")).toBe("#b98cff");
    expect(miniMapColor(undefined, "output")).toBe("#5fd0d0");
    expect(miniMapColor(undefined, "utility")).toBe("#8a93a3");
  });
});
