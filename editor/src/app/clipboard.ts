//! **Copy, Cut and Paste over the selection — one module, because five routes reach each of them**
//! (ADR-198).
//!
//! The third and last split of this shape, after `deleteSelection` (ADR-183) and `duplicateSelection`
//! (ADR-196), and the same finding underneath all three: a verb written once per surface drifts, and a
//! verb written to act on the PRIMARY says "the selection" everywhere it appears. `Copy` and `Cut`
//! were the last two in the authoring toolbar still doing it — their own descriptions said so, *"Copy
//! Weld Gun — one subtree at a time"* — and `Paste` had a different problem: it worked on whatever one
//! subtree had been taken, and put it down **exactly on top of the object it came from**.
//!
//! What the three verbs mean here is the engine's `copy_selection` / `cut_selection` /
//! `paste_clipboard`: every selected object with everything inside it, one undoable transaction, the
//! capabilities and the names carried across, and a paste that lands somewhere a person can see.
//!
//! This module owns the two things the engine deliberately does not — **the sentence** (five
//! presentations of one fact: a toast at the gesture, a status line, a toolbar row, a menu row, a
//! chord) and **what happens next**: a cut clears the selection it just deactivated, and a paste makes
//! the pasted objects the selection, so the drag that follows moves what was just put down.

import { projectionStore } from "../store/projection";
import { setClipboard, uiStore, type ClipboardInfo } from "../store/ui";
import { entityLabel } from "../store/selectionText";
import type { EditorClient } from "../transport/session";

/** What happened, in the caller's two channels: a sentence to say, and whether it went well. */
export interface ClipboardResult {
  sentence: string;
  ok: boolean;
}

/** A paste additionally reports what it made, because that is what the caller selects. */
export interface PasteResult extends ClipboardResult {
  created: string[];
}

/**
 * Name a set the way the rest of the editor names one, with the parts count added only when it says
 * something the object count does not.
 *
 * The same two-count rule `duplicateSentence` states: one selected assembly of forty parts is one
 * object and forty-one entities, and reporting only the second is "41" about a gesture on one row.
 */
export function clipboardSubject(objects: number, parts: number, label: string): string {
  const inside = parts > objects ? ` (${parts} parts)` : "";
  return `${label}${inside}`;
}

/** What to call what a gesture is about: the object's name for one, the count for many. */
function subjectLabel(ids: string[], objects: number): string {
  return objects === 1 ? entityLabel(ids[ids.length - 1] ?? ids[0] ?? "") : `${objects} objects`;
}

/**
 * Copy `ids` to the clipboard. A pure read — nothing in the document changes, and the selection
 * stays exactly where it is.
 */
export async function copySelection(client: EditorClient, ids: string[]): Promise<ClipboardResult> {
  if (!ids.length) return { sentence: "Select an object to copy", ok: false };
  let outcome;
  try {
    outcome = await client.copySelection(ids);
  } catch (error) {
    console.error("copy failed", error);
    return { sentence: "couldn't copy the selection", ok: false };
  }
  if (!outcome.objects) {
    // Every "no" explained (`<ux_quality>` 4): a selection that went stale under an open menu is a
    // different fact from a copy that failed, and only one of them is worth trying again.
    const sentence =
      outcome.missing === ids.length ? "those objects are no longer in the scene" : "couldn't copy the selection";
    return { sentence, ok: false };
  }
  const label = subjectLabel(ids, outcome.objects);
  rememberClipboard(outcome, label, false);
  return { sentence: `copied ${clipboardSubject(outcome.objects, outcome.parts, label)}`, ok: true };
}

/**
 * Cut `ids`: copy them, then deactivate the sources as ONE undoable transaction. Non-destructive
 * both ways — the data is on the clipboard *and* recoverable with Ctrl-Z.
 */
export async function cutSelection(client: EditorClient, ids: string[]): Promise<ClipboardResult> {
  if (!ids.length) return { sentence: "Select an object to cut", ok: false };
  let outcome;
  try {
    outcome = await client.cutSelection(ids);
  } catch (error) {
    console.error("cut failed", error);
    return { sentence: "couldn't cut the selection", ok: false };
  }
  if (!outcome.objects || !outcome.gone.length) {
    const sentence =
      outcome.missing === ids.length ? "those objects are no longer in the scene" : "couldn't cut the selection";
    return { sentence, ok: false };
  }
  // Name it BEFORE the store changes — the sentence is about what was there, and reading a label out
  // of a projection you have just told to forget is how a toast prints a raw loro key.
  const label = subjectLabel(ids, outcome.objects);
  const sentence = `cut ${clipboardSubject(outcome.objects, outcome.parts, label)} — Ctrl-Z to undo`;
  rememberClipboard(outcome, label, true);
  projectionStore.getState().markDeactivated(outcome.gone);
  projectionStore.getState().setSelection([]);
  // The ENGINE's selection too. A store-only clear leaves the renderer outlining objects that are no
  // longer there — the two-selections failure ADR-158 collapsed three selections into one to avoid.
  void client
    .selectEntities([])
    .catch((error) => console.error("selectEntities failed (engine selection may be out of sync)", error));
  return { sentence, ok: true };
}

/**
 * Paste the clipboard as ONE undoable transaction, then select what it made.
 *
 * The copies become the selection for the same reason a duplicate's do: a paste is almost never the
 * last step, and leaving the previous selection in place means the drag that follows moves the wrong
 * objects — silently, which looks exactly like nothing happened.
 */
export async function pasteClipboard(client: EditorClient): Promise<PasteResult> {
  const held = uiStore.getState().clipboard;
  if (!held.objects) return { sentence: "Copy or cut something first", ok: false, created: [] };
  let outcome;
  try {
    outcome = await client.pasteClipboard();
  } catch (error) {
    console.error("paste failed", error);
    return { sentence: "couldn't paste the clipboard", ok: false, created: [] };
  }
  if (!outcome.created.length) {
    return { sentence: "couldn't paste the clipboard", ok: false, created: [] };
  }
  projectionStore.getState().setSelection(outcome.created);
  void client
    .selectEntities(outcome.created)
    .catch((error) => console.error("selectEntities failed (engine selection may be out of sync)", error));
  return {
    sentence: `pasted ${clipboardSubject(outcome.created.length, outcome.entities, held.label)} — Ctrl-Z to undo`,
    ok: true,
    created: outcome.created,
  };
}

/** Record what is now held, so Paste can be enabled AND can name what it would paste. */
function rememberClipboard(outcome: { objects: number; parts: number }, label: string, cut: boolean): void {
  const info: ClipboardInfo = { objects: outcome.objects, parts: outcome.parts, cut, label };
  setClipboard(info);
}
