import { describe, expect, it } from "vitest";

import type { RunLogEntry } from "./runlog";
import {
  addRunRecord,
  parseRunHistory,
  RUN_HISTORY_CAP,
  runDurationMs,
  summarizeRun,
  type RunRecord,
} from "./runhistory";

const entry = (id: number): RunLogEntry => ({ id, t: 0, level: "info", message: `m${id}` });
const record = (id: string, over: Partial<RunRecord> = {}): RunRecord => ({
  id,
  kind: "run",
  startedAt: 1000,
  endedAt: 2500,
  outcome: "succeeded",
  backend: "browser preview",
  failedNodes: 0,
  entries: [entry(0)],
  ...over,
});

describe("addRunRecord", () => {
  it("prepends newest-first without mutating the input", () => {
    const list = [record("a")];
    const next = addRunRecord(list, record("b"));
    expect(next.map((r) => r.id)).toEqual(["b", "a"]);
    expect(list).toHaveLength(1);
  });

  it("trims to the cap, dropping the oldest", () => {
    let list: RunRecord[] = [];
    for (let i = 0; i < RUN_HISTORY_CAP + 3; i++) list = addRunRecord(list, record(`r${i}`));
    expect(list).toHaveLength(RUN_HISTORY_CAP);
    expect(list.some((r) => r.id === "r0")).toBe(false);
  });
});

describe("runDurationMs / summarizeRun", () => {
  it("computes a non-negative duration", () => {
    expect(runDurationMs(record("a"))).toBe(1500);
    expect(runDurationMs(record("a", { endedAt: 0 }))).toBe(0);
  });

  it("summarizes kind, outcome, duration and failures", () => {
    expect(summarizeRun(record("a"))).toBe("run · succeeded · 1.5s");
    expect(summarizeRun(record("a", { outcome: "failed", failedNodes: 2 }))).toBe(
      "run · failed · 1.5s, 2 failed",
    );
  });
});

describe("parseRunHistory", () => {
  it("keeps only well-formed records", () => {
    const raw = JSON.stringify([record("a"), { id: "bad" }, record("b")]);
    expect(parseRunHistory(raw).map((r) => r.id)).toEqual(["a", "b"]);
  });

  it("returns [] for non-array or invalid JSON", () => {
    expect(parseRunHistory("{}")).toEqual([]);
    expect(parseRunHistory("nope")).toEqual([]);
  });
});
