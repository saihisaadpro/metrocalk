//! ContextMenu (M3.3) — verified headless in jsdom: an entity's registry-derived actions render, an
//! unavailable action is greyed WITH its reason (every "no" explained), an AVAILABLE row dispatches the
//! right contract verb + closes, and a DISABLED row is inert (no dispatch, no close). Asserts REAL behavior
//! (the right client method called with the right id, the reason rendered, the menu closed), not "it rendered".

import { afterEach, expect, test, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { ContextMenu } from "./ContextMenu";
import { projectionStore } from "../store/projection";
import { fakeClient } from "../transport/test-client";
import type { ActionItem } from "../transport/protocol";

afterEach(() => projectionStore.getState().reset());

const ACTIONS: ActionItem[] = [
  { action: "remove", label: "Remove", available: true, mutates: true },
  { action: "bind", label: "Bind…", available: false, reason: "no unmet requirement", mutates: false },
];

test("actions render; an unavailable action is greyed WITH its reason; an available row dispatches + closes; a disabled row is inert", async () => {
  const removeEntity = vi.fn();
  const onClose = vi.fn();
  const client = fakeClient({
    entityActions: () => Promise.resolve(ACTIONS),
    removeEntity,
  });

  render(<ContextMenu client={client} id="e1" onClose={onClose} />);

  // (a) both rows render
  const rows = await screen.findAllByTestId("ctxitem");
  expect(rows).toHaveLength(2);

  const removeRow = rows.find((r) => r.dataset.action === "remove")!;
  const bindRow = rows.find((r) => r.dataset.action === "bind")!;
  expect(removeRow).toBeTruthy();
  expect(bindRow).toBeTruthy();

  // (b) the bind row is disabled AND its text carries the reason (every "no" explained)
  expect(bindRow.className).toContain("disabled");
  expect(bindRow.textContent).toContain("Bind…");
  expect(bindRow.textContent).toContain("no unmet requirement");
  // the available row is NOT disabled and shows only its label
  expect(removeRow.className).not.toContain("disabled");
  expect(removeRow.textContent).toBe("Remove");

  // (d) clicking the DISABLED Bind row does NOT dispatch and does NOT close
  fireEvent.click(bindRow);
  expect(onClose).not.toHaveBeenCalled();
  expect(projectionStore.getState().selectedId).toBeNull(); // bind would have select()ed

  // (c) clicking the AVAILABLE Remove row → client.removeEntity("e1") + onClose
  fireEvent.click(removeRow);
  expect(removeEntity).toHaveBeenCalledTimes(1);
  expect(removeEntity).toHaveBeenCalledWith("e1");
  expect(onClose).toHaveBeenCalledTimes(1);
});

test("focus action routes to client.focusEntity(id) and closes", async () => {
  const focusEntity = vi.fn();
  const onClose = vi.fn();
  const client = fakeClient({
    entityActions: () => Promise.resolve([{ action: "focus", label: "Focus", available: true, mutates: false }]),
    focusEntity,
  });

  render(<ContextMenu client={client} id="e7" onClose={onClose} />);
  const row = await screen.findByTestId("ctxitem");
  fireEvent.click(row);

  expect(focusEntity).toHaveBeenCalledWith("e7");
  expect(onClose).toHaveBeenCalledTimes(1);
});
