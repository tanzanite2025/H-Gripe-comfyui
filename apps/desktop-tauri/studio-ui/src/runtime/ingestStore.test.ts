import { afterEach, describe, expect, it, vi } from "vitest";
import {
  __dispatchIngestForTests as push,
  __resetIngestStoreForTests as reset,
  subscribeIngest,
  type IngestState,
} from "./ingestStore";

afterEach(() => reset());

describe("ingestStore", () => {
  it("delivers dims then thumb to a subscriber", () => {
    const seen: IngestState[] = [];
    subscribeIngest("/a.png", (s) => seen.push({ ...s }));

    push({ path: "/a.png", phase: "dims", width: 3840, height: 2160 });
    push({ path: "/a.png", phase: "thumb", data_url: "data:x", width: 256, height: 144 });

    expect(seen).toEqual([
      { dims: { w: 3840, h: 2160 } },
      { dims: { w: 3840, h: 2160 }, thumb: "data:x" },
    ]);
  });

  it("replays the latest cached state on late subscribe", () => {
    push({ path: "/b.png", phase: "dims", width: 100, height: 50 });
    push({ path: "/b.png", phase: "thumb", data_url: "data:y", width: 20, height: 10 });

    const fn = vi.fn();
    subscribeIngest("/b.png", fn);
    expect(fn).toHaveBeenCalledTimes(1);
    expect(fn).toHaveBeenCalledWith({ dims: { w: 100, h: 50 }, thumb: "data:y" });
  });

  it("keeps header dims even when the thumb event carries its own", () => {
    push({ path: "/c.png", phase: "dims", width: 8000, height: 6000 });
    const fn = vi.fn();
    subscribeIngest("/c.png", fn);
    push({ path: "/c.png", phase: "thumb", data_url: "data:z", width: 256, height: 192 });

    expect(fn).toHaveBeenLastCalledWith({ dims: { w: 8000, h: 6000 }, thumb: "data:z" });
  });

  it("marks a path failed on an error event", () => {
    const fn = vi.fn();
    subscribeIngest("/d.png", fn);
    push({ path: "/d.png", phase: "error", error: "boom" });
    expect(fn).toHaveBeenLastCalledWith({ failed: true });
  });

  it("routes events only to matching-path subscribers and stops after unsubscribe", () => {
    const a = vi.fn();
    const b = vi.fn();
    subscribeIngest("/a.png", a);
    const off = subscribeIngest("/b.png", b);

    push({ path: "/a.png", phase: "dims", width: 1, height: 1 });
    expect(a).toHaveBeenCalledTimes(1);
    expect(b).not.toHaveBeenCalled();

    off();
    push({ path: "/b.png", phase: "dims", width: 2, height: 2 });
    expect(b).not.toHaveBeenCalled();
  });
});
