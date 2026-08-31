//! The box you are dragging on the stage, and the one thing it has to tell you: **which objects it is
//! about to take.**
//!
//! The engine's marquee has two modes and picks between them from the drag DIRECTION — left-to-right
//! takes only what is fully inside, right-to-left takes everything it crosses. That is the right
//! convention (every CAD tool with both spells it this way) and it is completely invisible: two
//! identical-looking rectangles select different sets and nothing on screen says why. So the
//! rectangle says it, in the mode's own plain words, on the corner the drag is heading towards.
//!
//! `pointer-events: none` is load-bearing, not tidiness. `stageInput.ts` decides who owns a pointer by
//! asking whether the event's target IS the stage — an overlay that can become a target silently
//! takes the stage's own gestures, and this one is painted directly under the cursor of a gesture in
//! progress. A layer that cannot be hit is transparent to that rule for free.

import { color, fontSize, font, radius, space, z } from "../theme/tokens";
import type { MarqueeBox, MarqueeMode } from "./marquee";
import { marqueeCaption } from "./marquee";

/** Below this the caption is bigger than the box it labels, and a 3-px-tall rectangle with a word
 *  hanging off it reads as a rendering fault rather than a selection. The box still draws. */
const CAPTION_MIN_PX = 28;

export function StageMarquee({
  box,
  origin,
  mode,
}: {
  box: MarqueeBox;
  /** The stage's top-left in client coordinates — the box arrives in client pixels and is drawn as an
   *  absolutely-positioned child of the stage. */
  origin: { left: number; top: number };
  mode: MarqueeMode;
}) {
  const left = box.left - origin.left;
  const top = box.top - origin.top;
  const showCaption = box.width >= CAPTION_MIN_PX && box.height >= CAPTION_MIN_PX;
  return (
    <div
      data-testid="stage-marquee"
      data-marquee-mode={mode}
      aria-hidden="true"
      style={{
        position: "absolute",
        left,
        top,
        width: box.width,
        height: box.height,
        // Touch is the forgiving mode and reads as a dashed edge in every tool that draws the
        // distinction; enclose is the precise one and gets the solid line. The border carries the
        // difference as well as the caption does, so the mode survives a glance without reading.
        border: `1px ${mode === "enclose" ? "solid" : "dashed"} ${color.accent.solid}`,
        background: color.accent.subtle,
        borderRadius: radius.sm,
        pointerEvents: "none",
        zIndex: z.badge,
      }}
    >
      {showCaption && (
        <span
          style={{
            position: "absolute",
            // On the corner the drag is heading towards, so the caption is under the cursor rather
            // than behind the hand holding it.
            [mode === "enclose" ? "right" : "left"]: 0,
            bottom: -space.xxl,
            padding: `${space.xxs}px ${space.sm}px`,
            borderRadius: radius.sm,
            background: color.accent.solid,
            color: color.accent.onSolid,
            font: font.ui,
            fontSize: fontSize.meta,
            whiteSpace: "nowrap",
          }}
        >
          {marqueeCaption(mode)}
        </span>
      )}
    </div>
  );
}
