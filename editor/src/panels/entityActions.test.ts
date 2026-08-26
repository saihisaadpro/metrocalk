//! The shared entity-action dispatch — the ONE place `actions_for`'s result becomes engine calls.
//!
//! Two of these three cases are regressions for defects the extraction closed, not descriptions of
//! new behaviour: the silent `default: return` and the raw-id toast.

import { beforeEach, expect, test, vi } from "vitest";
import { projectionStore } from "../store/projection";
import { toastStore } from "../store/toasts";
import { uiStore } from "../store/ui";
import type { ActionItem } from "../transport/protocol";
import type { EditorClient } from "../transport/session";
import { plainReason, runEntityAction } from "./entityActions";

const action = (over: Partial<ActionItem> & Pick<ActionItem, "action" | "label">): ActionItem => ({
  available: true,
  mutates: false,
  ...over,
});

beforeEach(() => {
  toastStore.getState().reset();
  projectionStore.getState().bulkLoad([
    { id: "8f21c4", name: "Weld Gun 7", parentId: null, components: {} },
  ] as never);
});

test("a refusal never reaches the engine, whichever surface offered it", () => {
  const removeEntity = vi.fn();
  const client = { removeEntity } as unknown as EditorClient;
  runEntityAction(client, action({ action: "remove", label: "Remove", available: false, reason: "gone" }), "8f21c4");
  expect(removeEntity).not.toHaveBeenCalled();
  expect(toastStore.getState().toasts).toHaveLength(0);
});

test("the message names the OBJECT, not the entity id", () => {
  const client = { removeEntity: vi.fn() } as unknown as EditorClient;
  runEntityAction(client, action({ action: "remove", label: "Remove", mutates: true }), "8f21c4");

  const [toast] = toastStore.getState().toasts;
  expect(toast.text).toBe("Removed Weld Gun 7 · Ctrl-Z to undo");
  expect(uiStore.getState().status).toBe("Removed Weld Gun 7 · Ctrl-Z to undo");
  // The defect this replaced: the id was interpolated straight into the sentence, so after a CAD
  // import every confirmation named a hex string the outliner had been rebuilt to stop showing.
  expect(toast.text).not.toContain("8f21c4");
});

test("an action the engine offers and this build cannot run SAYS SO, rather than doing nothing", () => {
  // The `switch` this replaced ended in `default: return` — no toast, no status, not even the menu
  // closing. `Action` is a Rust enum serialized to a string, so a seventh variant compiles cleanly on
  // both sides and arrives here as an enabled row that silently does nothing when clicked. This is
  // the only assertion that can fail if that `return` ever comes back.
  const error = vi.spyOn(console, "error").mockImplementation(() => {});
  const client = {} as unknown as EditorClient;

  runEntityAction(client, action({ action: "explode" as ActionItem["action"], label: "Explode" }), "8f21c4");

  const [toast] = toastStore.getState().toasts;
  expect(toast.kind).toBe("error");
  expect(toast.text).toContain("Explode");
  expect(error).toHaveBeenCalled();
  error.mockRestore();
});

test("plainReason replaces the capability-graph sentence and passes anything else through", () => {
  expect(plainReason("no unmet requirement to bind")).toBe(
    "nothing to bind yet — this object already has what it needs",
  );
  expect(plainReason("entity no longer exists")).toBe("entity no longer exists");
});
