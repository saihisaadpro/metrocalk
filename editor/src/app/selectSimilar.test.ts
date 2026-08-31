//! "Select similar" answers a question a rectangle cannot ask, so its rule is asserted on content.

import { afterEach, describe, expect, it } from "vitest";
import { projectionStore } from "../store/projection";
import type { EntityProjection } from "../transport/protocol";
import { similarTo } from "./selectSimilar";

afterEach(() => projectionStore.getState().reset());

/** The projection AND the store, because the reason names the object through `entityLabel` — which
 *  reads `summaries`. A fixture that only built the map would produce reasons naming raw loro keys,
 *  which is the copy defect `selectionText.ts` exists to prevent. */
function scene(entities: EntityProjection[]): {
  displayed: Record<string, EntityProjection>;
  order: string[];
} {
  projectionStore.getState().bulkLoad(entities);
  const displayed: Record<string, EntityProjection> = {};
  for (const e of entities) displayed[e.id] = e;
  return { displayed, order: entities.map((e) => e.id) };
}

const bolt = (id: string): EntityProjection => ({
  id,
  name: `Bolt ${id}`,
  parentId: null,
  components: { Transform: { x: 0 }, MeshRenderer: { mesh: "mtkasset:bolt", material: "steel" } },
});

describe("the same geometry is the strongest match", () => {
  it("selects every entity drawing the same mesh, wherever it is in the scene", () => {
    const { displayed, order } = scene([
      bolt("b1"),
      { id: "frame", name: "Frame", parentId: null, components: { Transform: {}, MeshRenderer: { mesh: "mtkasset:frame" } } },
      bolt("b2"),
      bolt("b3"),
    ]);

    const match = similarTo(displayed, order, "b2");

    // The scattered set a box on screen cannot reach — and the frame between them is left alone.
    expect(match?.ids).toEqual(["b1", "b2", "b3"]);
    expect(match?.reason).toContain("geometry");
    expect(match?.reason).toContain("Bolt b2");
  });

  it("a different MATERIAL on the same mesh is still the same part", () => {
    const painted = bolt("b9");
    painted.components.MeshRenderer.material = "painted";
    const { displayed, order } = scene([bolt("b1"), painted]);

    expect(similarTo(displayed, order, "b1")?.ids).toEqual(["b1", "b9"]);
  });
});

describe("with no geometry, the match is what the thing IS", () => {
  it("groups entities carrying the same components — which is what 'the same kind' means in an ECS", () => {
    const light = (id: string): EntityProjection => ({
      id,
      name: id,
      parentId: null,
      components: { Transform: {}, Light: { kind: "point" } },
    });
    const { displayed, order } = scene([
      light("l1"),
      light("l2"),
      { id: "cam", name: "Camera", parentId: null, components: { Transform: {}, Camera: {} } },
    ]);

    const match = similarTo(displayed, order, "l1");

    expect(match?.ids).toEqual(["l1", "l2"]);
    // The reason NAMES the components, so a surprising set is explained rather than merely reported.
    expect(match?.reason).toContain("Light");
    expect(match?.reason).toContain("Transform");
  });

  it("component ORDER does not change the kind — the signature is sorted", () => {
    const { displayed, order } = scene([
      { id: "a", name: "A", parentId: null, components: { Transform: {}, Light: {} } },
      { id: "b", name: "B", parentId: null, components: { Light: {}, Transform: {} } },
    ]);

    expect(similarTo(displayed, order, "a")?.ids).toEqual(["a", "b"]);
  });

  it("a long component list is summarised rather than dumped into the status line", () => {
    const many = { Transform: {}, Light: {}, Rules: {}, Health: {}, Audio: {} };
    const { displayed, order } = scene([{ id: "a", name: "A", parentId: null, components: many }]);

    expect(similarTo(displayed, order, "a")?.reason).toContain("and 2 more");
  });
});

describe("every no is explained", () => {
  it("an entity with nothing to match on answers NULL rather than selecting the scene", () => {
    const { displayed, order } = scene([
      { id: "empty", name: "Empty", parentId: null, components: {} },
      { id: "other", name: "Other", parentId: null, components: {} },
    ]);

    // The dangerous failure this pins: an empty signature matching every other empty signature would
    // have "Select similar" on a bare entity quietly select everything bare in the scene.
    expect(similarTo(displayed, order, "empty")).toBeNull();
  });

  it("an id that is not in the projection answers NULL", () => {
    const { displayed, order } = scene([bolt("b1")]);
    expect(similarTo(displayed, order, "ghost")).toBeNull();
  });

  it("an empty-string mesh handle is not a match key", () => {
    const { displayed, order } = scene([
      { id: "a", name: "A", parentId: null, components: { MeshRenderer: { mesh: "" } } },
      { id: "b", name: "B", parentId: null, components: { MeshRenderer: { mesh: "" } } },
    ]);

    // Falls through to the signature rule (both carry `MeshRenderer`), and must NOT report that they
    // share geometry — they share the absence of it.
    expect(similarTo(displayed, order, "a")?.reason).not.toContain("geometry");
  });
});
