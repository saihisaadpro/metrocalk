//! Registry-derived actions **for the current selection**. Unavailable actions stay visible with a
//! reason ("every no explained"), and the menu follows the WAI-ARIA keyboard pattern: one tabbable
//! item, arrow-key navigation, Home/End, native Enter/Space activation, and Escape dismissal.
//!
//! **THE MENU IS ABOUT WHAT IS SELECTED, NOT ABOUT WHAT IS UNDER THE CURSOR** (ADR-183). The editor
//! has selected sets since M10.6 and has three producers of large ones — the marquee (ADR-158),
//! Ctrl+A and `Select similar` (ADR-176), the last of which routinely answers with 378 identical
//! bolts. This menu asked the engine about one id and acted on one object, so the most direct
//! surface in the product offered the weakest verbs in it. It now states its scope in a header, and
//! every row carries `appliesTo` from the engine — the whole set, a partial count for a mixed
//! selection, or `1` for the two verbs that are honestly primary-only.

import { useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { projectionStore } from "../store/projection";
import { entityLabel, selectionSentence } from "../store/selectionText";
import { setStatus } from "../store/ui";
import { pushToast, type ToastKind } from "../store/toasts";
import { PopoverSurface } from "../theme/Popover";
import { PopupMenuItem } from "../theme/workspace";
import { color, font, fontSize, space } from "../theme/tokens";
import { deleteSelection } from "../app/deleteSelection";
import { similarTo } from "../app/selectSimilar";
import type { EditorClient } from "../transport/session";
import type { ActionItem, SelectionActions } from "../transport/protocol";

/** Soften engine-internal rejection language into concise user-facing guidance. */
function plainReason(reason: string): string {
  if (/no unmet requirement to bind/i.test(reason)) {
    return "nothing to bind yet — this object already has what it needs";
  }
  return reason;
}

/** What a row's scope adds to its label, when the scope is not simply "everything selected".
 *
 *  Said only when it is NEWS. Over one object every verb acts on that object and a suffix on every
 *  row would be noise; over a set, `Duplicate` acting on one of 378 and `Make dynamic` acting on 12
 *  of them are both facts a person needs BEFORE they click, not in the toast afterwards. */
function scopeNote(action: ActionItem, count: number): string | null {
  if (!action.available || count <= 1) return null;
  if (action.appliesTo >= count) return null;
  if (action.appliesTo === 1) return "this one only";
  return `${action.appliesTo} of ${count}`;
}

type LoadState = "loading" | "ready" | "error";

const EMPTY: SelectionActions = { count: 0, missing: 0, items: [] };

export function ContextMenu({
  client,
  ids,
  onClose,
  onFocus,
}: {
  client: EditorClient;
  /** The whole selection this menu acts on, in selection order — the primary is the LAST id. */
  ids: string[];
  onClose: () => void;
  /** After framing the entity, hand the live camera distance up so App can raise the focus banner. */
  onFocus?: (id: string, dist: number) => void;
}) {
  const [answer, setAnswer] = useState<SelectionActions>(EMPTY);
  const [loadState, setLoadState] = useState<LoadState>("loading");
  const [activeIndex, setActiveIndex] = useState(0);
  const [insideMenu, setInsideMenu] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  const itemRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const previousFocus = useRef<HTMLElement | null>(null);

  // The primary is the last id — the projection store's own convention, and the one the engine's
  // `actions_for_selection` reads for the two primary-only verbs. Said once here so the rows and the
  // header cannot disagree about which object `Duplicate` and `Bind…` are about.
  const primary = ids[ids.length - 1] ?? "";
  // A stable key for the selection, so the effect below refetches when the SET changes rather than on
  // every re-render that hands it a fresh array with the same contents.
  const key = ids.join(",");

  useEffect(() => {
    previousFocus.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    return () => {
      if (previousFocus.current?.isConnected) previousFocus.current.focus();
    };
  }, []);

  useEffect(() => {
    let live = true;
    setLoadState("loading");
    setAnswer(EMPTY);
    client
      .entityActionsFor(key.length ? key.split(",") : [])
      .then((next) => {
        if (!live) return;
        setAnswer(next);
        setLoadState("ready");
      })
      .catch(() => {
        if (!live) return;
        setAnswer(EMPTY);
        setLoadState("error");
      });
    return () => {
      live = false;
    };
  }, [key, client]);

  const actions = answer.items;

  // `Select similar` is a SELECTION verb, not a mutation, so it is answered here rather than by the
  // engine's action model: the fact it matches on (the content-addressed mesh handle, else the
  // component signature) is already in the projection, which is why it costs nothing and works
  // offline. It is in this menu because right-click on one of 378 identical bolts is exactly where a
  // person wants the other 377 — ADR-176 shipped it into the palette and named this as owed.
  const similar = useMemo(() => {
    if (!primary) return null;
    const { displayed, order } = projectionStore.getState();
    const match = similarTo(displayed, order, primary);
    if (!match) return { ids: [], reason: "this object has nothing to match on" };
    // A match of exactly ONE is the object itself, and a row reading "1 sharing the geometry of X"
    // over a selection of X is a verb that would do nothing. The palette already says this in words
    // (ADR-176's `Only this one — nothing else …`); the row says it as a refusal, because here it is
    // a control that must not look live.
    if (match.ids.length <= 1) return { ids: [], reason: `nothing else ${match.reason}` };
    return match;
  }, [primary]);
  const canSelectSimilar = (similar?.ids.length ?? 0) > 1;

  // `Popover` already supplies the menu role in the integrated app. Render this surface as its labelled
  // action group there, while retaining a complete standalone menu contract for direct embedding/tests.
  useLayoutEffect(() => {
    setInsideMenu(Boolean(menuRef.current?.parentElement?.closest('[role="menu"]')));
  }, []);

  // A context menu owns focus while open, and it opens on the first row a person can actually USE.
  //
  // It used to open on row 0 whatever that was, and the first capture is the argument: `Bind…` is the
  // first row and it is refused for most objects, so the menu opened with a strong focus ring drawn
  // around the one row that does nothing — pointing the eye at a refusal and putting Enter on it.
  // Disabled rows stay in the arrow-key ring and keep their explanations, which is what ADR-016's
  // every-"no"-explained discipline actually asks for; none of it requires starting there. If nothing
  // is available (a stale selection, everything gone), row 0 is right again — there is no better
  // answer, and the reason is on it.
  useLayoutEffect(() => {
    if (loadState !== "ready" || actions.length === 0) return;
    const first = actions.findIndex((a) => a.available);
    const start = first >= 0 ? first : 0;
    setActiveIndex(start);
    itemRefs.current[start]?.focus();
  }, [actions, loadState]);

  function feedback(message: string, kind: ToastKind = "info") {
    setStatus(message);
    pushToast(message, kind);
  }

  /** What the verb just acted on, named the way every other surface names it (`selectionText`). */
  function subject(n: number): string {
    return n === 1 ? entityLabel(primary) : `${n} objects`;
  }

  function dispatch(action: ActionItem) {
    if (!action.available) return;
    const all = ids.slice();
    switch (action.action) {
      case "remove":
        // THE WHOLE SELECTION, IN ONE TRANSACTION — the same `deleteSelection` the authoring toolbar
        // and the Delete key call, so the three routes cannot mean three different things.
        void deleteSelection(client, all).then((outcome) => feedback(outcome.sentence, outcome.ok ? "info" : "error"));
        break;
      case "duplicate":
        void client
          .duplicateEntity(primary)
          .then((newId) => feedback(newId ? `duplicated ${entityLabel(primary)}` : `couldn't duplicate ${entityLabel(primary)}`, newId ? "success" : "error"))
          .catch((error) => {
            console.error("duplicate failed", error);
            feedback(`couldn't duplicate ${entityLabel(primary)}`, "error");
          });
        break;
      case "focus":
        client.focusEntity(primary);
        void client
          .focusDebug()
          .then(([distance]) => onFocus?.(primary, distance))
          .catch(() => onFocus?.(primary, 0));
        feedback(`focused ${entityLabel(primary)}`, "info");
        break;
      case "inspect":
        projectionStore.getState().setSelection(all);
        void client.selectEntities(all).catch((error) => console.error("selectEntities failed (engine selection may be out of sync)", error));
        feedback(`inspecting ${subject(all.length)}`, "info");
        break;
      case "bind":
        projectionStore.getState().select(primary);
        void client.gizmoSelect(primary).catch((error) => console.error("gizmoSelect failed (engine selection may be out of sync)", error));
        feedback(`binding ${entityLabel(primary)}`, "info");
        break;
      case "makedynamic":
        // `appliesTo` is the engine's own count of which members qualify, so a mixed selection makes
        // exactly those into bodies rather than refusing, and the sentence counts what it did.
        void Promise.all(all.map((id) => client.makeDynamic(id).catch(() => false)))
          .then((results) => {
            const made = results.filter(Boolean).length;
            feedback(made ? `made ${subject(made)} dynamic` : "couldn't make the selection dynamic", made ? "success" : "error");
          })
          .catch((error) => {
            console.error("make_dynamic failed", error);
            feedback("couldn't make the selection dynamic", "error");
          });
        break;
      default:
        return;
    }
  }

  function selectSimilar() {
    if (!similar || similar.ids.length <= 1) return;
    projectionStore.getState().setSelection(similar.ids);
    void client.selectEntities(similar.ids).catch((error) => console.error("selectEntities failed (engine selection may be out of sync)", error));
    feedback(`Selected ${similar.ids.length} objects ${similar.reason}`, "info");
  }

  // The similar row sits after the registry's actions and shares their roving-tabindex ring, so the
  // keyboard contract does not change shape depending on whether the object has something to match on.
  const rowCount = actions.length + (loadState === "ready" && actions.length > 0 ? 1 : 0);

  function focusItem(index: number) {
    if (rowCount === 0) return;
    const wrapped = (index + rowCount) % rowCount;
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
    if (rowCount === 0) return;

    let next: number | null = null;
    if (event.key === "ArrowDown") next = activeIndex + 1;
    else if (event.key === "ArrowUp") next = activeIndex - 1;
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = rowCount - 1;

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
      aria-label={`Actions for ${selectionSentence(ids.length, ids)}`}
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
      {/* THE SUBJECT LINE. A menu whose verbs act on 378 objects has to say so before the first verb,
          not after it — `<ux_quality>` 2, feedback at the gesture. Over a single object it names the
          object, because the name is the useful fact and the count is obvious. */}
      {loadState === "ready" && answer.count > 0 && (
        <div
          data-testid="ctxmenu-subject"
          style={{
            padding: `${space.xs}px ${space.md}px ${space.sm}px`,
            color: color.text.muted,
            fontSize: fontSize.meta,
            borderBottom: `1px solid ${color.border.subtle}`,
            marginBottom: space.xs,
          }}
        >
          {selectionSentence(answer.count, ids)}
          {answer.missing > 0 && ` · ${answer.missing} no longer exist`}
        </div>
      )}
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
      {/* ONE MENU ROW, THE SHARED ONE (`PopupMenuItem`). This surface drew its own `<button>` with its
          own paddings, its own colour rules and its own idea of where a secondary fact goes — the third
          spelling of a menu row in an editor that has an ADR about having had three (ADR-151). The
          shared row is also strictly better at the two things the first capture caught: `__label` is
          `nowrap` with an ellipsis, and `__meta` is a right-hand column, so a scope can never be
          mistaken for a refusal and a verb can never wrap. `onRequestClose` is the menu's own close, so
          a REFUSED row does not fire it — `PopupMenuItem` returns before both. */}
      {actions.map((action, index) => {
        const reason = !action.available && action.reason ? plainReason(action.reason) : undefined;
        const scope = scopeNote(action, answer.count);
        return (
          <PopupMenuItem
            key={action.action}
            ref={(node: HTMLButtonElement | null) => {
              itemRefs.current[index] = node;
            }}
            className={`ctxitem${action.available ? "" : " disabled"}`}
            data-action={action.action}
            data-testid="ctxitem"
            data-applies-to={action.appliesTo}
            label={action.label}
            meta={scope}
            disabled={!action.available}
            disabledReason={reason}
            tabIndex={index === activeIndex ? 0 : -1}
            onFocus={() => setActiveIndex(index)}
            onSelect={() => dispatch(action)}
            onRequestClose={onClose}
          />
        );
      })}
      {loadState === "ready" && actions.length > 0 && (
        <PopupMenuItem
          ref={(node: HTMLButtonElement | null) => {
            itemRefs.current[actions.length] = node;
          }}
          className={`ctxitem${canSelectSimilar ? "" : " disabled"}`}
          data-action="selectsimilar"
          data-testid="ctxitem"
          data-applies-to={canSelectSimilar ? similar!.ids.length : 0}
          label="Select similar"
          description={canSelectSimilar ? similar!.reason : undefined}
          meta={canSelectSimilar ? String(similar!.ids.length) : undefined}
          disabled={!canSelectSimilar}
          disabledReason={similar?.reason ?? "this object has nothing to match on"}
          tabIndex={actions.length === activeIndex ? 0 : -1}
          onFocus={() => setActiveIndex(actions.length)}
          onSelect={selectSimilar}
          onRequestClose={onClose}
        />
      )}
    </PopoverSurface>
  );
}
