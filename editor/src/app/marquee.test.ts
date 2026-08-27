//! The marquee's policy, tested where it is decided.
//!
//! The direction convention is stated in THREE places — `ScreenRect::from_drag` in the engine, this
//! module, and the caption the rectangle draws — and only one of them is the authority. What can be
//! tested here is that this module reads the direction the same way the engine does, and that the
//! caption cannot say one thing while the query does another, because both come from `marqueeMode`.

import { describe, expect, it } from "vitest";
import {
  MARQUEE_THRESHOLD_PX,
  isMarqueeDrag,
  marqueeBox,
  marqueeCaption,
  marqueeMode,
  marqueeResult,
} from "./marquee";

describe("the direction is the policy", () => {
  it("left-to-right encloses and right-to-left touches — the engine's own convention", () => {
    expect(marqueeMode(100, 400)).toBe("enclose");
    expect(marqueeMode(400, 100)).toBe("touch");
  });

  it("a purely vertical drag encloses, because only the horizontal component decides", () => {
    // `ScreenRect::from_drag` sets `reversed = end[0] < start[0]` and looks at nothing else. A rule
    // that also read the vertical axis here would disagree with the engine on exactly the drags a
    // careful user makes — straight down the side of a stack of parts.
    expect(marqueeMode(200, 200)).toBe("enclose");
  });

  it("the caption comes from the same function as the query, so it cannot describe the wrong set", () => {
    expect(marqueeCaption(marqueeMode(0, 50))).toBe("Fully inside");
    expect(marqueeCaption(marqueeMode(50, 0))).toBe("Touched");
  });
});

describe("a press is not a drag until it has travelled", () => {
  it("holds still below the threshold and commits at it", () => {
    const start = { x: 100, y: 100 };
    expect(isMarqueeDrag(start, { x: 100, y: 100 })).toBe(false);
    expect(isMarqueeDrag(start, { x: 100 + MARQUEE_THRESHOLD_PX - 1, y: 100 })).toBe(false);
    expect(isMarqueeDrag(start, { x: 100 + MARQUEE_THRESHOLD_PX, y: 100 })).toBe(true);
    // Either axis is enough: a box dragged straight down is still a box.
    expect(isMarqueeDrag(start, { x: 100, y: 100 - MARQUEE_THRESHOLD_PX })).toBe(true);
  });
});

describe("the drawn box", () => {
  it("normalizes in every direction, because CSS boxes have no negative width", () => {
    expect(marqueeBox({ x: 300, y: 200 }, { x: 100, y: 50 })).toEqual({
      left: 100,
      top: 50,
      width: 200,
      height: 150,
    });
    expect(marqueeBox({ x: 100, y: 50 }, { x: 300, y: 200 })).toEqual({
      left: 100,
      top: 50,
      width: 200,
      height: 150,
    });
  });
});

describe("what the gesture says it did", () => {
  it("counts, names the rule it used, and says plainly when it took nothing", () => {
    expect(marqueeResult(14, "enclose", false)).toBe("selected 14 objects fully inside the box");
    expect(marqueeResult(1, "touch", false)).toBe("selected 1 object touched by the box");
    expect(marqueeResult(3, "enclose", true)).toBe("added 3 objects fully inside the box");
    // An empty box is the commonest way to learn the enclose/touch rule the hard way, so it says
    // WHICH rule found nothing rather than a bare "0 selected".
    expect(marqueeResult(0, "enclose", false)).toBe("nothing was fully inside the box");
  });
});
