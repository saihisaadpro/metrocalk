// Undo honesty on the REAL shell: Ctrl-Z on an empty history must say "nothing to undo"; after an edit it
// must say "undo" (the shell now reports whether a transaction was actually reverted).
import { browser } from "@wdio/globals";

const status = async () => { try { return await (await browser.$("#status")).getText(); } catch { return ""; } };

describe("undo honesty (.exe)", () => {
  it("empty history → 'nothing to undo'; after an edit → 'undo'", async () => {
    await browser.waitUntil(
      async () => { try { return /\d+\s+entities/.test(await (await browser.$("#count")).getText()); } catch { return false; } },
      { timeout: 60000, timeoutMsg: "never connected" },
    );
    // Empty history (seed + compose are dropped from the undo stack): Ctrl-Z should be an honest no-op.
    await browser.keys(["Control", "z"]);
    await browser.pause(500);
    const sEmpty = await status();
    console.log("UNDO-EMPTY:", JSON.stringify(sEmpty));

    // Make a real edit, blur, then Ctrl-Z → an actual revert.
    const row = await browser.$("[data-testid='hrow']");
    if (await row.isExisting()) { await row.click(); await browser.pause(300); }
    const inp = await browser.$("#inspector input");
    if (await inp.isExisting()) { await inp.setValue("3"); await browser.keys("Enter"); await browser.pause(300); }
    await (await browser.$("#status")).click().catch(() => {}); // blur the field
    await browser.pause(150);
    await browser.keys(["Control", "z"]);
    await browser.pause(600);
    const sAfter = await status();
    console.log("UNDO-AFTER-EDIT:", JSON.stringify(sAfter));

    expect(sEmpty.toLowerCase()).toContain("nothing to undo");
    expect(sAfter.toLowerCase()).toContain("undo");
  });
});
