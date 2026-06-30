import { describe, expect, it } from "vitest";
import { PreviewLane } from "./previewLane";

// A deferred promise so tests can control when each preview job settles.
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

describe("PreviewLane (single-slot, latest-wins)", () => {
  it("applies a lone job's result", async () => {
    const lane = new PreviewLane();
    const out = await lane.run(async () => 42);
    expect(out).toEqual({ status: "applied", value: 42 });
    expect(lane.busy).toBe(false);
  });

  it("supersedes the older job and applies only the newest", async () => {
    const lane = new PreviewLane();
    const first = deferred<number>();
    const second = deferred<number>();

    const p1 = lane.run(() => first.promise);
    const p2 = lane.run(() => second.promise);

    // Resolve out of order: the stale (first) job settles last.
    second.resolve(2);
    first.resolve(1);

    expect(await p2).toEqual({ status: "applied", value: 2 });
    expect(await p1).toEqual({ status: "superseded" });
  });

  it("flips the previous job's signal to cancelled when superseded", async () => {
    const lane = new PreviewLane();
    let firstSignalCancelled = false;
    const first = deferred<number>();

    const p1 = lane.run(async (signal) => {
      const v = await first.promise;
      firstSignalCancelled = signal.cancelled;
      return v;
    });
    // Starting a second job must cancel the first's signal synchronously.
    const p2 = lane.run(async () => 9);

    first.resolve(1);
    await p1;
    await p2;
    expect(firstSignalCancelled).toBe(true);
  });

  it("cancel() aborts the in-flight job and clears busy", async () => {
    const lane = new PreviewLane();
    const job = deferred<number>();
    const p = lane.run(() => job.promise);
    expect(lane.busy).toBe(true);

    lane.cancel();
    expect(lane.busy).toBe(false);

    job.resolve(7);
    expect(await p).toEqual({ status: "superseded" });
  });

  it("is busy only while a job is in flight", async () => {
    const lane = new PreviewLane();
    const job = deferred<number>();
    const p = lane.run(() => job.promise);
    expect(lane.busy).toBe(true);
    job.resolve(1);
    await p;
    expect(lane.busy).toBe(false);
  });
});
