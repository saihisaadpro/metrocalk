//! AuthoringToolbar (M10.6 / ADR-036) — the compose-a-scene verbs over the current selection, each one
//! undoable transaction on the live engine (Ctrl-Z reverts). Acts on the **multi-selection** (group,
//! nudge) or the **primary** (ungroup, delete, copy, cut). Reparent lives in the hierarchy (drag); create
//! and paste don't need a selection. Stable ids (`#auth*`) for the prompt-40 gate. The verbs are the same
//! `editor-shell::capscene` functions the headless tests cover — this surfaces them live.

import { projectionStore, useMultiSelect, useSelectedId } from "../store/projection";
import { setStatus } from "../store/ui";
import { pushToast } from "../store/toasts";
import type { EditorClient } from "../transport/session";

export function AuthoringToolbar({ client }: { client: EditorClient }) {
  const sel = useMultiSelect();
  const primary = useSelectedId();
  const ids = sel.length ? sel : primary ? [primary] : [];
  const multi = sel.length >= 2;
  const hasOne = ids.length >= 1;

  const select = (id: string | null) => {
    if (id) {
      projectionStore.getState().select(id);
      void client.gizmoSelect(id).catch((e) => console.error("gizmoSelect failed (engine selection may be out of sync)", e));
    }
  };

  const btn = (
    id: string,
    label: string,
    onClick: () => void,
    enabled = true,
    title?: string,
  ) => (
    <button
      id={id}
      data-testid={id}
      disabled={!enabled}
      title={title}
      onClick={onClick}
      style={{
        background: enabled ? "#1c2433" : "#171b24",
        color: enabled ? "#cde" : "#566",
        border: "1px solid #2a3550",
        borderRadius: 4,
        padding: "3px 8px",
        cursor: enabled ? "pointer" : "not-allowed",
        font: "11px ui-monospace, monospace",
      }}
    >
      {label}
    </button>
  );

  return (
    <div
      id="authbar"
      data-testid="authbar"
      style={{ display: "flex", gap: 4, padding: "4px 8px", flexWrap: "wrap", borderBottom: "1px solid #222a3a" }}
    >
      {btn("authCreate", "+ Entity", async () => {
        const id = await client.createEntity(0, 1, 0, "Entity");
        select(id);
        setStatus("created an entity · Ctrl-Z to undo");
        pushToast("created an entity", "success");
      })}
      {/* M11.3 — author a Light entity (a point light above the origin, warm-white). One undoable commit. */}
      {btn("authLight", "+ Light", async () => {
        const id = await client.addLight("point", 0, 4, 0, 1, 0.96, 0.9, 60);
        select(id);
        setStatus("added a light · Ctrl-Z to undo");
        pushToast("added a point light", "success");
      })}
      {btn(
        "authGroup",
        "Group",
        async () => {
          const g = await client.groupEntities(ids, "Group");
          select(g);
          pushToast(`grouped ${ids.length} · Ctrl-Z to undo`, "success");
        },
        multi,
        multi ? undefined : "shift/ctrl-click ≥2 in the hierarchy to group",
      )}
      {btn(
        "authUngroup",
        "Ungroup",
        () => {
          if (primary) void client.ungroupEntity(primary).then((ok) => ok && pushToast("ungrouped", "info"));
        },
        !!primary,
      )}
      {btn(
        "authNudge",
        "Move ↑ (all)",
        () => {
          // A batched multi-edit: set Transform.y on EVERY selected entity in ONE undoable tx (the
          // adversarial guard: one Ctrl-Z restores all N, not just one).
          if (ids.length) {
            void client.multiEdit(ids, "Transform", "y", 5).then((ok) => {
              if (ok) {
                setStatus(`moved ${ids.length} (batched) · Ctrl-Z to undo`);
                pushToast(`moved ${ids.length} together`, "success");
              }
            });
          }
        },
        hasOne,
        "move every selected entity together (batched, undoable)",
      )}
      {btn(
        "authDelete",
        "Delete",
        () => {
          if (primary)
            void client.deleteDeactivate(primary).then((ok) => ok && pushToast("deleted (recoverable) · Ctrl-Z", "info"));
        },
        !!primary,
        "deactivate-not-destroy — Ctrl-Z restores it",
      )}
      {btn("authCopy", "Copy", () => {
        if (primary) {
          client.copySubtree(primary);
          pushToast("copied", "info");
        }
      }, !!primary)}
      {btn("authCut", "Cut", () => {
        if (primary) void client.cutSubtree(primary).then((ok) => ok && pushToast("cut (recoverable)", "info"));
      }, !!primary)}
      {btn("authPaste", "Paste", async () => {
        const p = await client.pasteClipboard();
        select(p);
        if (p) pushToast("pasted · Ctrl-Z to undo", "success");
      })}
    </div>
  );
}
