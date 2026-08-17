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

/** The five explanatory fields as the SHELL sends them when it has nothing to say: present, holding
 *  `null`. They are bare `Option<String>` with no `skip_serializing_if`, so a fixture that omitted
 *  them was testing a payload the real core cannot produce (ADR-123). */
const NOTHING_TO_SAY = { reference: null, strategy: null, reason: null, fix: null, sourceFormat: null };

const REPORT: CadReport = {
  total: 4,
  exactBrep: 1,
  tessellationOnly: 2,
  aiReconstructed: 0,
  proxy: 1,
  accessDenied: 0,
  failed: 0,
  parts: [
    { id: "e1", name: "Plate", fidelity: "exact-brep", ...NOTHING_TO_SAY },
    { id: "e2", name: "Weld Gun", fidelity: "tessellation-only", ...NOTHING_TO_SAY },
    { id: "e3", name: "Conveyor", fidelity: "tessellation-only", ...NOTHING_TO_SAY },
    { id: "e4", name: "Overhead Crane", fidelity: "proxy", ...NOTHING_TO_SAY },
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

// ── `null` is a value the shell sends, not a shape the fixtures forgot ────────────────────────────
//
// The five explanatory fields arrive as `"reason": null`, never as an absent key (ADR-123). Every
// fixture in this file used to OMIT them, so `undefined` was the only thing the panel was ever tested
// against — and `undefined` and `null` are the two inputs that behave differently in exactly the
// narrowing (`x !== undefined`) that the old `reason?: string` declaration made `tsc` bless. These two
// tests are the pair: nothing-to-say must fall back rather than render a hole, and something-to-say
// must render the part's own words rather than the class default.

test("a part whose explanatory fields are all null falls back to its class explanation, and shows no provenance line", async () => {
  seedScene();
  render(<ImportReport client={fakeClient({ cadReport: () => Promise.resolve(REPORT) })} />);
  await screen.findByTestId("import-report");

  // No provenance line anywhere: absent, not an empty strip of separators.
  expect(screen.queryAllByTestId("import-row-provenance")).toHaveLength(0);

  // Every below-exact row still explains itself — a null `fix` falls back to the class fix rather
  // than blanking the row. Keyed on the class's own token, not on the copy.
  const crane = screen.getByText("Overhead Crane").closest("[data-testid='import-row']")!;
  const fixLine = crane.querySelector("[data-testid='import-row-fix']");
  expect(fixLine).not.toBeNull();
  expect(fixLine!.textContent).not.toContain("null");
  // The exact-B-rep class has no fix at all, so its row must omit the line rather than print "null".
  const plate = screen.getByText("Plate").closest("[data-testid='import-row']")!;
  expect(plate.querySelector("[data-testid='import-row-fix']")).toBeNull();
  expect(plate.getAttribute("title")).not.toContain("null");
});

test("a part that carries its own reason/fix/provenance renders those, not the class default", async () => {
  seedScene();
  const detailed: CadReport = {
    ...REPORT,
    parts: [{
      id: "e9", name: "Gearbox", fidelity: "proxy",
      reference: "GEARBOX-A/1", strategy: "kernel-unavailable", sourceFormat: "CATPart",
      reason: "The licensed CAD kernel is not installed on this machine.",
      fix: "Install the kernel, or supply the STEP companion file.",
    }],
  };
  render(<ImportReport client={fakeClient({ cadReport: () => Promise.resolve(detailed) })} />);
  const row = (await screen.findByText("Gearbox")).closest("[data-testid='import-row']")!;

  expect(row.querySelector("[data-testid='import-row-provenance']")!.textContent)
    .toBe("GEARBOX-A/1 · kernel-unavailable · CATPart");
  expect(row.querySelector("[data-testid='import-row-fix']")!.textContent)
    .toContain("Install the kernel");
  // The part's own reason wins over the proxy class's default.
  expect(row.getAttribute("title")).toContain("licensed CAD kernel is not installed");
});
