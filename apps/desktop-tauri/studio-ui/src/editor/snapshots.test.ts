import { describe, expect, it } from "vitest";

import type { WorkflowGraph } from "../graph/model";
import {
  addSnapshot,
  parseSnapshots,
  removeSnapshot,
  renameSnapshot,
  SNAPSHOT_CAP,
  type Snapshot,
} from "./snapshots";

const graph: WorkflowGraph = { version: 1, nodes: [], edges: [] };
const snap = (id: string, name = id): Snapshot => ({ id, name, t: 0, graph });

describe("addSnapshot", () => {
  it("prepends newest-first without mutating the input", () => {
    const list = [snap("a")];
    const next = addSnapshot(list, snap("b"));
    expect(next.map((s) => s.id)).toEqual(["b", "a"]);
    expect(list).toHaveLength(1);
  });

  it("trims to the cap, dropping the oldest", () => {
    let list: Snapshot[] = [];
    for (let i = 0; i < SNAPSHOT_CAP + 3; i++) list = addSnapshot(list, snap(`s${i}`));
    expect(list).toHaveLength(SNAPSHOT_CAP);
    expect(list[0].id).toBe(`s${SNAPSHOT_CAP + 2}`);
    expect(list.some((s) => s.id === "s0")).toBe(false);
  });
});

describe("parseSnapshots", () => {
  it("parses a serialized array, keeping only well-formed entries", () => {
    const raw = JSON.stringify([snap("a"), { id: "bad" }, snap("b")]);
    expect(parseSnapshots(raw).map((s) => s.id)).toEqual(["a", "b"]);
  });

  it("returns [] for non-array or invalid JSON", () => {
    expect(parseSnapshots("{}")).toEqual([]);
    expect(parseSnapshots("not json")).toEqual([]);
  });
});

describe("removeSnapshot", () => {
  it("drops the matching id", () => {
    const list = [snap("a"), snap("b")];
    expect(removeSnapshot(list, "a").map((s) => s.id)).toEqual(["b"]);
  });
});

describe("renameSnapshot", () => {
  it("renames and trims", () => {
    const list = [snap("a", "old")];
    expect(renameSnapshot(list, "a", "  new  ")[0].name).toBe("new");
  });

  it("ignores a blank name", () => {
    const list = [snap("a", "old")];
    expect(renameSnapshot(list, "a", "   ")[0].name).toBe("old");
  });
});
