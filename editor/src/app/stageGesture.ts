//! **When a left-press on the stage becomes a DRAG — one rule, for every gesture that forks on it.**
//!
//! Three different things can begin under a left-press on the viewport, and until this module existed
//! only two of them agreed on how to tell them apart:
//!
//! | gesture | how it used to decide |
//! |---|---|
//! | box selection | the pointer moved ≥ 4 px (`isMarqueeDrag`) |
//! | camera orbit (right button) | the pointer moved > 6 px (`rightDrag.moved`) |
//! | **gizmo handle drag** | **the press itself** — a raycast fired on `pointerdown`, and a HIT started a native drag before the pointer had moved at all |
//!
//! The third row is the defect. A gizmo hit does not merely *start* something, it **eats the click**:
//! `onClick` returns early on `gizmoHit`, so the pick never runs. And the gizmo is drawn **at** the
//! selection, which means the handles sit on top of the object you would most want to click next.
//!
//! **That cost three separate live behaviours, all from one cause.**
//!
//! 1. **Ctrl-click on a selected object did nothing.** Ctrl is the gizmo's own snap modifier
//!    (`gizmo_pick_drag(x, y, ctrl)`) *and* the selection's toggle modifier, so a ctrl-press could not
//!    be excluded from the probe the way alt and shift were (ADR-191) without deleting snapped
//!    dragging. The probe therefore ran, hit a handle, and swallowed the toggle. Alt-click had exactly
//!    this bug until ADR-191, and the repair there — exclude the modifier — cannot be applied here,
//!    because ctrl genuinely means both things.
//! 2. **Every click on a selected object wrote an undo entry.** A hit sets `gizmo_dragging`; the
//!    release calls `gizmo_drag_end`, which **unconditionally** commits `GizmoCommit`. Press and
//!    release without moving a pixel and the engine records a transaction that changes nothing — one
//!    per click, silently filling the undo stack the user then has to walk back through.
//! 3. **A click faster than the probe left the object glued to the cursor.** The probe is async; the
//!    release is not. `onPointerUp` only calls `gizmo_drag_end` when `gizmoHit` is already true, so a
//!    press/release that completes before the answer arrives leaves `gizmo_dragging` set in the render
//!    loop — which drags the selection from the OS cursor with **no button held** (`render.rs`'s drag
//!    block tests the flag and nothing else) — and leaves `gizmoHit` standing to eat a later click.
//!
//! **The rule, stated once:** a press is a CLICK until the pointer has travelled
//! [`DRAG_THRESHOLD_PX`]; only then may it become a drag, and only then does anything ask the engine
//! for a handle. Nothing starts in the native layer while the gesture could still turn out to be a
//! click, so there is nothing to cancel, nothing to commit and nothing to leave running.
//!
//! The threshold is `marquee.ts`'s, deliberately and by import rather than by copy: the box and the
//! handle fork on the *same press*, and two numbers there would mean a 5 px twitch is a box for one
//! half of the handler and a click for the other.

import { MARQUEE_THRESHOLD_PX, isMarqueeDrag } from "./marquee";

/** How far a pointer must travel before a press stops being a click.
 *
 *  One number for the box and the handle, because they are the same press. (The right-button orbit's
 *  6 px is a different press with a different button and is left alone — `marquee.ts` explains why
 *  4 inside 6 is the right relationship.) */
export const DRAG_THRESHOLD_PX = MARQUEE_THRESHOLD_PX;

/** Whether a press that has reached `current` has travelled far enough to be a drag. */
export function beganDrag(
  press: { x: number; y: number },
  current: { x: number; y: number },
): boolean {
  return isMarqueeDrag(press, current, DRAG_THRESHOLD_PX);
}

/** What the gizmo probe has been asked, for the press currently down.
 *
 *  `"idle"` — not asked yet (the pointer has not travelled far enough, or nothing is selected).
 *  `"pending"` — asked, no answer yet. The box must not draw during this window or it flashes on
 *  screen for the round trip and is then withdrawn.
 *  `"hit"` / `"miss"` — answered. A hit owns the gesture; a miss releases it to the box. */
export type GizmoProbe = "idle" | "pending" | "hit" | "miss";

/** Whether a left-press could turn into a gizmo-handle drag at all.
 *
 *  Alt and shift say the gesture is a SELECTION, and the gizmo has no meaning for either (ADR-191).
 *  Ctrl is **not** in that list and must not be: it is the gizmo's snap modifier as well as the
 *  selection's toggle, so excluding it would delete snapped dragging. The press/drag threshold is what
 *  separates the two readings — which is the whole reason this module exists. */
export function armsGizmoDrag(gesture: {
  hasSelection: boolean;
  altKey: boolean;
  shiftKey: boolean;
}): boolean {
  return gesture.hasSelection && !gesture.altKey && !gesture.shiftKey;
}

/** What to do with a probe answer that arrived after its gesture already ended.
 *
 *  A HIT is not a note about the past: `gizmo_pick_drag` **started a native drag** as a side effect of
 *  answering, and the render loop moves the selection from the OS cursor for as long as the flag is
 *  set, button or no button. The release that would have ended it has already happened, so the late
 *  answer has to end it itself. A late MISS started nothing and is simply dropped.
 *
 *  This is defect 3 above, stated as a function so the caller cannot forget the asymmetry. */
export function lateProbeNeedsDragEnd(hit: boolean): boolean {
  return hit;
}
