//! What copy, cut and paste SAY, and what each of them leaves selected (ADR-198).
//!
//! The properties no engine test can hold, because they are about the editor's side of the three
//! verbs: the SENTENCE, which five routes share so they cannot drift into five different claims; the
//! SELECTION, which a cut clears and a paste moves onto what it just put down; and the CLIPBOARD
//! LABEL, which exists because paste is the one verb whose subject is not on screen anywhere — by
//! the time it is used, the selection has moved on.

import { afterEach, expect, test, vi } from "vitest";
import { projectionStore } from "../store/projection";
import { uiStore } from "../store/ui";
import { clipboardSubject, copySelection, cutSelection, pasteClipboard } from "./clipboard";
import { fakeClient } from "../transport/test-client";

afterEach(() => {
  projectionStore.getState().reset();
  uiStore.getState().setClipboard({ objects: 0, parts: 0, cut: false, label: "" });
  vi.restoreAllMocks();
});

const named = (id: string, name: string) => ({ id, name, parentId: null, components: {} });

test("the parts count is said only when it adds something the object count does not", () => {
  // One assembly of forty parts is one object and forty-one entities. Saying only the first hides
  // what the transaction did; saying only the second reports "41" about a gesture on one row.
  expect(clipboardSubject(1, 1, "Weld Gun")).toBe("Weld Gun");
  expect(clipboardSubject(1, 41, "Weld Gun")).toBe("Weld Gun (41 parts)");
  expect(clipboardSubject(14, 14, "14 objects")).toBe("14 objects");
});

test("copy names one and counts many, and changes nothing about the selection", async () => {
  projectionStore.getState().bulkLoad([named("a", "Bolt"), named("b", "Nut")]);
  projectionStore.getState().setSelection(["a", "b"]);
  const client = fakeClient();

  const one = await copySelection(client, ["a"]);
  expect(one.sentence).toBe("copied Bolt");
  expect(uiStore.getState().clipboard.label).toBe("Bolt");

  const many = await copySelection(client, ["a", "b"]);
  expect(many.sentence).toBe("copied 2 objects");
  // A copy is a read. The selection it was made over is still the selection.
  expect(projectionStore.getState().multiSelect).toEqual(["a", "b"]);
  expect(client.selectEntities).not.toHaveBeenCalled();
});

test("cut dims exactly what the engine confirmed, clears BOTH selections, and marks the clipboard cut", async () => {
  projectionStore.getState().bulkLoad([named("a", "Bolt"), named("b", "Nut")]);
  projectionStore.getState().setSelection(["a", "b"]);
  const client = fakeClient();
  // The engine says which ids went. Dimming the list we sent would badge a row that is still there.
  client.cutSelection = vi.fn(() =>
    Promise.resolve({ objects: 2, parts: 5, nested: 0, missing: 0, gone: ["a"] }),
  );

  const outcome = await cutSelection(client, ["a", "b"]);

  expect(outcome).toEqual({ sentence: "cut 2 objects (5 parts) — Ctrl-Z to undo", ok: true });
  expect(projectionStore.getState().deactivated).toEqual({ a: true });
  expect(projectionStore.getState().multiSelect).toEqual([]);
  // …AND the engine's selection, or the renderer keeps outlining objects that are no longer there.
  expect(client.selectEntities).toHaveBeenCalledWith([]);
  expect(uiStore.getState().clipboard.cut).toBe(true);
});

test("paste names what was HELD, and the pasted objects become the selection", async () => {
  projectionStore.getState().bulkLoad([named("a", "Weld Gun")]);
  const client = fakeClient();
  await copySelection(client, ["a"]);
  client.pasteClipboard = vi.fn(() => Promise.resolve({ created: ["p1", "p2"], entities: 7 }));

  const outcome = await pasteClipboard(client);

  // The label comes from the COPY, because by paste time the source may be deselected, cut away, or
  // in another project — there is nothing on screen to read a name off.
  expect(outcome.sentence).toBe("pasted Weld Gun (7 parts) — Ctrl-Z to undo");
  // A paste is almost never the last step: the next drag must move what was just put down.
  expect(projectionStore.getState().multiSelect).toEqual(["p1", "p2"]);
  expect(client.selectEntities).toHaveBeenCalledWith(["p1", "p2"]);
});

test("every refusal says which fact refused it, and asks the engine nothing", async () => {
  const client = fakeClient();

  expect(await copySelection(client, [])).toEqual({ sentence: "Select an object to copy", ok: false });
  expect(client.copySelection).not.toHaveBeenCalled();
  expect(await cutSelection(client, [])).toEqual({ sentence: "Select an object to cut", ok: false });
  expect(client.cutSelection).not.toHaveBeenCalled();
  // An empty clipboard is not a failed paste: it is a state with a next step, and the row says it.
  expect(await pasteClipboard(client)).toEqual({
    sentence: "Copy or cut something first",
    ok: false,
    created: [],
  });
  expect(client.pasteClipboard).not.toHaveBeenCalled();

  // A selection the scene has lost is a different fact from a transaction that failed — one of them
  // is worth trying again (`<ux_quality>` 4).
  client.copySelection = vi.fn(() => Promise.resolve({ objects: 0, parts: 0, nested: 0, missing: 2 }));
  expect((await copySelection(client, ["a", "b"])).sentence).toBe("those objects are no longer in the scene");
  client.copySelection = vi.fn(() => Promise.resolve({ objects: 0, parts: 0, nested: 0, missing: 0 }));
  expect((await copySelection(client, ["a", "b"])).sentence).toBe("couldn't copy the selection");
});

test("a copy that took nothing does not throw away what the clipboard already held", async () => {
  projectionStore.getState().bulkLoad([named("a", "Bolt")]);
  const client = fakeClient();
  await copySelection(client, ["a"]);
  expect(uiStore.getState().clipboard.objects).toBe(1);

  // A stale selection under an open menu must not silently empty a clipboard a person filled a
  // minute ago — the refusal is about the gesture, not about what is held.
  client.copySelection = vi.fn(() => Promise.resolve({ objects: 0, parts: 0, nested: 0, missing: 1 }));
  await copySelection(client, ["gone"]);
  expect(uiStore.getState().clipboard.objects).toBe(1);
  expect(uiStore.getState().clipboard.label).toBe("Bolt");
});
