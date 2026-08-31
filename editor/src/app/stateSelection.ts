//! **Stating a selection — on BOTH sides, once.**
//!
//! A selection lives in two places that must agree: the store the Inspector, the outliner and the
//! status line read, and the engine model the 3D outline is drawn from. ADR-158 collapsed three
//! selections into one seam (`selectEntities`) precisely because a verb that moved only the store put
//! N objects in the Inspector and outlined none of them — and the outline is the one the user is
//! looking at.
//!
//! This is that pairing written once. It was inline in `App` and had exactly one caller; the second
//! caller (the outliner, selecting the matches of a search) is what turns "one place" from a
//! description into a rule.

import { projectionStore } from "../store/projection";
import { setStatus } from "../store/ui";
import type { EditorClient } from "../transport/session";

/**
 * Select exactly `ids`, everywhere, and say what happened.
 *
 * The store is set first and synchronously — the rows and the Inspector are what the user sees change
 * — and the engine follows. A failed engine call is logged rather than thrown: the alternative is an
 * unhandled rejection that leaves the two sides disagreeing with nothing said about it.
 */
export function stateSelection(client: EditorClient, ids: string[], sentence: string): void {
  projectionStore.getState().setSelection(ids);
  void client
    .selectEntities(ids)
    .catch((e) => console.error("selectEntities failed (engine selection may be out of sync)", e));
  setStatus(sentence);
}
