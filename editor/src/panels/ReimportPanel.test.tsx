//! ReimportPanel (M15.10 / ADR-080) — verified headless: a re-import surfaces the never-silent diff, holds a
//! low-confidence match for confirm/reject (never auto-applied), flags a removed part's overrides, and the
//! confirm/reject buttons drive the resolve command. Asserts structured data-* + client calls, never prose.

import { afterEach, expect, test, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { ReimportPanel } from "./ReimportPanel";
import { projectionStore } from "../store/projection";
import { fakeClient } from "../transport/test-client";
import type { ReimportReport } from "../transport/protocol";

afterEach(() => projectionStore.getState().reset());

const REPORT: ReimportReport = {
  isReimport: true,
  rebound: 1,
  added: 1,
  removed: 1,
  adjudicate: 1,
  rows: [
    { newEntity: "1_10", name: "Bracket", kind: "matched", confidence: 0.9, reason: "kept — edited part matched", hadOverrides: true },
    { newEntity: "1_11", name: "Gusset", kind: "added", confidence: 0, reason: "new part", hadOverrides: false },
    { newEntity: null, name: "Plate", kind: "removed", confidence: 0, reason: "deleted from the CAD", hadOverrides: true },
  ],
  orphans: [{ oldId: "0000000000000002", name: "Plate", material: "gold", hasJoint: false }],
  pending: [{ oldId: "0000000000000003", newEntity: "1_12", name: "Cover", confidence: 0.66, material: "chrome", hasJoint: true }],
};

test("a first import renders nothing (no re-import surface)", () => {
  const { container } = render(<ReimportPanel client={fakeClient({ cadReimportReport: () => Promise.resolve({ isReimport: false, rebound: 0, added: 0, removed: 0, adjudicate: 0, rows: [], orphans: [], pending: [] }) })} />);
  expect(container.querySelector('[data-testid="reimport-panel"]')).toBeNull();
});

test("a re-import shows the diff, flags the removed part, and holds the low-confidence match", async () => {
  render(<ReimportPanel client={fakeClient({ cadReimportReport: () => Promise.resolve(REPORT) })} />);
  const panel = await screen.findByTestId("reimport-panel");
  expect(panel.getAttribute("data-rebound")).toBe("1");
  expect(panel.getAttribute("data-adjudicate")).toBe("1");
  // The removed part's overrides are flagged (an orphan), never silently dropped.
  expect(screen.getByTestId("reimport-orphan").getAttribute("data-old-id")).toBe("0000000000000002");
  // The low-confidence match is HELD for adjudication (a confirm/reject card), not auto-applied.
  expect(screen.getByTestId("reimport-adjudicate").getAttribute("data-old-id")).toBe("0000000000000003");
  expect(screen.getByTestId("reimport-confirm")).toBeTruthy();
  expect(screen.getByTestId("reimport-reject")).toBeTruthy();
  // The per-part diff accounts for every part.
  const kinds = screen.getAllByTestId("reimport-row").map((r) => r.getAttribute("data-kind"));
  expect(kinds).toEqual(["matched", "added", "removed"]);
});

test("confirm resolves the held match through the client (accept=true)", async () => {
  const resolved: ReimportReport = { ...REPORT, rebound: 2, adjudicate: 0, pending: [] };
  const cadReimportResolve = vi.fn(() => Promise.resolve(resolved));
  render(<ReimportPanel client={fakeClient({ cadReimportReport: () => Promise.resolve(REPORT), cadReimportResolve })} />);
  fireEvent.click(await screen.findByTestId("reimport-confirm"));
  await waitFor(() => expect(cadReimportResolve).toHaveBeenCalledWith("0000000000000003", true));
});

test("reject discards the uncertain match (accept=false) — never a silent wrong-bind", async () => {
  const cadReimportResolve = vi.fn(() => Promise.resolve({ ...REPORT, adjudicate: 0, pending: [] }));
  render(<ReimportPanel client={fakeClient({ cadReimportReport: () => Promise.resolve(REPORT), cadReimportResolve })} />);
  fireEvent.click(await screen.findByTestId("reimport-reject"));
  await waitFor(() => expect(cadReimportResolve).toHaveBeenCalledWith("0000000000000003", false));
});
