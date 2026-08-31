//! **What is under the pointer, said without clicking** (ADR-191).
//!
//! `viewport_peek` — the non-mutating "what is under this point" read — shipped with M3.3 and had **no
//! caller in `editor/src`**. `HoverTooltip`, a finished panel that names an entity and lists its
//! components and capability contract, was mounted by nothing but its own test. So the only way to
//! learn what an object on the stage IS was to select it and read the Inspector: a mutation, an undo
//! entry's worth of state change, and a panel away from where you were looking.
//!
//! **The cadence is the whole design, because a hover probe is a per-frame temptation.** Invariant 4
//! says the hot path never crosses the JS boundary, and a `pointermove` handler that asks the engine
//! anything is exactly that path. So:
//!
//! * the pointer position is written to a **ref**, never to React state — a move costs no render;
//! * the probe fires only after the pointer has been **still** for [`HOVER_SETTLE_MS`], because the
//!   question "what is this" is only asked by someone who has stopped moving;
//! * one probe is in flight at a time, and the answer re-renders only when it is **different**.
//!
//! One IPC per pause, zero per frame — which is what `viewport_peek`'s own doc comment asks for.

import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { HoverTooltip } from "../panels/HoverTooltip";
import { z } from "../theme/tokens";
import type { EditorClient } from "../transport/session";
import { normalizeSurfacePoint } from "./viewportCoordinates";

/** How long the pointer must be still before the engine is asked what is under it.
 *
 *  Long enough that crossing the stage on the way to a panel asks nothing, short enough that stopping
 *  on an object feels like the tooltip was already there. */
export const HOVER_SETTLE_MS = 130;

/** How far the tooltip sits from the cursor, so it never covers the thing it is describing. */
const CURSOR_OFFSET = 18;

/** Keep a box of `size` at `at` fully inside `bounds`, preferring below-right of the cursor.
 *
 *  Exported because it is the only part of this file jsdom can judge: a tooltip that opens past the
 *  right edge of the window is the same defect class the shots harness's R1/R5 invariants exist to
 *  catch, and it is invisible in a test that only asserts the tooltip rendered. */
export function tooltipPosition(
  at: { x: number; y: number },
  size: { width: number; height: number },
  bounds: { width: number; height: number },
): { left: number; top: number } {
  // Below-right is the default because that is where a cursor's own hotspot leaves room. Flipping to
  // the other side of the cursor (rather than merely clamping) is what keeps the tooltip off the
  // object near an edge — clamping alone would slide it back under the pointer.
  const right = at.x + CURSOR_OFFSET;
  const left = right + size.width <= bounds.width ? right : Math.max(0, at.x - CURSOR_OFFSET - size.width);
  const below = at.y + CURSOR_OFFSET;
  const top = below + size.height <= bounds.height ? below : Math.max(0, at.y - CURSOR_OFFSET - size.height);
  return { left, top };
}

export interface StageHoverState {
  /** The entity under the pointer. */
  id: string;
  /** Where the pointer was when the engine answered, in client coordinates. */
  x: number;
  y: number;
}

/** The hover probe. `enabled` is the gesture gate: a drag, Play, or an open menu all own the pointer,
 *  and a tooltip that follows a marquee around describes an object nobody is asking about. */
export function useStageHover(client: EditorClient, enabled: boolean) {
  const [hover, setHover] = useState<StageHoverState | null>(null);
  const point = useRef<{ x: number; y: number } | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const inFlight = useRef(false);

  const stopTimer = () => {
    if (timer.current !== null) {
      clearTimeout(timer.current);
      timer.current = null;
    }
  };

  const clear = useCallback(() => {
    point.current = null;
    stopTimer();
    setHover((previous) => (previous === null ? previous : null));
  }, []);

  // A gesture starting is the same event as the pointer leaving, as far as this is concerned: the
  // tooltip goes away and no probe is left pending behind it.
  useEffect(() => {
    if (!enabled) clear();
  }, [enabled, clear]);

  useEffect(() => stopTimer, []);

  const onMove = useCallback(
    (clientX: number, clientY: number) => {
      if (!enabled) return;
      point.current = { x: clientX, y: clientY };
      // Restarting the timer on every move is what turns a per-frame question into a per-PAUSE one.
      stopTimer();
      timer.current = setTimeout(() => {
        timer.current = null;
        const at = point.current;
        // One in flight at a time. The alternative is a queue of stale answers arriving out of order,
        // each one re-pointing the tooltip at an object the cursor has already left.
        if (!at || inFlight.current) return;
        inFlight.current = true;
        const { x: nx, y: ny } = normalizeSurfacePoint(at.x, at.y);
        client
          .viewportPeek(nx, ny)
          .then((id) => {
            setHover((previous) => {
              if (!id) return previous === null ? previous : null;
              if (previous && previous.id === id && previous.x === at.x && previous.y === at.y) return previous;
              return { id, x: at.x, y: at.y };
            });
          })
          .catch(() => setHover((previous) => (previous === null ? previous : null)))
          .finally(() => {
            inFlight.current = false;
          });
      }, HOVER_SETTLE_MS);
    },
    [client, enabled],
  );

  return { hover, onMove, clear };
}

/** The tooltip, placed at the cursor and kept on screen. Inert by construction: `pointerEvents: none`
 *  on the positioner, so the surface it floats over keeps every gesture. */
export function StageHoverTooltip({ client, hover }: { client: EditorClient; hover: StageHoverState | null }) {
  const box = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);

  useLayoutEffect(() => {
    if (!hover) {
      setPos(null);
      return;
    }
    const el = box.current;
    // Measured, not assumed: the tooltip's height depends on how many capability sections the entity
    // has, and a guessed height puts a five-line tooltip off the bottom of the window.
    const size = { width: el?.offsetWidth ?? 0, height: el?.offsetHeight ?? 0 };
    setPos(tooltipPosition(hover, size, { width: window.innerWidth, height: window.innerHeight }));
  }, [hover]);

  if (!hover) return null;
  return (
    <div
      ref={box}
      data-testid="stage-hover"
      style={{
        position: "fixed",
        // Hidden until measured, exactly as `Popover` does it — otherwise the first frame paints the
        // tooltip at the cursor and the second frame moves it, which reads as a flicker.
        left: pos?.left ?? -9999,
        top: pos?.top ?? -9999,
        visibility: pos ? "visible" : "hidden",
        zIndex: z.menu,
        pointerEvents: "none",
      }}
    >
      <HoverTooltip client={client} id={hover.id} />
    </div>
  );
}
