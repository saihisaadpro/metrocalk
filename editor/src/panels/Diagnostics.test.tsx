//! Diagnostics (M14.3 / ADR-059, the C6 closure) — verified headless: a requirer surfaces a STRUCTURED,
//! actionable diagnostic keyed off the real `rel` projection + the reveal (not a wall of text), with a
//! one-click bind FIX (a real transaction); the "why others can't bind" reasons are grouped + collapsible;
//! a fully-wired entity shows an honest all-clear. Asserts the structured `data-severity`/`data-kind` model
//! + the fix call, never the rendered prose.

import { afterEach, expect, test, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { Diagnostics } from "./Diagnostics";
import { projectionStore } from "../store/projection";
import { fakeClient } from "../transport/test-client";

afterEach(() => projectionStore.getState().reset());

test("a requirer surfaces a structured actionable diagnostic + a one-click bind fix (keyed off rel)", async () => {
  projectionStore.getState().bulkLoad([
    { id: "hb", name: "Health Bar", parentId: null, components: { HealthBar: { width: 1 } } },
  ]);
  projectionStore.getState().select("hb");
  const bind = vi.fn(() => "op");
  const revealTargets = vi.fn(() =>
    Promise.resolve({
      required: ["Health"],
      compatible: [{ id: "player", name: "Player", distance: 0, affinity: 90 }],
      greyed: [{ id: "rock", name: "Rock", reason: "doesn't provide Health" }],
      bound: [],
    }),
  );
  render(<Diagnostics client={fakeClient({ revealTargets, bind })} />);

  // the actionable diagnostic is a STRUCTURED row (severity=error), not just a line of text
  const row = await screen.findByTestId("diag-row");
  expect(row.getAttribute("data-severity")).toBe("error");
  expect(row.getAttribute("data-kind")).toBe("needs-binding");

  // the one-click fix binds the best-ranked compatible source (a real transaction)
  const fix = await screen.findByTestId("diag-fix");
  fireEvent.click(fix);
  expect(bind).toHaveBeenCalledWith("hb", "tracks", "player");
});

test("the 'why others can't bind' reasons are grouped + collapsible (the spam restructured)", async () => {
  projectionStore.getState().bulkLoad([
    { id: "hb", name: "Health Bar", parentId: null, components: { HealthBar: { width: 1 } } },
  ]);
  projectionStore.getState().select("hb");
  const revealTargets = () =>
    Promise.resolve({
      required: ["Health"],
      compatible: [],
      greyed: [
        { id: "r1", name: "Rock", reason: "doesn't provide Health" },
        { id: "r2", name: "Tree", reason: "already bound" },
      ],
      bound: [],
    });
  render(<Diagnostics client={fakeClient({ revealTargets })} />);

  const group = await screen.findByTestId("diag-greyed");
  expect(screen.queryAllByTestId("diag-greyed-row").length).toBe(0); // collapsed — no wall of reasons
  fireEvent.click(group.querySelector("button")!);
  expect(screen.getAllByTestId("diag-greyed-row").length).toBe(2); // expanded → the grouped reasons
});

test("a fully-wired entity shows an honest all-clear, not a blank pane", async () => {
  projectionStore.getState().bulkLoad([
    { id: "lamp", name: "Lamp", parentId: null, components: { MeshRenderer: { mesh: "lamp" } } },
  ]);
  projectionStore.getState().select("lamp");
  render(<Diagnostics client={fakeClient()} />); // default revealTargets → empty; Lamp doesn't need a binding
  expect(await screen.findByTestId("diag-clear")).toBeTruthy();
});
