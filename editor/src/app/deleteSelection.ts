//! **Deleting the selection — one function, because three routes reach it.**
//!
//! The authoring toolbar's `Delete` row, the right-click menu's `Delete` row and the Delete key are
//! three ways to ask for one thing, and written three times they drift: one deactivates the whole
//! selection, one destroys a single entity, and the third does not exist. That was literally the
//! state of it (ADR-183) — the toolbar batched, the context menu called M3.3's destructive
//! `remove_entity` on the primary, and **the most-pressed key in any editor was not bound at all**.
//!
//! What "delete" means here is ADR-026's non-destructive one: deactivate, free every dependent, ONE
//! undoable transaction over the whole set. The engine reports which ids actually went, so the rows
//! that dim are the rows that changed rather than the list the caller happened to send.

import { projectionStore } from "../store/projection";
import { entityLabel } from "../store/selectionText";
import type { EditorClient } from "../transport/session";

/** What happened, in the caller's two channels: a sentence to say, and whether it went well. */
export interface DeleteOutcome {
  sentence: string;
  ok: boolean;
  /** The ids the engine confirmed. Empty on a refusal. */
  gone: string[];
}

/**
 * Delete `ids` as one undoable transaction and reconcile both halves of the selection.
 *
 * Returns the sentence rather than saying it: the toolbar routes it through its pending/status
 * machinery, the menu through a toast at the gesture, and the key through the status line — three
 * presentations of one fact, which is the split that keeps them from drifting.
 */
export async function deleteSelection(client: EditorClient, ids: string[]): Promise<DeleteOutcome> {
  if (!ids.length) {
    return { sentence: "Nothing is selected", ok: false, gone: [] };
  }
  let gone: string[];
  try {
    gone = await client.deleteDeactivateMany(ids);
  } catch (error) {
    console.error("delete failed", error);
    return { sentence: "couldn't delete the selection", ok: false, gone: [] };
  }
  if (!gone.length) {
    return { sentence: "couldn't delete the selection", ok: false, gone: [] };
  }
  // Name it BEFORE the store changes: the sentence is about what was there, and reading a label out
  // of a projection you have just told to forget is how a toast ends up printing a raw loro key.
  const what = gone.length === 1 ? entityLabel(gone[0]) : `${gone.length} objects`;
  projectionStore.getState().markDeactivated(gone);
  projectionStore.getState().setSelection([]);
  // The ENGINE's selection too. A store-only clear leaves the renderer outlining objects that are no
  // longer there — the same two-selections failure ADR-158 collapsed three of them to avoid.
  void client.selectEntities([]).catch((error) =>
    console.error("selectEntities failed (engine selection may be out of sync)", error),
  );
  return { sentence: `deleted ${what} — recoverable with Ctrl-Z`, ok: true, gone };
}
