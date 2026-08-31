//! What the editor calls the thing you selected — asserted once, so the four surfaces that say it
//! cannot each invent their own answer.

import { afterEach, expect, test } from "vitest";
import { projectionStore } from "./projection";
import { entityLabel, selectionSentence } from "./selectionText";

afterEach(() => projectionStore.getState().reset());

test("an object is called what it was named, and an unnamed one is called by its id", () => {
  projectionStore.getState().bulkLoad([
    { id: "1_4a3f", name: "Weld Gun", parentId: null, components: {} },
    // A projection carries an entity's own id as its name until somebody names it, so `name !== id`
    // is the test for "named" — not `name != null`, which is true for everything.
    { id: "1_4a40", name: "1_4a40", parentId: null, components: {} },
  ]);
  expect(entityLabel("1_4a3f")).toBe("Weld Gun");
  expect(entityLabel("1_4a40")).toBe("1_4a40");
  // An id the projection has never seen is still said back, rather than becoming "undefined".
  expect(entityLabel("gone")).toBe("gone");
});

test("one is named, many are counted, none says so", () => {
  projectionStore.getState().bulkLoad([{ id: "a", name: "Bolt", parentId: null, components: {} }]);
  expect(selectionSentence(1, ["a"])).toBe("Bolt");
  expect(selectionSentence(3, ["a", "b", "c"])).toBe("3 objects selected");
  // Going blank reads as the editor having lost track of the selection, which is a different and
  // more alarming thing than having none.
  expect(selectionSentence(0)).toBe("nothing selected");
});
