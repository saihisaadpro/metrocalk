//! **The reusable floating-overlay primitives** (menus, dropdowns, popovers, context menus, modals) —
//! the single, correct way to render UI that floats ABOVE the app so it can never be clipped by an
//! ancestor's `overflow: hidden` or trapped below a sibling's stacking context.
//!
//! Why this exists: a naive dropdown is `position: absolute` inside its trigger's row. If any ancestor row
//! sets `overflow: hidden` (the app header does, to keep the toolbar tidy), the dropdown is CLIPPED to that
//! row; and a raised `z-index` cannot escape an ancestor stacking context. Both bugs vanish when the floating
//! content is **portaled to `document.body`** and positioned with `position: fixed` against the trigger's
//! screen rect. `Popover` does exactly that, and is **edge-aware** (it flips/clamps to stay on screen),
//! **dismissible** (Escape + outside-click), and **z-layered** via the shared [`z`] scale.
//!
//! Use `Popover` for anything anchored to a trigger (File menu, context menu, hover card, autocomplete);
//! use `Modal` for centered dialogs (guards, confirmations). Both render through the same overlay layer.

import { useEffect, useLayoutEffect, useRef, useState, type ReactNode, type RefObject } from "react";
import { createPortal } from "react-dom";
import { z } from "./tokens";

/** Which corner of the anchor the panel grows from (before edge-aware flipping/clamping). */
export type Placement = "bottom-start" | "bottom-end" | "top-start" | "top-end";

const GAP = 4; // px between the anchor and the panel
const EDGE = 6; // px min margin from the viewport edge

export interface PopoverProps {
  /** Whether the panel is shown. */
  open: boolean;
  /** The trigger element the panel anchors to (its on-screen rect drives positioning). Omit when using
   *  `anchorPoint` (a context menu anchored to a cursor position). */
  anchor?: RefObject<HTMLElement | null>;
  /** A screen point to anchor to (e.g. a right-click position) — takes precedence over `anchor`. */
  anchorPoint?: { x: number; y: number } | null;
  /** Called on Escape or an outside click. */
  onClose: () => void;
  /** Preferred corner (auto-flips/clamps to stay on screen). Default `bottom-start`. */
  placement?: Placement;
  /** Stack level. Default [`z.menu`]; raise (e.g. a menu opened from within a modal) as needed. */
  zIndex?: number;
  /** Optional label for a11y + tests. */
  id?: string;
  children: ReactNode;
}

/**
 * A floating panel anchored to a trigger element (`anchor`) or a cursor point (`anchorPoint`), rendered in a
 * portal on `document.body` so it is never clipped by an ancestor `overflow` or trapped in a stacking context.
 * Edge-aware (flips/clamps to stay fully on screen); dismissed by Escape or an outside click.
 */
export function Popover({ open, anchor, anchorPoint, onClose, placement = "bottom-start", zIndex = z.menu, id, children }: PopoverProps) {
  const panelRef = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);

  // Position against the anchor's live rect (or the cursor point), flipping/clamping to keep the panel fully
  // on screen. Recomputed when the panel mounts (so its measured size is known) and on resize/scroll.
  useLayoutEffect(() => {
    if (!open) {
      setPos(null);
      return;
    }
    const compute = () => {
      const ar = anchorPoint
        ? { left: anchorPoint.x, right: anchorPoint.x, top: anchorPoint.y, bottom: anchorPoint.y }
        : anchor?.current?.getBoundingClientRect();
      if (!ar) return;
      const pw = panelRef.current?.offsetWidth ?? 0;
      const ph = panelRef.current?.offsetHeight ?? 0;
      const vw = window.innerWidth;
      const vh = window.innerHeight;

      // horizontal: start = anchor left, end = anchor right; then clamp within the viewport.
      let left = placement.endsWith("end") ? ar.right - pw : ar.left;
      left = Math.max(EDGE, Math.min(left, vw - pw - EDGE));

      // vertical: below by default; flip above if it would overflow the bottom and there's room above.
      const below = ar.bottom + GAP;
      const above = ar.top - ph - GAP;
      let top = placement.startsWith("top") ? above : below;
      if (top + ph > vh - EDGE && above >= EDGE) top = above;
      top = Math.max(EDGE, Math.min(top, vh - ph - EDGE));

      setPos({ left, top });
    };
    compute();
    const raf = requestAnimationFrame(compute); // a second pass once the panel has a measured size
    window.addEventListener("resize", compute);
    window.addEventListener("scroll", compute, true);
    return () => {
      cancelAnimationFrame(raf);
      window.removeEventListener("resize", compute);
      window.removeEventListener("scroll", compute, true);
    };
  }, [open, anchor, anchorPoint, placement, children]);

  // Escape closes (capture-phase + stopPropagation so it doesn't also trigger app-level Esc handlers).
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.stopPropagation();
      onClose();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [open, onClose]);

  if (!open) return null;

  return createPortal(
    <>
      {/* Outside-click scrim (invisible, catches clicks anywhere else). `mousedown` so a click that starts
          outside dismisses immediately, before the target's own handler. */}
      <div onMouseDown={onClose} style={{ position: "fixed", inset: 0, zIndex: zIndex - 1 }} />
      <div
        ref={panelRef}
        id={id}
        role="menu"
        // Hidden (not unmounted) until positioned → measured once, then shown at the right spot (no flash).
        style={{
          position: "fixed",
          left: pos?.left ?? -9999,
          top: pos?.top ?? -9999,
          zIndex,
          visibility: pos ? "visible" : "hidden",
          animation: pos ? "mtk-pop-in 90ms ease-out" : undefined,
        }}
      >
        {children}
      </div>
    </>,
    document.body,
  );
}

export interface ModalProps {
  /** Whether the modal is shown. */
  open: boolean;
  /** Called on Escape or a backdrop click. */
  onClose: () => void;
  /** Stack level of the modal content. Default [`z.guard`]. */
  zIndex?: number;
  id?: string;
  children: ReactNode;
}

/**
 * A centered modal dialog over a dimmed backdrop, portaled to `document.body`. Escape or a backdrop click
 * dismisses it. Use for confirmations / guards; use [`Popover`] for anything anchored to a trigger.
 */
export function Modal({ open, onClose, zIndex = z.guard, id, children }: ModalProps) {
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.stopPropagation();
      onClose();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [open, onClose]);

  if (!open) return null;
  return createPortal(
    <div
      id={id}
      role="dialog"
      aria-modal="true"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose(); // click on the backdrop (not the content) dismisses
      }}
      style={{
        position: "fixed",
        inset: 0,
        zIndex,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        background: "#0009",
        animation: "mtk-fade-in 120ms ease-out",
      }}
    >
      {children}
    </div>,
    document.body,
  );
}
