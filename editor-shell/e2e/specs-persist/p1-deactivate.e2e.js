// R-NEXT-2 step 1: deactivate an entity (toolbar Delete) and record its id. The deactivate is now
// persisted to the replay log (Record::DeleteDeactivate), so step 2's relaunch will replay it.
import { browser } from "@wdio/globals";
import { writeFileSync, mkdirSync } from "node:fs";

const STATE = "x:\\Dev Research & Projects\\Metrocalk\\.uxtest\\audit\\data\\exe";
mkdirSync(STATE, { recursive: true });

describe("R-NEXT-2 persist · deactivate", () => {
  it("deactivates the first entity and persists it", async () => {
    await browser.waitUntil(
      async () => { try { return /\d+\s+entities/.test(await (await browser.$("#count")).getText()); } catch { return false; } },
      { timeout: 60000, timeoutMsg: "never connected" },
    );
    const row = await browser.$("[data-testid='hrow']");
    const id = await row.getAttribute("data-id");
    const name = (await row.getText()).trim();
    await row.click();
    await browser.pause(300);
    await (await browser.$("#authMore")).click();
    await browser.waitUntil(async () => {
      const control = await browser.$("#authDelete");
      return (await control.isExisting())
        && (await control.isDisplayed())
        && (await control.getAttribute("aria-disabled")) !== "true";
    }, {
      timeout: 5000,
      timeoutMsg: "Actions did not reveal an available Delete command",
    });
    await (await browser.$("#authDelete")).click();
    await browser.pause(600);
    writeFileSync(`${STATE}\\deact.json`, JSON.stringify({ id, name }));
    console.log("PERSIST-P1: deactivated id=", id, "name=", JSON.stringify(name));
    expect(id).toBeTruthy();
  });
});
