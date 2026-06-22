// Live acceptance — M10.10 editor UX-hardening (ADR-035) + the M10.1 viewport/vocabulary closeout, on the
// packaged .exe, React UI (MTK_UI=react). Asserts the fixes the dev/MockCore build can't exercise, against
// the REAL /core + the live WebView2 composite — keyed on the React UI's actual stable ids:
//   • boot      — the React shell LOADS in the packaged .exe and CONNECTS to the real /core.
//   • C6        — selecting a real requirer (the HealthBar marker) surfaces REAL /core data: the reveal
//                 runs the live compat query (ranked candidates) and the data-driven inspector renders the
//                 entity's real components (Transform/HealthBar/…). The panels now speak the real /core
//                 vocabulary (the M10.1 parity fix — they used to filter the MockCore "Socket"/"Provides").
//   • composite — the viewport div is TRANSPARENT, so the native wgpu scene composites through (ADR-008) —
//                 no longer an opaque div occluding the viewport.
//   • inv4      — orbiting the viewport stays ~0 IPC/frame (the native render loop polls the cursor; only
//                 drag_start/drag_end cross JS) — the hot path never crosses the boundary.
//   • C1        — describe→Create→Generate drives the real `generate`: a placeholder is PLACED on /core.
//   • C2        — Play is unmistakable ON THE STAGE: the on-stage badge + a reachable Stop appear live.
//
// Still owed (NOT asserted — see progress/M10.md): the generate's metered stream-in (render-coupled) + the
// live Stop-restores-scene (M10.4); C9 the native Save dialog (human); C10 the real first-run seed (backend);
// budgets/min-spec; the full scaffold-page-object remap + the frontendDist switch + scaffold deletion.

import { browser, $, $$, expect } from "@wdio/globals";
import { invoke, consoleErrors, clearConsole, ipcPerFrame } from "../../lib/acceptance.js";

const text = (sel) => $(sel).then((e) => e.getText());
const countEntities = async () => {
  const m = (await text("#count")).match(/(\d+)\s+entities/);
  return m ? Number(m[1]) : NaN;
};

// React-only surface (#genBtn / #playStageBadge / #count) — skip when the run targets the vanilla scaffold.
const reactOnly = process.env.MTK_UI === "react" ? describe : describe.skip;

reactOnly("acceptance / M10.10 editor UX hardening + viewport closeout (live, React UI on the .exe)", () => {
  before(async () => {
    await browser.waitUntil(async () => /\d+ entities/.test(await text("#count")), {
      timeout: 60000,
      timeoutMsg: "the React editor never connected to /core (#count empty)",
    });
    await clearConsole();
  });

  it("boot — the React editor loads in the .exe and connects to the REAL /core (live bridge)", async () => {
    expect(await countEntities()).toBeGreaterThan(0);
    expect(await invoke("wallet_info")).toBeTruthy();
  });

  it("C6 — selecting a real requirer surfaces REAL /core data: ranked reveal + a real-component inspector", async () => {
    const reqs = await $$("#requirers .cand");
    expect(reqs.length).toBeGreaterThan(0); // the real seed's HealthBars now surface (was 0 pre-fix)
    await reqs[0].click();

    // the reveal runs the LIVE compat query → ranked compatible targets appear (north-star #1, real core)
    await browser.waitUntil(async () => (await $$("#reveal .cand")).length > 0, {
      timeout: 10000,
      timeoutMsg: "the reveal never populated ranked candidates from the live /core compat query",
    });
    expect((await $$("#reveal .cand")).length).toBeGreaterThan(0);

    // the data-driven inspector renders REAL, EDITABLE properties from the live /core (not the placeholder,
    // not a blank pane): the schema is inferred from the projected component values, so editable inputs appear
    const insp = await text("#inspector");
    expect(insp).not.toContain("Select an entity to inspect");
    expect(insp).not.toContain("No editable properties yet");
    expect((await $$("#inspector input")).length).toBeGreaterThan(0); // editable real-/core property fields
  });

  it("composite — the viewport div is TRANSPARENT so the native wgpu scene composites through (ADR-008)", async () => {
    const bg = await browser.execute(() => {
      const el = document.getElementById("viewport");
      return el ? getComputedStyle(el).backgroundColor : null;
    });
    // transparent → rgba(0,0,0,0); NOT an opaque dark fill occluding the wgpu viewport
    expect(/rgba?\(0,\s*0,\s*0,\s*0\)|transparent/.test(String(bg))).toBe(true);
  });

  it("inv4 — orbiting the viewport stays ~0 IPC/frame (the native render loop polls the cursor)", async () => {
    const vp = await $("#viewport");
    const perFrame = await ipcPerFrame(async () => {
      await browser
        .action("pointer", { parameters: { pointerType: "mouse" } })
        .move({ origin: vp })
        .down({ button: 2 })
        .move({ origin: vp, x: 40, y: 20 })
        .move({ origin: vp, x: 80, y: 40 })
        .up({ button: 2 })
        .perform();
    }, 450);
    expect(perFrame).toBeLessThan(1); // far less than 1 IPC/frame → the hot path doesn't cross JS
  });

  it("C1 — describe→Create→Generate drives the REAL generate: a placeholder is PLACED on /core", async () => {
    const cBefore = await countEntities();

    await (await $("#describe")).setValue("a glowing crystal totem zzq");
    await (await $("#describeBtn")).click();
    await browser.waitUntil(async () => (await $("#genBtn")).isExisting(), {
      timeout: 15000,
      timeoutMsg: "a no-match describe never surfaced #genBtn on the .exe (the C1 dead-end would persist)",
    });
    expect(await (await $("#genBtn")).isDisplayed()).toBe(true);

    await (await $("#genBtn")).click();
    await browser.waitUntil(async () => (await countEntities()) > cBefore, {
      timeout: 20000,
      timeoutMsg: `Generate never placed a placeholder on /core (count stayed ${cBefore})`,
    });
    expect(await countEntities()).toBeGreaterThan(cBefore);
  });

  it("C2 — Play is unmistakable ON THE STAGE: the on-stage badge + a reachable Stop appear live", async () => {
    await (await $("#play")).click();
    await browser.waitUntil(async () => (await $("#playStageBadge")).isExisting(), {
      timeout: 10000,
      timeoutMsg: "Play did not show the on-stage badge (#playStageBadge) on the .exe",
    });
    const badge = await $("#playStageBadge");
    expect(await badge.isDisplayed()).toBe(true);
    expect(await badge.getText()).toMatch(/playing/i);
    expect(await (await $('[data-testid="stageStop"]')).isDisplayed()).toBe(true);
  });

  it("clean — no JS console errors across the live UX flows", async () => {
    expect(await consoleErrors()).toEqual([]);
  });
});
