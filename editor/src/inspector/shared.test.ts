//! The intersection rule, stated as tests before the panel that renders it existed.
//!
//! Every one of these is a claim about what the panel is allowed to OFFER: `engine.commit` is
//! all-or-nothing, so a field offered on an object that does not carry it turns one edit into a
//! refusal for the whole batch.

import { describe, expect, test } from "vitest";
import { describeKind, selectionMakeup, sharedShape, type Components } from "./shared";

const light = (intensity: number, kind = "point"): Components => ({
  Transform: { x: 0, y: 2, z: 0 },
  Light: { kind, intensity },
});

describe("sharedShape", () => {
  test("an empty selection has nothing to offer", () => {
    const s = sharedShape([]);
    expect(s.components).toEqual({});
    expect(s.partialFields).toBe(0);
  });

  test("a selection of one is its own intersection — nothing mixed, nothing left out", () => {
    const s = sharedShape([light(60)]);
    expect(s.components).toEqual({ Transform: { x: 0, y: 2, z: 0 }, Light: { kind: "point", intensity: 60 } });
    expect(s.mixed.size).toBe(0);
    expect(s.partialFields).toBe(0);
  });

  test("fields every entity carries are offered; agreeing values come through, disagreeing ones are mixed", () => {
    const s = sharedShape([light(60), light(60), light(12)]);
    expect(Object.keys(s.components)).toEqual(["Transform", "Light"]);
    // `kind` agrees on all three; `intensity` does not.
    expect(s.components.Light.kind).toBe("point");
    expect(s.mixed.has("Light.intensity")).toBe(true);
    expect(s.mixed.has("Light.kind")).toBe(false);
    // A MIXED FIELD IS `undefined`, NOT THE PRIMARY'S VALUE — the panel diffs its own form data to
    // decide what to emit, so seeding 60 here would make "set them all to 60" emit nothing.
    expect("intensity" in s.components.Light).toBe(true);
    expect(s.components.Light.intensity).toBeUndefined();
  });

  test("a component only some of the selection carries is withheld, counted and NAMED", () => {
    const s = sharedShape([light(60), { Transform: { x: 1, y: 0, z: 0 } }]);
    expect(Object.keys(s.components)).toEqual(["Transform"]);
    expect(s.partialComponents).toEqual(["Light"]);
    expect(s.partialFields).toBe(2); // Light.kind + Light.intensity
  });

  test("a field only some of the selection carries is withheld and counted", () => {
    const s = sharedShape([
      { Transform: { x: 0, y: 0, z: 0 } },
      { Transform: { x: 1, y: 0 } },
    ]);
    expect(Object.keys(s.components.Transform)).toEqual(["x", "y"]);
    expect(s.partialFields).toBe(1); // Transform.z
  });

  test("fields the OTHERS carry and the primary does not are counted too", () => {
    // A panel that reports "0 left out" while hiding a row is the same untruth in the other direction.
    const s = sharedShape([{ Transform: { x: 0 } }, { Transform: { x: 0 }, RigidBody: { mass: 5 } }]);
    expect(s.components).toEqual({ Transform: { x: 0 } });
    expect(s.partialFields).toBe(1);
    expect(s.partialComponents).toEqual(["RigidBody"]);
  });

  test("the primary fixes the order — the panel never re-orders because the store did", () => {
    const a: Components = { Zeta: { q: 1 }, Alpha: { p: 1 } };
    const b: Components = { Alpha: { p: 1 }, Zeta: { q: 1 } };
    expect(Object.keys(sharedShape([a, b]).components)).toEqual(["Zeta", "Alpha"]);
    expect(Object.keys(sharedShape([b, a]).components)).toEqual(["Alpha", "Zeta"]);
  });

  test("values compare by content, not by reference", () => {
    const s = sharedShape([
      { Tag: { list: [1, 2] as unknown as never } },
      { Tag: { list: [1, 2] as unknown as never } },
    ]);
    expect(s.mixed.size).toBe(0);
  });

  test("null is a value, and it disagrees with a number", () => {
    const s = sharedShape([{ C: { f: null } }, { C: { f: 1 } }]);
    expect(s.mixed.has("C.f")).toBe(true);
  });
});

describe("selectionMakeup", () => {
  test("counts each kind, most-common first, ties broken by name", () => {
    expect(selectionMakeup(["mesh", "light", "mesh", "camera"])).toEqual([
      { kind: "mesh", count: 2 },
      { kind: "camera", count: 1 },
      { kind: "light", count: 1 },
    ]);
  });

  test("an unknown kind is counted as the neutral one rather than dropped", () => {
    expect(selectionMakeup([undefined, undefined])).toEqual([{ kind: "default", count: 2 }]);
  });
});

describe("describeKind", () => {
  test("pluralises, and names the neutral kind 'object'", () => {
    expect(describeKind("light", 1)).toBe("1 light");
    expect(describeKind("mesh", 3)).toBe("3 meshes");
    expect(describeKind("default", 2)).toBe("2 objects");
  });
});
