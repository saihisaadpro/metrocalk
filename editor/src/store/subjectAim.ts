//! **Aiming a shot by pointing at the thing** — the session state for "click it in the viewport".
//!
//! WHAT WAS MISSING, AND WHERE IT ALREADY EXISTED. Three capabilities had already crossed the
//! boundary and never met: `viewport_peek` answers "what is under the cursor" WITHOUT touching the
//! selection (written for M3.3 hover and called by nothing in `editor/src` since — one of the dark
//! commands `command-reachability-audit` counts), `cinema_subject_catalog` ranks the scene's own
//! hierarchy around any object with a DRAWN-PARTS count on every row, and `cinema_set_shot_subject`
//! re-aims a shot as one undoable transaction. What a user could reach was a search box: in a
//! 15,711-part import, "film THAT one" meant knowing its name.
//!
//! WHY THE SELECTION IS NOT THE ANSWER. The obvious gesture — select the object, press "frame the
//! selection" — cannot work here, because the Cutscene panel is bound to the editor selection:
//! selecting the thing you want to film SWITCHES WHICH CUTSCENE IS ON SCREEN, and the shot you were
//! aiming is gone. So the aim reads the viewport through the non-mutating peek and leaves the
//! selection exactly where it was. That is the whole reason `viewport_peek` exists and the reason
//! this mode is a mode at all.
//!
//! WHY A STORE. The mode is STARTED in the bottom dock (the shot's Frames picker) and LIVED on the
//! stage (the badge, the pointer), which are different parts of the window — the same split
//! `cinemaPreview` exists for, and a second copy of "is an aim in flight" is a second thing that can
//! be wrong. The panel owns the COMMIT: a choice made on the stage lands here as `picked`, and the
//! panel commits it through the same one-undo path its own list uses, so a re-aim by clicking and a
//! re-aim by list are the same transaction and the same Ctrl-Z.

import { createStore } from "zustand/vanilla";
import { useStore } from "zustand";

/** One rung of the ladder under the cursor: the object itself, then what it is part of, outward.
 *
 *  THE LADDER IS THE POINT. A click on a CAD assembly lands on a LEAF — one bolt of a weld gun of a
 *  production line — and "film the bolt" is almost never the shot. The rungs are the same
 *  `SubjectCandidate` rows the picker draws, filtered to the object and its ancestors, so one click
 *  takes the part you pointed at and one click takes the machine it belongs to. */
export interface AimRung {
  /** The entity key `cinemaSetShotSubject` takes. */
  id: string;
  /** What the outliner calls it. */
  name: string;
  /** How many DRAWN parts sit under it — the number that tells a 378-part assembly apart from the
   *  one bracket sharing most of its name. Counted by the engine off the published render list. */
  parts: number;
  /** The engine's own heading for this rung ("This object" / "What it is part of"), never a word the
   *  editor invented for a rank the engine did not produce. */
  group: string;
}

export interface SubjectAimInfo {
  /** An aim is in flight: the next left click on the stage aims a shot instead of selecting. */
  active: boolean;
  /** The cutscene's own object — which panel owns this aim, so a different selection cannot commit it. */
  owner: string | null;
  /** Which shot is being aimed, 0-based. */
  shotIndex: number | null;
  /** How many shots the cutscene holds, so the badge can say "shot 2 of 5". */
  shots: number;
  /** The ladder under the cursor, nearest first. Empty when the cursor is over nothing drawn. */
  rungs: AimRung[];
  /** A peek is in flight — the badge says "looking" rather than claiming there is nothing there. */
  looking: boolean;
  /** The choice, handed to the panel that started the aim. `seq` makes a repeat of the same subject
   *  a new event: aiming twice at the same object is two gestures, and an effect keyed on the id
   *  alone would silently swallow the second. */
  picked: { subject: string; name: string; seq: number } | null;
}

const OFF: SubjectAimInfo = {
  active: false,
  owner: null,
  shotIndex: null,
  shots: 0,
  rungs: [],
  looking: false,
  picked: null,
};

interface SubjectAimState extends SubjectAimInfo {
  /** A counter that NEVER resets — not on a begin, not on a cancel. It is what `taken` checks, and a
   *  per-aim counter would restart at 1 every time: a cleanup from the previous aim, arriving late,
   *  would then consume the NEXT aim's choice and the shot would silently keep its old subject. */
  sequence: number;
  /** Begin aiming one shot of one cutscene. */
  begin(owner: string, shotIndex: number, shots: number): void;
  /** What the cursor is over now — the object and its ancestors, or nothing. */
  hover(rungs: AimRung[]): void;
  /** A peek is in flight. Named apart from the `looking` FLAG it sets, because a zustand store is
   *  one object and a field cannot also be its own setter. */
  look(looking: boolean): void;
  /** Take this object. Ends the mode and leaves the choice for the panel to commit. */
  pick(subject: string, name: string): void;
  /** The panel has committed (or refused) this choice. */
  taken(seq: number): void;
  /** Esc, a Cancel, Play starting, or the panel going away. */
  cancel(): void;
}

export const subjectAimStore = createStore<SubjectAimState>((set, get) => ({
  ...OFF,
  sequence: 0,
  begin: (owner, shotIndex, shots) =>
    set({ ...OFF, active: true, owner, shotIndex, shots }),
  hover: (rungs) => set({ rungs, looking: false }),
  look: (looking) => set({ looking }),
  pick: (subject, name) => {
    const state = get();
    if (!state.active) return;
    // The mode ends at the CHOICE, not at the commit. The stage stops intercepting clicks the
    // instant the user has said what to film; the panel's own busy state carries the rest, which is
    // where every other cutscene edit already shows it.
    const seq = state.sequence + 1;
    set({ active: false, rungs: [], looking: false, picked: { subject, name, seq }, sequence: seq });
  },
  taken: (seq) => {
    if (get().picked?.seq !== seq) return;
    set({ picked: null });
  },
  cancel: () => set(OFF),
}));

/** The whole aim state. Selector-free for the same reason `useCinemaPreview` is: a selector building
 *  an object literal returns a new reference every render, and zustand v5 compares with `Object.is`.
 *  This state object's identity changes on a begin, a hover settle, a pick or a cancel — never per
 *  frame. */
export const useSubjectAim = (): SubjectAimInfo => useStore(subjectAimStore);
