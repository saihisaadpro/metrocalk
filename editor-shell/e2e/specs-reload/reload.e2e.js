// RELOAD regression (Prompt 22), live against the real .exe. The app is launched with a pre-seeded
// scene log (see wdio.reload.conf.js) standing in for a prior session's binds. This asserts the FIX:
// the restored binding is *surfaced* on load — the requirer carries a "tracking" badge regardless of
// its id order, auto-focus selects it, and its "tracking" list is populated. On the pre-fix frontend
// (first-40-by-id requirers, no badge, no auto-focus) every one of these assertions failed: the bind
// was in the engine but invisible — exactly the user's "no tracking after relaunch".

import { browser, $, $$, expect } from "@wdio/globals";

describe("Metrocalk editor — restored binds surface on reload (live)", () => {
  it("connects to /core with the restored scene", async () => {
    await browser.waitUntil(async () => /\d+ entities/.test(await $("#count").getText()), {
      timeout: 60000,
      timeoutMsg: "editor never connected to /core",
    });
    expect(await $("#count").getText()).toMatch(/5000 entities/); // describe-create not in this log → exactly seed
  });

  it("auto-focuses the restored requirer (status shows 'restored …')", async () => {
    await browser.waitUntil(async () => (await $("#status").getText()).includes("restored"), {
      timeout: 20000,
      timeoutMsg: "no 'restored' auto-focus status on load — restored binds were not surfaced",
    });
    expect(await $("#status").getText()).toContain("tracking");
  });

  it("surfaces the bound HealthBar with a 'tracking' badge in the requirers (id-order-independent)", async () => {
    const cands = await $$("#requirers .cand");
    const texts = [];
    for (const c of cands) texts.push(await c.getText());
    const tracking = texts.filter((t) => t.includes("tracking"));
    expect(tracking.length).toBeGreaterThan(0); // pre-fix: 1_1129 (high id) never made the first-40 list
    expect(tracking[0]).toContain("1_1129");
  });

  it("shows the restored bindings under 'tracking' without a manual click", async () => {
    // auto-focus selected the bound requirer → its reveal panel lists what it tracks (the two providers)
    await browser.waitUntil(async () => (await $$("#reveal .boundrow")).length >= 2, {
      timeout: 20000,
      timeoutMsg: "the restored 'tracking' list never populated for the auto-focused requirer",
    });
    expect((await $$("#reveal .boundrow")).length).toBeGreaterThanOrEqual(2);
  });
});
