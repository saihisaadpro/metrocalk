import { beforeEach, describe, expect, it } from "vitest";
import { DeltaClient } from "../transport/client";
import { MockCore } from "../transport/mock-core";
import type { EntityProjection } from "../transport/protocol";
import { inProcessPair } from "../transport/transport";
import { projectionStore } from "./projection";

function world(): EntityProjection[] {
  return [
    { id: "p", name: "HealthPack", parentId: null, components: { Provides: { capability: "Health" } } },
    { id: "u", name: "Button", parentId: null, components: { Socket: { accepts: "Click" } } },
    { id: "d", name: "Door", parentId: null, components: { Socket: { accepts: "Health" } } },
  ];
}

beforeEach(() => projectionStore.getState().reset());

describe("optimistic echo + reconciliation", () => {
  it("field edit shows instantly, then reconciles to a no-op on the authoritative confirm", () => {
    const [uiT, coreT] = inProcessPair();
    const seed = world();
    projectionStore.getState().bulkLoad(seed);
    const core = new MockCore(coreT, world(), { defer: true }); // independent authoritative copy
    const client = new DeltaClient(uiT);

    client.setField("p", "Provides", "capability", "Shield");

    // optimistic: visible immediately, still pending, authoritative base unchanged
    let s = projectionStore.getState();
    expect(s.displayed["p"].components.Provides.capability).toBe("Shield");
    expect(Object.keys(s.pending)).toHaveLength(1);
    expect(s.base["p"].components.Provides.capability).toBe("Health");

    core.flush(); // authoritative echo arrives

    s = projectionStore.getState();
    expect(Object.keys(s.pending)).toHaveLength(0); // pending dropped on confirm
    expect(s.base["p"].components.Provides.capability).toBe("Shield"); // authoritative
    expect(s.displayed["p"].components.Provides.capability).toBe("Shield"); // no-op reconcile (no flicker)
    expect(s.rejections).toHaveLength(0);
  });

  it("a rejected bind removes the optimistic edge and surfaces the core's reason (every 'no' explained')", () => {
    const [uiT, coreT] = inProcessPair();
    const seed = world();
    projectionStore.getState().bulkLoad(seed);
    const core = new MockCore(coreT, world(), { defer: true }); // independent authoritative copy
    const client = new DeltaClient(uiT);

    const opId = client.bind("p", "BindsTo", "u"); // HealthPack → Button: incompatible

    // optimistic edge shown as pending
    let s = projectionStore.getState();
    expect(s.edges["p|BindsTo|u"]?.status).toBe("pending");

    core.flush(); // core rejects

    s = projectionStore.getState();
    expect(s.edges["p|BindsTo|u"]).toBeUndefined(); // optimistic edge removed
    expect(s.rejections).toHaveLength(1);
    expect(s.rejections[0].clientOpId).toBe(opId);
    expect(s.rejections[0].reason).toMatch(/Health/); // the explained reason
    expect(Object.keys(s.pending)).toHaveLength(0);
  });

  it("a compatible bind is confirmed and the edge becomes authoritative", () => {
    const [uiT, coreT] = inProcessPair();
    const seed = world();
    projectionStore.getState().bulkLoad(seed);
    const core = new MockCore(coreT, world(), { defer: true }); // independent authoritative copy
    const client = new DeltaClient(uiT);

    client.bind("p", "BindsTo", "d"); // HealthPack → Door: accepts Health
    core.flush();

    const s = projectionStore.getState();
    expect(s.edges["p|BindsTo|d"]?.status).toBe("confirmed");
    expect(s.rejections).toHaveLength(0);
  });

  // A `project_full` (server-initiated full re-projection — connect/undo/sim-restart/open) REPLACES the
  // scene; an incremental delta MERGES. This is the fix for the live "undo doesn't drop the bound edge"
  // desync: undo emits project_full (no removeEdge), so a merge would leave the stale edge.
  it("a FULL re-projection REPLACES the scene — stale entities + edges are dropped (the undo sync)", () => {
    const store = projectionStore.getState();
    store.applyDelta({
      ops: [
        { op: "upsert", id: "a", name: "A", parentId: null },
        { op: "upsert", id: "b", name: "B", parentId: null },
        { op: "addEdge", from: "a", rel: "tracks", to: "b" },
      ],
    });
    let s = projectionStore.getState();
    expect(s.order).toEqual(["a", "b"]);
    expect(s.edges["a|tracks|b"]).toBeTruthy();

    // a full re-projection carrying ONLY "a" (b removed + the bind undone) drops the rest
    store.applyDelta({ ops: [{ op: "upsert", id: "a", name: "A", parentId: null }], full: true });
    s = projectionStore.getState();
    expect(s.order).toEqual(["a"]); // b dropped
    expect(s.base["b"]).toBeUndefined();
    expect(s.displayed["b"]).toBeUndefined();
    expect(s.edges["a|tracks|b"]).toBeUndefined(); // the stale edge is gone — the undo-tracking fix
  });

  it("an INCREMENTAL delta MERGES — it never drops existing entities", () => {
    const store = projectionStore.getState();
    store.applyDelta({ ops: [{ op: "upsert", id: "a", name: "A", parentId: null }] });
    store.applyDelta({ ops: [{ op: "upsert", id: "c", name: "C", parentId: null }] }); // no `full`
    expect(projectionStore.getState().order).toEqual(["a", "c"]);
  });
});
