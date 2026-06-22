// M10.5 — first-run ONBOARDING: a short, skippable, NON-nagging "make your first thing" on-ramp. The
// acceptance dimensions here are principle-1 (an on-ramp, never a wall) + honesty (the copy promises ONLY
// what M10 ships — over-promising a not-yet capability is the adversarial failure the can/can't doc guards).
// React build only (the card is React chrome). Re-summons the first-run state by clearing its localStorage
// flag + reloading, independent of whatever prior runs left on disk.

import { browser, expect, $ } from "@wdio/globals";
import { page } from "../../pages/scaffold.js";
import { report, consoleErrors } from "../../lib/acceptance.js";

const ui = page();
const FLAG = "mtk.onboarded.v1";

const onboardingShown = async () => {
  const c = await $("#onboarding");
  return (await c.isExisting()) && (await c.isDisplayed());
};

describe("acceptance / M10.5 onboarding — first-run 'make your first thing' (skippable · no nagging · honest copy)", () => {
  before(async () => {
    await ui.waitConnected();
  });

  it("appears on first run, promises only what M10 ships, Skip dismisses it, and it never nags again", async () => {
    // ── summon the first-run state: clear the seen-flag + reload ────────────────────────────────────
    await browser.execute((flag) => {
      try {
        localStorage.removeItem(flag);
      } catch {
        /* no storage */
      }
    }, FLAG);
    await browser.refresh();
    await ui.waitConnected();
    await browser.waitUntil(onboardingShown, { timeout: 10000, timeoutMsg: "onboarding did not appear on first run" });

    // ── HONESTY: the copy must NOT promise a capability M10 doesn't ship (scripting/Rules → M12;
    //    materials/lighting/audio → M11; build/export → later). Cross-checks the can/can't doc. ────────
    const text = (await (await $("#onboarding")).getText()).toLowerCase();
    for (const forbidden of ["script", "material", "lighting", "audio", "export", "shader", "rules engine"]) {
      expect(text).not.toContain(forbidden); // no over-promise
    }
    // it DOES name the real, shipped loop (place/describe · bind · Play · Save).
    expect(text).toContain("play");
    expect(text).toContain("save");
    expect(text).toContain("bind");

    // ── SKIPPABLE: one click dismisses it (principle 1 — an on-ramp, never a wall) ──────────────────
    await (await $("#onboardSkip")).click();
    await browser.waitUntil(async () => !(await onboardingShown()), { timeout: 5000, timeoutMsg: "Skip did not dismiss the onboarding card" });

    // ── NO NAGGING: reload → it stays gone (the dismissal persisted) ────────────────────────────────
    await browser.refresh();
    await ui.waitConnected();
    await browser.pause(600);
    expect(await onboardingShown()).toBe(false);

    report.workflow(
      "onboarding/first-run-skippable-no-nag",
      { functional: true, p1_interactions: true, clean: (await consoleErrors()).length === 0 },
      { commands: [] }
    );
  });
});
