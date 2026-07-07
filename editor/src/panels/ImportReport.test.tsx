//! Import report (M15.7 / ADR-077) — verified headless: the never-silent per-part surface renders the
//! fidelity BREAKDOWN, filters by honesty class (the ECS "show tessellation-only parts" query), explains
//! each below-exact part with a fix, selects the entity on click, and stays out of the way when there is no
//! CAD. Asserts the STRUCTURED data-* signals + the fidelity tokens, never the drifting prose.

import { afterEach, expect, test } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { ImportReport } from "./ImportReport";
import { projectionStore } from "../store/projection";
import { fakeClient } from "../transport/test-client";
import type { CadReport } from "../transport/protocol";

afterEach(() => projectionStore.getState().reset());

const REPORT: CadReport = {
  total: 4,
  exactBrep: 1,
  tessellationOnly: 2,
  aiReconstructed: 0,
  proxy: 1,
  accessDenied: 0,
  failed: 0,
  parts: [
    { id: "e1", name: "Plate", fidelity: "exact-brep" },
    { id: "e2", name: "Weld Gun", fidelity: "tessellation-only" },
    { id: "e3", name: "Conveyor", fidelity: "tessellation-only" },
    { id: "e4", name: "Overhead Crane", fidelity: "proxy" },
  ],
};

// A one-entity scene so the panel's baseCount refetch effect fires (it keys on the projection base size).
function seedScene() {
  projectionStore.getState().bulkLoad([{ id: "e1", name: "Plate", parentId: null, components: { CadPart: { fidelity: "exact-brep" } } }]);
}

test("renders the fidelity breakdown + every part, then filters to one honesty class", async () => {
  seedScene();
  render(<ImportReport client={fakeClient({ cadReport: () => Promise.resolve(REPORT) })} />);

  // The header breakdown accounts for every part (never-silent), keyed on structured data-* not prose.
  const panel = await screen.findByTestId("import-report");
  expect(panel.getAttribute("data-total")).toBe("4");
  expect(panel.getAttribute("data-below-exact")).toBe("3");

  // All four rows present; each carries its stable fidelity token.
  expect(screen.getAllByTestId("import-row")).toHaveLength(4);
  expect(screen.getByText("Overhead Crane").closest("[data-testid='import-row']")?.getAttribute("data-fidelity")).toBe("proxy");

  // "Show tessellation-only parts": the filter chip narrows the list to exactly that class.
  fireEvent.click(screen.getByTestId("filter-tessellation-only"));
  await waitFor(() => expect(screen.getAllByTestId("import-row")).toHaveLength(2));
  for (const row of screen.getAllByTestId("import-row")) {
    expect(row.getAttribute("data-fidelity")).toBe("tessellation-only");
  }
});

test("clicking a part selects its entity", async () => {
  seedScene();
  render(<ImportReport client={fakeClient({ cadReport: () => Promise.resolve(REPORT) })} />);
  const row = await screen.findByText("Weld Gun");
  fireEvent.click(row);
  expect(projectionStore.getState().selectedId).toBe("e2");
});

test("renders nothing when the scene has no CAD (total 0)", async () => {
  seedScene();
  const { container } = render(<ImportReport client={fakeClient()} />); // fakeClient default = an all-zero report
  await waitFor(() => expect(container.querySelector("[data-testid='import-report']")).toBeNull());
});
