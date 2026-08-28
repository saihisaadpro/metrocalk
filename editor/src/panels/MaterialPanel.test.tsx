//! MaterialPanel (ADR-164) — verified headless in jsdom.
//!
//! THE ASSERTION THAT IS THE WHOLE POINT: picking a finish emits a `setField` and NEVER calls `aiEdit`.
//! Before this panel, the Inspector's only material control was six buttons that each spent two tokens
//! to write the string `setField` writes for free, so a user with an empty wallet — or no network —
//! could not change a material at all. A test that only checked "the material changed" would have
//! passed against the old panel too; the client method is the thing that has to be named.

import { afterEach, expect, test, vi } from "vitest";
import { cleanup, render, screen, fireEvent } from "@testing-library/react";
import { MaterialPanel } from "./MaterialPanel";
import { projectionStore } from "../store/projection";
import { walletStore } from "../store/wallet";
import { uiStore } from "../store/ui";
import { toastStore } from "../store/toasts";
import { fakeClient } from "../transport/test-client";

afterEach(() => {
  // Unmount FIRST. Vitest runs `afterEach` hooks in reverse registration order, so the setup file's
  // `cleanup()` runs LAST — resetting the stores before it fires a React update into a still-mounted
  // tree, which is the "not wrapped in act(...)" warning this suite prints elsewhere.
  cleanup();
  projectionStore.getState().reset();
  walletStore.getState().reset();
  uiStore.getState().setStatus("");
  toastStore.getState().reset();
});

function selectShadeable(material?: string) {
  const MeshRenderer: Record<string, string> = { mesh: "rover" };
  if (material !== undefined) MeshRenderer.material = material;
  projectionStore.getState().bulkLoad([{ id: "e1", name: "Rover", parentId: null, components: { MeshRenderer } }]);
  projectionStore.getState().select("e1");
}

test("no selection → nothing to pick (the panel renders nothing)", () => {
  render(<MaterialPanel client={fakeClient()} />);
  expect(screen.queryByTestId("materialPicker")).toBeNull();
});

test("a finish is a FREE, undoable field write — never the metered AI edit", () => {
  selectShadeable();
  const setField = vi.fn(() => "op");
  const aiEdit = vi.fn();
  render(<MaterialPanel client={fakeClient({ setField, aiEdit })} />);

  fireEvent.click(screen.getByTestId("material-gold"));

  expect(setField).toHaveBeenCalledWith("e1", "MeshRenderer", "material", "gold");
  expect(aiEdit).not.toHaveBeenCalled();
  // No wallet movement of any kind — the balance is untouched because nothing was spent.
  expect(walletStore.getState().balance).toBe(walletStore.getInitialState().balance);
  // Feedback at the gesture, and a stable status token that NAMES the finish.
  expect(uiStore.getState().status).toBe("material gold");
  expect(toastStore.getState().toasts.some((t) => /gold/i.test(t.text))).toBe(true);
});

test("the panel says what the object is made of — the stored finish carries the selection", () => {
  selectShadeable("chrome");
  render(<MaterialPanel client={fakeClient()} />);
  expect(screen.getByTestId("material-chrome").getAttribute("aria-pressed")).toBe("true");
  expect(screen.getByTestId("material-gold").getAttribute("aria-pressed")).toBe("false");
  expect(screen.getByTestId("material-default").getAttribute("aria-pressed")).toBe("false");
});

test("an ALIAS the renderer accepts selects its own swatch — 'steel' is metal, not 'nothing set'", () => {
  selectShadeable("steel");
  render(<MaterialPanel client={fakeClient()} />);
  expect(screen.getByTestId("material-metal").getAttribute("aria-pressed")).toBe("true");
  expect(screen.getByTestId("material-default").getAttribute("aria-pressed")).toBe("false");
});

test("nothing set → Default carries the selection, and picking Default clears the override", () => {
  selectShadeable();
  const setField = vi.fn(() => "op");
  render(<MaterialPanel client={fakeClient({ setField })} />);
  expect(screen.getByTestId("material-default").getAttribute("aria-pressed")).toBe("true");

  fireEvent.click(screen.getByTestId("material-default"));
  expect(setField).toHaveBeenCalledWith("e1", "MeshRenderer", "material", "default");
});

test("an imported finish is NAMED rather than silently unselected", () => {
  selectShadeable("mtkasset:9f2c11");
  render(<MaterialPanel client={fakeClient()} />);
  const note = screen.getByTestId("material-foreign");
  expect(note.textContent).toMatch(/mtkasset:9f2c11/);
  // And the grid does not pretend the object is un-finished while the note says otherwise.
  expect(screen.getByTestId("material-default").getAttribute("aria-pressed")).toBe("false");
});

test("an object with no mesh REFUSES BEFORE the click, and says why", () => {
  projectionStore.getState().bulkLoad([{ id: "e1", name: "Trigger", parentId: null, components: { Transform: { px: 0 } } }]);
  projectionStore.getState().select("e1");
  const setField = vi.fn(() => "op");
  render(<MaterialPanel client={fakeClient({ setField })} />);

  const gold = screen.getByTestId("material-gold") as HTMLButtonElement;
  expect(gold.disabled).toBe(true);
  // The reason is reachable from the control itself, not only from a paragraph somewhere near it.
  expect(gold.getAttribute("title")).toMatch(/no mesh to shade/i);
  expect(screen.getByTestId("material-unavailable").textContent).toMatch(/no mesh to shade/i);

  fireEvent.click(gold);
  expect(setField).not.toHaveBeenCalled();

  // AND THE PRICED CONTROL TOO. `ai_edit` patches the same component through the same validator, so
  // it cannot land here either — an enabled button offering to spend two tokens on an edit that must
  // fail is `<ux_quality>` 6 exactly, and it was live until the `material-no-mesh` capture showed it
  // sitting under a greyed grid.
  const ai = screen.getByTestId("rustier") as HTMLButtonElement;
  expect(ai.disabled).toBe(true);
  expect(ai.getAttribute("title")).toMatch(/no mesh to shade/i);

  // Nothing reads as SET, either. A lit Default tile under a sentence saying no finish is possible
  // is two statements disagreeing in one column.
  expect(screen.getByTestId("material-default").getAttribute("aria-pressed")).toBe("false");
});

test("the AI suggestion is still here, below the free palette, and still priced", () => {
  selectShadeable();
  render(<MaterialPanel client={fakeClient()} />);
  const picker = screen.getByTestId("materialPicker");
  const ai = screen.getByTestId("aiEdit");
  expect(picker.contains(ai)).toBe(true);
  // The free grid comes FIRST in the reading order — the assisted, priced action is the fallback.
  expect(picker.querySelector(".mtk-swatch-grid")!.compareDocumentPosition(ai) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  expect(screen.getByTestId("rustier").textContent).toMatch(/tokens/);
});

test("every swatch names its action, so the grid is operable without seeing the spheres", () => {
  selectShadeable();
  render(<MaterialPanel client={fakeClient()} />);
  for (const id of ["default", "metal", "chrome", "gold", "copper", "rusty", "plastic"]) {
    const tile = screen.getByTestId(`material-${id}`);
    expect(tile.getAttribute("aria-label"), id).toMatch(/\w/);
    expect(tile.textContent, id).toMatch(/\w/);
  }
});
