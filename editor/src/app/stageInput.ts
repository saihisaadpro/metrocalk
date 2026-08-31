//! Who owns a pointer on the stage: the wgpu surface, or a piece of UI chrome floating over it.
//!
//! ONE SENTENCE, IN ONE PLACE. The 3D viewport is a `position: relative` region with real handlers on
//! it — a left click picks, a right press starts a native orbit, a wheel zooms, a right click opens
//! the entity menu — and the shell paints seven overlays *inside* that region: the tool rail, the
//! viewport toolbar, Pipe Forge, the PLAYING badge, the first-run card, the empty-state card and the
//! toast host. A DOM event on any of them bubbles to the viewport's own handler, so pressing a button
//! that is *on* the stage also drives the stage *underneath* it.
//!
//! THIS WAS MEASURED, NOT ARGUED, AND IT WAS EVERYWHERE. Before this module existed, **35 of 35**
//! interactive controls the shell paints inside `#viewport` leaked at least one event into the native
//! seam (vitest, four shell states): all 35 turned a wheel into a camera zoom, and four also turned
//! their own click into a `viewport_pick`. In a real Chromium — which can fire the `pointerdown`
//! jsdom cannot — **11 of 11** leaked, three of them starting a native ORBIT (`drag_start`) under a
//! button labelled "Import file…".
//!
//! WHY THE GUARD LIVES HERE AND NOT ON THE OVERLAYS. It used to live on the overlays, as a
//! hand-copied `e.stopPropagation()` idiom, and five overlays had **five different subsets** of the
//! six events — PlayBadge and the first-run card stopped two, the toolbar three, the tool rail four,
//! Pipe Forge six, and EmptyState and ToastHost stopped none at all. Every new overlay was one more
//! chance to guess wrong, and the guess was silent. The seam is the one place that knows the whole
//! rule, and a rule stated once cannot be stated inconsistently.
//!
//! `event.target === event.currentTarget` is not an approximation of "the pointer was over bare
//! stage" — it *is* that fact, decided by the browser's own hit test. An overlay with
//! `pointer-events: none` never becomes a target, so a decorative layer stays transparent to the
//! stage for free, which is exactly the behaviour the old idiom had to remember not to break.
//!
//! Superseding `src/input/ownership.ts` (M2.6, deleted with this): it answered the same question with
//! rectangle arithmetic — the viewport rect minus a hand-maintained list of overlay rects. That is a
//! second statement of a contract the DOM already states exactly, of the kind
//! `<test_and_ci_discipline>` 6 is written against, and it had been imported by nothing since it was
//! written.

import type { SyntheticEvent } from "react";

/** True when the event happened on the stage SURFACE itself — the wgpu composite — rather than on a
 *  piece of UI chrome the shell floats over it.
 *
 *  Read it in the viewport's *initiating* handlers (`pointerdown`, `click`, `wheel`, `contextmenu`):
 *  those begin something in the native layer, and a gesture that began on a button did not begin on
 *  the stage.
 *
 *  Do **not** read it in the *completing* handlers (`pointermove`, `pointerup`). A right-drag that
 *  starts on bare stage and is released with the cursor over the tool rail must still call
 *  `drag_end`, or the native orbit never stops. That asymmetry is the bug the old per-overlay idiom
 *  could not express and got wrong: `PipeForge` stopped `pointerup` along with everything else, so an
 *  orbit released over the Pipe Forge panel left the camera spinning.
 *
 *  **There is a third kind of reader, and it belongs on the initiating side of that line** (ADR-191).
 *  The hover probe asks the engine what is under the pointer, and it is fed from `pointermove` — but
 *  it neither begins nor completes a gesture, so the reason `pointermove` is excluded above does not
 *  reach it. Nothing is left running that a later event has to stop; the question is simply *"what is
 *  the pointer over"*, and a pointer over the "Import file…" button is over the button. Reading it
 *  there is right for the same reason reading it in `pointerdown` is: **the browser's hit test is the
 *  fact**. What must NOT acquire this guard is the marquee/orbit bookkeeping in the same handler,
 *  which is completing a gesture and is excluded for the reason stated above. */
export function onStageSurface(event: SyntheticEvent): boolean {
  return event.target === event.currentTarget;
}
