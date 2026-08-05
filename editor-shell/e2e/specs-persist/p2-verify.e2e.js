// R-NEXT-2 step 2: a FRESH relaunch (the log persisted — beforeSession does not clean it). The replayed
// DeleteDeactivate re-deactivates the entity, and project_full re-emits active:false on connect, so the
// hierarchy row is dimmed/struck "· hidden" with NO optimistic action here — proving cross-reload persistence.
import { browser } from "@wdio/globals";
import { readFileSync } from "node:fs";

const STATE = "x:\\Dev Research & Projects\\Metrocalk\\.uxtest\\audit\\data\\exe";

describe("R-NEXT-2 persist · verify after relaunch", () => {
  it("the deactivated entity is still hidden after reload (wire-driven)", async () => {
    const { id, name } = JSON.parse(readFileSync(`${STATE}\\deact.json`, "utf8"));
    await browser.waitUntil(
      async () => { try { return /\d+\s+entities/.test(await (await browser.$("#count")).getText()); } catch { return false; } },
      { timeout: 60000, timeoutMsg: "never connected" },
    );
    const row = await browser.$(`[data-testid='hrow'][data-id='${id}']`);
    const exists = await row.isExisting();
    const txt = exists ? (await row.getText()).trim() : "";
    let deco = "";
    try { deco = (await row.getCSSProperty("text-decoration-line")).value; } catch { /* fine */ }
    const hidden = /hidden/i.test(txt) || /line-through/.test(deco);
    console.log("PERSIST-P2: id=", id, "name=", JSON.stringify(name), "exists=", exists, "text=", JSON.stringify(txt), "deco=", deco, "=> hidden=", hidden);
    expect(exists).toBe(true);
    expect(hidden).toBe(true);
  });
});
