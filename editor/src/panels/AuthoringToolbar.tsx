//! Scene-authoring verbs over the current selection. The hierarchy keeps only two compact entry points;
//! shared, focus-managed popup menus reveal creation and selection actions when they are relevant.
//! Every command retains its stable `#auth*` id and remains one undoable engine transaction.

import { useRef, useState } from "react";
import { projectionStore, useMultiSelect, useSelectedId } from "../store/projection";
import { setStatus, setClipboard, useClipboardHasContent } from "../store/ui";
import { pushToast } from "../store/toasts";
import { Icon } from "../theme/icons";
import { type ButtonVariant } from "../theme/primitives";
import { color, space } from "../theme/tokens";
import { MenuPopup, PopupMenuGroup, PopupMenuItem } from "../theme/workspace";
import type { EditorClient } from "../transport/session";

export function AuthoringToolbar({ client }: { client: EditorClient }) {
  const multiSelection = useMultiSelect();
  const primary = useSelectedId();
  const hasClipboard = useClipboardHasContent();
  const ids = multiSelection.length ? multiSelection : primary ? [primary] : [];
  const hasMultiple = multiSelection.length >= 2;
  const hasSelection = ids.length > 0;
  const [pendingAction, setPendingAction] = useState<string | null>(null);
  const pendingRef = useRef<string | null>(null);

  function select(id: string | null) {
    if (!id) return;
    projectionStore.getState().select(id);
    void client.gizmoSelect(id).catch((error) =>
      console.error("gizmoSelect failed (engine selection may be out of sync)", error),
    );
  }

  async function runAction(
    id: string,
    label: string,
    pendingLabel: string,
    operation: () => void | Promise<void>,
  ) {
    if (pendingRef.current) return;
    pendingRef.current = id;
    setPendingAction(id);
    setStatus(pendingLabel);
    try {
      await operation();
    } catch (error) {
      const actionName = label.replace(/^\+\s*/, "").toLowerCase();
      console.error(`${id} failed`, error);
      setStatus(`${actionName} failed — try again`);
      pushToast(`${actionName} failed`, "error");
    } finally {
      if (pendingRef.current === id) pendingRef.current = null;
      setPendingAction((current) => (current === id ? null : current));
    }
  }

  function action(
    id: string,
    label: string,
    onClick: () => void | Promise<void>,
    close: () => void,
    enabled = true,
    description?: string,
    unavailableReason?: string,
    variant: ButtonVariant = "ghost",
    pendingLabel = "Working…",
  ) {
    const busy = pendingAction === id;
    const blocked = pendingAction !== null;
    const disabledReason = blocked
      ? busy
        ? `${label} is in progress`
        : "Wait for the current scene action to finish"
      : enabled
        ? undefined
        : unavailableReason;
    return (
      <PopupMenuItem
        id={id}
        data-testid={id}
        variant={variant}
        label={busy ? <><span className="mtk-spinner" aria-hidden="true" /> {pendingLabel}</> : label}
        description={!busy && enabled ? description : undefined}
        disabled={!enabled || blocked}
        disabledReason={disabledReason}
        aria-busy={busy || undefined}
        onSelect={() => runAction(id, label, pendingLabel, onClick)}
        onRequestClose={close}
      />
    );
  }

  return (
    <section
      id="authbar"
      data-testid="authbar"
      aria-label="Scene authoring"
      style={{ borderBottom: `1px solid ${color.border.subtle}`, padding: `${space.sm}px ${space.md}px` }}
    >
      <div role="toolbar" aria-label="Scene actions" style={{ display: "flex", gap: space.xs }}>
        <MenuPopup
          id="authAddPopup"
          label="Add scene object"
          trigger={<><Icon name="plus" size="sm" /> Add <Icon name="chevron-down" size="sm" /></>}
          triggerProps={{
            id: "authAdd",
            "data-testid": "authAdd",
            variant: "primary",
            compact: true,
            style: { flex: "1 1 0", justifyContent: "center" },
          }}
        >
          {(close) => (
            <PopupMenuGroup label="Create">
              {action(
                "authCreate",
                "Entity",
                async () => {
                  const id = await client.createEntity(0, 1, 0, "Entity");
                  if (id) {
                    select(id);
                    setStatus("created an entity · Ctrl-Z to undo");
                    pushToast("created an entity", "success");
                  } else {
                    setStatus("couldn't create an entity");
                    pushToast("couldn't create an entity", "error");
                  }
                },
                close,
                true,
                "Empty object one metre above the world origin",
                undefined,
                "ghost",
                "Creating entity…",
              )}
              {action(
                "authLight",
                "Light",
                async () => {
                  const id = await client.addLight("point", 0, 4, 0, 1, 0.96, 0.9, 60);
                  if (id) {
                    select(id);
                    setStatus("added a light · Ctrl-Z to undo");
                    pushToast("added a point light", "success");
                  } else {
                    setStatus("couldn't add a light");
                    pushToast("couldn't add a light", "error");
                  }
                },
                close,
                true,
                "Warm point light above the origin",
                undefined,
                "ghost",
                "Adding light…",
              )}
            </PopupMenuGroup>
          )}
        </MenuPopup>

        <MenuPopup
          id="authActionsPopup"
          label="Selection and clipboard actions"
          trigger={<>Actions{hasSelection ? ` · ${ids.length}` : ""} <Icon name="chevron-down" size="sm" /></>}
          triggerProps={{
            id: "authMore",
            "data-testid": "authoring-more",
            variant: "secondary",
            compact: true,
            style: { flex: "1 1 0", justifyContent: "center" },
          }}
        >
          {(close) => (
            <>
              <PopupMenuGroup label={<span id="auth-selection-heading">Selection</span>}>
                {action(
                  "authDuplicate",
                  hasMultiple ? `Duplicate ${ids.length}` : "Duplicate",
                  async () => {
                    if (!ids.length) return;
                    // ADR-169 — ONE transaction for the whole selection. This trigger has printed
                    // "Actions · 12" since M10.6 while this verb cloned the primary alone, so the menu
                    // counted twelve and did one; N calls would have fixed the count and left twelve
                    // undo steps behind a toast that promises one.
                    const clones = await client.duplicateEntities(ids);
                    if (!clones.length) {
                      setStatus("couldn't duplicate the selection");
                      pushToast("couldn't duplicate the selection", "error");
                      return;
                    }
                    select(clones[0]);
                    const what = clones.length === 1 ? "duplicated" : `duplicated ${clones.length}`;
                    setStatus(`${what} · Ctrl-Z to undo`);
                    pushToast(`${what} · Ctrl-Z to undo`, "success");
                  },
                  close,
                  hasSelection,
                  hasMultiple
                    ? `Clone all ${ids.length} selected objects in one undoable edit`
                    : "Clone the selected object",
                  "Select an object to duplicate",
                  "ghost",
                  "Duplicating…",
                )}
                {action(
                  "authDelete",
                  hasMultiple ? `Delete ${ids.length}` : "Delete",
                  async () => {
                    if (!ids.length) return;
                    // ADR-169 — the whole selection, in ONE transaction, so the "Ctrl-Z restores it"
                    // this control promises is true for all of it and not only for the last object.
                    const ok = await client.deleteDeactivateMany(ids);
                    if (!ok) {
                      setStatus("couldn't delete the selection");
                      pushToast("couldn't delete the selection", "error");
                      return;
                    }
                    projectionStore.getState().markDeactivated(ids);
                    projectionStore.getState().select(null);
                    const what = ids.length === 1 ? "deactivated" : `deactivated ${ids.length}`;
                    setStatus(`${what} — recoverable with Ctrl-Z`);
                    pushToast(`${what} (recoverable) · Ctrl-Z`, "info");
                  },
                  close,
                  hasSelection,
                  hasMultiple
                    ? `Deactivate all ${ids.length} selected objects; one Ctrl-Z restores them`
                    : "Deactivate the selection; Ctrl-Z restores it",
                  "Select an object to delete",
                  "danger",
                  "Deleting…",
                )}
                {action(
                  "authGroup",
                  "Group",
                  async () => {
                    const group = await client.groupEntities(ids, "Group");
                    if (group) {
                      select(group);
                      setStatus(`grouped ${ids.length} · Ctrl-Z to undo`);
                      pushToast(`grouped ${ids.length} · Ctrl-Z to undo`, "success");
                    } else {
                      setStatus("couldn't group the selection");
                      pushToast("couldn't group the selection", "error");
                    }
                  },
                  close,
                  hasMultiple,
                  "Place the selected objects in a new group",
                  "Shift/Ctrl-click at least two objects to group",
                  "ghost",
                  "Grouping…",
                )}
                {action(
                  "authUngroup",
                  "Ungroup",
                  async () => {
                    if (!primary) return;
                    const ok = await client.ungroupEntity(primary);
                    if (ok) {
                      setStatus("ungrouped · Ctrl-Z to undo");
                      pushToast("ungrouped", "info");
                    } else {
                      setStatus("couldn't ungroup the selection");
                      pushToast("couldn't ungroup the selection", "error");
                    }
                  },
                  close,
                  !!primary,
                  "Move this group's children up one level",
                  "Select a group to ungroup",
                  "ghost",
                  "Ungrouping…",
                )}
                {action(
                  "authNudge",
                  "Set height to 5 m",
                  async () => {
                    if (!ids.length) return;
                    const r = await client.multiEdit(ids, "Transform", "y", 5);
                    if (!r.ok) {
                      // The engine's own sentence, not a generic one: a batch can be refused because
                      // the selection disagrees about the component, and "couldn't move" says none of
                      // that (ADR-169 / `<ux_quality>` 4).
                      const said = r.reason ?? "couldn't move the selection";
                      setStatus(said);
                      pushToast(said, "error");
                      return;
                    }
                    setStatus(`moved ${r.changed} (batched) · Ctrl-Z to undo`);
                    pushToast(`moved ${r.changed} together`, "success");
                  },
                  close,
                  hasSelection,
                  // IT SETS, IT DOES NOT MOVE. `multiEdit` writes ONE value to N entities, so this
                  // assigns y = 5 — it always did, and the label read "Move up 5 m", which is true
                  // only for an object that happens to be at y = 0 and is a DOWNWARD move above it.
                  "Set every selected object to 5 m high, in one undoable edit",
                  "Select one or more objects",
                  "ghost",
                  "Moving…",
                )}
              </PopupMenuGroup>

              <PopupMenuGroup label={<span id="auth-clipboard-heading">Clipboard</span>}>
                {action(
                  "authCopy",
                  "Copy",
                  () => {
                    if (!primary) return;
                    client.copySubtree(primary);
                    setClipboard(true);
                    pushToast("copied", "info");
                  },
                  close,
                  !!primary,
                  "Copy the selected subtree",
                  "Select an object to copy",
                )}
                {action(
                  "authCut",
                  "Cut",
                  async () => {
                    if (!primary) return;
                    const ok = await client.cutSubtree(primary);
                    if (!ok) {
                      setStatus("couldn't cut the selection");
                      pushToast("couldn't cut the selection", "error");
                      return;
                    }
                    setClipboard(true);
                    projectionStore.getState().markDeactivated([primary]);
                    projectionStore.getState().select(null);
                    setStatus("cut · recoverable with Ctrl-Z");
                    pushToast("cut (recoverable)", "info");
                  },
                  close,
                  !!primary,
                  "Copy then deactivate the selected subtree",
                  "Select an object to cut",
                  "ghost",
                  "Cutting…",
                )}
                {action(
                  "authPaste",
                  "Paste",
                  async () => {
                    const pasted = await client.pasteClipboard();
                    select(pasted);
                    if (!pasted) return;
                    setStatus("pasted · Ctrl-Z to undo");
                    pushToast("pasted · Ctrl-Z to undo", "success");
                  },
                  close,
                  hasClipboard,
                  "Paste a fresh copy",
                  "Copy or cut something first",
                  "ghost",
                  "Pasting…",
                )}
              </PopupMenuGroup>
            </>
          )}
        </MenuPopup>
      </div>
    </section>
  );
}
