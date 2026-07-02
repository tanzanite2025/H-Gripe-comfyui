import { describe, expect, it } from "vitest";

import {
  appendLog,
  describeNodeStatus,
  formatErrorDetail,
  formatLogText,
  formatTime,
  levelForStatus,
  RUN_LOG_CAP,
  type RunLogEntry,
} from "./runlog";

const entry = (id: number): RunLogEntry => ({ id, t: 0, level: "info", message: `e${id}` });

describe("appendLog", () => {
  it("appends without mutating the input", () => {
    const log: RunLogEntry[] = [entry(1)];
    const next = appendLog(log, entry(2));
    expect(next.map((e) => e.id)).toEqual([1, 2]);
    expect(log).toHaveLength(1);
  });

  it("trims oldest entries past the cap", () => {
    let log: RunLogEntry[] = [];
    for (let i = 0; i < RUN_LOG_CAP + 5; i++) log = appendLog(log, entry(i));
    expect(log).toHaveLength(RUN_LOG_CAP);
    expect(log[0].id).toBe(5);
    expect(log[log.length - 1].id).toBe(RUN_LOG_CAP + 4);
  });

  it("respects a custom cap", () => {
    let log: RunLogEntry[] = [];
    for (let i = 0; i < 4; i++) log = appendLog(log, entry(i), 2);
    expect(log.map((e) => e.id)).toEqual([2, 3]);
  });
});

describe("levelForStatus", () => {
  it("maps statuses to levels", () => {
    expect(levelForStatus("succeeded")).toBe("success");
    expect(levelForStatus("cached")).toBe("success");
    expect(levelForStatus("failed")).toBe("error");
    expect(levelForStatus("skipped")).toBe("warn");
    expect(levelForStatus("cancelled")).toBe("warn");
    expect(levelForStatus("running")).toBe("info");
  });
});

describe("describeNodeStatus", () => {
  it("includes duration for succeeded nodes", () => {
    expect(describeNodeStatus("succeeded", { durationMs: 12.4 })).toBe("done in 12 ms");
    expect(describeNodeStatus("succeeded")).toBe("done");
  });

  it("includes the error for failed nodes", () => {
    expect(describeNodeStatus("failed", { error: "boom" })).toBe("failed: boom");
    expect(describeNodeStatus("failed")).toBe("failed: unknown error");
  });

  it("appends structured error context for failed nodes", () => {
    expect(
      describeNodeStatus("failed", {
        error: "server exploded",
        detail: {
          message: "server exploded",
          code: "http_500",
          retryable: true,
          provider: "openai_compatible",
          operation: "image.generate",
          provider_request_id: "req-42",
        },
      }),
    ).toBe(
      "failed: server exploded [provider=openai_compatible op=image.generate code=http_500 request=req-42 retryable]",
    );
  });
});

describe("formatErrorDetail", () => {
  it("is empty for missing or message-only details", () => {
    expect(formatErrorDetail(null)).toBe("");
    expect(formatErrorDetail(undefined)).toBe("");
    expect(formatErrorDetail({ message: "boom" })).toBe("");
  });

  it("emits only the fields that are present", () => {
    expect(formatErrorDetail({ message: "m", code: "poll_timeout", task_id: "studio-n1-1" })).toBe(
      "code=poll_timeout task=studio-n1-1",
    );
  });
});

describe("formatTime", () => {
  it("zero-pads to HH:MM:SS", () => {
    const d = new Date(2020, 0, 1, 3, 5, 9).getTime();
    expect(formatTime(d)).toBe("03:05:09");
  });
});

describe("formatLogText", () => {
  it("renders one line per entry with level and optional node", () => {
    const t = new Date(2020, 0, 1, 1, 2, 3).getTime();
    const text = formatLogText([
      { id: 0, t, level: "info", message: "run started" },
      { id: 1, t, level: "error", node: "n1", message: "failed: boom" },
    ]);
    expect(text).toBe("01:02:03 [info] run started\n01:02:03 [error] n1 failed: boom");
  });
});
