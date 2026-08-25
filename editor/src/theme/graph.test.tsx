//! The graph framework's decisions, tested where they can be — which is NOT the canvas.
//!
//! READ THIS BEFORE ADDING A RENDER TEST HERE. React Flow measures its pane with
//! `getBoundingClientRect`, and jsdom implements no layout, so the pane is 0x0 and **no node renders
//! at all**. That is why `BindingGraph` and `StateGraph` shipped for months with a layout that put six
//! nodes in one row and overflowed their own pane: there was no test that could have seen it, and no
//! capture had ever been taken. The captures (`editor/scripts/shots`, scenes `binding-graph-*` and
//! `state-graph-door`) are the gate on what the canvas looks like.
//!
//! What IS testable is every decision made BEFORE the canvas: where a column goes, how far a state is
//! from the start, whether a query matches, and — the one that ties legibility to navigation —
//! whether the fit had to give up on showing everything.

import { expect, test } from "vitest";
import {
  GRAPH_CARD_WIDTH,
  GRAPH_FIT_MIN_ZOOM,
  GRAPH_LABEL_RUN,
  columnLayout,
  graphEdge,
  graphEdgeStyle,
  graphFitDecision,
  rankByDistance,
} from "./graph";

// ── columnLayout ──────────────────────────────────────────────────────────────────────────────────

test("columns are spaced by the card plus a clear run for the relation label", () => {
  const p = columnLayout([["a"], ["b"], ["c"]]);
  expect(p.b.x - p.a.x).toBe(GRAPH_CARD_WIDTH + GRAPH_LABEL_RUN);
  expect(p.c.x - p.b.x).toBe(GRAPH_CARD_WIDTH + GRAPH_LABEL_RUN);
});

test("a short column is centred against the tallest one, not top-aligned", () => {
  // The whole point of the layout: the subject sits on the axis the rest is arranged around. A
  // top-aligned single node beside a three-node column reads as a list, not as a flow.
  const p = columnLayout([["a", "b", "c"], ["subject"]], { rowGap: 100 });
  expect(p.a.y).toBe(0);
  expect(p.c.y).toBe(200);
  expect(p.subject.y).toBe(100); // the middle of the tall column
});

test("an empty column is not given a gap it does not need", () => {
  // `BindingGraph` filters empty sides out before calling; this asserts the contract it relies on —
  // two columns are two columns, wherever they came from.
  const p = columnLayout([["a"], ["b"]]);
  expect(p.b.x - p.a.x).toBe(GRAPH_CARD_WIDTH + GRAPH_LABEL_RUN);
});

// ── rankByDistance ────────────────────────────────────────────────────────────────────────────────

const t = (from: string, to: string) => ({ from, to });

test("a state is as far right as it is far from the start", () => {
  const cols = rankByDistance(
    ["Locked", "Closed", "Opening", "Open"],
    [t("Locked", "Closed"), t("Closed", "Opening"), t("Opening", "Open")],
    ["Locked"],
  );
  expect(cols).toEqual([["Locked"], ["Closed"], ["Opening"], ["Open"]]);
});

test("two states the same distance from the start share a column", () => {
  const cols = rankByDistance(
    ["A", "B", "C"],
    [t("A", "B"), t("A", "C")],
    ["A"],
  );
  expect(cols).toEqual([["A"], ["B", "C"]]);
});

test("a back edge does not drag its target backwards", () => {
  // `Closing -> Closed` must not move `Closed` to the end; BFS takes the FIRST time it reaches a node.
  const cols = rankByDistance(
    ["Closed", "Opening", "Closing"],
    [t("Closed", "Opening"), t("Opening", "Closing"), t("Closing", "Closed")],
    ["Closed"],
  );
  expect(cols).toEqual([["Closed"], ["Opening"], ["Closing"]]);
});

test("a state nothing can reach is placed, not dropped", () => {
  // A node the layout cannot rank is still a node the user authored. Silently omitting it is the
  // defect class this whole harness exists to catch, so it lands in the final column instead.
  const cols = rankByDistance(["A", "B", "Orphan"], [t("A", "B")], ["A"]);
  expect(cols.flat()).toContain("Orphan");
  expect(cols.at(-1)).toEqual(["Orphan"]);
});

// ── graphFitDecision ──────────────────────────────────────────────────────────────────────────────

const fit = (bounds: { width: number; height: number }, canvas: { width: number; height: number }) =>
  graphFitDecision({ bounds, canvas, nodeCount: 6, minimapFrom: 12 });

test("a graph that fits is drawn at the zoom that fits it, with no mini map", () => {
  const d = fit({ width: 300, height: 200 }, { width: 900, height: 600 });
  expect(d.clamped).toBe(false);
  expect(d.minimap).toBe(false);
  expect(d.zoom).toBeGreaterThan(GRAPH_FIT_MIN_ZOOM);
});

test("a graph too wide to fit legibly is clamped to the floor, and says so with a mini map", () => {
  // The measured case: three 208px columns need 960px and a 728px dock canvas has 700 of it.
  const d = fit({ width: 960, height: 270 }, { width: 726, height: 330 });
  expect(d.wanted).toBeLessThan(GRAPH_FIT_MIN_ZOOM);
  expect(d.zoom).toBe(GRAPH_FIT_MIN_ZOOM);
  expect(d.clamped).toBe(true);
  expect(d.minimap).toBe(true);
});

test("a canvas too small to host a mini map gets Fit and panning instead", () => {
  // The 300px Inspector track. The graph is clamped exactly as above, but a 132x96 thumbnail in a
  // 268px canvas is furniture, not navigation — so the answer is the Fit button, which is also what
  // the capture harness's pan/zoom rule checks for.
  const d = fit({ width: 960, height: 270 }, { width: 268, height: 300 });
  expect(d.clamped).toBe(true);
  expect(d.minimap).toBe(false);
});

test("a big graph gets a mini map on node count alone, before it has to be clamped", () => {
  const d = graphFitDecision({
    bounds: { width: 300, height: 200 },
    canvas: { width: 900, height: 600 },
    nodeCount: 17,
    minimapFrom: 12,
  });
  expect(d.clamped).toBe(false);
  expect(d.minimap).toBe(true);
});

test("zero-size bounds do not divide by zero", () => {
  const d = fit({ width: 0, height: 0 }, { width: 600, height: 400 });
  expect(Number.isFinite(d.wanted)).toBe(true);
});

// ── edges ─────────────────────────────────────────────────────────────────────────────────────────

test("an edge carries the shared type, so no subsystem can draw its own wire", () => {
  const e = graphEdge({ id: "e1", source: "a", target: "b", state: "confirmed", label: "PowerSource" });
  expect(e.type).toBe("mtkRelation");
  expect(e.label).toBe("PowerSource");
  expect(e.animated).toBe(false);
});

test("an unlabelled edge carries no label rather than an empty one", () => {
  // The fan rule in `BindingGraph` omits `label`; an empty string would still draw a pill.
  expect(graphEdge({ id: "e1", source: "a", target: "b" }).label).toBeUndefined();
});

test("only a pending edge is dashed, and the legend reads the same table", () => {
  // The legend swatch and the wire both come from `graphEdgeStyle`, which is what stops a key from
  // disagreeing with its own diagram — the defect that shipped when React Flow's `animated` class
  // dashed an "active" edge whose swatch was drawn solid.
  expect(graphEdgeStyle("pending").strokeDasharray).toBeDefined();
  expect(graphEdgeStyle("active").strokeDasharray).toBeUndefined();
  expect(graphEdgeStyle("confirmed").strokeDasharray).toBeUndefined();
  expect(graphEdgeStyle("rejected").strokeDasharray).toBeUndefined();
});

test("every edge state resolves to a distinct-enough stroke, not to undefined", () => {
  for (const state of ["default", "selected", "confirmed", "pending", "rejected", "active", "disabled"] as const) {
    expect(graphEdgeStyle(state).stroke).toMatch(/^var\(--mtk-/);
  }
});
