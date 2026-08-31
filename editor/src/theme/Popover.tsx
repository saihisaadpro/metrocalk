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

import {
  useEffect,
  forwardRef,
  useLayoutEffect,
  useRef,
  useState,
  type AriaRole,
  type HTMLAttributes,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
  type RefObject,
} from "react";
import { createPortal } from "react-dom";
import { color, motion, z } from "./tokens";

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
  /** Semantic role of the anchored content. Menus default to `menu`; tooltips/listboxes may override it. */
  role?: AriaRole;
  /** Accessible name when the popover role requires one and no visible label id is available. */
  ariaLabel?: string;
  /** Id of the visible element that labels this popover. */
  ariaLabelledBy?: string;
  /** Element that regains focus after dismissal. Defaults to the anchor. */
  returnFocus?: RefObject<HTMLElement | null>;
  children: ReactNode;
}

/**
 * A floating panel anchored to a trigger element (`anchor`) or a cursor point (`anchorPoint`), rendered in a
 * portal on `document.body` so it is never clipped by an ancestor `overflow` or trapped in a stacking context.
 * Edge-aware (flips/clamps to stay fully on screen); dismissed by Escape or an outside click.
 */
export function Popover({
  open,
  anchor,
  anchorPoint,
  onClose,
  placement = "bottom-start",
  zIndex = z.menu,
  id,
  role = "menu",
  ariaLabel,
  ariaLabelledBy,
  returnFocus,
  children,
}: PopoverProps) {
  const panelRef = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  const focusTarget = returnFocus ?? anchor;

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

  // Menus use the platform desktop interaction contract. Compact dialog popovers also move focus into the
  // surface so a keyboard user never opens an interactive panel behind their current tab stop.
  useLayoutEffect(() => {
    if (!open) return;
    if (role === "menu") {
      panelRef.current
        ?.querySelector<HTMLElement>("[role^='menuitem']:not(:disabled)")
        ?.focus();
      return;
    }
    if (role === "dialog") {
      panelRef.current
        ?.querySelector<HTMLElement>("button:not(:disabled), [href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex='-1'])")
        ?.focus();
    }
  }, [open, role]);

  useEffect(() => {
    if (!open) return;
    const panel = panelRef.current;
    return () => {
      // By the time this cleanup runs the panel's children are already detached, so a menu item that held
      // focus has handed it back to <body> — checking only `panel.contains(activeElement)` would miss the
      // common case (selecting an item) and strand focus at the document root. Treat "focus went nowhere"
      // as ours to restore; leave it alone when something else legitimately took it.
      const active = document.activeElement;
      const focusWasDropped = active == null || active === document.body;
      if (focusWasDropped || panel?.contains(active)) focusTarget?.current?.focus();
    };
  }, [focusTarget, open]);

  // Escape closes (capture-phase + stopPropagation so it doesn't also trigger app-level Esc handlers).
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.preventDefault();
      e.stopPropagation();
      onClose();
      focusTarget?.current?.focus();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [focusTarget, open, onClose]);

  const onMenuKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (role !== "menu" || !["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
    const items = Array.from(
      event.currentTarget.querySelectorAll<HTMLElement>(
        "[role^='menuitem']:not(:disabled)",
      ),
    );
    if (items.length === 0) return;
    event.preventDefault();
    event.stopPropagation();
    const currentIndex = items.indexOf(document.activeElement as HTMLElement);
    let nextIndex = currentIndex;
    if (event.key === "Home") nextIndex = 0;
    else if (event.key === "End") nextIndex = items.length - 1;
    else if (event.key === "ArrowDown") nextIndex = currentIndex < 0 ? 0 : (currentIndex + 1) % items.length;
    else nextIndex = currentIndex < 0 ? items.length - 1 : (currentIndex - 1 + items.length) % items.length;
    items[nextIndex]?.focus();
  };

  const onPopoverKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    onMenuKeyDown(event);
    if (role !== "dialog" || event.key !== "Tab") return;
    const focusable = Array.from(
      event.currentTarget.querySelectorAll<HTMLElement>(
        "button:not(:disabled), [href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex='-1'])",
      ),
    );
    if (focusable.length === 0) {
      event.preventDefault();
      return;
    }
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  };

  if (!open) return null;

  return createPortal(
    <>
      {/* Outside-click scrim (invisible, catches clicks anywhere else). `mousedown` so a click that starts
          outside dismisses immediately, before the target's own handler. */}
      <div onMouseDown={onClose} style={{ position: "fixed", inset: 0, zIndex: zIndex - 1 }} />
      <div
        ref={panelRef}
        id={id}
        role={role}
        aria-label={ariaLabel}
        aria-labelledby={ariaLabelledBy}
        onKeyDown={onPopoverKeyDown}
        // Hidden (not unmounted) until positioned → measured once, then shown at the right spot (no flash).
        style={{
          position: "fixed",
          left: pos?.left ?? -9999,
          top: pos?.top ?? -9999,
          zIndex,
          visibility: pos ? "visible" : "hidden",
          animation: pos ? `mtk-pop-in ${motion.instant}` : undefined,
        }}
      >
        {children}
      </div>
    </>,
    document.body,
  );
}

/** Shared anchored overlay surface. Popover owns geometry/focus; this owns visual hierarchy and spacing. */
export interface PopoverSurfaceProps extends HTMLAttributes<HTMLDivElement> {
  children: ReactNode;
}

export const PopoverSurface = forwardRef<HTMLDivElement, PopoverSurfaceProps>(function PopoverSurface({
  children,
  className,
  style,
  ...rest
}, ref) {
  return (
    <div ref={ref} className={["mtk-popover-surface", className].filter(Boolean).join(" ")} style={style} {...rest}>
      {children}
    </div>
  );
});

/** Shared modal content surface. Modal owns the scrim, focus trap and dismissal behaviour. */
export interface DialogSurfaceProps extends HTMLAttributes<HTMLDivElement> {
  /** Drop the surface's own padding so a region inside it can reach the rounded edge.
   *
   *  A confirmation is text on a padded card and that is the default. A TASK dialog is a composition
   *  — a rail beside a pane, a footer band — and every one of those has an edge the shared padding
   *  would hold off the corner, which is exactly the "arbitrary fixed layout" a local override would
   *  become. The variant is here rather than in the caller so the radius, the shadow and the viewport
   *  clamp stay stated once. */
  flush?: boolean;
  children: ReactNode;
}

export function DialogSurface({
  flush = false,
  children,
  className,
  style,
  ...rest
}: DialogSurfaceProps) {
  return (
    <div
      className={["mtk-dialog-surface", flush && "mtk-dialog-surface--flush", className].filter(Boolean).join(" ")}
      style={style}
      {...rest}
    >
      {children}
    </div>
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
  /** Preferred initial focus target. Falls back to the first interactive control, then the dialog itself. */
  initialFocusRef?: RefObject<HTMLElement | null>;
  /** Accessible dialog name when the content does not provide a visible labelled heading. */
  ariaLabel?: string;
  /** Id of the visible element that labels this dialog. */
  ariaLabelledBy?: string;
  /** Id of supporting copy that describes this dialog. */
  ariaDescribedBy?: string;
  children: ReactNode;
}

const FOCUSABLE_SELECTOR = [
  "a[href]",
  "button:not([disabled])",
  "input:not([disabled])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  "[tabindex]:not([tabindex='-1'])",
  "[contenteditable='true']",
].join(",");

function focusableElements(root: HTMLElement): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)).filter(
    (element) => element.tabIndex >= 0 && !element.closest("[hidden], [aria-hidden='true']"),
  );
}

/**
 * A centered modal dialog over a dimmed backdrop, portaled to `document.body`. Escape or a backdrop click
 * dismisses it. Use for confirmations / guards; use [`Popover`] for anything anchored to a trigger.
 */
export function Modal({
  open,
  onClose,
  zIndex = z.guard,
  id,
  initialFocusRef,
  ariaLabel,
  ariaLabelledBy,
  ariaDescribedBy,
  children,
}: ModalProps) {
  const dialogRef = useRef<HTMLDivElement>(null);

  // Move focus into the modal after its portal commits, then return focus to the invoking control when the
  // modal closes or unmounts. `open` is deliberately the only dependency: changing dialog copy must not steal
  // focus from a user who is already interacting with a field.
  useLayoutEffect(() => {
    if (!open) return;
    const previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const dialog = dialogRef.current;
    // A modal is not only a keyboard focus trap: background application content must also be unavailable
    // to touch, pointer, and screen-reader virtual navigation. The portal is a direct body child, so isolate
    // its siblings with the platform `inert` primitive and restore their exact prior state on close. The
    // snapshots make nested modals unwind correctly (the outer dialog remains inert until the inner closes).
    const background = dialog
      ? Array.from(document.body.children)
        .filter((element): element is HTMLElement => element instanceof HTMLElement && element !== dialog)
        .map((element) => ({
          element,
          inert: element.inert === true || element.hasAttribute("inert"),
          ariaHidden: element.getAttribute("aria-hidden"),
        }))
      : [];
    for (const { element } of background) {
      element.inert = true;
      element.setAttribute("aria-hidden", "true");
    }
    if (dialog) {
      // A PREFERRED TARGET THAT CANNOT TAKE FOCUS IS NOT A TARGET. Every task dialog in this editor
      // points `initialFocusRef` at its primary action, and every one of them opens with that action
      // DISABLED while the catalogue it needs is still being read — the import dialog's is disabled
      // on its first frame by construction. `focus()` on a disabled button is a silent no-op, and the
      // element it would have left focus on has just been made `inert` two statements above, so the
      // keyboard user starts outside a dialog that has trapped nothing.
      const preferred = initialFocusRef?.current;
      const reachable = preferred != null && dialog.contains(preferred) && preferred.matches(FOCUSABLE_SELECTOR);
      if (reachable) preferred.focus();
      else (focusableElements(dialog)[0] ?? dialog).focus();
    }
    return () => {
      for (const { element, inert, ariaHidden } of background) {
        element.inert = inert;
        if (ariaHidden == null) element.removeAttribute("aria-hidden");
        else element.setAttribute("aria-hidden", ariaHidden);
      }
      if (previousFocus?.isConnected) previousFocus.focus();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        onClose();
        return;
      }
      if (e.key !== "Tab") return;

      const dialog = dialogRef.current;
      if (!dialog) return;
      const focusable = focusableElements(dialog);
      if (focusable.length === 0) {
        e.preventDefault();
        dialog.focus();
        return;
      }

      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      const active = document.activeElement;
      if (!dialog.contains(active)) {
        e.preventDefault();
        (e.shiftKey ? last : first).focus();
      } else if (e.shiftKey && active === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && active === last) {
        e.preventDefault();
        first.focus();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [open, onClose]);

  if (!open) return null;
  return createPortal(
    <div
      ref={dialogRef}
      id={id}
      role="dialog"
      aria-modal="true"
      aria-label={ariaLabel}
      aria-labelledby={ariaLabelledBy}
      aria-describedby={ariaDescribedBy}
      tabIndex={-1}
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
        background: color.overlay.scrim,
        animation: `mtk-fade-in ${motion.fast}`,
      }}
    >
      {children}
    </div>,
    document.body,
  );
}
