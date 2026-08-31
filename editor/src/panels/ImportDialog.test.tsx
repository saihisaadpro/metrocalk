//! ADR-178 — the import task dialog: the catalogue it offers, the filter it sends, and the three
//! different things it says afterwards.
//!
//! The claims here are the ones a capture cannot make. `shots` photographs the composition; these
//! assert the WIRE — which extensions the chosen format sends to the native dialog, and that a
//! dismissed picker and an unreadable file produce two different surfaces rather than one shrug.

import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, test, vi } from "vitest";
import { ImportDialog, joinWords, missingCapabilities } from "./ImportDialog";
import { projectionStore } from "../store/projection";
import { projectStore } from "../store/project";
import { toastStore } from "../store/toasts";
import { fakeClient } from "../transport/test-client";
import type { CadReport, FormatSpec, ImportDialogResponse } from "../transport/protocol";

const NONE: FormatSpec["carries"] = {
  geometry: false,
  hierarchy: false,
  materials: false,
  textures: false,
  skinning: false,
  animation: false,
  cameras: false,
  metadata: false,
  physics: false,
};

const spec = (over: Partial<FormatSpec>): FormatSpec => ({
  id: "gltf",
  label: "glTF 2.0 / GLB",
  extensions: ["glb", "gltf"],
  domain: "Real-time",
  direction: "both",
  fidelity: "full",
  carries: { ...NONE, geometry: true, hierarchy: true, materials: true, textures: true },
  note: "The interchange format this engine round-trips.",
  available: true,
  ...over,
});

const CATALOGUE: FormatSpec[] = [
  spec({}),
  spec({
    id: "step",
    label: "STEP AP242",
    extensions: ["stp", "step"],
    domain: "CAD",
    direction: "both",
    fidelity: "subset",
    carries: { ...NONE, geometry: true, hierarchy: true, metadata: true },
    note: "Tessellated and B-rep AP242, with per-part colour.",
  }),
  // Written by this build and NOT read by it — the row that must not appear in an import rail.
  spec({ id: "writeonly", label: "Write-only Format", extensions: ["wof"], direction: "export" }),
  // Read by the format registry but not compiled into this build.
  spec({ id: "absent", label: "Unavailable Reader", extensions: ["abs"], direction: "import", available: false }),
];

const CAD: CadReport = {
  total: 378,
  exactBrep: 240,
  tessellationOnly: 130,
  aiReconstructed: 0,
  proxy: 8,
  accessDenied: 0,
  failed: 0,
  parts: [],
};

function client(over: {
  reply?: ImportDialogResponse;
  report?: CadReport;
  spy?: (extensions?: readonly string[]) => void;
} = {}) {
  return fakeClient({
    formatCatalog: () => Promise.resolve(CATALOGUE),
    cadReport: () => Promise.resolve(over.report ?? { ...CAD, total: 0 }),
    importAssetDialog: vi.fn((extensions?: readonly string[]) => {
      over.spy?.(extensions);
      return Promise.resolve(
        over.reply ?? { entityId: "e-1", outcome: "imported" as const, message: "Imported crane.step." },
      );
    }),
  });
}

/** The footer renders before `formatCatalog()` resolves, and the primary action is refused until it
 *  does. A test that clicks before then is clicking a disabled button and asserting nothing. */
const settled = () => screen.findAllByRole("tab");

beforeEach(() => {
  toastStore.getState().reset();
  projectionStore.setState({ base: {}, deactivated: {} } as never, false);
  projectStore.getState().refresh({ path: "C:/work/cell.mtk", dirty: false, recents: [], error: null });
});

test("the rail is every READABLE format, and never one this build only writes or lacks", async () => {
  render(<ImportDialog open client={client()} onClose={() => {}} />);
  const tabs = await screen.findAllByRole("tab");

  // "Any supported file" leads, then the two readable formats. The write-only row and the row this
  // build was compiled without are absent — membership is `readsScenes`, not "everything declared".
  expect(tabs.map((t) => t.textContent)).toEqual(["Any supported file", "glTF 2.0 / GLB", "STEP AP242"]);
  expect(screen.queryByText("Write-only Format")).toBeNull();
  expect(screen.queryByText("Unavailable Reader")).toBeNull();
});

test("the default pane names every extension this build opens, deduplicated and sorted", async () => {
  render(<ImportDialog open client={client()} onClose={() => {}} />);
  const list = await screen.findByTestId("importExtensions");

  expect([...list.querySelectorAll("li")].map((li) => li.textContent)).toEqual([
    ".glb",
    ".gltf",
    ".step",
    ".stp",
  ]);
});

test("choosing a format sends THAT format's extensions to the native dialog", async () => {
  const seen: (readonly string[] | undefined)[] = [];
  render(<ImportDialog open client={client({ spy: (e) => seen.push(e) })} onClose={() => {}} />);

  fireEvent.click(await screen.findByRole("tab", { name: /STEP AP242/ }));
  // The primary action NAMES the choice, so what is about to open is legible before it opens.
  expect(screen.getByTestId("importConfirm").textContent).toBe("Choose a STEP AP242 file…");
  await act(async () => {
    fireEvent.click(screen.getByTestId("importConfirm"));
  });

  // Not decoration: the rail choice IS the file dialog's filter.
  expect(seen).toEqual([["stp", "step"]]);
});

test("with no format chosen the filter is left off entirely", async () => {
  const seen: (readonly string[] | undefined)[] = [];
  render(<ImportDialog open client={client({ spy: (e) => seen.push(e) })} onClose={() => {}} />);

  await settled();
  await act(async () => {
    fireEvent.click(screen.getByTestId("importConfirm"));
  });

  expect(seen).toEqual([undefined]);
});

test("a format states what it will NOT bring in, before any file is chosen", async () => {
  render(<ImportDialog open client={client()} onClose={() => {}} />);

  fireEvent.click(await screen.findByRole("tab", { name: /STEP AP242/ }));
  const cost = await screen.findByTestId("importCost");

  // Named capabilities, never a count: nothing has been opened yet, so a number would be invented.
  expect(cost.textContent).toContain("materials");
  expect(cost.textContent).toContain("animation");
  expect(cost.textContent).not.toMatch(/\d/);
});

test("a CANCELLED import says so, and does not dress a dismissed picker as a refusal", async () => {
  const reply: ImportDialogResponse = {
    entityId: null,
    outcome: "cancelled",
    message: "No file was chosen. Nothing in the scene changed.",
  };
  render(<ImportDialog open client={client({ reply })} onClose={() => {}} />);

  await settled();
  await act(async () => {
    fireEvent.click(screen.getByTestId("importConfirm"));
  });

  const refused = await screen.findByTestId("importRefused");
  expect(refused.getAttribute("data-outcome")).toBe("cancelled");
  expect(refused.textContent).toContain("No file was chosen");
  // A cancellation is not an error and must not toast like one.
  expect(toastStore.getState().toasts).toHaveLength(0);
});

test("a FAILED import is a different surface, and it toasts", async () => {
  const reply: ImportDialogResponse = {
    entityId: null,
    outcome: "failed",
    message: "sketch.dwg could not be read by this build.",
  };
  render(<ImportDialog open client={client({ reply })} onClose={() => {}} />);

  await settled();
  await act(async () => {
    fireEvent.click(screen.getByTestId("importConfirm"));
  });

  const refused = await screen.findByTestId("importRefused");
  expect(refused.getAttribute("data-outcome")).toBe("failed");
  expect(toastStore.getState().toasts.map((t) => t.kind)).toEqual(["error"]);
});

test("a successful CAD import shows the per-part breakdown in the dialog that caused it", async () => {
  render(<ImportDialog open client={client({ report: CAD })} onClose={() => {}} />);

  await settled();
  await act(async () => {
    fireEvent.click(screen.getByTestId("importConfirm"));
  });

  await waitFor(() => expect(screen.getByTestId("importFidelity")).toBeTruthy());
  const rows = [...screen.getByTestId("importFidelity").querySelectorAll("li")];
  // Only the classes that OCCUR: six zeroes would be noise dressed as an account.
  expect(rows.map((li) => li.getAttribute("data-fidelity"))).toEqual([
    "exactBrep",
    "tessellationOnly",
    "proxy",
  ]);
  expect(screen.getByTestId("importResult").textContent).toContain("378 parts accounted for");
  expect(screen.getByTestId("importEntity").textContent).toContain("e-1");
});

test("a successful NON-CAD import reports the outcome and draws no empty fidelity table", async () => {
  render(<ImportDialog open client={client()} onClose={() => {}} />);

  await settled();
  await act(async () => {
    fireEvent.click(screen.getByTestId("importConfirm"));
  });

  await waitFor(() => expect(screen.getByTestId("importResult")).toBeTruthy());
  expect(screen.queryByTestId("importFidelity")).toBeNull();
  expect(screen.getByTestId("importMessage").textContent).toContain("Imported crane.step.");
});

test("a successful import selects the placed entity and marks the project dirty", async () => {
  render(<ImportDialog open client={client()} onClose={() => {}} />);

  await settled();
  await act(async () => {
    fireEvent.click(screen.getByTestId("importConfirm"));
  });

  await waitFor(() => expect(projectionStore.getState().selectedId).toBe("e-1"));
  expect(projectStore.getState().dirty).toBe(true);
});

test("missingCapabilities names what a reader drops, in the shared CARRIES order", () => {
  expect(missingCapabilities({ ...NONE, geometry: true, hierarchy: true, metadata: true })).toEqual([
    "materials",
    "textures",
    "skinning",
    "animation",
    "cameras",
    "physics",
  ]);
  expect(joinWords(["materials", "textures", "animation"])).toBe("materials, textures and animation");
  expect(joinWords(["materials"])).toBe("materials");
  expect(joinWords([])).toBe("");
});
