//! Binding graph — React Flow, **neighborhood-scoped**: only the selected entity and its
//! bound/candidate neighbours, never 5k nodes at once. Memoized so unrelated store deltas don't
//! rebuild it. Sigma.js is the documented 50k+ fallback (not built — see the layers note).

import { useMemo } from "react";
import { Background, ReactFlow, type Edge as RfEdge, type Node as RfNode } from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { useStore } from "zustand";
import { projectionStore, useEdges, useSelectedId } from "../store/projection";
import { graphEdgeStyle, graphNodeStyle, graphTheme } from "../theme/graph";
import { EmptyPanelState } from "../theme/workspace";

const STATUS_STYLE: Record<string, React.CSSProperties> = {
  confirmed: graphEdgeStyle("confirmed"),
  pending: graphEdgeStyle("pending"),
  rejected: graphEdgeStyle("rejected"),
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
      style: graphNodeStyle(id === selected ? "selected" : "default"),
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
    return (
      <EmptyPanelState
        compact
        icon="⌘"
        title="Select an object to inspect relationships"
        description="Its binding neighbourhood and typed connections will appear here."
      />
    );
  }
  return (
    <div className="mtk-graph-surface" style={{ height: "100%", minHeight: 240 }}>
      <ReactFlow nodes={nodes} edges={rfEdges} fitView proOptions={{ hideAttribution: true }}>
        <Background color={graphTheme.grid} />
      </ReactFlow>
    </div>
  );
}
