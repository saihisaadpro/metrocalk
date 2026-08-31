//! The scene query — parsed, matched, and enumerated from the scene itself.
//!
//! Asserts the semantic signal (which ids answer, which tokens a facet carries), never the copy.

import { expect, test } from "vitest";
import {
  facetsOf,
  matchesQuery,
  parseQuery,
  queryHasFacet,
  queryIsEmpty,
  queryNeedsComponents,
  toggleFacet,
} from "./sceneQuery";
import type { EntityProjection, EntitySummary } from "../transport/protocol";

function summary(id: string, name: string, kind: string, needsBinding = false): EntitySummary {
  return {
    id,
    name,
    parentId: null,
    kind,
    rel: { requires: [], provides: [], bound: 0, needsBinding, isGroup: kind === "group" },
  };
}

function entity(id: string, components: Record<string, Record<string, never>>): EntityProjection {
  return { id, name: id, parentId: null, components };
}

test("a bare word is still a name/id substring — the box behaves as it always has", () => {
  const q = parseQuery("lamp");
  expect(q).toEqual({ text: "lamp", kinds: [], components: [], needsBinding: false });
  // Either side answers: the id (what an import names a part) or the name (what a person renamed it to).
  expect(matchesQuery(q, "lamp-01", summary("lamp-01", "Fixture 3", "light"), undefined)).toBe(true);
  expect(matchesQuery(q, "e17", summary("e17", "Work Lamp", "light"), undefined)).toBe(true);
  expect(matchesQuery(q, "valve-01", summary("valve-01", "Intake Valve", "mesh"), undefined)).toBe(false);
});

test("an UNKNOWN key stays text — a part number is not a failed filter", () => {
  // The rule that keeps `bolt:M8` searchable. A query language that swallowed every `word:word` would
  // make part numbers — the single most likely thing typed into this box on a CAD import — unfindable,
  // and would report it as "no matching objects".
  const q = parseQuery("bolt:M8");
  expect(q.text).toBe("bolt:m8");
  expect(q.kinds).toEqual([]);
  expect(matchesQuery(q, "x", summary("x", "Bolt:M8 flange", "mesh"), undefined)).toBe(true);
});

test("kind: and type: are the same question, and several are alternatives", () => {
  const q = parseQuery("kind:light type:camera");
  expect(q.kinds).toEqual(["light", "camera"]);
  expect(matchesQuery(q, "a", summary("a", "Key", "light"), undefined)).toBe(true);
  expect(matchesQuery(q, "b", summary("b", "Cam", "camera"), undefined)).toBe(true);
  expect(matchesQuery(q, "c", summary("c", "Bolt", "mesh"), undefined)).toBe(false);
});

test("a facet NARROWS the free text rather than replacing it", () => {
  const q = parseQuery("weld kind:mesh");
  expect(q.text).toBe("weld");
  expect(q.kinds).toEqual(["mesh"]);
  expect(matchesQuery(q, "a", summary("a", "Weld Gun", "mesh"), undefined)).toBe(true);
  expect(matchesQuery(q, "b", summary("b", "Weld Lamp", "light"), undefined)).toBe(false);
  expect(matchesQuery(q, "c", summary("c", "Bolt", "mesh"), undefined)).toBe(false);
});

test("has: reads the component map and every named component must be present", () => {
  const q = parseQuery("has:rigidbody has:meshrenderer");
  expect(queryNeedsComponents(q)).toBe(true);
  expect(queryNeedsComponents(parseQuery("kind:light"))).toBe(false);

  const both = entity("a", { RigidBody: {}, MeshRenderer: {} });
  const one = entity("b", { MeshRenderer: {} });
  expect(matchesQuery(q, "a", summary("a", "A", "physics"), both)).toBe(true);
  expect(matchesQuery(q, "b", summary("b", "B", "mesh"), one)).toBe(false);
  // Absent component map with a `has:` query is a NO, not an accidental yes.
  expect(matchesQuery(q, "c", summary("c", "C", "mesh"), undefined)).toBe(false);
});

test("needs:binding reads the live requirer truth, and needs:<anything else> stays text", () => {
  const q = parseQuery("needs:binding");
  expect(q.needsBinding).toBe(true);
  expect(matchesQuery(q, "a", summary("a", "Bar", "requirer", true), undefined)).toBe(true);
  expect(matchesQuery(q, "b", summary("b", "Bar", "requirer", false), undefined)).toBe(false);

  const odd = parseQuery("needs:paint");
  expect(odd.needsBinding).toBe(false);
  expect(odd.text).toBe("needs:paint");
});

test("an empty query is empty however much whitespace it is made of", () => {
  expect(queryIsEmpty(parseQuery(""))).toBe(true);
  expect(queryIsEmpty(parseQuery("   "))).toBe(true);
  expect(queryIsEmpty(parseQuery("kind:light"))).toBe(false);
  expect(queryIsEmpty(parseQuery("a"))).toBe(false);
});

test("facets are counted from the scene, largest first, and never offered when they match everything", () => {
  const order = ["m1", "m2", "m3", "l1", "c1", "r1"];
  const summaries: Record<string, EntitySummary> = {
    m1: summary("m1", "Bolt", "mesh"),
    m2: summary("m2", "Bolt", "mesh"),
    m3: summary("m3", "Bolt", "mesh"),
    l1: summary("l1", "Key", "light"),
    c1: summary("c1", "Cam", "camera"),
    r1: summary("r1", "Bar", "requirer", true),
  };
  const facets = facetsOf(order, summaries);
  // Largest first; ties read alphabetically by the word a person sees, so the row is stable between
  // renders rather than ordered by whatever the map happened to iterate.
  expect(facets.map((f) => f.token)).toEqual([
    "kind:mesh",
    "kind:camera",
    "kind:light",
    "needs:binding",
    "kind:requirer",
  ]);
  expect(facets[0]).toMatchObject({ label: "Meshes", count: 3 });

  // A scene that is ALL meshes offers no mesh chip: a filter that removes nothing is not a filter.
  const allMesh = facetsOf(["m1", "m2", "m3"], summaries);
  expect(allMesh).toEqual([]);
  // And a scene of one object offers nothing at all.
  expect(facetsOf(["m1"], summaries)).toEqual([]);
});

test("a kind the engine adds and this module has no word for still appears, under its own name", () => {
  // The KTX2 rule: a hand-written list is how a capability becomes unofferable. Only the WORDS are
  // written here; the SET comes from the scene.
  const facets = facetsOf(["a", "b"], {
    a: summary("a", "A", "terrain"),
    b: summary("b", "B", "mesh"),
  });
  expect(facets.map((f) => [f.token, f.label])).toEqual([
    ["kind:mesh", "Meshes"],
    ["kind:terrain", "Terrain"],
  ]);
});

test("a chip is a toggle: it adds its token, and removes it again, keeping everything else typed", () => {
  const lights = { token: "kind:light", label: "Lights", count: 2 };
  expect(toggleFacet("", lights)).toBe("kind:light");
  expect(toggleFacet("weld", lights)).toBe("weld kind:light");
  expect(toggleFacet("weld kind:light", lights)).toBe("weld");
  // The alias and the casing describe the ONE state, so the chip that reads pressed is the chip that
  // clears it — a toggle whose "on" and "off" disagreed would append a second token forever.
  expect(queryHasFacet(parseQuery("TYPE:Light"), lights)).toBe(true);
  expect(toggleFacet("weld TYPE:Light", lights)).toBe("weld");

  const needs = { token: "needs:binding", label: "Needs a binding", count: 1 };
  expect(toggleFacet("kind:light", needs)).toBe("kind:light needs:binding");
  expect(toggleFacet("kind:light needs:binding", needs)).toBe("kind:light");
  expect(queryHasFacet(parseQuery("kind:light"), needs)).toBe(false);
});
