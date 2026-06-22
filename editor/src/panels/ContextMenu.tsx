//! ContextMenu (M3.3) — right-click an entity → its registry-derived actions appear, each one **explained**:
//! an unavailable action is greyed WITH its reason ("every 'no' explained", ADR-016), so the menu teaches
//! why an edit is blocked rather than silently hiding it. Reads `entity_actions(id)` over the `EditorClient`,
//! dispatches the chosen verb through the contract, then closes + posts a status line. A disabled row is
//! inert (no dispatch, no close). Mirrors the vanilla scaffold's stable `.ctxitem` / `data-action` hooks.

import { useEffect, useState } from "react";
import { projectionStore } from "../store/projection";
import { setStatus } from "../store/ui";
import { pushToast, type ToastKind } from "../store/toasts";
import type { EditorClient } from "../transport/session";
import type { ActionItem } from "../transport/protocol";

/** Soften the engine-internal phrasings of an "every-no-explained" reason into plain user words (C11).
 *  Unknown reasons pass through unchanged (the backend already explains them as a sentence). */
function plainReason(reason: string): string {
  if (/no unmet requirement to bind/i.test(reason)) {
    return "nothing to bind yet — this object already has what it needs";
  }
  return reason;
}

export function ContextMenu({
  client,
  id,
  onClose,
}: {
  client: EditorClient;
  id: string;
  onClose: () => void;
}) {
  const [actions, setActions] = useState<ActionItem[]>([]);

  useEffect(() => {
    let live = true;
    client
      .entityActions(id)
      .then((a) => {
        if (live) setActions(a);
      })
      .catch(() => {
        if (live) setActions([]);
      });
    return () => {
      live = false;
    };
  }, [id, client]);

  // Feedback at the gesture (C11): a transient toast next to the action AND the footer status line.
  function feedback(msg: string, kind: ToastKind = "info") {
    setStatus(msg);
    pushToast(msg, kind);
  }

  function dispatch(a: ActionItem) {
    if (!a.available) return; // a disabled row does NOTHING — no dispatch, no close
    switch (a.action) {
      case "remove":
        client.removeEntity(id);
        feedback("removed " + id + " · Ctrl-Z to undo", "info");
        break;
      case "duplicate":
        client.duplicateEntity(id);
        feedback("duplicated " + id, "success");
        break;
      case "focus":
        client.focusEntity(id);
        feedback("focused " + id, "info");
        break;
      case "inspect":
        projectionStore.getState().select(id);
        feedback("inspecting " + id, "info");
        break;
      case "bind":
        projectionStore.getState().select(id); // opens the reveal
        feedback("binding " + id, "info");
        break;
      case "makedynamic":
        client.makeDynamic(id);
        feedback("made " + id + " dynamic", "success");
        break;
      default:
        return; // unknown verb → don't close
    }
    onClose();
  }

  return (
    <div id="ctxmenu" data-testid="ctxmenu" style={{ minWidth: 180, background: "#161a26", border: "1px solid #2a3550", borderRadius: 6, padding: 4, fontSize: 13 }}>
      {actions.map((a) => (
        <div
          key={a.action}
          className={a.available ? "ctxitem" : "ctxitem disabled"}
          data-action={a.action}
          data-testid="ctxitem"
          aria-disabled={!a.available}
          onClick={() => dispatch(a)}
          title={a.available || !a.reason ? undefined : plainReason(a.reason)}
          style={{
            padding: "5px 8px",
            borderRadius: 4,
            cursor: a.available ? "pointer" : "default",
            color: a.available ? "#cde" : "#667",
          }}
        >
          {a.available || !a.reason ? a.label : a.label + " — " + plainReason(a.reason)}
        </div>
      ))}
    </div>
  );
}
