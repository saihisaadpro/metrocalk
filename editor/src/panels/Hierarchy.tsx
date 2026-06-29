//! Hierarchy / list panel — virtualized rows over the **summary projection** for 5k entities. Each row
//! subscribes only to its `{id,name,parentId,kind,rel}` summary + its own selection membership, so a field
//! edit (which changes `displayed[id]` but not the summary) never re-renders the tree, and a selection or a
//! relational-status change re-renders only the affected rows. Manual windowing keeps it dependency-free;
//! only the visible ~30 rows mount.
//!
//! **M14.2 (ADR-058) — the accepted tier:** every row shows a **live viewport thumbnail** of its entity (the
//! flagship — the real render, falling back to a styled type-icon when not ready) and the scene's **live
//! relational truth** keyed off the **real `/core` projection** (the C6 closure): an entity that *needs a
//! binding*, its *bound* count, group membership, and the active/selected entity — so the user can debug the
//! scene graph by **looking**. Only the visible rows request a thumbnail (M2.5: the 5000-row list never
//! generates 5000 thumbnails); editing one entity refreshes only that one entity's thumbnail.
//!
//! **M10.6 — a real tree editor:** drag a row onto another → **reparent** (`node.move`, cycle-safe on the
//! engine); shift/ctrl-click → **multi-select**; ArrowUp/Down navigate the selection (scrolled into view).

import { memo, useEffect, useRef, useState } from "react";
import {
  projectionStore,
  useEntityOrder,
  useIsDeactivated,
  useIsMultiSelected,
  useSelectedId,
  useSummary,
} from "../store/projection";
import { thumbnailStore } from "../store/thumbnails";
import type { EditorClient } from "../transport/session";
import type { EntitySummary } from "../transport/protocol";
import { Thumbnail } from "../theme/Thumbnail";
import { Badge } from "../theme/primitives";
import { color, font, fontSize, space } from "../theme/tokens";

const ROW_H = 32;
const VIEW_H = 560;
const THUMB = 24;
const OVERSCAN = 6;
const INDENT = 12;
const DRAG_MIME = "text/mtk-id";

/** The type-icon/thumbnail kind for a summary — prefers the server-classified `kind`, else derives a sane
 *  one from the relational summary (so a row needs no component subscription — M2.5 safe). */
function kindOf(s: EntitySummary | undefined): string {
  if (s?.kind) return s.kind;
  if (s?.rel?.isGroup) return "group";
  if (s?.rel?.needsBinding) return "requirer";
  return "mesh";
}

/** Nesting depth via a bounded, non-reactive parent walk (the summary carries only `parentId`; the row
 *  re-renders when ITS parent changes — an ancestor reparent re-projects the moved subtree). */
function depthOf(id: string): number {
  const sums = projectionStore.getState().summaries;
  let d = 0;
  let cur = sums[id]?.parentId ?? null;
  while (cur && d < 16) {
    d += 1;
    cur = sums[cur]?.parentId ?? null;
  }
  return d;
}

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

  const rel = s?.rel;
  const kind = kindOf(s);
  const depth = s?.parentId ? depthOf(id) : 0;
  const named = !!s?.name && s.name !== id;

  // Selection: shift = range, ctrl/cmd = toggle, else single. The engine gizmo selection follows the
  // primary so the inspector/gizmo/viewport track it (cross-panel coherence, no desync).
  function click(e: React.MouseEvent) {
    const st = projectionStore.getState();
    if (e.shiftKey) st.selectRange(id);
    else if (e.ctrlKey || e.metaKey) st.toggleSelect(id);
    else st.select(id);
    void client.gizmoSelect(id).catch((e) => console.error("gizmoSelect failed (engine selection may be out of sync)", e));
  }

  const cls = ["mtk-hrow", primary && "is-selected", !primary && inMulti && "is-multi", dropTarget && "is-drop"].filter(Boolean).join(" ");

  return (
    <div
      className={cls}
      data-testid="hrow"
      data-id={id}
      data-kind={kind}
      data-needs-binding={rel?.needsBinding ? "1" : "0"}
      draggable
      onClick={click}
      onContextMenu={(e) => {
        // Right-click an entity in the LIST opens the same registry-driven context menu the viewport offers.
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
        gap: space.sm,
        padding: `0 ${space.md}px 0 ${space.md + depth * INDENT}px`,
        cursor: "pointer",
        opacity: deactivated ? 0.5 : 1,
      }}
    >
      <Thumbnail id={id} kind={kind} size={THUMB} selected={primary} title={named ? s?.name : id} />
      <span
        style={{
          flex: 1,
          minWidth: 0,
          overflow: "hidden",
          textOverflow: "ellipsis",
          whiteSpace: "nowrap",
          font: named ? font.ui : font.mono,
          fontSize: fontSize.body,
          color: deactivated ? color.text.muted : primary || inMulti ? color.text.primary : color.text.secondary,
          textDecoration: deactivated ? "line-through" : "none",
        }}
      >
        {named ? s?.name : id}
      </span>
      {/* Live relational truth (C6) — the actionable requirer signal + the bound count, each explained. */}
      {!deactivated && rel?.needsBinding && (
        <Badge tone="accent" title={`requires ${rel.requires.join(", ") || "a capability"} — not yet bound (click to bind)`}>
          needs bind
        </Badge>
      )}
      {!deactivated && rel && rel.bound > 0 && (
        <Badge tone="success" title={`${rel.bound} active binding${rel.bound > 1 ? "s" : ""}`}>
          ⛓ {rel.bound}
        </Badge>
      )}
      {deactivated && <span style={{ ...text_hidden }}>hidden</span>}
    </div>
  );
});

const text_hidden: React.CSSProperties = { font: font.mono, fontSize: fontSize.micro, color: color.text.faint, fontStyle: "italic" };

export function Hierarchy({
  client,
  onContextMenu,
}: {
  client: EditorClient;
  onContextMenu?: (id: string, x: number, y: number) => void;
}) {
  const order = useEntityOrder();
  const [scrollTop, setScrollTop] = useState(0);
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const start = Math.max(0, Math.floor(scrollTop / ROW_H) - OVERSCAN);
  const end = Math.min(order.length, Math.ceil((scrollTop + VIEW_H) / ROW_H) + OVERSCAN);
  const visible = order.slice(start, end);

  // Report the visible window to the thumbnail store (the visible-only gate, M2.5): only these ≤~30 rows
  // request a live thumbnail — the 5000-row list never generates 5000. Re-runs on scroll + scene change.
  useEffect(() => {
    thumbnailStore.getState().setVisible(order.slice(start, end));
  }, [start, end, order]);

  // Keyboard nav (improve where straightforward — preserve every existing flow): ArrowUp/Down move the
  // selection and scroll it into view; the engine selection follows (cross-panel coherence).
  function onKeyDown(e: React.KeyboardEvent) {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    if (!order.length) return;
    e.preventDefault();
    const sel = projectionStore.getState().selectedId;
    const i = sel ? order.indexOf(sel) : -1;
    const ni = e.key === "ArrowDown" ? Math.min(order.length - 1, i + 1) : Math.max(0, i < 0 ? 0 : i - 1);
    const nid = order[ni];
    if (!nid) return;
    projectionStore.getState().select(nid);
    void client.gizmoSelect(nid).catch(() => {});
    // Scroll the selected row into view if it's outside the window.
    const el = scrollRef.current;
    if (el) {
      const rowTop = ni * ROW_H;
      if (rowTop < el.scrollTop) el.scrollTop = rowTop;
      else if (rowTop + ROW_H > el.scrollTop + VIEW_H) el.scrollTop = rowTop + ROW_H - VIEW_H;
    }
  }

  return (
    <div data-testid="hierarchy">
      <div style={{ display: "flex", alignItems: "baseline", gap: space.sm, padding: `${space.md}px ${space.lg}px`, ...text_title }}>
        <span>Scene</span>
        {/* `#count` — the scaffold's stable connect signal ("N entities") the prompt-40 harness reads. */}
        <span id="count" style={{ font: font.mono, fontSize: fontSize.meta, color: color.text.muted, fontWeight: 400, letterSpacing: 0 }}>
          {order.length} entities
        </span>
      </div>
      <div
        ref={scrollRef}
        className="mtk-scroll"
        tabIndex={0}
        onKeyDown={onKeyDown}
        onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
        style={{ height: VIEW_H, overflowY: "auto", position: "relative", outline: "none" }}
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

const text_title: React.CSSProperties = {
  font: font.ui,
  fontSize: fontSize.meta,
  fontWeight: 600,
  letterSpacing: 0.4,
  textTransform: "uppercase",
  color: color.text.secondary,
};
