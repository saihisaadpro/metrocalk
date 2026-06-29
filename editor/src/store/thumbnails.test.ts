//! Live-thumbnail policy store (M14.2 / ADR-058) — verified headless: the prompt's hard guardrails are all
//! testable without a GPU. Asserts the STRUCTURED behaviour (which entity invalidated · how many fired ·
//! ready vs fallback), never a styled string.

import { afterEach, beforeEach, expect, test } from "vitest";
import { thumbnailStore, DEFAULT_BUDGET, MINSPEC_BUDGET } from "./thumbnails";
import type { EditorClient } from "../transport/session";
import type { ProjectionDelta } from "../transport/protocol";

const flush = () => new Promise((r) => setTimeout(r, 0));

beforeEach(() => {
  thumbnailStore.getState().reset();
  thumbnailStore.getState().setClient(null);
  thumbnailStore.getState().setMinSpec(false);
});
afterEach(() => thumbnailStore.getState().setClient(null));

/** A stub renderer that records every (id,size) it's asked to render and answers per `answer`. */
function stubClient(answer: (id: string) => string | null = (id) => `data:thumb/${id}`) {
  const calls: { id: string; size: number }[] = [];
  const client = {
    thumbnail: (id: string, size: number) => {
      calls.push({ id, size });
      return Promise.resolve(answer(id));
    },
  } as unknown as EditorClient;
  return { client, calls };
}

test("dirty-tracking is SILHOUETTE-only: mesh/material/transform/visibility invalidate; other fields do not", () => {
  const s = thumbnailStore.getState();
  s.setClient(null); // no client ⇒ drain no-ops ⇒ the dirty set persists for inspection

  const ingest = (delta: ProjectionDelta) => s.ingestDelta(delta);

  // A Health field edit is NOT silhouette-affecting → never dirties a thumbnail (the key guarantee).
  ingest({ ops: [{ op: "setField", id: "a", component: "Health", field: "hp", value: 5 }] });
  expect(thumbnailStore.getState().dirty["a"]).toBeUndefined();

  // Mesh, material, and transform edits ARE silhouette-affecting.
  ingest({ ops: [{ op: "setField", id: "a", component: "MeshRenderer", field: "material", value: "rusty" }] });
  expect(thumbnailStore.getState().dirty["a"]).toBe(true);
  ingest({ ops: [{ op: "setField", id: "b", component: "Transform", field: "x", value: 9 }] });
  expect(thumbnailStore.getState().dirty["b"]).toBe(true);

  // A non-silhouette op (an edge, a removeField) does not dirty.
  ingest({ ops: [{ op: "addEdge", from: "c", rel: "tracks", to: "d" }] });
  expect(thumbnailStore.getState().dirty["c"]).toBeUndefined();

  // An upsert (new entity / visibility flip) dirties; a remove drops the cached entry.
  ingest({ ops: [{ op: "upsert", id: "e", active: false }] });
  expect(thumbnailStore.getState().dirty["e"]).toBe(true);
});

test("visible-only + budget cap: a 5000-entity scene fires ≤ budget per window, only for VISIBLE rows", () => {
  const { client, calls } = stubClient();
  const s = thumbnailStore.getState();

  // Dirty an off-screen entity BEFORE wiring the client (so its drain can't perturb the injected budget
  // clock) — it must NEVER fire (visible-only).
  s.ingestDelta({ ops: [{ op: "setField", id: "offscreen", component: "MeshRenderer", field: "mesh", value: "x" }] });
  s.setClient(client);
  const visible = Array.from({ length: 5000 }, (_, i) => `e${i}`);

  // One window: at most DEFAULT_BUDGET.maxPerInterval fire — never 5000.
  s.setVisible(visible, 1000);
  expect(calls.length).toBe(DEFAULT_BUDGET.maxPerInterval);
  expect(calls.every((c) => c.id.startsWith("e"))).toBe(true); // never the off-screen entity
  expect(calls.some((c) => c.id === "offscreen")).toBe(false);

  // Same window again → budget exhausted, nothing more fires.
  const n = calls.length;
  s.drain(1100);
  expect(calls.length).toBe(n);

  // A new window (≥ intervalMs later) → the next batch fires (the backlog drains over time, not all at once).
  s.drain(1000 + DEFAULT_BUDGET.intervalMs + 1);
  expect(calls.length).toBe(2 * DEFAULT_BUDGET.maxPerInterval);
});

test("a ready render → status 'ready' (the real url); a null → 'fallback' (the styled icon)", async () => {
  const { client } = stubClient((id) => (id === "good" ? "data:image/png;base64,AAAA" : null));
  const s = thumbnailStore.getState();
  s.setClient(client);
  s.setVisible(["good", "bad"], 1000);
  await flush();

  const e = thumbnailStore.getState().entries;
  expect(e["good"].status).toBe("ready");
  expect(e["good"].url).toContain("data:image/png");
  expect(e["bad"].status).toBe("fallback");
  expect(e["bad"].url).toBeNull();
});

test("min-spec scales the request resolution down (a measured budget gate)", () => {
  const { client, calls } = stubClient();
  const s = thumbnailStore.getState();
  s.setMinSpec(true);
  s.setClient(client);
  s.setVisible(["a"], 1000);
  expect(calls[0].size).toBe(MINSPEC_BUDGET.size);
  expect(MINSPEC_BUDGET.size).toBeLessThan(DEFAULT_BUDGET.size);
});

test("editing ONE entity refreshes only THAT entity's thumbnail (selective), not the others", async () => {
  const { client, calls } = stubClient();
  const s = thumbnailStore.getState();
  s.setClient(client);
  s.setVisible(["a", "b", "c"], 1000);
  await flush(); // a,b,c rendered once each
  const base = calls.length;
  expect(base).toBe(3);

  // A silhouette edit to ONLY `b` → only `b` re-renders (a and c stay cached).
  s.ingestDelta({ ops: [{ op: "setField", id: "b", component: "MeshRenderer", field: "mesh", value: "y" }] });
  await flush();
  const fresh = calls.slice(base);
  expect(fresh.map((c) => c.id)).toEqual(["b"]);
});

test("a FULL re-projection drops the caches (the scene was replaced)", async () => {
  const { client } = stubClient();
  const s = thumbnailStore.getState();
  s.setClient(client);
  s.setVisible(["a"], 1000);
  await flush();
  expect(thumbnailStore.getState().entries["a"]).toBeDefined();

  s.ingestDelta({ full: true, ops: [{ op: "upsert", id: "a" }] });
  expect(thumbnailStore.getState().entries["a"]).toBeUndefined(); // cache dropped; will re-render when visible
});
