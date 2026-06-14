//! Binding graph — React Flow, **neighborhood-scoped**: only the selected entity and its
//! bound/candidate neighbours, never 5k nodes at once. Memoized so unrelated store deltas don't
//! rebuild it. Sigma.js is the documented 50k+ fallback (not built — see the layers note).

import { useMemo } from "react";
import { Background, ReactFlow, type Edge as RfEdge, type Node as RfNode } from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { useStore } from "zustand";
import { projectionStore, useEdges, useSelectedId } from "../store/projection";

const STATUS_STYLE: Record<string, React.CSSProperties> = {
  confirmed: { stroke: "#4ade80" },
  pending: { stroke: "#fbbf24", strokeDasharray: "4 3" },
  rejected: { stroke: "#f87171" },
};

export function BindingGraph() {
  const selected = useSelectedId();
  const edges = useEdges();
  const summaries = useStore(projectionStore, (s) => s.summaries);

  const { nodes, rfEdges } = useMemo(() => {
    if (!selected || !summaries[selected]) return { nodes: [] as RfNode[], rfEdges: [] as RfEdge[] };
    const neighbourIds = new Set<string>([selected]);
    const related = Object.values(edges).filter((e) => e.from === selected || e.to === selected);
    for (const e of related) {
      neighbourIds.add(e.from);
      neighbourIds.add(e.to);
    }
    const ids = [...neighbourIds].slice(0, 50); // neighborhood cap — never the whole graph
    const nodes: RfNode[] = ids.map((id, i) => ({
      id,
      position: id === selected ? { x: 240, y: 200 } : { x: (i % 6) * 150, y: i < 6 ? 40 : 360 },
      data: { label: summaries[id]?.name ?? id },
      style: {
        padding: 6,
        borderRadius: 6,
        fontSize: 12,
        border: id === selected ? "2px solid #60a5fa" : "1px solid #555",
        background: "#1a1c22",
        color: "#e8e8e8",
      },
    }));
    const rfEdges: RfEdge[] = related.slice(0, 200).map((e) => ({
      id: e.id,
      source: e.from,
      target: e.to,
      label: e.rel,
      animated: e.status === "pending",
      style: STATUS_STYLE[e.status],
    }));
    return { nodes, rfEdges };
  }, [selected, edges, summaries]);

  if (!selected) {
    return <div style={{ padding: 12, color: "#888" }}>Select an entity to see its binding neighborhood.</div>;
  }
  return (
    <div style={{ height: "100%", minHeight: 240 }}>
      <ReactFlow nodes={nodes} edges={rfEdges} fitView proOptions={{ hideAttribution: true }}>
        <Background color="#222" />
      </ReactFlow>
    </div>
  );
}
