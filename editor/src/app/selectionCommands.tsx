//! **The four selection verbs, as data** (ADR-176).
//!
//! Three of them have no gesture and no gesture could be invented for them — "everything", "nothing",
//! "the rest" are not shapes you can draw on a stage. The fourth asks a question about what the
//! objects ARE rather than where they are, which a rectangle cannot ask either. So the command palette
//! is not a convenience route to these; it is the only route, and whether the group reads as a group
//! is the whole of whether they are discoverable.
//!
//! WHY THEY LIVE IN THEIR OWN MODULE RATHER THAN INLINE IN `App` WITH THE OTHERS. Because they have to
//! be PHOTOGRAPHED. `App` builds its command list inside its own render, so a `shots` scene over these
//! rows would otherwise have to write out a copy of them — and a capture of a copy proves the copy is
//! legible, which is exactly the class of evidence this repository keeps finding to be worthless. One
//! exported function; the shell calls it, the scene calls it, and the picture is of the shipped rows.

import { projectionStore } from "../store/projection";
import type { EditorCommand } from "../panels/CommandPalette";
import { similarTo } from "./selectSimilar";

export interface SelectionCommandDeps {
  /** State the whole selection — both halves, the store the Inspector reads AND the engine model the
   *  picture is outlined from. A verb that moved only the store would put 378 objects in the Inspector
   *  and outline none of them, which is the failure ADR-158 collapsed three selections into one to
   *  make impossible. */
  apply(ids: string[], sentence: string): void;
  /** Say something without changing the selection — the refusal path. */
  say(sentence: string): void;
  /** Whether anything is selected, read reactively by the shell so the rows enable and disable with it. */
  hasSelection: boolean;
  /** Whether there is anything to select at all. */
  sceneEmpty: boolean;
}

/** Select every object. Its own function because two routes reach it — the palette row and the
 *  Ctrl/Cmd-A chord — and written twice they would drift the first time either sentence changed. The
 *  row is where a user LEARNS the chord, so a row that did something slightly different teaches a lie. */
export function selectAllWith(deps: Pick<SelectionCommandDeps, "apply">): () => void {
  return () => {
    const all = projectionStore.getState().order;
    deps.apply(all, all.length ? `Selected all ${all.length} objects` : "Nothing to select — the scene is empty");
  };
}

function selectSimilarWith(deps: Pick<SelectionCommandDeps, "apply" | "say">): () => void {
  return () => {
    const { displayed, order, selectedId } = projectionStore.getState();
    if (!selectedId) return;
    const match = similarTo(displayed, order, selectedId);
    // Every no explained (`<ux_quality>` 4): an object with neither geometry nor components gets a
    // reason, not an empty selection it would have to work out for itself.
    if (!match) {
      deps.say("Nothing to match on — this object has no geometry and no components");
      return;
    }
    deps.apply(
      match.ids,
      match.ids.length === 1
        ? `Only this one — nothing else ${match.reason}`
        : `Selected ${match.ids.length} objects ${match.reason}`,
    );
  };
}

/** The Selection category, in the order it reads: scope, scope, scope, then kind. */
export function selectionCommands(deps: SelectionCommandDeps): EditorCommand[] {
  return [
    {
      id: "select-all",
      label: "Select all",
      category: "Selection",
      shortcut: ["Ctrl", "A"],
      description: "Select every object in the scene",
      disabled: deps.sceneEmpty,
      disabledReason: "The scene is empty",
      execute: selectAllWith(deps),
    },
    {
      id: "select-none",
      label: "Select none",
      category: "Selection",
      description: "Clear the selection",
      disabled: !deps.hasSelection,
      disabledReason: "Nothing is selected",
      execute: () => deps.apply([], "Selection cleared"),
    },
    {
      id: "select-invert",
      label: "Invert selection",
      category: "Selection",
      description: "Select everything that is not selected now",
      disabled: deps.sceneEmpty,
      disabledReason: "The scene is empty",
      execute: () => {
        const { order, multiSelect } = projectionStore.getState();
        const inverted = order.filter((id) => !multiSelect.includes(id));
        deps.apply(
          inverted,
          inverted.length
            ? `Selected ${inverted.length} objects · inverted`
            : "Everything was selected — nothing left to invert to",
        );
      },
    },
    {
      id: "select-similar",
      label: "Select similar",
      category: "Selection",
      description: "Select every object of the same kind — the same geometry, or the same make-up",
      keywords: ["same", "matching", "all copies", "instances", "duplicates"],
      disabled: !deps.hasSelection,
      disabledReason: "Select an object first",
      execute: selectSimilarWith(deps),
    },
  ];
}
