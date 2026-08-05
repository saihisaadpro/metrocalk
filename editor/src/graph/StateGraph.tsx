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
import {
  graphEdgeStyle,
  graphNodeStyle,
  graphTheme,
  type GraphNodeEmphasis,
} from "../theme/graph";
import { EmptyPanelState } from "../theme/workspace";

/** Node border: green = the live current state (M12.5 seam), blue = the initial state, grey = other. */
function nodeEmphasis(state: string, machine: StateMachine, current?: string | null): GraphNodeEmphasis {
  if (state === current) return "live";
  if (state === machine.initial) return "initial";
  return "default";
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
      style: graphNodeStyle(nodeEmphasis(s, machine, current)),
    }));
    // Transitions → edges, keyed by the stable transition id (a draft transition not yet committed has no
    // id yet — fall back to a positional render key so React Flow stays happy until the save stamps one).
    const rfEdges: RfEdge[] = machine.transitions.map((t, i) => ({
      id: t.id || `draft-edge-${i}`,
      source: t.from,
      target: t.to,
      label: t.rule.event,
      style: graphEdgeStyle(),
    }));
    return { nodes, rfEdges };
  }, [machine, current]);

  if (machine.states.length === 0) {
    return (
      <div data-testid="state-graph">
        <EmptyPanelState
          compact
          icon="◇"
          title="Add a state to start the graph"
          description="States and their event-driven transitions will appear on this shared canvas."
        />
      </div>
    );
  }
  return (
    <div className="mtk-graph-surface" data-testid="state-graph" style={{ height: 240, minHeight: 180 }}>
      <ReactFlow nodes={nodes} edges={rfEdges} fitView proOptions={{ hideAttribution: true }}>
        <Background color={graphTheme.grid} />
      </ReactFlow>
    </div>
  );
}
