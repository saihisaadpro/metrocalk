//! ContextMenu (M3.3) — right-click an entity → its registry-derived actions appear, each one **explained**:
//! an unavailable action is greyed WITH its reason ("every 'no' explained", ADR-016), so the menu teaches
//! why an edit is blocked rather than silently hiding it. Reads `entity_actions(id)` over the `EditorClient`,
//! dispatches the chosen verb through the contract, then closes + posts a status line. A disabled row is
//! inert (no dispatch, no close). Mirrors the vanilla scaffold's stable `.ctxitem` / `data-action` hooks.

import { useEffect, useState } from "react";
import { projectionStore } from "../store/projection";
import { setStatus } from "../store/ui";
import type { EditorClient } from "../transport/session";
import type { ActionItem } from "../transport/protocol";

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

  function dispatch(a: ActionItem) {
    if (!a.available) return; // a disabled row does NOTHING — no dispatch, no close
    switch (a.action) {
      case "remove":
        client.removeEntity(id);
        setStatus("removed " + id + " · Ctrl-Z to undo");
        break;
      case "duplicate":
        client.duplicateEntity(id);
        setStatus("duplicated " + id);
        break;
      case "focus":
        client.focusEntity(id);
        setStatus("focused " + id);
        break;
      case "inspect":
        projectionStore.getState().select(id);
        setStatus("inspecting " + id);
        break;
      case "bind":
        projectionStore.getState().select(id); // opens the reveal
        setStatus("binding " + id);
        break;
      case "makedynamic":
        client.makeDynamic(id);
        setStatus("made " + id + " dynamic");
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
          style={{
            padding: "5px 8px",
            borderRadius: 4,
            cursor: a.available ? "pointer" : "default",
            color: a.available ? "#cde" : "#667",
          }}
        >
          {a.available || !a.reason ? a.label : a.label + " — " + a.reason}
        </div>
      ))}
    </div>
  );
}
