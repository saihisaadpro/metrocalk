//! The ONE place an entity action becomes engine calls — shared by the right-click menu and the
//! command palette.
//!
//! WHY IT WAS EXTRACTED. `ContextMenu` owned a `switch` over `action.action` and was the only surface
//! that could reach `actions_for` at all, so the engine's flagship query — "what can I do to this
//! object?", the M3.3 action model, the half of the north star that is *click anything → see what
//! fits* — was reachable by exactly one gesture, on one surface, with the dispatch written inline in
//! that surface's component. Offering the same actions in the palette meant either importing a menu
//! or writing the `switch` a second time, and a contract stated twice in two places is the drift this
//! repository gates for everywhere else.
//!
//! AND THE `switch` ENDED IN `default: return`. Six variants in, six variants handled — true today,
//! and true only because nobody has added a seventh. `Action` is a Rust enum serialized to a
//! lowercase string (`editor-shell/src/actions.rs`); adding a variant compiles cleanly on both sides,
//! renders as an enabled row the engine says is available, and does nothing at all when clicked — no
//! toast, no status, not even the menu closing. That is `<ux_quality>` 6 ("an enabled button always
//! does something or says why it can't") failing silently, in the surface whose entire subject is
//! what an object can do. The default case below reports instead of returning.

import { projectionStore } from "../store/projection";
import { setStatus } from "../store/ui";
import { pushToast, type ToastKind } from "../store/toasts";
import type { ActionItem } from "../transport/protocol";
import type { EditorClient } from "../transport/session";

/** The engine's vocabulary, lowercase exactly as `Action` serializes it. Listed so a surface can say
 *  what it knows how to run; the dispatch below is still the only thing that runs it. */
export const ENTITY_ACTIONS = ["bind", "remove", "duplicate", "focus", "inspect", "makedynamic"] as const;

/** What an object is CALLED, for a sentence a user reads.
 *
 *  The dispatch used to interpolate the raw entity id — "removed 8f21c4 · Ctrl-Z to undo". That is a
 *  sentence about the engine's bookkeeping, and after a CAD import the ids are exactly the hex the
 *  outliner was rebuilt to stop showing (ADR-077's folder tree exists because "QA › Machine Cabinet ›
 *  Weld Gun 7" is the name and the id is not). The projection already holds the name; the toast is
 *  the one place it was not being read. */
export function entityLabel(id: string): string {
  return projectionStore.getState().summaries[id]?.name?.trim() || id;
}

/** Soften engine-internal rejection language into concise user-facing guidance.
 *
 *  Shared for the same reason the dispatch is. This lived in `ContextMenu`, so the moment the command
 *  palette started offering the same actions it would have shown "no unmet requirement to bind" — an
 *  engine sentence about the capability graph — in the surface a newcomer reaches for first. That is
 *  `<ux_quality>` 4 exactly ("no engine-internal jargon in user copy; describe the EFFECT"), and it
 *  would have arrived as a side effect of reusing the query rather than as anyone's decision. */
export function plainReason(reason: string): string {
  if (/no unmet requirement to bind/i.test(reason)) {
    return "nothing to bind yet — this object already has what it needs";
  }
  return reason;
}

/** Raise a message where the user acted and in the status line — `<ux_quality>` 2. */
function announce(message: string, kind: ToastKind = "info") {
  setStatus(message);
  pushToast(message, kind);
}

export interface RunEntityActionOptions {
  /** After framing, the live camera distance so the caller can raise the focus banner. */
  onFocus?: (id: string, distance: number) => void;
}

/** Run one action from `actions_for` against `id`. Unavailable actions are refused here as well as
 *  greyed in the surface, so a caller that forgets to check cannot mutate the scene. */
export function runEntityAction(
  client: EditorClient,
  action: ActionItem,
  id: string,
  { onFocus }: RunEntityActionOptions = {},
): void {
  if (!action.available) return;
  const what = entityLabel(id);
  switch (action.action) {
    case "remove":
      client.removeEntity(id);
      announce(`Removed ${what} · Ctrl-Z to undo`, "info");
      break;
    case "duplicate":
      void client
        .duplicateEntity(id)
        .then((newId) =>
          announce(
            newId ? `Duplicated ${what} · Ctrl-Z to undo` : `Couldn't duplicate ${what}`,
            newId ? "success" : "error",
          ),
        )
        .catch((error) => {
          console.error("duplicate failed", error);
          announce(`Couldn't duplicate ${what}`, "error");
        });
      break;
    case "focus":
      client.focusEntity(id);
      void client
        .focusDebug()
        .then(([distance]) => onFocus?.(id, distance))
        .catch(() => onFocus?.(id, 0));
      announce(`Focused ${what}`, "info");
      break;
    case "inspect":
      select(client, id);
      announce(`Inspecting ${what}`, "info");
      break;
    case "bind":
      select(client, id);
      announce(`Binding ${what}`, "info");
      break;
    case "makedynamic":
      void client
        .makeDynamic(id)
        .then((ok) =>
          announce(ok ? `${what} is now dynamic · Ctrl-Z to undo` : `Couldn't make ${what} dynamic`, ok ? "success" : "error"),
        )
        .catch((error) => {
          console.error("make_dynamic failed", error);
          announce(`Couldn't make ${what} dynamic`, "error");
        });
      break;
    default:
      // NEVER SILENT. See the header: the previous `return` here made an engine action nothing in the
      // UI could run indistinguishable from one that worked. The label is the engine's own, so the
      // message names the thing the user actually clicked.
      console.error(`no dispatch for entity action ${JSON.stringify(action.action)}`);
      announce(`"${action.label}" is offered by the engine but this build cannot run it yet`, "error");
      break;
  }
}

/** Select in the projection AND in the engine — the two must not drift, and every caller wanted both. */
function select(client: EditorClient, id: string): void {
  projectionStore.getState().select(id);
  void client
    .gizmoSelect(id)
    .catch((error) => console.error("gizmoSelect failed (engine selection may be out of sync)", error));
}
