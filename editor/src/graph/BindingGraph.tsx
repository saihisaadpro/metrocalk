//! Binding graph — **north-star #1 as a picture**, and the constitution's named signature interface.
//!
//! Neighborhood-scoped: only the selected entity and its bound/candidate neighbours, never 5k nodes at
//! once. Memoized so unrelated store deltas don't rebuild it. Sigma.js is the documented 50k+ fallback
//! (not built — see the layers note).
//!
//! WHAT CHANGED AND WHY IT IS NOT COSMETIC. This panel used to place its nodes at `(i % 6) * 150,
//! i < 6 ? 40 : 360` — arithmetic over an arbitrary index. On a real six-node bind neighbourhood that
//! puts every neighbour in one top row with the selected entity adrift below-left, five edges cutting
//! through four unrelated cards, four edge labels stacked on the same crossing point, and the whole
//! composition 98px wider than its own pane after `fitView`. Every one of those is visible in
//! `progress/graph-framework/before/binding-graph-neighbourhood.png`, and none of it was reachable by
//! a test: React Flow measures its pane with `getBoundingClientRect`, which is 0×0 under jsdom, so the
//! populated graph renders **no nodes at all** there. It was the one surface in this editor whose
//! real state could only ever be seen in a browser, and nothing had ever opened one.
//!
//! The layout is now the SEMANTICS: what powers this on the left, the thing you selected in the
//! middle, what it feeds on the right — the same reading order the reference binding screen uses, and
//! the same sentence the Reveal panel says in words. `columnLayout` centres the columns on a shared
//! axis so the subject sits on the line the rest is arranged around.
//!
//! DEPENDENCY TRACING IS THE HOVER. The constitution asks for "highlighted paths · hover previews ·
//! dependency tracing"; all three are one behaviour — point at a node and everything not on a path
//! through it fades. It costs one `data-dimmed` attribute and it is the difference between a diagram
//! and a thing you can ask a question of.

import { useMemo, useState } from "react";
import { type Edge as RfEdge } from "@xyflow/react";
import { useStore } from "zustand";
import { projectionStore, useEdges, useSelectedId, type BindEdge } from "../store/projection";
import {
  GraphSurface,
  columnLayout,
  graphEdge,
  useGraphSearch,
  type GraphCardNode,
  type GraphEdgeState,
  type GraphLegendItem,
} from "../theme/graph";
import { Icon } from "../theme/icons";
import { SearchField } from "../theme/primitives";
import { EmptyPanelState } from "../theme/workspace";

/** The neighbourhood cap — never the whole graph. Stated once and used by both the node build and the
 *  "…and N more" notice, so the number the user is told is the number that was actually applied. */
const NEIGHBOUR_CAP = 48;

/** How many wires can share one relation and one direction before their labels are left to the cards.
 *  Three is the number that fits in the run between two columns without a pill touching its neighbour
 *  at the tightest vertical spacing this layout produces. */
const LABEL_FAN_MAX = 3;

/** A bind edge's status IS its colour rule, and the legend below reads the same table — so a status
 *  that gains a colour gains its explanation in the same edit. */
const EDGE_STATE: Record<BindEdge["status"], GraphEdgeState> = {
  confirmed: "confirmed",
  pending: "pending",
  rejected: "rejected",
};

const LEGEND: GraphLegendItem[] = [
  { label: "bound", state: "confirmed" },
  { label: "in flight", state: "pending" },
  { label: "refused", state: "rejected" },
];

export function BindingGraph() {
  const selected = useSelectedId();
  const edges = useEdges();
  const summaries = useStore(projectionStore, (s) => s.summaries);
  const [hovered, setHovered] = useState<string | null>(null);

  /** The neighbourhood, split by DIRECTION — the whole point of the layout. `into` is what binds to
   *  the selection (its providers), `outOf` is what the selection binds to (its consumers). An entity
   *  can legitimately be both; it is placed as a provider so it appears exactly once. */
  const { nodes, rfEdges, hiddenCount } = useMemo(() => {
    const empty = { nodes: [] as GraphCardNode[], rfEdges: [] as RfEdge[], hiddenCount: 0 };
    if (!selected || !summaries[selected]) return empty;

    const related = Object.values(edges).filter((e) => e.from === selected || e.to === selected);
    const into: string[] = [];
    const outOf: string[] = [];
    const seen = new Set<string>([selected]);
    for (const e of related) {
      const other = e.from === selected ? e.to : e.from;
      if (seen.has(other)) continue;
      seen.add(other);
      (e.to === selected ? into : outOf).push(other);
    }
    const capped = new Set<string>([selected, ...into, ...outOf].slice(0, NEIGHBOUR_CAP));
    const hiddenCount = seen.size - capped.size;

    const columns = [
      into.filter((id) => capped.has(id)),
      [selected],
      outOf.filter((id) => capped.has(id)),
    ];
    // A neighbourhood with nothing on one side is TWO columns, not three with a hole: an empty column
    // still consumes its gap, which pushes the subject off-centre for no reason a reader can see.
    const positions = columnLayout(columns.filter((c) => c.length > 0));

    /** The relation an entity carries into the subject — the eyebrow line. A node bound by two
     *  relations shows both, because "what is this to me" is the question the card answers. */
    const roleOf = (id: string) => {
      const rels = related.filter((e) => e.from === id || e.to === id).map((e) => e.rel);
      const unique = [...new Set(rels)];
      if (unique.length === 0) return undefined;
      return `${related.some((e) => e.to === id) ? "receives" : "provides"} ${unique.join(" · ")}`;
    };

    const nodes: GraphCardNode[] = [...capped].map((id) => ({
      id,
      type: "mtkCard" as const,
      position: positions[id] ?? { x: 0, y: 0 },
      data: {
        title: summaries[id]?.name ?? id,
        eyebrow: id === selected ? "selected" : roleOf(id),
        kind: summaries[id]?.kind ?? "default",
        emphasis: id === selected ? ("selected" as const) : ("default" as const),
        targetPort: outOf.includes(id) || id === selected,
        sourcePort: into.includes(id) || id === selected,
      },
    }));

    const drawn = related.filter((e) => capped.has(e.from) && capped.has(e.to)).slice(0, 200);

    /** A LABEL PER WIRE STOPS BEING INFORMATION AND STARTS BEING NOISE. Nine feeders into one
     *  controller is nine edges converging on one port, and their labels — all reading `Supplies`,
     *  because that is what makes them a fan — stack on top of each other in the only gap there is.
     *  The first dense capture shows exactly that pile, over the wires it is meant to explain.
     *
     *  A fan is one fact, not N. Above the threshold the relation is left to the cards, which already
     *  state it in their eyebrow ("provides Supplies") and have room for it; below it, every wire
     *  keeps its own pill, which is what makes a small neighbourhood readable at a glance. */
    const fan = new Map<string, number>();
    for (const e of drawn) {
      const key = `${e.rel}|${e.to === selected ? "in" : "out"}`;
      fan.set(key, (fan.get(key) ?? 0) + 1);
    }

    const rfEdges: RfEdge[] = drawn.map((e) => {
      const key = `${e.rel}|${e.to === selected ? "in" : "out"}`;
      return graphEdge({
        id: e.id,
        source: e.from,
        target: e.to,
        state: EDGE_STATE[e.status],
        label: (fan.get(key) ?? 0) <= LABEL_FAN_MAX ? e.rel : undefined,
        animated: e.status === "pending",
      });
    });

    return { nodes, rfEdges, hiddenCount };
  }, [selected, edges, summaries]);

  const { query, setQuery, matches } = useGraphSearch(nodes);

  /** Two independent reasons to fade a node, resolved in one place: a search that does not match it,
   *  or a hover that is tracing a path it is not on. The subject is never faded — a view that hides
   *  what you selected has answered a question nobody asked. */
  const traced = useMemo(() => {
    if (hovered == null) return null;
    const keep = new Set<string>([hovered]);
    for (const e of rfEdges) {
      if (e.source === hovered) keep.add(e.target);
      if (e.target === hovered) keep.add(e.source);
    }
    return keep;
  }, [hovered, rfEdges]);

  const shownNodes = useMemo(
    () =>
      nodes.map((n) => {
        const dimmed =
          (matches != null && !matches.has(n.id) && n.id !== selected) ||
          (traced != null && !traced.has(n.id));
        return dimmed === Boolean(n.data.dimmed) ? n : { ...n, data: { ...n.data, dimmed } };
      }),
    [nodes, matches, traced, selected],
  );

  /** The wire and its label live in two different DOM layers, so the fade has to be said twice — once
   *  as a class on the SVG group, once as a flag the HTML pill can read out of the edge's `data`. */
  const shownEdges = useMemo(
    () =>
      rfEdges.map((e) => {
        const on = traced == null || e.source === hovered || e.target === hovered;
        if (on === (e.className !== "is-dimmed")) return e;
        return on
          ? { ...e, className: undefined, data: { ...e.data, dimmed: false } }
          : { ...e, className: "is-dimmed", data: { ...e.data, dimmed: true } };
      }),
    [rfEdges, traced, hovered],
  );

  if (!selected) {
    return (
      <EmptyPanelState
        compact
        icon={<Icon name="relations" size="xl" />}
        title="Select an object to see what it is wired to"
        description="Its binding neighbourhood — what powers it, what it feeds, and what has been refused — appears here."
      />
    );
  }

  if (nodes.length <= 1) {
    return (
      <EmptyPanelState
        compact
        icon={<Icon name="relations" size="xl" />}
        title="Nothing is wired to this yet"
        description="Bind it from the Relations panel and the connection will appear on this canvas."
      />
    );
  }

  return (
    <GraphSurface
      data-testid="binding-graph"
      label="Binding neighbourhood"
      nodes={shownNodes}
      edges={shownEdges}
      legend={LEGEND}
      onNodeHover={setHovered}
      toolbar={
        <div className="mtk-graph-toolbar__row">
          <SearchField
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Filter connections…"
            aria-label="Filter the binding neighbourhood"
            data-testid="binding-graph-search"
          />
          {matches != null && (
            <span className="mtk-graph-toolbar__count" data-testid="binding-graph-match-count">
              {matches.size} of {nodes.length} match
            </span>
          )}
          {hiddenCount > 0 && (
            <span className="mtk-graph-toolbar__count" data-testid="binding-graph-capped">
              showing {NEIGHBOUR_CAP} of {NEIGHBOUR_CAP + hiddenCount}
            </span>
          )}
        </div>
      }
    />
  );
}
