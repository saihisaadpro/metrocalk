//! AiEditPanel (M10.10 / C3·C4) — verified headless in jsdom: the AI-edit suggestion only shows with a
//! selection, in PLAIN language (no "rustier" jargon visible to the user), and the spend is LEGIBLE +
//! DELIBERATE: a click opens a CONFIRM (price + the before/after), and only Apply charges — the new
//! balance + the change land (debit-on-success), surfaced as a toast (feedback at the gesture). A
//! refuse-when-broke is EXPLAINED and leaves the balance UNCHANGED. Asserts real behaviour (the client
//! called, the store balance, the toast/status), not "it rendered".
//!
//! ADR-164 narrowed this panel to the ONE action that needs a model. The six-finish palette it used to
//! carry — two tokens per click for a field write the core does for free — is `MaterialPanel`, and its
//! ABSENCE from here is asserted below.

import { afterEach, expect, test, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { AiEditPanel } from "./AiEditPanel";
import { projectionStore } from "../store/projection";
import { walletStore, setBalance } from "../store/wallet";
import { uiStore } from "../store/ui";
import { toastStore } from "../store/toasts";
import { fakeClient } from "../transport/test-client";
import type { EditorClient } from "../transport/session";
import type { EconResponse } from "../transport/protocol";

afterEach(() => {
  projectionStore.getState().reset();
  walletStore.getState().reset();
  uiStore.getState().setStatus("");
  toastStore.getState().reset();
});

/** An object the AI-edit can actually land on. `MeshRenderer` has to be THERE: the patch this panel
 *  applies is a `SetField` on that component, so an entity with no components at all — which is what
 *  this fixture used to be — is one the button now correctly refuses (ADR-164). */
function selectAnEntity() {
  projectionStore.getState().bulkLoad([
    { id: "e1", name: "Rover", parentId: null, components: { MeshRenderer: { mesh: "rover" } } },
  ]);
  projectionStore.getState().select("e1");
}

test("no selection → nothing to edit (the panel renders nothing)", () => {
  render(<AiEditPanel client={fakeClient()} />);
  expect(screen.queryByTestId("aiEdit")).toBeNull();
});

test("plain language — the suggestion describes the EFFECT, not the 'rustier' in-joke", () => {
  selectAnEntity();
  render(<AiEditPanel client={fakeClient()} />);
  const trigger = screen.getByTestId("rustier"); // stable id kept; copy is plain
  expect(trigger.textContent).toMatch(/weathered-metal look/i);
  expect(trigger.textContent).not.toMatch(/rustier/i);
});

test("legible + deliberate spend: click → confirm (price + before/after) → Apply debits + shows the result", async () => {
  selectAnEntity();
  setBalance(100);
  const aiEdit = vi.fn(() => Promise.resolve<EconResponse>({ ok: true, balance: 98, cost: 2, message: null }));
  render(<AiEditPanel client={fakeClient({ aiEdit })} />);

  // a single click does NOT spend — it opens a confirm showing the price + what changes (no debit yet)
  fireEvent.click(screen.getByTestId("rustier"));
  expect(aiEdit).not.toHaveBeenCalled();
  const confirm = screen.getByTestId("rustierConfirm");
  expect(confirm.textContent).toMatch(/~2 tokens/);
  expect(confirm.textContent).toMatch(/material/i); // the before/after preview

  // Apply → the charge lands (debit-on-success): the store balance + the toast/status name the cost
  fireEvent.click(screen.getByTestId("rustierApply"));
  // M11.2: the weathered-metal suggestion now names its preset explicitly ("rusty").
  await waitFor(() => expect(aiEdit).toHaveBeenCalledWith("e1", "rusty"));
  await waitFor(() => expect(walletStore.getState().balance).toBe(98));
  expect(uiStore.getState().status).toContain("−2");
  expect(toastStore.getState().toasts.some((t) => /applied/i.test(t.text))).toBe(true);
});

test("ADR-164: the free material palette is NOT here — this panel spends, and only the suggestion does", () => {
  selectAnEntity();
  render(<AiEditPanel client={fakeClient()} />);
  // The six-finish palette moved to `MaterialPanel`, where a pick is one free `setField`. Leaving a
  // copy behind would put two controls for the same field in the same section at two different prices.
  expect(screen.queryByTestId("materialPalette")).toBeNull();
  expect(screen.queryByTestId("material-chrome")).toBeNull();
  // What remains states its price before it is pressed.
  expect(screen.getByTestId("rustier").textContent).toMatch(/~2 tokens/);
});

test("the confirm shows an explicit before → after (the entity's CURRENT material → weathered metal)", () => {
  // M14.3: the AI card reads the real current material from the projection for an honest before/after.
  projectionStore.getState().bulkLoad([
    { id: "e1", name: "Sword", parentId: null, components: { MeshRenderer: { mesh: "sword", material: "gold" } } },
  ]);
  projectionStore.getState().select("e1");
  render(<AiEditPanel client={fakeClient()} />);
  fireEvent.click(screen.getByTestId("rustier"));
  const confirm = screen.getByTestId("rustierConfirm");
  // The BEFORE is the real current material, in the SAME words the picker above uses for it —
  // "Gold", not "gold" — so one material is never described two ways in one section (ADR-164).
  expect(confirm.textContent).toMatch(/Gold/);
  expect(confirm.textContent).toMatch(/weathered metal/i); // the AFTER
});

test("an object with no mesh REFUSES BEFORE the click — never a priced button that cannot land", () => {
  // The patch is a `SetField` on `MeshRenderer`, so `apply_ai_patch` rejects when the component is
  // absent. Offering to spend two tokens on that is `<ux_quality>` 6, and it was live until the
  // `material-no-mesh` capture put a greyed free grid directly above an enabled priced button.
  projectionStore.getState().bulkLoad([{ id: "e1", name: "Trigger", parentId: null, components: { Transform: { px: 0 } } }]);
  projectionStore.getState().select("e1");
  const aiEdit = vi.fn();
  render(<AiEditPanel client={fakeClient({ aiEdit })} />);

  const trigger = screen.getByTestId("rustier") as HTMLButtonElement;
  expect(trigger.disabled).toBe(true);
  expect(trigger.getAttribute("title")).toMatch(/no mesh to shade/i);
  fireEvent.click(trigger);
  expect(screen.queryByTestId("rustierConfirm")).toBeNull();
  expect(aiEdit).not.toHaveBeenCalled();
});

test("Cancel aborts the confirm — no spend", () => {
  selectAnEntity();
  const aiEdit = vi.fn();
  render(<AiEditPanel client={fakeClient({ aiEdit })} />);
  fireEvent.click(screen.getByTestId("rustier"));
  fireEvent.click(screen.getByTestId("rustierCancel"));
  expect(screen.queryByTestId("rustierConfirm")).toBeNull();
  expect(aiEdit).not.toHaveBeenCalled();
});

test("refuse-when-broke is EXPLAINED: !ok surfaces the message and the balance is UNCHANGED", async () => {
  selectAnEntity();
  setBalance(100);
  const refuse = vi.fn(() =>
    Promise.resolve<EconResponse>({ ok: false, balance: 0, cost: null, message: "insufficient balance" }),
  );
  const client: EditorClient = fakeClient({ aiEdit: refuse });
  render(<AiEditPanel client={client} />);

  fireEvent.click(screen.getByTestId("rustier"));
  fireEvent.click(screen.getByTestId("rustierApply"));

  await waitFor(() => expect(uiStore.getState().status).toBe("insufficient balance"));
  expect(refuse).toHaveBeenCalledTimes(1);
  // NO charge landed: the displayed balance is left exactly as it was.
  expect(walletStore.getState().balance).toBe(100);
});
