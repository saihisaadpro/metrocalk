//! Hierarchy / list panel — virtualized rows over the **summary projection** for 5k entities. Each
//! row subscribes only to its `{id,name,parentId}` summary, so a field edit (which changes
//! `displayed[id]` but not the summary) never re-renders the tree. Manual windowing keeps it
//! dependency-free; only the visible ~30 rows mount.

import { memo, useState } from "react";
import { projectionStore, useEntityOrder, useSelectedId, useSummary } from "../store/projection";

const ROW_H = 22;
const VIEW_H = 560;
const OVERSCAN = 6;

const Row = memo(function Row({ id, top }: { id: string; top: number }) {
  const s = useSummary(id);
  const selected = useSelectedId() === id;
  return (
    <div
      onClick={() => projectionStore.getState().select(id)}
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
        color: selected ? "#fff" : "#cfd2d6",
        background: selected ? "#2a4365" : "transparent",
        whiteSpace: "nowrap",
      }}
    >
      {s?.parentId ? "· " : ""}
      {s?.name ?? id}
    </div>
  );
});

export function Hierarchy() {
  const order = useEntityOrder();
  const [scrollTop, setScrollTop] = useState(0);
  const start = Math.max(0, Math.floor(scrollTop / ROW_H) - OVERSCAN);
  const end = Math.min(order.length, Math.ceil((scrollTop + VIEW_H) / ROW_H) + OVERSCAN);
  const visible = order.slice(start, end);
  return (
    <div>
      <div style={{ padding: "8px", fontWeight: 700, color: "#e8e8e8" }}>
        hierarchy <span style={{ opacity: 0.6, fontWeight: 400 }}>({order.length})</span>
      </div>
      <div
        onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
        style={{ height: VIEW_H, overflowY: "auto", position: "relative" }}
      >
        <div style={{ height: order.length * ROW_H, position: "relative" }}>
          {visible.map((id, i) => (
            <Row key={id} id={id} top={(start + i) * ROW_H} />
          ))}
        </div>
      </div>
    </div>
  );
}
