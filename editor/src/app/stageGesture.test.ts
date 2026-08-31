//! The two facts about the press/drag fork that a reader would otherwise have to take on trust.
//!
//! Both are *relationships between two statements*, which is exactly the kind of thing a compiler
//! cannot check and a behavioural test in `StageSelection.test.tsx` only checks by accident: that
//! suite would still pass if the gizmo forked at 40 px and the box at 4.

import { describe, expect, it } from "vitest";
import { MARQUEE_THRESHOLD_PX } from "./marquee";
import { DRAG_THRESHOLD_PX, armsGizmoDrag, beganDrag, lateProbeNeedsDragEnd } from "./stageGesture";

describe("the press/drag fork", () => {
  it("uses ONE distance for the box and the handle — they are the same press", () => {
    // Two numbers here would mean a 5 px twitch is a box for one half of the pointer handler and a
    // click for the other, and the gesture that lands in the gap belongs to neither.
    expect(DRAG_THRESHOLD_PX).toBe(MARQUEE_THRESHOLD_PX);
    expect(beganDrag({ x: 100, y: 100 }, { x: 100 + MARQUEE_THRESHOLD_PX, y: 100 })).toBe(true);
    expect(beganDrag({ x: 100, y: 100 }, { x: 100 + MARQUEE_THRESHOLD_PX - 1, y: 100 })).toBe(false);
  });

  it("keeps CTRL armed — it is the one modifier the keyboard cannot decide", () => {
    // Alt and shift declare a selection gesture and disarm the gizmo (ADR-191). Ctrl cannot join them:
    // it is the gizmo's snap flag as well as the selection's toggle, so disarming on it would delete
    // snapped dragging, and arming on it ate every ctrl-click. Only the threshold separates the two —
    // which is the whole reason this module exists, and the assertion that stops someone "fixing" the
    // ctrl-click bug by adding one character to the arming rule.
    const selected = { hasSelection: true, altKey: false, shiftKey: false };
    expect(armsGizmoDrag(selected)).toBe(true);
    expect(armsGizmoDrag({ ...selected, altKey: true })).toBe(false);
    expect(armsGizmoDrag({ ...selected, shiftKey: true })).toBe(false);
    // Nothing selected, no gizmo drawn, nothing to grab.
    expect(armsGizmoDrag({ ...selected, hasSelection: false })).toBe(false);
  });

  it("says a late HIT still owes a drag_end, and a late MISS owes nothing", () => {
    // Asymmetric because the probe is not a question: answering HIT sets `gizmo_dragging` in the render
    // loop, which then drags from the OS cursor with no button held. A miss started nothing.
    expect(lateProbeNeedsDragEnd(true)).toBe(true);
    expect(lateProbeNeedsDragEnd(false)).toBe(false);
  });
});
