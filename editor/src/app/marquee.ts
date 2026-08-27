//! **Box selection on the stage** — the geometry and the policy, with no React and no DOM in it.
//!
//! The engine has had `viewport_pick_region` since picking was rebuilt: a real marquee query with a
//! real convention, `left-to-right takes only objects fully enclosed, right-to-left takes everything
//! the rectangle touches`, decided from the drag direction at the input boundary so every caller
//! gets the same answer. It had **no caller**. A left-drag on the stage did nothing at all, so the
//! only way to select more than one object was the outliner — and the toolbar's own refusal told you
//! to `Shift/Ctrl-click at least two objects`, naming a gesture the 3D view did not have.
//!
//! The direction convention is not decoration. Enclose is the precise one and reads left-to-right
//! like the language; touch is the forgiving one you reach for when the thing you want is partly off
//! screen. Every CAD tool that has both spells them this way, which is the argument for not inventing
//! a third spelling — and the reason the mode is **named on the rectangle while you drag it**, so the
//! rule is learned by using it rather than by reading a manual nobody opens.

/** A rectangle in client (CSS pixel) coordinates, ready for absolute positioning. */
export interface MarqueeBox {
  left: number;
  top: number;
  width: number;
  height: number;
}

/** Which objects the rectangle takes. Spelled as the engine spells it. */
export type MarqueeMode = "enclose" | "touch";

/** A point far enough from the press to mean "I am dragging a box", not "I clicked and my hand moved".
 *
 *  4 px matches the shell's other press-versus-drag thresholds closely enough to feel like one rule and
 *  is comfortably inside the 6 px the right-drag orbit uses to suppress its context menu — a marquee
 *  that needed a longer drag than an orbit would read as the stage ignoring you. */
export const MARQUEE_THRESHOLD_PX = 4;

/** Whether the pointer has travelled far enough for this to be a marquee rather than a click. */
export function isMarqueeDrag(
  start: { x: number; y: number },
  current: { x: number; y: number },
  threshold = MARQUEE_THRESHOLD_PX,
): boolean {
  return Math.abs(current.x - start.x) >= threshold || Math.abs(current.y - start.y) >= threshold;
}

/** The mode the drag direction asks for — the engine's own convention, read from the same two points
 *  the engine reads it from, so the caption on screen cannot say one thing while the query does
 *  another. Only the horizontal component decides, exactly as `ScreenRect::from_drag` does. */
export function marqueeMode(startX: number, currentX: number): MarqueeMode {
  return currentX < startX ? "touch" : "enclose";
}

/** The drawn rectangle. Normalized, because a drag runs in any direction and CSS boxes do not. */
export function marqueeBox(start: { x: number; y: number }, current: { x: number; y: number }): MarqueeBox {
  return {
    left: Math.min(start.x, current.x),
    top: Math.min(start.y, current.y),
    width: Math.abs(current.x - start.x),
    height: Math.abs(current.y - start.y),
  };
}

/** What the rectangle says about itself while it is being dragged.
 *
 *  Plain language, not the enum (`<ux_quality>` 4): "Enclosed" and "Touched" describe what will
 *  happen to the objects, which is the thing the user is deciding about. */
export function marqueeCaption(mode: MarqueeMode): string {
  return mode === "enclose" ? "Fully inside" : "Touched";
}

/** What the gesture produced, said in the user's terms — the count first, because the count is the
 *  answer to "did that do what I meant". */
export function marqueeResult(count: number, mode: MarqueeMode, added: boolean): string {
  const what = mode === "enclose" ? "fully inside the box" : "touched by the box";
  if (count === 0) return `nothing was ${what}`;
  const noun = count === 1 ? "object" : "objects";
  return added ? `added ${count} ${noun} ${what}` : `selected ${count} ${noun} ${what}`;
}
