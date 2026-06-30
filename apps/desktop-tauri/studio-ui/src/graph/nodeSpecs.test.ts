import { describe, expect, it } from "vitest";
import { NODE_SPECS, type Executor } from "./nodeSpecs";

const VALID: Executor[] = ["graph", "local", "compute", "api", "hybrid"];

describe("nodeSpecs executor tagging", () => {
  it("tags every node kind with a valid executor", () => {
    for (const [kind, spec] of Object.entries(NODE_SPECS)) {
      expect(VALID, `${kind} has a valid executor`).toContain(spec.executor);
    }
  });

  it("routes PSD bridge cards to local and provider cards to api", () => {
    const expected: Record<string, Executor> = {
      psdContextAnalyze: "local",
      matchLightColor: "local",
      refineMaskEdge: "local",
      imageEnhance: "local",
      detailWatchdog: "local",
      psdExport: "local",
      subjectMask: "compute",
      generate: "api",
      detailRepaint: "api",
      promptOptimize: "hybrid",
      prompt: "graph",
    };
    for (const [kind, executor] of Object.entries(expected)) {
      expect(NODE_SPECS[kind]?.executor, kind).toBe(executor);
    }
  });
});
