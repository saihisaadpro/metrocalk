//! The relational summary (M14.2 / ADR-058, the C6 closure) — verified headless: it speaks the real `/core`
//! vocabulary and the projection threads it into the hierarchy SUMMARY (so a row re-renders only when its
//! relational status FLIPS, not on a field edit — M2.5 preserved).

import { afterEach, expect, test } from "vitest";
import { deriveKind, deriveRel } from "./relSummary";
import { projectionStore } from "./projection";
import type { RelSummary } from "../transport/protocol";

afterEach(() => projectionStore.getState().reset());

const rel = (over: Partial<RelSummary>): RelSummary => ({ requires: [], provides: [], bound: 0, needsBinding: false, isGroup: false, ...over });

test("deriveKind/deriveRel key off the real component vocabulary (not MockCore Socket/Provides)", () => {
  expect(deriveKind({ HealthBar: { width: 1 } })).toBe("requirer");
  expect(deriveKind({ MeshRenderer: { mesh: "x" } })).toBe("mesh");
  expect(deriveKind({ Light: {} })).toBe("light");
  expect(deriveKind({ Camera: {} })).toBe("camera");

  const r = deriveRel({ HealthBar: { width: 1 } });
  expect(r.requires).toEqual(["Health"]);
  expect(r.needsBinding).toBe(true); // a requirer with no binding NEEDS one (the authoritative requirer signal)
  expect(deriveRel({ HealthBar: { width: 1 } }, 1).needsBinding).toBe(false); // a binding satisfies it
  expect(deriveRel({ Health: { hp: 100 } }).provides).toEqual(["Health"]); // a provider
});

test("a bind flips needsBinding in the projected summary; the summary ref changes ONLY on the flip (M2.5)", () => {
  const s = projectionStore.getState();
  s.applyDelta({ ops: [{ op: "upsert", id: "hb", name: "Health Bar", kind: "requirer", rel: rel({ requires: ["Health"], needsBinding: true }) }] });
  const sm1 = projectionStore.getState().summaries["hb"];
  expect(sm1.rel?.needsBinding).toBe(true);

  // a non-silhouette field edit → summary ref UNCHANGED (a field edit never re-renders the tree)
  s.applyDelta({ ops: [{ op: "setField", id: "hb", component: "Transform", field: "x", value: 1 }] });
  expect(projectionStore.getState().summaries["hb"]).toBe(sm1);

  // a re-projection carrying the SAME relational status → still the same ref (no churn)
  s.applyDelta({ ops: [{ op: "upsert", id: "hb", kind: "requirer", rel: rel({ requires: ["Health"], needsBinding: true }) }] });
  expect(projectionStore.getState().summaries["hb"]).toBe(sm1);

  // the bind: needsBinding → false, bound → 1 → a NEW summary ref (exactly that one row re-renders)
  s.applyDelta({ ops: [{ op: "upsert", id: "hb", kind: "requirer", rel: rel({ requires: ["Health"], bound: 1, needsBinding: false }) }] });
  const sm2 = projectionStore.getState().summaries["hb"];
  expect(sm2).not.toBe(sm1);
  expect(sm2.rel?.needsBinding).toBe(false);
  expect(sm2.rel?.bound).toBe(1);
});
