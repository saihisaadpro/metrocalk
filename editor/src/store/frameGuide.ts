//! ADR-193 — the **frame guide** store: is the stage drawing the delivery frame, and which one.
//!
//! THE GAP IT CLOSES. `cinema_set_shot_camera` films "exactly the view on the stage". Until this
//! existed, the view on the stage was composed for whatever shape the docks had left it — so an
//! author framing a 2.39:1 shot on a 1.55 stage was composing a picture nothing would ever deliver,
//! and the first time they saw the real frame was after the shot was stored.
//!
//! TWO FACTS, AND THEY ARE NOT THE SAME FACT.
//!
//! * `wanted` is the AUTHOR'S standing answer to "show me the frame while I fly the camera". It
//!   persists across sessions, because it is a working preference and not a property of any project.
//! * `drawn` is what the stage is ACTUALLY showing right now, which is `wanted` narrowed by the
//!   things that can make a guide meaningless: no cutscene selected, a cutscene delivered to
//!   "match viewport" (the absence of a delivery frame), or a preview already holding the camera.
//!
//! Keeping them apart is what lets the stage badge be honest. A badge driven by `wanted` would claim
//! a guide on a stage that is not drawing one; a preference driven by `drawn` would forget the
//! author's answer every time they deselected. And the badge exists at all for the reason the
//! PREVIEW badge does: the control that turns the guide on lives in the bottom dock, which the author
//! may have closed — so the way OUT has to be on the stage, where the bars are.

import { createStore } from "zustand/vanilla";
import { useStore } from "zustand";
import type { DeliveryFrame } from "./../transport/protocol";

/** Where the author's standing answer is kept between sessions. */
const PREF_KEY = "mtk.frameGuide";

/** The author's last answer, or `true` — a guide is the useful default for the same reason a
 *  viewfinder is: the cost of seeing the frame you are composing for is a dimmed margin, and the
 *  cost of not seeing it is a shot that has to be re-taken. Storage can be absent or locked down
 *  (a private window, a hardened WebView), and a preference that throws on read would take the
 *  panel down with it. */
function readWanted(): boolean {
  try {
    return window.localStorage.getItem(PREF_KEY) !== "off";
  } catch {
    return true;
  }
}

function writeWanted(on: boolean): void {
  try {
    window.localStorage.setItem(PREF_KEY, on ? "on" : "off");
  } catch {
    // A locked-down WebView still keeps the preference for this session.
  }
}

/** The frame the stage is drawing, named the way the ENGINE names it.
 *
 *  The label travels with the key rather than being looked up here, because the list of delivery
 *  frames and their names is `Delivery::label()` in the animation crate and reaches the editor in the
 *  framing catalog. A second table of names in the front end is a second table to drift — and it
 *  would drift silently, since a badge reading "16:9" over a scope guide is wrong in a way nothing
 *  fails on. The panel has the catalog; it passes what it read. */
export interface DrawnFrame {
  key: DeliveryFrame;
  label: string;
}

export interface FrameGuideState {
  /** The author's standing answer: draw the delivery frame while I fly the camera. Persisted. */
  wanted: boolean;
  /** The frame the stage is drawing right now, or `null` when it is drawing none. */
  drawn: DrawnFrame | null;
  /** Turn the guide on or off. Persists, because it is a preference and not project state. */
  setWanted(on: boolean): void;
  /** Mirror what the shell was actually asked to draw. Written by the one effect that asks. */
  setDrawn(frame: DrawnFrame | null): void;
}

export const frameGuideStore = createStore<FrameGuideState>((set) => ({
  wanted: readWanted(),
  drawn: null,
  setWanted: (on) => {
    writeWanted(on);
    set({ wanted: on });
  },
  setDrawn: (frame) => set({ drawn: frame }),
}));

/** The whole guide state. Selector-free for the reason `useCinemaPreview` is: a selector that builds
 *  an object literal returns a new reference every render, and zustand v5 compares with `Object.is`.
 *  This state object's identity changes on a toggle or a delivery change — never per frame. */
export const useFrameGuide = (): FrameGuideState => useStore(frameGuideStore);
