//! **How the editor says what is selected** — one place, because it is said in four.
//!
//! The stage's status line, the toolbar's trigger, a toast after a marquee and a menu row all have to
//! answer "what am I acting on". Said four times they drift: one counts, one names, one prints the
//! raw loro key (`removed 1_4a3f` about a row reading `Weld Gun`), and the user has to work out that
//! all four mean the same thing. `<ux_quality>` 4 — plain language, no engine-internal ids in copy.

import { projectionStore } from "./projection";

/** What to call an object on screen.
 *
 *  A projection carries `name` for every entity, but an unnamed one carries its own id as its name —
 *  so `name !== id` is the test for "somebody named this", and the id is the honest fallback rather
 *  than an invented "Object 3" that matches nothing the user could search for. */
export function entityLabel(id: string): string {
  const summary = projectionStore.getState().summaries[id];
  return summary?.name && summary.name !== id ? summary.name : id;
}

/** What the selection is, in one clause.
 *
 *  One object is named, because the name is the useful fact and the count is obvious. More than one
 *  is counted, because eleven names is not a status line. Nothing selected says so plainly rather
 *  than going blank, which reads as the editor having lost track. */
export function selectionSentence(count: number, ids?: readonly string[]): string {
  if (count <= 0) return "nothing selected";
  if (count === 1) return ids?.[0] ? entityLabel(ids[0]) : "1 object selected";
  return `${count} objects selected`;
}
