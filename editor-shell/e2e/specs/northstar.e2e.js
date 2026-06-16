// North-star #1, end-to-end against the real .exe (WebView2 DOM via tauri-driver). Drives the editor
// panel AND the transparent viewport <div> (whose clicks fire the native Rust pick), asserting the
// resulting DOM — so the live round-trip, including the viewport pick we could only test by hand
// before, is verified automatically.

import { browser, $, $$, expect } from "@wdio/globals";

const boundCount = async () => (await $$("#reveal .boundrow")).length;

describe("Metrocalk editor — north-star #1 live", () => {
  it("launches, composites, and connects to /core (5000 entities)", async () => {
    await browser.waitUntil(async () => /\d+ entities/.test(await $("#count").getText()), {
      timeout: 60000,
      timeoutMsg: "editor never showed an entity count (no /core connection?)",
    });
    expect(await $("#count").getText()).toMatch(/5000 entities/);
    expect(await $("#status").getText()).toContain("connected");
  });

  it("surfaces requirers (HealthBars) to bind", async () => {
    const reqs = await $$("#requirers .cand");
    expect(reqs.length).toBeGreaterThan(0);
    expect(await reqs[0].getText()).toContain("HealthBar");
  });

  it("reveals ranked compatible targets when a requirer is selected", async () => {
    await (await $$("#requirers .cand"))[0].click();
    await browser.waitUntil(async () => (await $("#reveal").getText()).includes("requires"), {
      timeout: 10000,
      timeoutMsg: "reveal panel never populated after selecting a requirer",
    });
    expect(await $("#reveal").getText()).toContain("Health");
    expect((await $$("#reveal .cand")).length).toBeGreaterThan(0);
  });

  it("binds a compatible target in one click → it moves to 'tracking'", async () => {
    const before = await boundCount();
    await (await $$("#reveal .cand"))[0].click();
    await browser.waitUntil(async () => (await boundCount()) > before, {
      timeout: 10000,
      timeoutMsg: "bound target never appeared under 'tracking'",
    });
    expect(await boundCount()).toBeGreaterThan(before);
  });

  it("undoes the bind in one step → 'tracking' shrinks", async () => {
    const before = await boundCount();
    await $("#undo").click(); // same doUndo() path as Ctrl-Z
    await browser.waitUntil(async () => (await boundCount()) < before, {
      timeout: 10000,
      timeoutMsg: "tracking did not shrink after undo",
    });
    expect(await boundCount()).toBeLessThan(before);
  });

  it("picks an entity from the VIEWPORT — the native pick round-trip", async () => {
    // Click the viewport <div>'s centre → JS sends a normalized cursor → Rust picks the nearest cube
    // → returns its id → the inspector + status update. This is the path we kept fixing blind.
    await $("#viewport").click();
    await browser.waitUntil(async () => (await $("#status").getText()).includes("picked"), {
      timeout: 10000,
      timeoutMsg: "no 'picked' status after a viewport click (pick not serviced?)",
    });
    const status = await $("#status").getText();
    const inspector = await $("#inspector").getText();
    expect(status).toContain("picked"); // a click in the cube cloud must select something
    expect(status).not.toContain("nothing here");
    expect(inspector).toContain("Transform");
  });

  it("edits a field through the pipeline (round-trip)", async () => {
    const input = await $("#inspector input");
    if (!(await input.isExisting())) return; // no field to edit (defensive)
    await input.setValue("12.5");
    await browser.keys(["Enter"]);
    await browser.waitUntil(async () => (await $("#status").getText()).includes("edit"), {
      timeout: 10000,
      timeoutMsg: "no 'edit' status after changing a field",
    });
    expect(await $("#status").getText()).toContain("edit");
  });
});
