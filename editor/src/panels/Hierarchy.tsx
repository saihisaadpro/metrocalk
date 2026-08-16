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

import { memo, useEffect, useMemo, useRef, useState } from "react";
import { useStore } from "zustand";
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
import { Badge, Button } from "../theme/primitives";
import { EmptyPanelState } from "../theme/workspace";
import { color, font, fontSize, space } from "../theme/tokens";

const ROW_H = 32;
const VIEW_H = 560;
const THUMB = 24;
const OVERSCAN = 6;
const INDENT = 12;
const DRAG_MIME = "text/mtk-id";

function rowDomId(id: string): string {
  return `hierarchy-item-${id.replace(/[^a-zA-Z0-9_-]/g, "-")}`;
}

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
  position,
  setSize,
  client,
  onContextMenu,
}: {
  id: string;
  top: number;
  position: number;
  setSize: number;
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
      id={rowDomId(id)}
      className={cls}
      data-testid="hrow"
      data-id={id}
      data-kind={kind}
      data-needs-binding={rel?.needsBinding ? "1" : "0"}
      role="treeitem"
      aria-level={depth + 1}
      aria-posinset={position}
      aria-setsize={setSize}
      aria-selected={primary || inMulti}
      aria-disabled={deactivated || undefined}
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
  const selectedId = useSelectedId();
  const [query, setQuery] = useState("");
  const [scrollTop, setScrollTop] = useState(0);
  const [viewHeight, setViewHeight] = useState(VIEW_H);
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const normalizedQuery = query.trim().toLocaleLowerCase();
  // Avoid subscribing the whole hierarchy to summary edits until search actually needs names.
  const searchableSummaries = useStore(projectionStore, (state) => normalizedQuery ? state.summaries : null);
  const filteredOrder = useMemo(() => {
    if (!normalizedQuery) return order;
    const summaries = searchableSummaries ?? projectionStore.getState().summaries;
    return order.filter((id) => {
      const summary = summaries[id];
      return id.toLocaleLowerCase().includes(normalizedQuery) || summary?.name?.toLocaleLowerCase().includes(normalizedQuery);
    });
  }, [normalizedQuery, order, searchableSummaries]);
  const start = Math.max(0, Math.floor(scrollTop / ROW_H) - OVERSCAN);
  const end = Math.min(filteredOrder.length, Math.ceil((scrollTop + viewHeight) / ROW_H) + OVERSCAN);
  const visible = filteredOrder.slice(start, end);

  useEffect(() => {
    setScrollTop(0);
    if (scrollRef.current) scrollRef.current.scrollTop = 0;
  }, [normalizedQuery]);

  useEffect(() => {
    const element = scrollRef.current;
    if (!element) return;
    const update = () => {
      if (element.clientHeight > 0) setViewHeight(element.clientHeight);
    };
    update();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(update);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  // Report the visible window to the thumbnail store (the visible-only gate, M2.5): only these ≤~30 rows
  // request a live thumbnail — the 5000-row list never generates 5000. Re-runs on scroll + scene change.
  useEffect(() => {
    thumbnailStore.getState().setVisible(filteredOrder.slice(start, end));
  }, [start, end, filteredOrder]);

  // Keyboard nav (improve where straightforward — preserve every existing flow): ArrowUp/Down move the
  // selection and scroll it into view; the engine selection follows (cross-panel coherence).
  function onKeyDown(e: React.KeyboardEvent) {
    if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(e.key)) return;
    if (!filteredOrder.length) return;
    e.preventDefault();
    const sel = projectionStore.getState().selectedId;
    const i = sel ? filteredOrder.indexOf(sel) : -1;
    const ni = e.key === "Home"
      ? 0
      : e.key === "End"
        ? filteredOrder.length - 1
        : e.key === "ArrowDown"
          ? Math.min(filteredOrder.length - 1, i + 1)
          : Math.max(0, i < 0 ? 0 : i - 1);
    const nid = filteredOrder[ni];
    if (!nid) return;
    projectionStore.getState().select(nid);
    void client.gizmoSelect(nid).catch(() => {});
    // Scroll the selected row into view if it's outside the window.
    const el = scrollRef.current;
    if (el) {
      const rowTop = ni * ROW_H;
      if (rowTop < el.scrollTop) el.scrollTop = rowTop;
      else if (rowTop + ROW_H > el.scrollTop + viewHeight) el.scrollTop = rowTop + ROW_H - viewHeight;
    }
  }

  return (
    <section
      data-testid="hierarchy"
      aria-labelledby="hierarchy-heading"
      style={{ display: "flex", flexDirection: "column", height: "100%", minHeight: 0 }}
    >
      <div style={{ display: "flex", alignItems: "baseline", gap: space.sm, padding: `${space.md}px ${space.lg}px ${space.xs}px`, ...text_title }}>
        <h3 id="hierarchy-heading" style={{ margin: 0, font: "inherit", fontSize: "inherit", fontWeight: "inherit" }}>Objects</h3>
        {/* `#count` remains the stable scene-count signal used by packaged acceptance. */}
        <span
          id="count"
          role="status"
          aria-live="polite"
          style={{ font: font.mono, fontSize: fontSize.meta, color: color.text.muted, fontWeight: 400, letterSpacing: 0 }}
        >
          {normalizedQuery ? `${filteredOrder.length} of ${order.length} entities` : `${order.length} entities`}
        </span>
      </div>

      {order.length > 0 && (
        <div style={{ display: "flex", gap: space.xs, padding: `${space.xs}px ${space.md}px ${space.sm}px` }}>
          <input
            type="search"
            className="mtk-input"
            aria-label="Search scene objects"
            placeholder="Search objects…"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Escape" && query) {
                event.stopPropagation();
                setQuery("");
              }
            }}
            style={{ minWidth: 0, flex: "1 1 auto" }}
          />
          {query && (
            <Button icon compact variant="ghost" aria-label="Clear object search" title="Clear search" onClick={() => setQuery("")}>
              ×
            </Button>
          )}
        </div>
      )}

      {order.length === 0 ? (
        <EmptyPanelState
          compact
          title="No objects in this scene"
          description="Add an entity above, or open Build in the Engines rail to draw, import or browse assets."
          icon="◇"
          style={{ margin: space.md }}
        />
      ) : filteredOrder.length === 0 ? (
        <EmptyPanelState
          compact
          title="No matching objects"
          description={`Nothing matches “${query.trim()}”. Try a name or object ID.`}
          icon="⌕"
          primaryAction={<Button compact variant="secondary" onClick={() => setQuery("")}>Clear search</Button>}
          style={{ margin: space.md }}
        />
      ) : (
        <div
          ref={scrollRef}
          className="mtk-scroll"
          role="tree"
          aria-label="Scene objects"
          aria-multiselectable="true"
          aria-activedescendant={selectedId && filteredOrder.includes(selectedId) ? rowDomId(selectedId) : undefined}
          tabIndex={0}
          onKeyDown={onKeyDown}
          onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}
          style={{ flex: "1 1 180px", minHeight: 160, overflowY: "auto", position: "relative", outline: "none" }}
        >
          <div style={{ height: filteredOrder.length * ROW_H, position: "relative" }}>
            {visible.map((id, index) => (
              <Row
                key={id}
                id={id}
                top={(start + index) * ROW_H}
                position={start + index + 1}
                setSize={filteredOrder.length}
                client={client}
                onContextMenu={onContextMenu}
              />
            ))}
          </div>
        </div>
      )}
    </section>
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
