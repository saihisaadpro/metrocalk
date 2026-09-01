//! What duplicating the selection says, and what it leaves selected (ADR-196).
//!
//! Two properties that no engine test can hold, because they are about the editor's side of the
//! verb: the SENTENCE, which four routes share so they cannot drift into four different claims, and
//! the SELECTION, which has to move onto the copies — a duplicate is almost never the last step, and
//! leaving the originals selected means the drag that follows moves the originals on top of the
//! copy, which looks exactly like nothing happened.

import { afterEach, expect, test, vi } from "vitest";
import { projectionStore } from "../store/projection";
import { duplicateSelection, duplicateSentence } from "./duplicateSelection";
import { fakeClient } from "../transport/test-client";

afterEach(() => {
  projectionStore.getState().reset();
  vi.restoreAllMocks();
});

const named = (id: string, name: string) => ({ id, name, parentId: null, components: {} });

test("one copy is NAMED, a set is counted, and the parts inside are said only when they add something", () => {
  projectionStore.getState().bulkLoad([named("1_4a3f", "Weld Gun")]);
  // One object, nothing inside it: the name is the useful fact and "1 part" would be noise.
  expect(duplicateSentence(["1_4a3f"], ["1_5000"], 1)).toBe("duplicated Weld Gun — Ctrl-Z to undo");
  // One object with forty parts under it: the second number is the whole reason the transaction is
  // bigger than the gesture, and it is the thing a person needs in order to trust Ctrl-Z.
  expect(duplicateSentence(["1_4a3f"], ["1_5000"], 41)).toBe("duplicated Weld Gun (41 parts) — Ctrl-Z to undo");
  // A set is counted rather than named — fourteen names is not a status line (`selectionText`).
  expect(duplicateSentence(["a", "b", "c"], ["x", "y", "z"], 3)).toBe("duplicated 3 objects — Ctrl-Z to undo");
});

test("the COPIES become the selection, in both halves", async () => {
  projectionStore.getState().bulkLoad([named("a", "Bolt"), named("b", "Nut")]);
  projectionStore.getState().setSelection(["a", "b"]);
  const client = fakeClient();

  const outcome = await duplicateSelection(client, ["a", "b"]);

  expect(outcome.ok).toBe(true);
  expect(outcome.created).toEqual(["a-copy", "b-copy"]);
  // The store the Inspector reads…
  expect(projectionStore.getState().multiSelect).toEqual(["a-copy", "b-copy"]);
  // …AND the engine model the picture is outlined from. A store-only move leaves the renderer
  // outlining the originals (ADR-158).
  expect(client.selectEntities).toHaveBeenCalledWith(["a-copy", "b-copy"]);
});

test("nothing selected is a refusal that says what to do, and asks the engine nothing", async () => {
  const client = fakeClient();
  const outcome = await duplicateSelection(client, []);
  expect(outcome).toEqual({ sentence: "Select an object to duplicate", ok: false, created: [] });
  expect(client.duplicateSelection).not.toHaveBeenCalled();
});

test("a selection the scene has lost is told apart from a transaction that failed", async () => {
  const client = fakeClient();
  // Every "no" explained (`<ux_quality>` 4) — one of these is worth retrying and the other is not.
  client.duplicateSelection = vi.fn(() => Promise.resolve({ created: [], entities: 0, nested: 0, missing: 2 }));
  expect((await duplicateSelection(client, ["a", "b"])).sentence).toBe("those objects are no longer in the scene");

  client.duplicateSelection = vi.fn(() => Promise.resolve({ created: [], entities: 0, nested: 0, missing: 0 }));
  expect((await duplicateSelection(client, ["a", "b"])).sentence).toBe("couldn't duplicate the selection");
});

test("a rejected call is reported, not thrown at the caller, and the selection does not move", async () => {
  projectionStore.getState().bulkLoad([named("a", "Bolt")]);
  projectionStore.getState().setSelection(["a"]);
  const client = fakeClient();
  client.duplicateSelection = vi.fn(() => Promise.reject(new Error("engine gone")));
  vi.spyOn(console, "error").mockImplementation(() => {});

  const outcome = await duplicateSelection(client, ["a"]);

  expect(outcome.ok).toBe(false);
  // Still on the original: a failed duplicate that also cleared the selection would cost the user
  // the set they had built as well as the copy they did not get.
  expect(projectionStore.getState().multiSelect).toEqual(["a"]);
});
