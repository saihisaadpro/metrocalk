// Build-acceptance — M11.3 LIGHTING (ADR-042): lights are ENTITIES, a directional caster casts shadows,
// authoring is ONE undoable commit, and the lit/shadow result is a render PROJECTION (0-IPC, ADR-021).
// The transparent viewport can't be asserted by pixels, so the gate keys off the stable `lighting_debug`
// signal: [authored light entities (doc truth), render light count (incl. the synthesized default key
// light), shadow-caster index or -1, caster kind 0=dir/1=point/2=spot]. Drives `add_light` — the same
// command the React toolbar feeds.

import { browser, expect, $ } from "@wdio/globals";
import { report, invoke, consoleErrors, clearConsole, ipcPerFrame } from "../../lib/acceptance.js";
import { page } from "../../pages/scaffold.js";

const ui = page();
const countEntities = async () => {
  const m = (await $("#count").getText()).match(/(\d+)\s+entities/);
  return m ? Number(m[1]) : NaN;
};

describe("acceptance / M11.3 — lighting: an authored directional light is the shadow caster (live)", () => {
  before(async () => {
    await browser.waitUntil(async () => (await countEntities()) > 0, {
      timeout: 20000,
      timeoutMsg: "editor never connected (#count empty)",
    });
    await clearConsole();
  });

  it("add a directional light → one undoable authored entity that becomes the shadow caster; undo restores the default; lighting stays a render projection (0-IPC orbit)", async () => {
    await clearConsole();

    // Baseline: the scene ALWAYS has a shadow caster (the synthesized default key light when nothing is
    // authored, ADR-042) — never an unlit scene. Record `authored` so the add/undo delta is exact.
    const [authored0, , caster0] = await invoke("lighting_debug");
    expect(caster0).toBeGreaterThanOrEqual(0);

    // Author a DIRECTIONAL light — the headline shadow case (a directional caster casts by default).
    const id = await invoke("add_light", {
      kind: "directional",
      x: 0,
      y: 8,
      z: 0,
      r: 1,
      g: 1,
      b: 1,
      intensity: 3,
    });
    expect(typeof id).toBe("string"); // a real authored light entity id

    const [authored1, , caster1, kind1] = await invoke("lighting_debug");
    const authoredGrew = authored1 === authored0 + 1; // one NEW authored light (doc truth)
    const casts = caster1 >= 0 && kind1 === 0; // a DIRECTIONAL light is the shadow caster

    // Invariant 4 / ADR-021: the lit + shadow result is a render PROJECTION — an orbit with the light
    // mounted holds 0 IPC/frame (the shadow pass runs render-side, never crossing JS per-frame).
    const perFrame = await ipcPerFrame(() => ui.orbit(120, 60), 450);
    const zeroIpc = perFrame < 1;

    // ONE undoable commit — Ctrl-Z removes the authored light; the render falls back to the default caster.
    await invoke("undo");
    const [authored2, , caster2] = await invoke("lighting_debug");
    const undone = authored2 === authored0 && caster2 >= 0; // authored gone; scene still lit (default)

    const errs = await consoleErrors();
    const clean = errs.length === 0;
    if (!clean) report.consoleErrorCount += errs.length;
    report.workflow(
      "lighting/author + shadow-caster",
      { functional: authoredGrew && casts && undone, inv1: true, inv4: zeroIpc, clean, offline: true },
      { commands: ["add_light", "lighting_debug", "undo", "ipc_count"] }
    );
    expect(authoredGrew).toBe(true);
    expect(casts).toBe(true);
    expect(zeroIpc).toBe(true);
    expect(undone).toBe(true);
    expect(clean).toBe(true);
  });
});
