//! The export dialog's three moments — the cost before, the one gesture, the ledger after — plus the
//! two refusals that must speak (a build with no writer, a writer the command cannot address).

import { afterEach, expect, test, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { ExportDialog, sceneOmissions } from "./ExportDialog";
import { projectStore } from "../store/project";
import { projectionStore } from "../store/projection";
import { uiStore } from "../store/ui";
import { toastStore } from "../store/toasts";
import { fakeClient } from "../transport/test-client";
import type { FormatSpec, SceneExportResponse } from "../transport/protocol";
import type { EditorClient } from "../transport/session";

afterEach(() => {
  projectStore.getState().reset();
  projectionStore.getState().reset();
  uiStore.getState().setStatus("");
  toastStore.getState().reset();
});

const NO_CARRIES: FormatSpec["carries"] = {
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

const spec = (over: Partial<FormatSpec> & Pick<FormatSpec, "id">): FormatSpec => ({
  label: over.id,
  extensions: ["glb"],
  domain: "Real-time",
  direction: "both",
  fidelity: "full",
  carries: { ...NO_CARRIES },
  note: "A note long enough to read like a sentence about what survives and what does not.",
  available: true,
  ...over,
});

const GLTF = spec({
  id: "gltf",
  label: "glTF 2.0 / GLB",
  extensions: ["glb", "gltf"],
  carries: { ...NO_CARRIES, geometry: true, hierarchy: true, materials: true, textures: true, skinning: true, animation: true },
});
const STEP = spec({
  id: "step",
  label: "STEP AP242",
  extensions: ["stp", "step"],
  domain: "CAD",
  fidelity: "subset",
  carries: { ...NO_CARRIES, geometry: true, hierarchy: true, materials: true, metadata: true },
});
const OBJ = spec({ id: "obj", label: "Wavefront OBJ", extensions: ["obj"], direction: "import" });

const reply = (over: Partial<SceneExportResponse> = {}): SceneExportResponse => ({
  ok: true,
  message: "Exported 12 objects to scene.glb",
  format: "glb",
  exportedPath: "C:/work/scene.glb",
  nodes: 12,
  meshes: 4,
  skins: 1,
  animations: 2,
  fidelity: [],
  ...over,
});

const open = (client: EditorClient) => render(<ExportDialog open client={client} onClose={() => {}} />);

test("the rail is the catalogue's writable half — an import-only format never appears", async () => {
  open(fakeClient({ formatCatalog: () => Promise.resolve([GLTF, OBJ, STEP]) }));

  await waitFor(() => expect(screen.getByTestId("exportPane-gltf")).toBeTruthy());
  const rail = screen.getByRole("tablist", { name: "Export formats" });
  const names = Array.from(rail.querySelectorAll("[role='tab']")).map((t) => t.textContent);
  expect(names.some((n) => n?.includes("glTF"))).toBe(true);
  expect(names.some((n) => n?.includes("STEP"))).toBe(true);
  // OBJ is `direction: "import"`. A dialog that offered it would offer a button that cannot work.
  expect(names.some((n) => n?.includes("OBJ"))).toBe(false);
});

test("the capability checklist marks carried and not-carried apart by more than colour", async () => {
  open(fakeClient({ formatCatalog: () => Promise.resolve([STEP]) }));

  await waitFor(() => expect(screen.getByTestId("exportCarries")).toBeTruthy());
  expect(screen.getByTestId("exportCarry-geometry").getAttribute("data-carried")).toBe("true");
  expect(screen.getByTestId("exportCarry-animation").getAttribute("data-carried")).toBe("false");
  // Meaning must not depend on the mark's colour: the row also says it in words, for a reader who
  // gets the page as text.
  expect(screen.getByTestId("exportCarry-animation").textContent).toContain("not written");
  // "written" is a substring of "not written", so the carried row's claim has to be the negative one
  // or it would pass on the very state it is distinguishing itself from.
  expect(screen.getByTestId("exportCarry-geometry").textContent).toContain("written");
  expect(screen.getByTestId("exportCarry-geometry").textContent).not.toContain("not written");
});

test("what THIS scene loses is counted before the click, and changes with the format", async () => {
  projectionStore.getState().bulkLoad([
    { id: "cam", name: "Camera 1", parentId: null, components: { Camera: {} } },
    { id: "rig", name: "Weld Gun 7", parentId: null, components: { Transform: {}, Animator: {} } },
    { id: "box", name: "Crate", parentId: null, components: { Transform: {}, RigidBody: {} } },
    { id: "pad", name: "Pad", parentId: null, components: { Transform: {}, Collider: {} } },
  ] as never);
  open(fakeClient({ formatCatalog: () => Promise.resolve([GLTF, STEP]) }));

  // glTF carries animation, so only the camera and the two physics objects are lost.
  await waitFor(() => expect(screen.getByTestId("exportCost")).toBeTruthy());
  expect(screen.getByTestId("exportCost").textContent).toContain("1 camera");
  expect(screen.getByTestId("exportCost").textContent).toContain("2 physics bodies");
  expect(screen.getByTestId("exportCost").textContent).not.toContain("animated object");

  // STEP carries none of the three, so the animated object joins the list.
  fireEvent.click(screen.getByRole("tab", { name: /STEP/ }));
  await waitFor(() => expect(screen.getByTestId("exportCost").textContent).toContain("1 animated object"));
  expect(screen.getByTestId("exportSubject").textContent).toContain("4 objects in this scene");
});

test("the exporter's ledger replaces the options in place, and the path is on screen", async () => {
  const sceneExport = vi.fn(() => Promise.resolve(reply({
    fidelity: [
      { status: "converted", feature: "authored_visibility", count: 1, detail: "Stored in node extras" },
      { status: "omitted", feature: "physics", count: 3, detail: "glTF has no rigid-body representation" },
    ],
  })));
  open(fakeClient({ formatCatalog: () => Promise.resolve([GLTF]), sceneExport }));

  await waitFor(() => expect(screen.getByTestId("exportConfirm")).toBeTruthy());
  fireEvent.click(screen.getByTestId("exportConfirm"));

  // The canonical extension IS the argument — `extensions[0]`, no second mapping table.
  await waitFor(() => expect(sceneExport).toHaveBeenCalledWith("glb"));
  await waitFor(() => expect(screen.getByTestId("exportResult")).toBeTruthy());
  expect(screen.queryByTestId("exportCarries")).toBeNull();
  expect(screen.getByTestId("exportCount-objects").textContent).toContain("12");
  expect(screen.getByTestId("exportPath").textContent).toBe("C:/work/scene.glb");
  const ledger = screen.getByTestId("exportFidelity");
  expect(ledger.textContent).toContain("authored_visibility");
  expect(ledger.textContent).toContain("glTF has no rigid-body representation");
  expect(screen.getByTestId("exportDone")).toBeTruthy();
});

test("STEP is addressed by its canonical extension, which is 'stp' and not 'step'", async () => {
  // The regression this pins: the old menu hardcoded three arguments and STEP's was "step". The
  // dialog derives instead, and `stp` is what `formats.rs` lists first — a mapping table restated
  // here would be free to disagree with the one the command actually accepts.
  const sceneExport = vi.fn(() => Promise.resolve(reply({ format: "stp" })));
  open(fakeClient({ formatCatalog: () => Promise.resolve([STEP]), sceneExport }));

  await waitFor(() => expect(screen.getByTestId("exportConfirm")).toBeTruthy());
  fireEvent.click(screen.getByTestId("exportConfirm"));
  await waitFor(() => expect(sceneExport).toHaveBeenCalledWith("stp"));
});

test("a failed export says so in the dialog and re-arms the action", async () => {
  const sceneExport = vi.fn(() => Promise.resolve(reply({
    ok: false,
    message: "Complete-scene export is available in the packaged desktop editor.",
    exportedPath: null,
  })));
  open(fakeClient({ formatCatalog: () => Promise.resolve([GLTF]), sceneExport }));

  await waitFor(() => expect(screen.getByTestId("exportConfirm")).toBeTruthy());
  fireEvent.click(screen.getByTestId("exportConfirm"));

  await waitFor(() => expect(screen.getByTestId("exportFailure")).toBeTruthy());
  expect(screen.getByTestId("exportFailure").textContent).toContain("packaged desktop editor");
  // Still the export action, not "Done" — nothing was written, so the workflow is not finished.
  expect(screen.getByTestId("exportConfirm").hasAttribute("disabled")).toBe(false);
  expect(screen.queryByTestId("exportDone")).toBeNull();
});

test("a format the command cannot address is refused with a reason rather than offered", async () => {
  // `.3mf` is not in `formats::EXPORT_ARGS`. A future writer landing with an unaccepted canonical
  // extension must be visibly unavailable here, not a button that fails at the click.
  const future = spec({ id: "3mf", label: "3MF", extensions: ["3mf"], carries: { ...NO_CARRIES, geometry: true } });
  const sceneExport = vi.fn();
  open(fakeClient({ formatCatalog: () => Promise.resolve([future]), sceneExport }));

  await waitFor(() => expect(screen.getByTestId("exportConfirm")).toBeTruthy());
  const confirm = screen.getByTestId("exportConfirm");
  expect(confirm.hasAttribute("disabled")).toBe(true);
  expect(confirm.getAttribute("title")).toContain("3MF");
  fireEvent.click(confirm);
  expect(sceneExport).not.toHaveBeenCalled();
});

test("a build with no writer gets an empty state that names the cause", async () => {
  open(fakeClient({ formatCatalog: () => Promise.resolve([OBJ]) }));

  await waitFor(() => expect(screen.getByTestId("exportEmpty")).toBeTruthy());
  expect(screen.getByTestId("exportEmpty").textContent).toContain("build problem");
  expect(screen.getByTestId("exportConfirm").hasAttribute("disabled")).toBe(true);
});

test("the primary action names what it will write", async () => {
  projectStore.getState().refresh({ path: "C:/work/weld-cell.mtk", dirty: false, recents: [], error: null });
  open(fakeClient({ formatCatalog: () => Promise.resolve([GLTF]) }));

  await waitFor(() => expect(screen.getByTestId("exportConfirm")).toBeTruthy());
  expect(screen.getByTestId("exportConfirm").textContent).toContain("weld-cell");
});

test("sceneOmissions counts objects, not components, and stays silent about what it cannot see", () => {
  const entities = [
    { id: "a", name: "a", parentId: null, components: { Collider: {}, RigidBody: {} } },
    { id: "b", name: "b", parentId: null, components: { Camera: {} } },
  ] as never;
  // One object carries BOTH physics components; it is one loss, not two.
  const physicsOnly = sceneOmissions(entities, { ...NO_CARRIES, cameras: true });
  expect(physicsOnly).toEqual([{ key: "physics", count: 1, text: "1 physics body" }]);
  // Materials and textures have no probe, so nothing is claimed about them either way.
  expect(sceneOmissions(entities, NO_CARRIES).map((o) => o.key)).toEqual(["cameras", "physics"]);
  // A format that carries everything probed reports nothing at all.
  expect(sceneOmissions(entities, { ...NO_CARRIES, cameras: true, animation: true, physics: true })).toEqual([]);
});
