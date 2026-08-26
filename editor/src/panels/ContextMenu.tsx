//! Registry-derived entity actions. Unavailable actions stay visible with a reason ("every no explained"),
//! and the menu follows the WAI-ARIA keyboard pattern: one tabbable item, arrow-key navigation, Home/End,
//! native Enter/Space activation, and Escape dismissal.

import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { plainReason, runEntityAction } from "./entityActions";
import { PopoverSurface } from "../theme/Popover";
import { color, font, fontSize, radius, space } from "../theme/tokens";
import type { EditorClient } from "../transport/session";
import type { ActionItem } from "../transport/protocol";

type LoadState = "loading" | "ready" | "error";

export function ContextMenu({
  client,
  id,
  onClose,
  onFocus,
}: {
  client: EditorClient;
  id: string;
  onClose: () => void;
  /** After framing the entity, hand the live camera distance up so App can raise the focus banner. */
  onFocus?: (id: string, dist: number) => void;
}) {
  const [actions, setActions] = useState<ActionItem[]>([]);
  const [loadState, setLoadState] = useState<LoadState>("loading");
  const [activeIndex, setActiveIndex] = useState(0);
  const [insideMenu, setInsideMenu] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  const itemRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const previousFocus = useRef<HTMLElement | null>(null);

  useEffect(() => {
    previousFocus.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    return () => {
      if (previousFocus.current?.isConnected) previousFocus.current.focus();
    };
  }, []);

  useEffect(() => {
    let live = true;
    setLoadState("loading");
    setActions([]);
    client
      .entityActions(id)
      .then((nextActions) => {
        if (!live) return;
        setActions(nextActions);
        setLoadState("ready");
      })
      .catch(() => {
        if (!live) return;
        setActions([]);
        setLoadState("error");
      });
    return () => {
      live = false;
    };
  }, [id, client]);

  // `Popover` already supplies the menu role in the integrated app. Render this surface as its labelled
  // action group there, while retaining a complete standalone menu contract for direct embedding/tests.
  useLayoutEffect(() => {
    setInsideMenu(Boolean(menuRef.current?.parentElement?.closest('[role="menu"]')));
  }, []);

  // A context menu owns focus while open. Start on the first action (including a disabled action, whose
  // explanation remains discoverable), then keep exactly one menu item in the document tab order.
  useLayoutEffect(() => {
    if (loadState !== "ready" || actions.length === 0) return;
    setActiveIndex(0);
    itemRefs.current[0]?.focus();
  }, [actions, loadState]);

  // The dispatch lives in `entityActions.ts`, not here. This surface and the command palette both
  // offer `actions_for`'s result, and a `switch` over the engine's action vocabulary written once per
  // surface is the same contract stated twice with nothing comparing the two.
  function dispatch(action: ActionItem) {
    if (!action.available) return;
    runEntityAction(client, action, id, { onFocus });
    onClose();
  }

  function focusItem(index: number) {
    if (actions.length === 0) return;
    const wrapped = (index + actions.length) % actions.length;
    setActiveIndex(wrapped);
    itemRefs.current[wrapped]?.focus();
  }

  function onMenuKeyDown(event: ReactKeyboardEvent<HTMLDivElement>) {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      onClose();
      return;
    }
    if (actions.length === 0) return;

    let next: number | null = null;
    if (event.key === "ArrowDown") next = activeIndex + 1;
    else if (event.key === "ArrowUp") next = activeIndex - 1;
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = actions.length - 1;

    if (next !== null) {
      event.preventDefault();
      event.stopPropagation();
      focusItem(next);
    }
  }

  return (
    <PopoverSurface
      ref={menuRef}
      id="ctxmenu"
      data-testid="ctxmenu"
      role={insideMenu ? "group" : "menu"}
      aria-label={`Actions for ${id}`}
      aria-busy={loadState === "loading"}
      onKeyDown={onMenuKeyDown}
      style={{
        minWidth: 220,
        maxWidth: `min(320px, calc(100vw - ${space.xxl}px))`,
        padding: space.xs,
        color: color.text.secondary,
        fontFamily: font.ui,
        fontSize: fontSize.label,
      }}
    >
      {loadState === "loading" && (
        <div
          data-testid="ctxmenu-loading"
          role="status"
          aria-live="polite"
          style={{ display: "flex", alignItems: "center", gap: space.sm, padding: `${space.md}px ${space.lg}px`, color: color.text.muted }}
        >
          <span className="mtk-spinner" aria-hidden="true" /> Loading actions…
        </div>
      )}
      {loadState === "error" && (
        <div
          data-testid="ctxmenu-error"
          role="alert"
          aria-live="assertive"
          style={{ padding: `${space.md}px ${space.lg}px`, color: color.danger.text }}
        >
          Actions could not be loaded. Close the menu and try again.
        </div>
      )}
      {loadState === "ready" && actions.length === 0 && (
        <div role="status" aria-live="polite" style={{ padding: `${space.md}px ${space.lg}px`, color: color.text.muted }}>
          No actions available.
        </div>
      )}
      {actions.map((action, index) => {
        const reason = !action.available && action.reason ? plainReason(action.reason) : null;
        const reasonId = `ctxreason-${id}-${action.action}`;
        return (
          <button
            key={action.action}
            ref={(node) => {
              itemRefs.current[index] = node;
            }}
            type="button"
            role="menuitem"
            className={`mtk-btn mtk-btn--ghost ctxitem${action.available ? "" : " disabled"}`}
            data-action={action.action}
            data-testid="ctxitem"
            aria-disabled={!action.available}
            aria-describedby={reason ? reasonId : undefined}
            tabIndex={index === activeIndex ? 0 : -1}
            onFocus={() => setActiveIndex(index)}
            onClick={() => dispatch(action)}
            title={reason ?? undefined}
            style={{
              width: "100%",
              justifyContent: "flex-start",
              gap: space.xs,
              padding: `${space.sm}px ${space.md}px`,
              borderColor: "transparent",
              borderRadius: radius.md,
              color: action.available ? color.text.secondary : color.text.faint,
              cursor: action.available ? "pointer" : "default",
              textAlign: "left",
              whiteSpace: "normal",
            }}
          >
            <span>{action.label}</span>
            {reason && (
              <span id={reasonId} style={{ color: color.text.muted, fontSize: fontSize.meta }}>
                — {reason}
              </span>
            )}
          </button>
        );
      })}
    </PopoverSurface>
  );
}
