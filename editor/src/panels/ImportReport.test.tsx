//! Import report (M15.7 / ADR-077) — verified headless: the never-silent per-part surface renders the
//! fidelity BREAKDOWN, filters by honesty class (the ECS "show tessellation-only parts" query), explains
//! each below-exact part with a fix, selects the entity on click, and stays out of the way when there is no
//! CAD. Asserts the STRUCTURED data-* signals + the fidelity tokens, never the drifting prose.
//!
//! ADR-163 added the half that makes it usable on a real assembly, and these tests are written the way
//! that half has to be tested: the search, the class filter and the paging all run in the SHELL, so what
//! is verified here is (a) the question the panel asks, argument by argument, and (b) that it renders the
//! answer faithfully and says what the answer is. A stub that re-implemented the shell's query would be a
//! third statement of a contract that already has two.

import { afterEach, expect, test, vi } from "vitest";
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
  matched: 4,
  offset: 0,
  parts: [
    { id: "e1", name: "Plate", fidelity: "exact-brep", ...NOTHING_TO_SAY },
    { id: "e2", name: "Weld Gun", fidelity: "tessellation-only", ...NOTHING_TO_SAY },
    { id: "e3", name: "Conveyor", fidelity: "tessellation-only", ...NOTHING_TO_SAY },
    { id: "e4", name: "Overhead Crane", fidelity: "proxy", ...NOTHING_TO_SAY },
  ],
};

/** A shell stand-in that RECORDS the question and answers from a table. It never filters anything
 *  itself — the point of every assertion below is either the arguments it was handed or the payload it
 *  handed back. */
function reportClient(answer: (query: string, fidelity: string, offset: number) => CadReport) {
  const asked: { query: string; fidelity: string; offset: number; limit: number }[] = [];
  const client = fakeClient({
    cadReport: () => Promise.resolve(answer("", "all", 0)),
    cadReportPage: (query: string, fidelity: string, offset: number, limit: number) => {
      asked.push({ query, fidelity, offset, limit });
      return Promise.resolve(answer(query, fidelity, offset));
    },
    gizmoSelect: vi.fn(() => Promise.resolve(true)),
    focusEntity: vi.fn(),
  });
  return { client, asked };
}

const fixed = (r: CadReport) => reportClient(() => r);

// A one-entity scene so the panel's baseCount refetch effect fires (it keys on the projection base size).
function seedScene() {
  projectionStore.getState().bulkLoad([{ id: "e1", name: "Plate", parentId: null, components: { CadPart: { fidelity: "exact-brep" } } }]);
}

test("renders the fidelity breakdown + every part, and says which rows are on screen", async () => {
  seedScene();
  render(<ImportReport client={fixed(REPORT).client} />);

  // The header breakdown accounts for every part (never-silent), keyed on structured data-* not prose.
  const panel = await screen.findByTestId("import-report");
  expect(panel.getAttribute("data-total")).toBe("4");
  expect(panel.getAttribute("data-below-exact")).toBe("3");
  // ADR-163: the list's own size is now a signal too, so "how many rows are there" and "how many did
  // the query find" can never silently be read as the same number again.
  expect(panel.getAttribute("data-matched")).toBe("4");
  expect(panel.getAttribute("data-shown")).toBe("4");

  // All four rows present; each carries its stable fidelity token.
  expect(screen.getAllByTestId("import-row")).toHaveLength(4);
  expect(screen.getByText("Overhead Crane").closest("[data-testid='import-row']")?.getAttribute("data-fidelity")).toBe("proxy");
  expect(screen.getByTestId("import-showing").textContent).toContain("Showing 1–4 of 4 parts");
});

test("'show tessellation-only parts' is asked of the ENGINE, and the answer is what the list shows", async () => {
  seedScene();
  const TESSELLATION: CadReport = {
    ...REPORT,
    matched: 2,
    parts: REPORT.parts.filter((p) => p.fidelity === "tessellation-only"),
  };
  const { client, asked } = reportClient((_q, fidelity) => (fidelity === "tessellation-only" ? TESSELLATION : REPORT));
  render(<ImportReport client={client} />);
  await screen.findByTestId("import-report");

  fireEvent.click(screen.getByTestId("filter-tessellation-only"));
  await waitFor(() => expect(screen.getAllByTestId("import-row")).toHaveLength(2));
  for (const row of screen.getAllByTestId("import-row")) {
    expect(row.getAttribute("data-fidelity")).toBe("tessellation-only");
  }
  // The class token crossed the boundary — the filter is a query, not a client-side `Array.filter`
  // over whatever page happened to arrive. That distinction IS the defect ADR-163 closed.
  expect(asked.some((a) => a.fidelity === "tessellation-only" && a.offset === 0)).toBe(true);
  // The chips still count the whole query, so you can see what else is in it while inside one class.
  expect(screen.getByTestId("filter-proxy").textContent).toBe("Proxy 1");
  expect(screen.getByTestId("import-showing").textContent).toContain("Showing 1–2 of 2 tessellation-only parts");
});

test("typing searches in the engine, once the typing settles", async () => {
  seedScene();
  const { client, asked } = reportClient((query) =>
    query.trim() ? { ...REPORT, total: 1, exactBrep: 0, tessellationOnly: 1, proxy: 0, matched: 1, parts: [REPORT.parts[1]] } : REPORT,
  );
  render(<ImportReport client={client} />);
  await screen.findByTestId("import-report");

  fireEvent.change(screen.getByTestId("import-search"), { target: { value: "weld" } });
  await waitFor(() => expect(screen.getAllByTestId("import-row")).toHaveLength(1));
  expect(screen.getByText("Weld Gun")).toBeTruthy();
  expect(asked.some((a) => a.query === "weld")).toBe(true);
  expect(screen.getByTestId("import-showing").textContent).toContain("matching “weld”");
});

// ── The defect ADR-163 closed, as a standing assertion ────────────────────────────────────────────
//
// The panel asked for the first 500 rows and filtered them in the BROWSER. On the 15,711-part assembly
// this surface exists for, a chip could read "Proxy 412" over a list of zero rows — those 412 were
// simply not among the 500 alphabetically-first rows the shell had sent — and the panel printed not one
// word about either fact. Both halves are now impossible: the count and the list come from one query,
// and an empty list is a sentence with a way out beside it.

test("an empty result is a stated result, with the way back out beside it", async () => {
  seedScene();
  const NONE: CadReport = { ...REPORT, total: 0, exactBrep: 0, tessellationOnly: 0, proxy: 0, matched: 0, parts: [] };
  const { client } = reportClient((query) => (query.trim() ? NONE : { ...REPORT, total: 15_711, exactBrep: 15_299, tessellationOnly: 0, proxy: 412 }));
  render(<ImportReport client={client} />);
  await screen.findByTestId("import-report");

  fireEvent.change(screen.getByTestId("import-search"), { target: { value: "gasket" } });
  await waitFor(() => expect(screen.queryAllByTestId("import-row")).toHaveLength(0));

  // NOT a blank space under a chip: the panel says what it looked for and how much it looked through.
  const empty = screen.getByTestId("import-empty");
  expect(empty.textContent).toContain("15,711");
  expect(screen.getByTestId("import-showing").textContent).toContain("No CAD part matches “gasket”");

  // And the panel does not VANISH when its own search finds nothing — hiding on `total === 0` would
  // take the search box that produced the empty result with it.
  fireEvent.click(screen.getByTestId("import-clear"));
  await waitFor(() => expect(screen.getAllByTestId("import-row").length).toBeGreaterThan(0));
});

test("a list longer than a page says where it is, and Previous refuses in words", async () => {
  seedScene();
  const PAGED: CadReport = { ...REPORT, total: 15_711, exactBrep: 15_299, tessellationOnly: 0, proxy: 412, matched: 412, offset: 0 };
  const { client, asked } = reportClient(() => PAGED);
  render(<ImportReport client={client} />);
  await screen.findByTestId("import-report");

  expect(screen.getByTestId("import-showing").textContent).toContain("Showing 1–4 of 412 parts");
  const prev = screen.getByTestId("import-prev") as HTMLButtonElement;
  expect(prev.disabled).toBe(true);
  // A disabled control explains itself — `<ux_quality>` 4, and R9 in the shots gate.
  expect(prev.getAttribute("title")).toContain("first page");

  fireEvent.click(screen.getByTestId("import-next"));
  await waitFor(() => expect(asked.some((a) => a.offset === 200)).toBe(true));
});

test("a page that fell off the end of a report that shrank goes back to the first page", async () => {
  seedScene();
  // The shell answers offset 400 over a 4-row report with an empty page and `matched: 4` — the one
  // state where "no rows" and "no matches" are different things. The panel must not print "No CAD
  // part" over a report that has four of them.
  // `matched: 412` so the pager exists, and every page past the first comes back empty — the shape a
  // report that shrank under the reader produces.
  const { client, asked } = reportClient((_q, _f, offset) =>
    offset > 0 ? { ...REPORT, matched: 412, offset, parts: [] } : { ...REPORT, matched: 412 },
  );
  render(<ImportReport client={client} />);
  await screen.findByTestId("import-report");

  fireEvent.click(screen.getByTestId("import-next"));
  await waitFor(() => expect(asked.some((a) => a.offset === 200)).toBe(true));
  // It asked for page 2, got nothing back, and asked for page 1 again rather than showing an empty
  // list captioned as an empty report.
  await waitFor(() => expect(asked.filter((a) => a.offset === 0).length).toBeGreaterThan(1));
  expect(screen.queryAllByTestId("import-empty")).toHaveLength(0);
  expect(screen.getAllByTestId("import-row")).toHaveLength(4);
});

test("a single page shows no pager at all", async () => {
  seedScene();
  render(<ImportReport client={fixed(REPORT).client} />);
  await screen.findByTestId("import-report");
  expect(screen.queryByTestId("import-next")).toBeNull();
});

test("clicking a part selects its entity in the store AND in the engine", async () => {
  seedScene();
  const { client } = fixed(REPORT);
  render(<ImportReport client={client} />);
  const row = await screen.findByText("Weld Gun");
  fireEvent.click(row);
  expect(projectionStore.getState().selectedId).toBe("e2");
  // The half this panel never had: without it the row highlights here while the viewport, the gizmo
  // and the inspector stay on whatever was selected before.
  expect(client.gizmoSelect).toHaveBeenCalledWith("e2");
});

test("Frame moves the camera to the part, which is the only way to see one in a 262 m assembly", async () => {
  seedScene();
  const { client } = fixed(REPORT);
  render(<ImportReport client={client} />);
  const row = (await screen.findByText("Overhead Crane")).closest("[data-testid='import-row']")!;
  fireEvent.click(row.querySelector("[data-testid='import-frame']")!);
  expect(client.focusEntity).toHaveBeenCalledWith("e4");
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
  render(<ImportReport client={fixed(REPORT).client} />);
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
  expect(plate.querySelector("[data-testid='import-select']")!.getAttribute("title")).not.toContain("null");
});

test("a part that carries its own reason/fix/provenance renders those, not the class default", async () => {
  seedScene();
  const detailed: CadReport = {
    ...REPORT,
    matched: 1,
    parts: [{
      id: "e9", name: "Gearbox", fidelity: "proxy",
      reference: "GEARBOX-A/1", strategy: "kernel-unavailable", sourceFormat: "CATPart",
      reason: "The licensed CAD kernel is not installed on this machine.",
      fix: "Install the kernel, or supply the STEP companion file.",
    }],
  };
  render(<ImportReport client={fixed(detailed).client} />);
  const row = (await screen.findByText("Gearbox")).closest("[data-testid='import-row']")!;

  expect(row.querySelector("[data-testid='import-row-provenance']")!.textContent)
    .toBe("GEARBOX-A/1 · kernel-unavailable · CATPart");
  expect(row.querySelector("[data-testid='import-row-fix']")!.textContent)
    .toContain("Install the kernel");
  // The part's own reason wins over the proxy class's default.
  expect(row.querySelector("[data-testid='import-select']")!.getAttribute("title")).toContain("licensed CAD kernel is not installed");
});
