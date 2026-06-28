//! M12.2 (ADR-046) — the **visual state-graph**, drawn with the **same React Flow layer** as the M2.5
//! neighborhood graph ([`BindingGraph`]) — states are nodes, transitions are edges. No new graph dep: this
//! is the M2.5 projection layer (ADR-010) re-pointed at state-machine data. The graph is a **projection**
//! (invariant 1) — every edit flows back through the commit pipeline as an `author_state_machine` tx
//! (`StateGraphPanel`), never a direct graph-lib mutation. Node ids are the **state names**, edge ids are
//! the **stable transition ids** — so an e2e (and the renderer) keys off stable ids, never label copy.

import { useMemo } from "react";
import { Background, ReactFlow, type Edge as RfEdge, type Node as RfNode } from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import type { StateMachine } from "../transport/protocol";

/** Node border: green = the live current state (M12.5 seam), blue = the initial state, grey = other. */
function nodeBorder(state: string, machine: StateMachine, current?: string | null): string {
  if (state === current) return "2px solid #4ade80";
  if (state === machine.initial) return "2px solid #60a5fa";
  return "1px solid #555";
}

export function StateGraph({
  machine,
  current,
}: {
  machine: StateMachine;
  current?: string | null;
}) {
  const { nodes, rfEdges } = useMemo(() => {
    // States → nodes. A deterministic left-to-right flow (staggered y to reduce edge overlap). Small node
    // counts, so no neighborhood cap is needed — but the build is memoized (like BindingGraph) so an
    // unrelated re-render doesn't rebuild it.
    const nodes: RfNode[] = machine.states.map((s, i) => ({
      id: s,
      position: { x: (i % 4) * 200, y: Math.floor(i / 4) * 150 + (i % 2) * 36 },
      data: { label: s },
      style: {
        padding: 6,
        borderRadius: 6,
        fontSize: 12,
        border: nodeBorder(s, machine, current),
        background: "#1a1c22",
        color: "#e8e8e8",
      },
    }));
    // Transitions → edges, keyed by the stable transition id (a draft transition not yet committed has no
    // id yet — fall back to a positional render key so React Flow stays happy until the save stamps one).
    const rfEdges: RfEdge[] = machine.transitions.map((t, i) => ({
      id: t.id || `draft-edge-${i}`,
      source: t.from,
      target: t.to,
      label: t.rule.event,
      style: { stroke: "#8aa" },
    }));
    return { nodes, rfEdges };
  }, [machine, current]);

  if (machine.states.length === 0) {
    return (
      <div data-testid="state-graph" style={{ padding: 12, color: "#888" }}>
        Add a state to start the graph.
      </div>
    );
  }
  return (
    <div data-testid="state-graph" style={{ height: 240, minHeight: 180 }}>
      <ReactFlow nodes={nodes} edges={rfEdges} fitView proOptions={{ hideAttribution: true }}>
        <Background color="#222" />
      </ReactFlow>
    </div>
  );
}
