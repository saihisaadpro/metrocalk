//! Hierarchy / list panel — virtualized rows over the **summary projection** for 5k entities. Each
//! row subscribes only to its `{id,name,parentId}` summary + its own selection membership, so a field
//! edit (which changes `displayed[id]` but not the summary) never re-renders the tree, and a selection
//! change re-renders only the rows whose membership flipped. Manual windowing keeps it dependency-free;
//! only the visible ~30 rows mount.
//!
//! **M10.6 — a real tree editor:** drag a row onto another → **reparent** (`node.move`, cycle-safe on the
//! engine); shift/ctrl-click → **multi-select** (the batched multi-edit / group / delete act on it). The
//! reparent + selection re-project over the live commands; the native viewport hot path is untouched.

import { memo, useState } from "react";
import {
  projectionStore,
  useEntityOrder,
  useIsDeactivated,
  useIsMultiSelected,
  useSelectedId,
  useSummary,
} from "../store/projection";
import type { EditorClient } from "../transport/session";

const ROW_H = 22;
const VIEW_H = 560;
const OVERSCAN = 6;
const DRAG_MIME = "text/mtk-id";

const Row = memo(function Row({
  id,
  top,
  client,
  onContextMenu,
}: {
  id: string;
  top: number;
  client: EditorClient;
  onContextMenu?: (id: string, x: number, y: number) => void;
}) {
  const s = useSummary(id);
  const primary = useSelectedId() === id;
  const inMulti = useIsMultiSelected(id);
  const deactivated = useIsDeactivated(id);
  const [dropTarget, setDropTarget] = useState(false);

  // Selection: shift = range, ctrl/cmd = toggle, else single. The engine gizmo selection follows the
  // primary so the inspector/gizmo track it (no desync).
  function click(e: React.MouseEvent) {
    const st = projectionStore.getState();
    if (e.shiftKey) st.selectRange(id);
    else if (e.ctrlKey || e.metaKey) st.toggleSelect(id);
    else st.select(id);
    void client.gizmoSelect(id).catch((e) => console.error("gizmoSelect failed (engine selection may be out of sync)", e));
  }

  return (
    <div
      data-testid="hrow"
      data-id={id}
      draggable
      onClick={click}
      onContextMenu={(e) => {
        // Right-click an entity in the LIST opens the same registry-driven context menu the viewport offers
        // (it was previously reachable only by right-clicking the stage). Select the row first so the menu
        // (and the inspector) act on it.
        if (!onContextMenu) return;
        e.preventDefault();
        projectionStore.getState().select(id);
        void client.gizmoSelect(id).catch(() => {});
        onContextMenu(id, e.clientX, e.clientY);
      }}
      onDragStart={(e) => {
        e.dataTransfer.setData(DRAG_MIME, id);
        e.dataTransfer.effectAllowed = "move";
      }}
      onDragOver={(e) => {
        e.preventDefault();
        e.dataTransfer.dropEffect = "move";
        if (!dropTarget) setDropTarget(true);
      }}
      onDragLeave={() => setDropTarget(false)}
      onDrop={(e) => {
        e.preventDefault();
        setDropTarget(false);
        const dragged = e.dataTransfer.getData(DRAG_MIME);
        // Reparent the dragged entity UNDER this row (node.move). Self-drop is a no-op; the engine rejects
        // a cycle (CyclicMoveError) so dropping a parent onto its own child is refused, not corrupting.
        if (dragged && dragged !== id) client.reparentPart(dragged, id);
      }}
      style={{
        position: "absolute",
        top,
        height: ROW_H,
        left: 0,
        right: 0,
        display: "flex",
        alignItems: "center",
        padding: "0 8px",
        cursor: "pointer",
        font: "12px ui-monospace, monospace",
        // A deactivated ("deleted", recoverable) entity is dimmed + struck so Delete visibly lands (C11).
        color: deactivated ? "#6b7280" : primary || inMulti ? "#fff" : "#cfd2d6",
        textDecoration: deactivated ? "line-through" : "none",
        background: dropTarget ? "#3a5a3a" : primary ? "#2a4365" : inMulti ? "#23344f" : "transparent",
        outline: dropTarget ? "1px solid #6c6" : "none",
        whiteSpace: "nowrap",
      }}
    >
      {s?.parentId ? "· " : ""}
      {s?.name ?? id}
      {deactivated && <span style={{ opacity: 0.6, fontStyle: "italic" }}> · hidden</span>}
    </div>
  );
});

export function Hierarchy({
  client,
  onContextMenu,
}: {
  client: EditorClient;
  onContextMenu?: (id: string, x: number, y: number) => void;
}) {
  const order = useEntityOrder();
  const [scrollTop, setScrollTop] = useState(0);
  const start = Math.max(0, Math.floor(scrollTop / ROW_H) - OVERSCAN);
  const end = Math.min(order.length, Math.ceil((scrollTop + VIEW_H) / ROW_H) + OVERSCAN);
  const visible = order.slice(start, end);
  return (
    <div>
      <div style={{ padding: "8px", fontWeight: 700, color: "#e8e8e8" }}>
        hierarchy{" "}
        {/* `#count` — the scaffold's stable connect signal ("N entities") the prompt-40 harness reads. */}
        <span id="count" style={{ opacity: 0.6, fontWeight: 400 }}>
          {order.length} entities
        </span>
      </div>
      <div
        onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
        style={{ height: VIEW_H, overflowY: "auto", position: "relative" }}
      >
        <div style={{ height: order.length * ROW_H, position: "relative" }}>
          {visible.map((id, i) => (
            <Row key={id} id={id} top={(start + i) * ROW_H} client={client} onContextMenu={onContextMenu} />
          ))}
        </div>
      </div>
    </div>
  );
}
