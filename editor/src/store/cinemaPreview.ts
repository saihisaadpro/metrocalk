//! The **shot preview** store — is the cutscene camera holding the viewport, and on what.
//!
//! ONE STATE, TWO SURFACES. The preview is started from the cutscene timeline in the bottom dock and
//! it is *visible* on the stage, which is a different part of the window; both need the same answer
//! to "is the viewport mine right now", and a second copy of that answer is a second thing that can
//! be wrong. The panel's toggle and the stage badge's Exit both read and write here.
//!
//! The shell is authoritative — the engine thread owns the camera and every field below arrives in a
//! `CinemaPreviewReply`. This mirrors the last one so the badge can be rendered without the panel
//! being mounted at all: closing the bottom dock while previewing must not silently strand the
//! viewport in a shot with nothing on screen saying so.

import { createStore } from "zustand/vanilla";
import { useStore } from "zustand";
import type { CinemaPreviewReply } from "../transport/protocol";

/** What the stage badge needs in order to name what it is showing. */
export interface CinemaPreviewInfo {
  /** The cutscene camera is holding the viewport. */
  active: boolean;
  /** The object whose cutscene is on screen — the id the exit command must be sent for. */
  entity: string | null;
  /** Which shot, 0-based. */
  shotIndex: number | null;
  /** How many shots the cutscene holds. */
  shots: number;
  /** Where on the cutscene clock the preview stands, seconds. */
  seconds: number;
  /** The display name of the object the shot FRAMES. */
  subjectName: string;
  /** True while the frame is a transition between two shots. */
  blending: boolean;
}

const OFF: CinemaPreviewInfo = {
  active: false,
  entity: null,
  shotIndex: null,
  shots: 0,
  seconds: 0,
  subjectName: "",
  blending: false,
};

interface CinemaPreviewState extends CinemaPreviewInfo {
  /** Mirror a reply from the shell. A refusal (`reason` set) clears the store rather than half-
   *  filling it: the engine has already said the camera did not move, and a badge left standing
   *  after a refusal is the inert-control failure in its most confusing form — it offers an Exit
   *  from something that never started. */
  from(reply: CinemaPreviewReply): void;
  reset(): void;
}

export const cinemaPreviewStore = createStore<CinemaPreviewState>((set) => ({
  ...OFF,
  from: (reply) =>
    set(
      reply.active && !reply.reason
        ? {
            active: true,
            entity: reply.entity,
            shotIndex: reply.shotIndex,
            shots: reply.shots,
            seconds: reply.seconds,
            subjectName: reply.subjectName,
            blending: reply.blending,
          }
        : OFF,
    ),
  reset: () => set(OFF),
}));

/** The whole preview state. Selector-free on purpose: a selector building an object literal returns
 *  a new reference on every render, which zustand v5 compares with `Object.is` and re-renders on
 *  forever. The state object's identity changes only when `set` runs, and `set` runs on a toggle or
 *  a scrub — never per frame. */
export const useCinemaPreview = (): CinemaPreviewInfo => useStore(cinemaPreviewStore);
