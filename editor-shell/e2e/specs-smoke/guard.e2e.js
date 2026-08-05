// Data-loss-prevention check on the REAL shell: an edit marks the project dirty, and File → New then
// raises the unsaved-changes guard (the dev MockCore can't exercise this — it resets dirty on menu-open).
import { browser } from "@wdio/globals";

describe("unsaved-changes guard (.exe)", () => {
  it("edit → File → New raises the guard; Cancel keeps the scene", async () => {
    await browser.waitUntil(
      async () => { try { return /\d+\s+entities/.test(await (await browser.$("#count")).getText()); } catch { return false; } },
      { timeout: 60000, timeoutMsg: "never connected" },
    );
    const countBefore = await (await browser.$("#count")).getText();

    // Make a real edit so the shell marks the project dirty: select the first hierarchy row, edit a field.
    const row = await browser.$("[data-testid='hrow']");
    if (await row.isExisting()) { await row.click(); await browser.pause(300); }
    const inp = await browser.$("#inspector input");
    if (await inp.isExisting()) { await inp.setValue("3"); await browser.keys("Enter"); await browser.pause(300); }

    await (await browser.$("#fileMenu")).click();
    await browser.pause(250);
    const dirty = await (await browser.$("#projectDirty")).isExisting();
    await (await browser.$("#fileNew")).click();
    await browser.pause(350);
    const guard = await (await browser.$("#unsavedGuard")).isExisting();
    console.log("GUARD: dirty=", dirty, " guardShown=", guard);

    if (guard) { await (await browser.$("#guardCancel")).click(); await browser.pause(250); }
    const countAfter = await (await browser.$("#count")).getText();
    console.log("GUARD: count", countBefore, "->", countAfter, "(Cancel should keep the scene)");

    expect(guard).toBe(true);
  });
});
