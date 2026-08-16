//! panelLayout (M10.10 / C8) — the **stage is layout-priority**: the side tracks are fixed px that yield
//! (and collapse to icon rails below a breakpoint) while the MIDDLE (stage) track is always the flex
//! `minmax(STAGE_MIN, 1fr)` — so the viewport never collapses first. Unit-testable without real layout
//! (jsdom has none): the assertion is on the grid template the resize produces.

import { expect, test } from "vitest";
import { dockGridColumns, panelLayout, STAGE_MIN, COLLAPSE_BELOW, OVERLAY_BELOW, RAIL_W, ENGINE_RAIL_W, ENGINE_RAIL_W_COMPACT } from "./layout";

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
  expect(dockGridColumns(wide, false, false)).toBe(`${E}340px minmax(${STAGE_MIN}px, 1fr) 300px`);
  expect(dockGridColumns(wide, false, true)).toBe(`${E}340px minmax(${STAGE_MIN}px, 1fr) ${RAIL_W}px`);
  expect(dockGridColumns(wide, true, false)).toBe(`${E}${RAIL_W}px minmax(${STAGE_MIN}px, 1fr) 300px`);

  const responsive = panelLayout(COLLAPSE_BELOW - 1);
  expect(dockGridColumns(responsive, false, false)).toBe(
    `${ENGINE_RAIL_W_COMPACT}px ${RAIL_W}px minmax(${STAGE_MIN}px, 1fr) ${RAIL_W}px`,
  );
});
