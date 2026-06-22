//! Requirers (the bindable starting points for north-star test #1) — verified headless in jsdom: only
//! the entities that REQUIRE a capability (carry a `Socket`/accepts component) surface as quick-pick
//! rows, and clicking one SELECTS it so the Reveal panel populates. Asserts real behaviour — the
//! filtered set, the named requirer, and the store selection read back after the click — not "it rendered".

import { afterEach, expect, test } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { Requirers } from "./Requirers";
import { projectionStore } from "../store/projection";

afterEach(() => projectionStore.getState().reset());

test("only Socket-bearing requirers list, and clicking one selects it (→ Reveal populates)", () => {
  projectionStore.getState().bulkLoad([
    // a requirer: carries the real `/core` requirer MARKER component `HealthBar` (it requires Health — a
    // cap, an ECS pair, not a projected field)
    { id: "hb1", name: "HealthBar", parentId: null, components: { HealthBar: { width: 1 } } },
    // a plain entity: no HealthBar → NOT a requirer, must be filtered out
    { id: "p1", name: "Lamp", parentId: null, components: { Transform: { x: 0, y: 0, z: 0 } } },
  ]);

  render(<Requirers />);

  // exactly one requirer row renders, and it names the requirer
  const rows = screen.getAllByTestId("requirer");
  expect(rows).toHaveLength(1);
  expect(rows[0].dataset.id).toBe("hb1");
  expect(rows[0].textContent).toContain("HealthBar");

  // clicking the row selects it in the projection store (so the Reveal panel keys off selectedId)
  expect(projectionStore.getState().selectedId).toBeNull();
  fireEvent.click(rows[0]);
  expect(projectionStore.getState().selectedId).toBe("hb1");
});
