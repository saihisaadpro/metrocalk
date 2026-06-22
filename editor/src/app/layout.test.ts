//! panelLayout (M10.10 / C8) — the **stage is layout-priority**: the side tracks are fixed px that yield
//! (and collapse to icon rails below a breakpoint) while the MIDDLE (stage) track is always the flex
//! `minmax(STAGE_MIN, 1fr)` — so the viewport never collapses first. Unit-testable without real layout
//! (jsdom has none): the assertion is on the grid template the resize produces.

import { expect, test } from "vitest";
import { panelLayout, STAGE_MIN, COLLAPSE_BELOW } from "./layout";

test("the stage is ALWAYS the flex region with a protected floor (it never collapses first)", () => {
  for (const w of [1920, 1366, 1024, 900, 768, 500]) {
    const l = panelLayout(w);
    // the MIDDLE track is the stage — the only flex (1fr) track, with a protected minimum
    expect(l.gridColumns).toContain(`minmax(${STAGE_MIN}px, 1fr)`);
    expect(l.gridColumns.match(/1fr/g)?.length).toBe(1); // exactly one flex track: the stage
    expect(l.gridColumns.indexOf("1fr")).toBeGreaterThan(0); // it's the MIDDLE track, panels flank it
  }
});

test("panels shrink, then collapse to icon rails below the breakpoint (the stage keeps the space)", () => {
  const wide = panelLayout(1440);
  expect(wide.collapsed).toBe(false);
  expect(wide.left).toBe(260);

  const mid = panelLayout(1024);
  expect(mid.collapsed).toBe(false);
  expect(mid.left).toBeLessThan(wide.left); // panels yielded but stay open

  const narrow = panelLayout(COLLAPSE_BELOW - 1);
  expect(narrow.collapsed).toBe(true); // collapsed to icon rails
  expect(narrow.left).toBeLessThan(mid.left);
});
