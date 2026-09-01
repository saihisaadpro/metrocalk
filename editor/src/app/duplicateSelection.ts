//! **Duplicating the selection — one function, because four routes reach it** (ADR-196).
//!
//! The same split `deleteSelection` made for the same reason (ADR-183). `Delete` was three routes
//! that had drifted into three different verbs; `Duplicate` was four routes that agreed with each
//! other and were all wrong the same way — every one of them cloned the PRIMARY and left the rest of
//! the selection alone, under copy that said "the selection". The authoring toolbar's description
//! said it out loud: *"Clone Weld Gun — the other 13 are left alone"*.
//!
//! What "duplicate" means here is the engine's `duplicate_selection`: every selected object, with
//! everything inside it, one undoable transaction, distinct names. This module owns the two things
//! the engine deliberately does not — **the sentence** (four presentations of one fact: a toast at
//! the gesture, a status line, a menu, a chord) and **what happens next**: the copies become the
//! selection, so the move that almost always follows a duplicate lands on the copy rather than
//! putting the original back where the copy already is.

import { projectionStore } from "../store/projection";
import { entityLabel } from "../store/selectionText";
import type { EditorClient } from "../transport/session";

/** What happened, in the caller's two channels: a sentence to say, and whether it went well. */
export interface DuplicateResult {
  sentence: string;
  ok: boolean;
  /** The copies, in the order the selection gave their sources. Empty on a refusal. */
  created: string[];
}

/**
 * Name what was made, in the vocabulary the rest of the editor uses (`selectionText`).
 *
 * A duplicate has TWO counts and they differ whenever anything has children — one selected assembly
 * of forty parts is one copy and forty-one entities. Saying only the first hides what the
 * transaction did to the document; saying only the second reports "41" about a gesture on one row.
 * So the parts count appears only when it says something the object count does not.
 */
export function duplicateSentence(sourceIds: string[], created: string[], entities: number): string {
  const what =
    created.length === 1
      ? entityLabel(sourceIds[sourceIds.length - 1] ?? created[0])
      : `${created.length} objects`;
  const inside = entities > created.length ? ` (${entities} parts)` : "";
  return `duplicated ${what}${inside} — Ctrl-Z to undo`;
}

/**
 * Duplicate `ids` as one undoable transaction, then select the copies.
 *
 * Returns the sentence rather than saying it: the toolbar routes it through its pending/status
 * machinery, the menu through a toast at the gesture, the chord through the status line — three
 * presentations of one fact, which is the split that keeps them from drifting.
 */
export async function duplicateSelection(client: EditorClient, ids: string[]): Promise<DuplicateResult> {
  if (!ids.length) {
    return { sentence: "Select an object to duplicate", ok: false, created: [] };
  }
  let outcome;
  try {
    outcome = await client.duplicateSelection(ids);
  } catch (error) {
    console.error("duplicate failed", error);
    return { sentence: "couldn't duplicate the selection", ok: false, created: [] };
  }
  if (!outcome.created.length) {
    // Every "no" explained (`<ux_quality>` 4). A selection that has gone stale under a menu is a
    // different fact from a transaction that failed, and only one of them is worth retrying.
    const sentence =
      outcome.missing === ids.length
        ? "those objects are no longer in the scene"
        : "couldn't duplicate the selection";
    return { sentence, ok: false, created: [] };
  }
  // Name it BEFORE the store changes — the sentence is about what was selected, and reading a label
  // out of a projection you have just told to point somewhere else is how a toast prints a raw id.
  const sentence = duplicateSentence(ids, outcome.created, outcome.entities);
  // THE COPIES ARE THE SELECTION. A duplicate is almost never the last step: the next gesture moves
  // what was just made, and leaving the originals selected means that gesture moves them instead —
  // silently, on top of the copy, which looks like nothing happened at all.
  projectionStore.getState().setSelection(outcome.created);
  // The ENGINE's selection too — a store-only change leaves the renderer outlining the originals,
  // the two-selections failure ADR-158 collapsed three of them to avoid.
  void client
    .selectEntities(outcome.created)
    .catch((error) => console.error("selectEntities failed (engine selection may be out of sync)", error));
  return { sentence, ok: true, created: outcome.created };
}
