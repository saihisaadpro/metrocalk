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
});
