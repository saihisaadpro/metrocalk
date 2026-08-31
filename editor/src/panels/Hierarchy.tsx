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
//!
//! **Finding, and then acting on what was found (ADR-185).** The box searches by what an object IS as
//! well as by what it is called (`../app/sceneQuery`), the chips beneath it are the kinds this
//! particular scene actually contains, and the result carries a verb: `Select all N` states the whole
//! match through ADR-158's one seam, so every verb the editor already has — the Inspector's shared-field
//! edit, Delete, the object menu — applies to a question the user typed. Shift-click ranges and the
//! keyboard both walk the rows that are DRAWN, never the scene behind the filter.

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
import { useObjectSearchRequest } from "../store/find";
import {
  FILTER_KEYS,
  facetsOf,
  matchesQuery,
  parseQuery,
  queryHasFacet,
  queryIsEmpty,
  queryNeedsComponents,
  toggleFacet,
} from "../app/sceneQuery";
import { stateSelection } from "../app/stateSelection";
import type { EditorClient } from "../transport/session";
import type { EntitySummary } from "../transport/protocol";
import { Thumbnail } from "../theme/Thumbnail";
import { Icon } from "../theme/icons";
import { Badge, Button, SearchField } from "../theme/primitives";
import { EmptyPanelState } from "../theme/workspace";
import { color, font, fontSize, space } from "../theme/tokens";

const ROW_H = 32;
const VIEW_H = 560;
/** How many facet chips are drawn before the rest go behind a named toggle.
 *
 *  THE PANEL'S JOB IS OBJECTS. Measured in the packaged `.exe` on the 27-object sample scene, which
 *  classifies into SEVEN kinds: the chips took 98 px of a 355 px panel — 28% of the outliner spent on
 *  the way in rather than on what it is a list of — and the row area was pushed to its 180 px flex
 *  basis, showing five rows of twenty-seven. So the collapsed row is capped at three CONTROLS: three
 *  facets when there are three or fewer, otherwise two facets and a toggle naming how many are
 *  hidden. Never a silent truncation — the count is on the control.
 *
 *  On the content this is actually for the cap costs nothing: a CAD import is meshes end to end, one
 *  kind is the whole scene, and `facetsOf` does not offer a facet that matches everything. */
const FACETS_COLLAPSED = 3;
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
  rangeScope,
  client,
  onContextMenu,
}: {
  id: string;
  top: number;
  position: number;
  setSize: number;
  /** The rows currently DRAWN, in order — what a shift-click range may span. Not `order`: see
   *  `selectRange`. Stable by reference while the query and the scene are unchanged. */
  rangeScope: readonly string[];
  client: EditorClient;
  onContextMenu?: (ids: string[], x: number, y: number) => void;
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

  // Selection: shift = range, ctrl/cmd = toggle, else single.
  //
  // THE WHOLE SELECTION GOES TO THE ENGINE, not just the primary. This used to send `gizmoSelect(id)`
  // — one id — after building a multi-selection in the store, so ctrl-clicking forty rows highlighted
  // forty rows in the list and outlined exactly ONE object in the 3D view. The list and the stage were
  // two selections that never compared notes, and the stage's answer was the one the user was looking
  // at. `selectEntities` states the whole set through the same seam a viewport click uses.
  function click(e: React.MouseEvent) {
    const st = projectionStore.getState();
    if (e.shiftKey) st.selectRange(id, rangeScope);
    else if (e.ctrlKey || e.metaKey) st.toggleSelect(id);
    else st.select(id);
    const ids = projectionStore.getState().multiSelect;
    void client
      .selectEntities(ids.length ? ids : [id])
      .catch((e) => console.error("selectEntities failed (engine selection may be out of sync)", e));
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
        //
        // RIGHT-CLICKING A SELECTED ROW MUST NOT DESTROY THE SELECTION. This called `select(id)`
        // unconditionally: ctrl-click forty rows, right-click one of them, and the other thirty-nine
        // were gone before the menu had opened — over a set the user had just spent forty gestures
        // building. Every direct-manipulation surface a person has used (a file manager, Blender,
        // Unity, Figma) draws the same line: a member of the selection acts on the selection; a
        // NON-member replaces it, because right-clicking somewhere else is a statement about where
        // you are pointing.
        if (!onContextMenu) return;
        e.preventDefault();
        const st = projectionStore.getState();
        const ids = st.multiSelect.includes(id)
          ? st.multiSelect
          : (st.select(id), projectionStore.getState().multiSelect);
        // The whole set goes to the engine, for the same reason the left-click handler above sends it:
        // a list that highlights forty and a stage that outlines one are two selections.
        void client.selectEntities(ids.length ? ids : [id]).catch(() => {});
        onContextMenu(ids.length ? ids : [id], e.clientX, e.clientY);
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
          <Icon name="link" size="sm" /> {rel.bound}
        </Badge>
      )}
      {deactivated && <span style={{ ...text_hidden }}>hidden</span>}
    </div>
  );
});

const text_hidden: React.CSSProperties = { font: font.mono, fontSize: fontSize.micro, color: color.text.muted, fontStyle: "italic" };

export function Hierarchy({
  client,
  onContextMenu,
}: {
  client: EditorClient;
  onContextMenu?: (ids: string[], x: number, y: number) => void;
}) {
  const order = useEntityOrder();
  const selectedId = useSelectedId();
  const [query, setQuery] = useState("");
  const [scrollTop, setScrollTop] = useState(0);
  const [viewHeight, setViewHeight] = useState(VIEW_H);
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const searchRef = useRef<HTMLInputElement | null>(null);
  const parsed = useMemo(() => parseQuery(query), [query]);
  const filtering = !queryIsEmpty(parsed);
  const wantsComponents = queryNeedsComponents(parsed);
  // The summary map — `{name, kind, rel}` per entity, the same one a row renders from. Subscribed
  // unconditionally now, because the facet chips report LIVE counts and a count that lags the scene is
  // a lie a person acts on. It costs one O(N) pass per projection delta (two map reads per entity);
  // rows stay individually memoized, and `filteredOrder` is returned BY REFERENCE as `order` when
  // nothing is being searched, so an unfiltered list re-renders no rows at all.
  const summaries = useStore(projectionStore, (state) => state.summaries);
  // The COMPONENT map is a different matter and stays conditional: it changes on every field edit, and
  // only a `has:` query needs it (M2.5 — a row renders from its summary).
  const searchableComponents = useStore(projectionStore, (state) => (wantsComponents ? state.displayed : null));
  const filteredOrder = useMemo(() => {
    if (!filtering) return order;
    const displayed = searchableComponents ?? undefined;
    return order.filter((id) => matchesQuery(parsed, id, summaries[id], displayed?.[id]));
  }, [filtering, order, parsed, summaries, searchableComponents]);
  // What this scene can be narrowed BY, counted from the scene itself — the discoverability half. A
  // query language nobody is taught is a query language nobody uses.
  const facets = useMemo(() => facetsOf(order, summaries), [order, summaries]);
  const [facetsOpen, setFacetsOpen] = useState(false);
  // A PRESSED facet is always drawn, whatever the cap says: a filter whose own control is hidden is a
  // state with no way out of it, and the chip is the way out.
  const shownFacets = useMemo(() => {
    if (facetsOpen || facets.length <= FACETS_COLLAPSED) return facets;
    const head = facets.slice(0, FACETS_COLLAPSED - 1);
    const pressed = facets.filter((f) => queryHasFacet(parsed, f) && !head.includes(f));
    return [...head, ...pressed];
  }, [facets, facetsOpen, parsed]);
  const hiddenFacets = facets.length - shownFacets.length;
  const focusRequest = useObjectSearchRequest();

  // Ctrl/Cmd-F lands here. Select the existing text too, so the chord starts a NEW search rather than
  // appending to the last one — the behaviour of every find field a person has used.
  useEffect(() => {
    if (!focusRequest) return;
    const field = searchRef.current;
    if (!field) return;
    field.focus();
    field.select();
  }, [focusRequest]);

  const start = Math.max(0, Math.floor(scrollTop / ROW_H) - OVERSCAN);
  const end = Math.min(filteredOrder.length, Math.ceil((scrollTop + viewHeight) / ROW_H) + OVERSCAN);
  const visible = filteredOrder.slice(start, end);

  useEffect(() => {
    setScrollTop(0);
    if (scrollRef.current) scrollRef.current.scrollTop = 0;
  }, [query]);

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
    // ONE seam for stating a selection (ADR-158). This sent `gizmoSelect(nid)` — the single-id route
    // the click handler above carries a paragraph about having replaced — so the keyboard and the
    // mouse reached the engine two different ways to say the same thing.
    void client.selectEntities([nid]).catch(() => {});
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
        {/* `#count` remains the stable scene-count signal used by packaged acceptance — which reads it
            with `/(\d+)\s+entities/` in seven `.exe` specs, i.e. keyed on COPY, the thing
            `<test_and_ci_discipline>` 3 says a gate must never be keyed on. The numbers are published
            as structured attributes so those specs have somewhere honest to migrate to, and the word
            "entities" (engine vocabulary, in a panel titled "Objects" above a box that says "Search
            objects") can then be corrected without a blind edit to a suite this run cannot re-run. */}
        <span
          id="count"
          data-entities={order.length}
          data-matches={filtering ? filteredOrder.length : undefined}
          role="status"
          aria-live="polite"
          style={{ font: font.mono, fontSize: fontSize.meta, color: color.text.muted, fontWeight: 400, letterSpacing: 0 }}
        >
          {filtering ? `${filteredOrder.length} of ${order.length} entities` : `${order.length} entities`}
        </span>
      </div>

      {order.length > 0 && (
        <div style={{ display: "flex", flexDirection: "column", gap: space.xs, padding: `${space.xs}px ${space.md}px ${space.sm}px` }}>
          <div style={{ display: "flex", gap: space.xs }}>
            <SearchField
              ref={searchRef}
              className="mtk-search--own-clear"
              aria-label="Search scene objects"
              placeholder="Search objects, or kind:light"
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

          {/* WHAT THIS SCENE CAN BE NARROWED BY, counted from the scene. The chips are the only place
              the filter vocabulary is taught, and they are toggles rather than shortcuts so the
              pressed one is also the way back out. */}
          {facets.length > 0 && (
            <div
              role="group"
              aria-label="Narrow by what the objects are"
              data-testid="scene-facets"
              style={{ display: "flex", flexWrap: "wrap", gap: space.xs }}
            >
              {shownFacets.map((facet) => {
                const on = queryHasFacet(parsed, facet);
                return (
                  <Button
                    key={facet.token}
                    compact
                    variant="toggle"
                    active={on}
                    data-facet={facet.token}
                    title={`${on ? "Stop showing only" : "Show only"} the ${facet.count} ${facet.label.toLocaleLowerCase()} in this scene`}
                    onClick={() => setQuery(toggleFacet(query, facet))}
                  >
                    {facet.label}
                    <span style={{ font: font.mono, fontSize: fontSize.micro, color: color.text.muted, marginLeft: space.xs }}>
                      {facet.count.toLocaleString()}
                    </span>
                  </Button>
                );
              })}
              {(hiddenFacets > 0 || facetsOpen) && (
                <Button
                  compact
                  variant="ghost"
                  data-testid="more-facets"
                  aria-expanded={facetsOpen}
                  title={
                    facetsOpen
                      ? "Show only the largest kinds, and give the room back to the object list"
                      : `Also filter by ${facets.slice(FACETS_COLLAPSED - 1).map((f) => f.label.toLocaleLowerCase()).join(", ")}`
                  }
                  onClick={() => setFacetsOpen(!facetsOpen)}
                >
                  {facetsOpen ? "Fewer" : `+${hiddenFacets} more`}
                </Button>
              )}
            </div>
          )}

          {/* THE VERB ON THE RESULT. The panel could always NAME a set — "1,247 of 15,711" — and had no
              way to act on it, so a search that found the right 1,247 objects still cost 1,247 clicks.
              Selecting them states the set on both sides through the one seam (ADR-158), which is what
              makes every verb the editor already has — Delete, the Inspector edit, the menu — apply to
              a question you typed. */}
          {/* LEFT-ALIGNED, in the column the chips and the row labels are read down. It was
              right-aligned, and the capture showed a control floating in the gap between the chips
              and the list, belonging to neither. */}
          {filtering && filteredOrder.length > 0 && (
            <div style={{ display: "flex" }}>
              <Button
                compact
                variant="secondary"
                data-testid="select-matches"
                data-count={filteredOrder.length}
                aria-label={`Select all ${filteredOrder.length} matching objects`}
                title="Select every object matching this search, so the Inspector, Delete and the object menu act on all of them"
                onClick={() =>
                  stateSelection(
                    client,
                    filteredOrder,
                    `Selected ${filteredOrder.length.toLocaleString()} ${filteredOrder.length === 1 ? "object" : "objects"} matching “${query.trim()}”`,
                  )
                }
              >
                Select all {filteredOrder.length.toLocaleString()}
              </Button>
            </div>
          )}
        </div>
      )}

      {order.length === 0 ? (
        <EmptyPanelState
          compact
          title="No objects in this scene"
          description="Add an entity above, or open Build in the Engines rail to draw, import or browse assets."
          icon={<Icon name="requirer" size="xl" />}
          style={{ margin: space.md }}
        />
      ) : filteredOrder.length === 0 ? (
        <EmptyPanelState
          compact
          title="No matching objects"
          // The keys come from the parser, so this sentence cannot fall behind it — and this is the
          // one moment a person is looking for a way to ask a better question.
          description={`Nothing matches “${query.trim()}”. Search a name or an object ID, or narrow by ${FILTER_KEYS.join(" · ")}.`}
          icon={<Icon name="search" size="xl" />}
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
                rangeScope={filteredOrder}
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
