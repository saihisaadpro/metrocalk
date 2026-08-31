// **Finding objects by what they ARE, and selecting what was found — on the packaged `.exe` and the
// real engine.** (ADR-185)
//
// The vitest suite proves what the panel computes from a store it was handed. It cannot prove the two
// things that matter here, because both live on the other side of the IPC boundary:
//
//   1. **The chips describe the REAL scene.** `EntitySummary.kind` is computed in Rust
//      (`bridge::enrich_relational` → `classify_kind`) and mirrored in TypeScript for the dev mock
//      (`store/relSummary.ts`). Two statements of one contract, which the compiler checks separately
//      and never against each other — the exact shape `<test_and_ci_discipline>` 6 is about. A jsdom
//      test necessarily photographs the MIRROR. This one reads the projection the real `/core` sent.
//   2. **`Select all N` reaches the ENGINE.** The panel could set its own store and outline nothing,
//      which is the failure ADR-158 collapsed three selections into one seam to make impossible. The
//      authority is `selection_ids` — an engine READ. A read cannot fake a capability.
//
// Every gesture here is driven through the UI: real clicks on the chips and the button, real keys for
// the chord. The only commands invoked directly are reads.
//
// Run: node "node_modules\@wdio\cli\bin\wdio.js" run wdio.selecttest.conf.js --spec specs-selecttest/outliner-find.e2e.js

import { browser } from "@wdio/globals";
import { mkdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const dir = path.dirname(fileURLToPath(import.meta.url));
const SHOT_DIR = process.env.MTK_SHOT_DIR || path.resolve(dir, "../.shots-outliner-find");
mkdirSync(SHOT_DIR, { recursive: true });

/**
 * A WEBDRIVER capture, deliberately, and the sibling spec's OS capture is why.
 *
 * `.uxtest/audit/exe/capture-window-fg.ps1` grabs the FOREGROUND window. Run on a box where anything
 * else holds the foreground — which is every unattended run — it silently returns a picture of that
 * other window, and the spec still passes, because a capture helper that swallows its errors reports
 * the same thing whether it photographed the editor or a wallpaper. This spec's first run produced
 * five PNGs of somebody else's fullscreen app, all five captioned as the outliner.
 *
 * The right instrument depends on the claim. `<visual_acceptance>` 1 forbids passing a DOM screenshot
 * off as THE RENDER, because the viewport is a transparent WebView2 over a native wgpu surface and a
 * WebDriver shot photographs a black hole where the 3D is. **This feature is entirely DOM** — a
 * search box, chips, a button and a virtualized list, all in the left dock — so the DOM screenshot
 * photographs exactly the thing under test, and photographs it from inside the packaged `.exe` where
 * it cannot be pointed at the wrong window.
 */
const shot = async (label) => {
  await browser.pause(400);
  const out = path.join(SHOT_DIR, `${label}.png`);
  await browser.saveScreenshot(out);
  console.log("  shot", out);
};

/** What the ENGINE says is selected — the authority, not the React store's mirror of it. */
const engineSelection = () => browser.execute(() => window.__TAURI__.core.invoke("selection_ids"));

/** The outliner's own structured read-out: the counts it publishes and the chips it is offering. */
const panel = () =>
  browser.execute(() => {
    const count = document.getElementById("count");
    const chips = Array.from(document.querySelectorAll("[data-facet]"));
    const rows = Array.from(document.querySelectorAll('[data-testid="hrow"]'));
    return {
      entities: Number(count?.getAttribute("data-entities") ?? -1),
      matches: count?.hasAttribute("data-matches") ? Number(count.getAttribute("data-matches")) : null,
      query: document.querySelector('input[aria-label="Search scene objects"]')?.value ?? null,
      facets: chips.map((c) => ({
        token: c.getAttribute("data-facet"),
        pressed: c.getAttribute("aria-pressed") === "true",
        label: c.textContent,
      })),
      more: document.querySelector('[data-testid="more-facets"]')?.textContent ?? null,
      rows: rows.map((r) => ({
        id: r.getAttribute("data-id"),
        kind: r.getAttribute("data-kind"),
        selected: r.getAttribute("aria-selected") === "true",
      })),
      selectMatches: (() => {
        const b = document.querySelector('[data-testid="select-matches"]');
        return b ? Number(b.getAttribute("data-count")) : null;
      })(),
    };
  });

const clickFacet = (token) =>
  browser.execute((t) => document.querySelector(`[data-facet="${t}"]`).click(), token);

const sameSet = (a, b) => {
  const x = [...new Set(a)].sort();
  const y = [...new Set(b)].sort();
  return x.length === y.length && x.every((v, i) => v === y[i]);
};

describe("the outliner can find objects by what they are, and select what it found", () => {
  before(async () => {
    await browser.waitUntil(
      async () => (await browser.execute(() => !!(window.__TAURI__ && window.__TAURI__.core))) === true,
      { timeout: 30000, timeoutMsg: "TAURI bridge never appeared" },
    );
    // The first-run card sits over the stage; dismiss it so nothing is covering the left dock either.
    await browser.execute(() => {
      try {
        localStorage.setItem("mtk.onboarded.v1", "1");
      } catch (e) {
        void e;
      }
    });
    await browser.refresh();
    await browser.waitUntil(
      async () => (await browser.execute(() => document.querySelectorAll('[data-testid="hrow"]').length)) > 4,
      { timeout: 30000, timeoutMsg: "the seeded scene never reached the outliner" },
    );
  });

  it("offers the kinds the REAL projection classified, counted, and never one that is the whole scene", async () => {
    const collapsed = await panel();
    // THE COLLAPSED ROW IS THREE CONTROLS, and what it withholds is on the control rather than
    // silently absent. The cap exists because of a measurement taken in THIS build: the uncapped row
    // took 98px of a 355px panel on this scene, which classifies into seven kinds.
    expect(collapsed.facets.length).toBeLessThanOrEqual(3);
    expect(collapsed.more).toMatch(/^\+\d+ more$/);

    await browser.execute(() => document.querySelector('[data-testid="more-facets"]').click());
    await browser.pause(200);
    const p = await panel();
    console.log("  scene:", p.entities, "entities · facets:", p.facets.map((f) => f.token).join(" "));
    expect(p.more).toBe("Fewer");
    expect(Number(collapsed.more.match(/\+(\d+)/)[1])).toBe(p.facets.length - collapsed.facets.length);

    // The chips come from `EntitySummary.kind`, which on this build is Rust's `classify_kind` and not
    // the TypeScript mirror a jsdom test would exercise.
    expect(p.facets.length).toBeGreaterThan(1);
    expect(p.matches).toBe(null); // nothing asked yet
    expect(p.selectMatches).toBe(null); // and therefore no verb offered

    for (const f of p.facets) {
      expect(f.token).toMatch(/^(kind:[a-z]+|needs:binding)$/);
      expect(f.pressed).toBe(false);
      const n = Number((f.label.match(/(\d[\d,]*)\s*$/) || [])[1]?.replace(/,/g, ""));
      expect(Number.isFinite(n)).toBe(true);
      expect(n).toBeGreaterThan(0);
      // The invariant `facetsOf` guarantees, stated against the real scene: a filter that removes
      // nothing is not offered.
      expect(n).toBeLessThan(p.entities);
    }
    await shot("outliner-find-facets");
  });

  it("a chip narrows to exactly its kind, and says how many of how many", async () => {
    const before = await panel();
    const token = before.facets[0].token;
    await clickFacet(token);
    await browser.pause(200);

    const after = await panel();
    expect(after.query).toBe(token);
    expect(after.facets.find((f) => f.token === token).pressed).toBe(true);
    expect(after.matches).toBeGreaterThan(0);
    expect(after.matches).toBeLessThan(after.entities);

    if (token.startsWith("kind:")) {
      const kind = token.slice("kind:".length);
      // Every row DRAWN is of that kind. The outliner is virtualized, so this is a claim about the
      // mounted window — the honest shape of the claim over a virtualized list.
      for (const r of after.rows) expect(r.kind).toBe(kind);
    }
    console.log("  narrowed:", token, "→", after.matches, "of", after.entities);
    await shot("outliner-find-narrowed");
  });

  it("Select all N states the match in the ENGINE, not only in the panel", async () => {
    // THE CLAIM ONLY THE .exe CAN MAKE. The panel could set its own store and outline nothing; the
    // authority is the engine's own selection, read back.
    const p = await panel();
    expect(p.selectMatches).toBe(p.matches);
    expect(p.selectMatches).toBeGreaterThan(0);

    await browser.execute(() => document.querySelector('[data-testid="select-matches"]').click());
    await browser.pause(400);

    const engine = await engineSelection();
    expect(engine.length).toBe(p.matches);

    // And the rows the panel is DRAWING are all in it, AND SHOW THAT THEY ARE — the agreement claim,
    // restricted to the mounted window because the list is virtualized. Both halves matter: a row
    // that is in the engine's set and does not render selected is the list and the stage disagreeing,
    // which is the failure ADR-158 collapsed three selections into one seam to make impossible.
    const after = await panel();
    const drawn = after.rows.map((r) => r.id);
    const set = new Set(engine);
    expect(drawn.filter((id) => !set.has(id))).toEqual([]);
    expect(after.rows.filter((r) => !r.selected).map((r) => r.id)).toEqual([]);
    console.log("  engine selection after Select all:", engine.length);
    await shot("outliner-find-selected");
  });

  it("a shift-click range inside a filter cannot reach past the rows it is drawn from", async () => {
    // ADR-185 defect 3, live. `selectRange` walked `order` — every entity in the scene — while the
    // panel drew `filteredOrder`, so this gesture used to take everything BETWEEN the two rows in the
    // unfiltered scene: on an import, hundreds of objects the user cannot see, one keystroke away from
    // Delete.
    const p = await panel();
    expect(p.rows.length).toBeGreaterThan(1);
    const first = p.rows[0].id;
    const last = p.rows[p.rows.length - 1].id;

    await browser.execute((id) => document.querySelector(`[data-testid="hrow"][data-id="${id}"]`).click(), first);
    await browser.pause(200);
    await browser.execute((id) => {
      const row = document.querySelector(`[data-testid="hrow"][data-id="${id}"]`);
      row.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, shiftKey: true }));
    }, last);
    await browser.pause(400);

    const engine = await engineSelection();
    const drawnIds = p.rows.map((r) => r.id);
    // Every selected object is one the filter matched. Not a count — a property: a count alone would
    // still pass if the range had drifted by a row at each end.
    expect(engine.filter((id) => !drawnIds.includes(id))).toEqual([]);
    expect(sameSet(engine, drawnIds)).toBe(true);
    console.log("  range over a filtered list:", engine.length, "selected,", p.entities, "in the scene");
    await shot("outliner-find-range");
  });

  it("Ctrl+F opens the Scene workspace and puts the caret in the box that searches it", async () => {
    // The chord was unbound. It has to open the panel BEFORE asking for focus, because the outliner is
    // a `hidden` tabpanel in every other workspace and focus into `display:none` is focus nobody sees.
    await browser.execute(() => document.getElementById("engine-tab-build")?.click());
    await browser.pause(300);
    expect(await browser.execute(() => document.getElementById("engine-panel-scene").hasAttribute("hidden"))).toBe(true);

    await browser.execute(() => document.body.focus());
    await browser.keys(["Control", "f"]);
    await browser.pause(400);

    const state = await browser.execute(() => ({
      hidden: document.getElementById("engine-panel-scene").hasAttribute("hidden"),
      focused: document.activeElement?.getAttribute("aria-label") ?? null,
    }));
    expect(state.hidden).toBe(false);
    expect(state.focused).toBe("Search scene objects");
    await shot("outliner-find-chord");
  });
});
