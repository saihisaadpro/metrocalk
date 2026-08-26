//! M12.2 (ADR-046) — the **visual state-graph**, drawn with the **same graph framework** as the M2.5
//! neighborhood graph ([`BindingGraph`]) — states are nodes, transitions are edges. No new graph dep:
//! this is the M2.5 projection layer (ADR-010) re-pointed at state-machine data. The graph is a
//! **projection** (invariant 1) — every edit flows back through the commit pipeline as an
//! `author_state_machine` tx (`StateGraphPanel`), never a direct graph-lib mutation. Node ids are the
//! **state names**, edge ids are the **stable transition ids** — so an e2e (and the renderer) keys off
//! stable ids, never label copy.
//!
//! THE TWO MARKS THAT USED TO BE ONE. `initial` and `current` are different questions — where the
//! machine starts, and where it is right now — and the previous version answered both with a coloured
//! border, so at a glance they were the same kind of thing in two hues. They now carry a **named
//! chip**: `start` on the initial state, `live` on the current one. A machine sitting in its initial
//! state shows `live`, because "where it is" is the more urgent fact and the two coincide only by
//! accident.
//!
//! AND THE COLUMNS MEAN SOMETHING. `(i % 4) * 200, floor(i / 4) * 150 + (i % 2) * 36` placed states by
//! their index in an array — a staggered grid whose only property was that adjacent array entries were
//! adjacent on screen. `rankByDistance` puts a state as far right as it is far from the start, so a
//! machine reads left-to-right the way its author described it, and a state nothing can reach lands in
//! the last column rather than being hidden inside a row it has no relationship to.

import { useMemo } from "react";
import { type Edge as RfEdge } from "@xyflow/react";
import type { StateMachine } from "../transport/protocol";
import {
  GRAPH_CARD_WIDTH_COMPACT,
  GRAPH_LABEL_RUN,
  GraphSurface,
  columnLayout,
  graphEdge,
  rankByDistance,
  type GraphCardNode,
  type GraphNodeEmphasis,
} from "../theme/graph";
import { Icon } from "../theme/icons";
import { EmptyPanelState } from "../theme/workspace";

/** Live beats initial: a machine parked in its own start state is *live* there, and saying "start"
 *  would answer the question nobody is asking while the simulation runs. */
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
    const columns = rankByDistance(machine.states, machine.transitions, [machine.initial]);
    const positions = columnLayout(columns, {
      columnGap: GRAPH_CARD_WIDTH_COMPACT + GRAPH_LABEL_RUN,
      rowGap: 104,
    });

    /** Every state's degree, so a card says something a name cannot: how many ways out of here.
     *  A terminal state is the one a reader most needs to spot and the one a name never announces. */
    const outCount = new Map<string, number>();
    for (const t of machine.transitions) outCount.set(t.from, (outCount.get(t.from) ?? 0) + 1);

    const nodes: GraphCardNode[] = machine.states.map((s) => {
      const outs = outCount.get(s) ?? 0;
      return {
        id: s,
        type: "mtkCard" as const,
        position: positions[s] ?? { x: 0, y: 0 },
        data: {
          title: s,
          compact: true,
          eyebrow: "state",
          meta: outs === 0 ? "no way out" : `${outs} exit${outs === 1 ? "" : "s"}`,
          emphasis: nodeEmphasis(s, machine, current),
          targetPort: machine.transitions.some((t) => t.to === s),
          sourcePort: outs > 0,
        },
      };
    });

    // NOT `animated`, and the reason is the legend. React Flow's `animated` flag is a CSS class that
    // adds `stroke-dasharray` and a marching-ants animation, which the inline style from
    // `graphEdgeStyle` cannot see — so an "active" edge rendered DASHED while the legend swatch beside
    // it, drawn from the same `graphEdgeStyle("active")`, rendered SOLID. A key that does not match its
    // own diagram is worse than no key. Motion belongs to the state that is genuinely in flight — the
    // binding graph's `pending` edge, which is dashed in both places.
    const rfEdges: RfEdge[] = machine.transitions.map((t, i) =>
      // A draft transition not yet committed has no id yet — fall back to a positional render key so
      // React Flow stays happy until the save stamps one.
      graphEdge({
        id: t.id || `draft-edge-${i}`,
        source: t.from,
        target: t.to,
        state: t.from === current ? "active" : "default",
        label: t.rule.event,
      }),
    );
    return { nodes, rfEdges };
  }, [machine, current]);

  if (machine.states.length === 0) {
    return (
      <div data-testid="state-graph">
        <EmptyPanelState
          compact
          icon={<Icon name="logic" size="xl" />}
          title="Add a state to start the graph"
          description="States and their event-driven transitions will appear on this shared canvas."
        />
      </div>
    );
  }
  return (
    <GraphSurface
      data-testid="state-graph"
      label={`${machine.name} state graph`}
      nodes={nodes}
      edges={rfEdges}
      height={260}
      minHeight={200}
      legend={[
        { label: "reachable now", state: "active" },
        { label: "transition", state: "default" },
      ]}
    />
  );
}
