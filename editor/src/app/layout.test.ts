//! panelLayout (M10.10 / C8) — the **stage is layout-priority**: the side tracks are fixed px that yield
//! (and collapse to icon rails below a breakpoint) while the MIDDLE (stage) track is always the flex
//! `minmax(STAGE_MIN, 1fr)` — so the viewport never collapses first. Unit-testable without real layout
//! (jsdom has none): the assertion is on the grid template the resize produces.

import { expect, test } from "vitest";
import { dockForm, dockGridColumns, panelLayout, STAGE_MIN, COLLAPSE_BELOW, OVERLAY_BELOW, RAIL_W, ENGINE_RAIL_W, ENGINE_RAIL_W_COMPACT } from "./layout";

test("the stage is ALWAYS the flex region with a protected floor (it never collapses first)", () => {
  for (const w of [1920, 1366, 1024, 900, 768, 500]) {
    const l = panelLayout(w);
    // the MIDDLE track is the stage — the only flex (1fr) track, with a protected minimum
    expect(l.gridColumns).toContain(w < OVERLAY_BELOW ? "minmax(0, 1fr)" : `minmax(${STAGE_MIN}px, 1fr)`);
    expect(l.gridColumns.match(/1fr/g)?.length).toBe(1); // exactly one flex track: the stage
    if (!l.overlay) expect(l.gridColumns.indexOf("1fr")).toBeGreaterThan(0); // it's the MIDDLE track when docks flank it
  }
});

test("panels shrink, then collapse to icon rails below the breakpoint (the stage keeps the space)", () => {
  const wide = panelLayout(1440);
  expect(wide.collapsed).toBe(false);
  // The engine workspace column is the wider one: it is where the authoring happens.
  expect(wide.left).toBe(340);
  expect(wide.left).toBeGreaterThan(wide.right);

  const mid = panelLayout(1024);
  expect(mid.collapsed).toBe(false);
  expect(mid.left).toBeLessThan(wide.left); // panels yielded but stay open

  const narrow = panelLayout(COLLAPSE_BELOW - 1);
  expect(narrow.collapsed).toBe(true); // collapsed to icon rails
  expect(narrow.left).toBeLessThan(mid.left);

  const phone = panelLayout(OVERLAY_BELOW - 1);
  expect(phone.overlay).toBe(true);
  expect(phone.left).toBe(0);
  expect(phone.gridColumns).toBe("minmax(0, 1fr)");
});

test("each workstation dock can independently yield to a rail without changing the protected stage track", () => {
  const wide = panelLayout(1440);
  // The Engines rail is ALWAYS the first track: it is the index of what the editor can do, and an index you
  // have to open a drawer to reach is not one. Only the phone-width overlay layout folds it away.
  const E = `${ENGINE_RAIL_W}px `;
  expect(dockGridColumns(wide, false, false, 1440)).toBe(`${E}340px minmax(${STAGE_MIN}px, 1fr) 300px`);
  expect(dockGridColumns(wide, false, true, 1440)).toBe(`${E}340px minmax(${STAGE_MIN}px, 1fr) ${RAIL_W}px`);
  expect(dockGridColumns(wide, true, false, 1440)).toBe(`${E}${RAIL_W}px minmax(${STAGE_MIN}px, 1fr) 300px`);

  const responsive = panelLayout(COLLAPSE_BELOW - 1);
  expect(dockGridColumns(responsive, false, false, COLLAPSE_BELOW - 1)).toBe(
    `${ENGINE_RAIL_W_COMPACT}px ${RAIL_W}px minmax(${STAGE_MIN}px, 1fr) ${RAIL_W}px`,
  );
});

test("the tracks are made to FIT: when they do not, the panels give and the stage keeps its floor", () => {
  // The defect this pins was found by measuring a real browser, not by reading this function. At
  // 1000 px with both docks open the template read `132px 300px minmax(320px, 1fr) 260px` — 1012 px of
  // tracks in a 1000 px window — and every assertion in the test above was green, because each one
  // compares a string and none of them adds the string up against a width.
  expect(dockGridColumns(panelLayout(1000), false, false, 1000)).toBe(
    `${ENGINE_RAIL_W}px 300px minmax(${STAGE_MIN}px, 1fr) 248px`,
  );

  // The sum is the assertion, for every combination — the property, not one example of it.
  const px = (t: string) => [...t.matchAll(/(\d+)px/g)].map((m) => Number(m[1]));
  for (const width of [1440, 1200, 1199, 1100, 1012, 1000, 990, 980, 700, 620]) {
    for (const l of [false, true]) {
      for (const r of [false, true]) {
        const layout = panelLayout(width);
        const parts = px(dockGridColumns(layout, l, r, width));
        // engines + left + STAGE_MIN + right — the minimum the template can occupy.
        const floor = parts[0] + parts[1] + parts[2] + parts[3];
        expect(floor, `${width}px, left${l ? " rail" : ""}, right${r ? " rail" : ""}`).toBeLessThanOrEqual(width);
        // and the stage's floor is never what was traded away to make it fit
        expect(parts[2]).toBe(STAGE_MIN);
        // nor is a dock ever squeezed below the rail it collapses to
        expect(Math.min(parts[1], parts[3])).toBeGreaterThanOrEqual(RAIL_W);
      }
    }
  }
});

test("the RIGHT dock is the one that gives — the left is where the authoring happens", () => {
  const layout = panelLayout(1000);
  // Exactly at the sum, nothing has to move.
  expect(dockGridColumns(layout, false, false, 1012)).toBe(
    `${ENGINE_RAIL_W}px 300px minmax(${STAGE_MIN}px, 1fr) 260px`,
  );
  // 12 px short, and it comes out of the selection read-out — not out of the sub-engine the author
  // is working in, and not out of the stage.
  expect(dockGridColumns(layout, false, false, 1000)).toBe(
    `${ENGINE_RAIL_W}px 300px minmax(${STAGE_MIN}px, 1fr) 248px`,
  );
  // The worst reachable case: the narrowest window that still keeps both docks open. 980 needs 32 px,
  // and the right dock has 216 px of give, so the left one is never asked. The `left` branch in
  // `dockGridColumns` is therefore unreachable with today's constants and is kept because the
  // function's contract is "these tracks fit", not "these tracks fit at these four numbers" — the
  // property test above is what turns red if a constant ever makes it necessary.
  expect(dockGridColumns(panelLayout(COLLAPSE_BELOW), false, false, COLLAPSE_BELOW)).toBe(
    `${ENGINE_RAIL_W}px 300px minmax(${STAGE_MIN}px, 1fr) 228px`,
  );
});

// ── the vertical axis ─────────────────────────────────────────────────────────────────────────────
// `dockGridColumns` makes the side docks yield until they cannot; `dockForm` is the answer to what
// happens after "cannot" on the other axis. These tests state the threshold in the terms the
// function is written in, so the numbers here are arithmetic and not a second copy of the design.

test("the dock is a track while both floors fit, and a sheet the moment they do not", () => {
  const BAR = 42; // --mtk-bottom-bar-height at (pointer: fine)
  const CONTENT = 188; // --mtk-dock-content-min
  const both = STAGE_MIN + BAR + CONTENT; // 550px of stage column

  // Exactly enough is enough: the boundary belongs to the form that costs the user nothing.
  expect(dockForm(both, BAR, CONTENT)).toBe("docked");
  expect(dockForm(both + 1, BAR, CONTENT)).toBe("docked");
  expect(dockForm(both - 1, BAR, CONTENT)).toBe("sheet");
  // A 900px window (819px of column) is an ordinary laptop and must never float its dock.
  expect(dockForm(819, BAR, CONTENT)).toBe("docked");
  // A 520px window (439px of column) is where HEAD gave the workspace 77px of its 188.
  expect(dockForm(439, BAR, CONTENT)).toBe("sheet");

  // The touch block widens the bar to 48px, which moves the threshold by exactly that much. This is
  // why the two minimums are read off the live element instead of copied into TypeScript: a constant
  // here would be wrong on every coarse-pointer device, and wrong silently.
  expect(dockForm(both, 48, CONTENT)).toBe("sheet");
  expect(dockForm(both + 6, 48, CONTENT)).toBe("docked");
});

test("an unmeasured column is not a short one", () => {
  // ADR-118's rule inside a layout function: the unknown answer must not be the one that changes
  // behaviour. jsdom has no layout, so every measurement here is 0 — and the first version of this
  // function read that as "float the dock and withdraw the stage's overlays", which took an
  // unrelated end-to-end test down with it.
  expect(dockForm(0, 42, 188)).toBe("docked");
  expect(dockForm(Number.NaN, 42, 188)).toBe("docked");
  expect(dockForm(-1, 42, 188)).toBe("docked");
});
