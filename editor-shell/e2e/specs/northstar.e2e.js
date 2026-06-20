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

  // ── north-star test #2: describe-to-create (live) ──────────────────────────────────────────────
  it("describes a component into existence and offers its attach", async () => {
    await $("#describe").setValue("health bar");
    await $("#describeBtn").click();
    await browser.waitUntil(async () => (await $("#status").getText()).includes("created"), {
      timeout: 10000,
      timeoutMsg: "no 'created' status after describe",
    });
    expect(await $("#status").getText()).toContain("HealthBar"); // resolved to the right kind
    // the created HealthBar is selected → its reveal panel offers Health providers to attach (≤2 total)
    await browser.waitUntil(async () => (await $("#reveal").getText()).includes("requires"), {
      timeout: 10000,
      timeoutMsg: "the described entity's attach panel never populated",
    });
    expect(await $("#reveal").getText()).toContain("Health");
  });

  it("a no-local-match describe resolves the MARKETPLACE tier — pre-componentized, not faked (M5)", async () => {
    await $("#describe").setValue("rusty medieval sword");
    await $("#describeBtn").click();
    await browser.waitUntil(async () => (await $("#status").getText()).includes("marketplace"), {
      timeout: 10000,
      timeoutMsg: "no marketplace resolution on a no-local-match describe",
    });
    // M5: the marketplace tier is real — the entry applies pre-componentized (the Weapon component) with
    // its inert token-price seam, rather than the old M3.2 "would query marketplace" stub.
    const status = await $("#status").getText();
    expect(status).toContain("marketplace:");
    expect(status).toContain("tokens");
  });

  // ── M3.3: viewport context actions + hover details (the "context reveal") ─────────────────────────
  const ctxVisible = async () => (await $("#ctxmenu").getCSSProperty("display")).value !== "none";

  it("right-clicks a viewport entity → a context menu of its valid actions appears", async () => {
    const vp = await $("#viewport");
    // A right-button press+release with no movement (a click, not an orbit) at the viewport centre.
    await browser.action("pointer", { parameters: { pointerType: "mouse" } })
      .move({ origin: vp })
      .down({ button: 2 })
      .up({ button: 2 })
      .perform();
    await browser.waitUntil(ctxVisible, { timeout: 10000, timeoutMsg: "context menu never opened on right-click" });
    const items = await $$("#ctxmenu .ctxitem");
    expect(items.length).toBeGreaterThanOrEqual(5); // Bind / Remove / Duplicate / Focus / Inspect
    // every unavailable action carries a reason (the explain-every-"no" discipline)
    for (const it of items) {
      if ((await it.getAttribute("class")).includes("disabled")) {
        expect(await it.getText()).toContain("—"); // "Action  —  reason"
      }
    }
  });

  it("Remove from the menu deletes the entity → status says removed (Ctrl-Z to undo)", async () => {
    // Click the Remove item.
    const remove = await $('#ctxmenu .ctxitem[data-action="remove"]');
    await remove.click();
    await browser.waitUntil(async () => (await $("#status").getText()).includes("removed"), {
      timeout: 10000,
      timeoutMsg: "no 'removed' status after clicking Remove",
    });
    expect(await $("#status").getText()).toContain("Ctrl-Z");
    // and the menu closed.
    expect(await ctxVisible()).toBe(false);
  });

  it("Ctrl-Z restores the removed entity (one undoable transaction)", async () => {
    await browser.keys(["Control", "z"]);
    await browser.waitUntil(async () => (await $("#status").getText()).includes("undo"), {
      timeout: 10000,
      timeoutMsg: "no 'undo' status after Ctrl-Z",
    });
    expect(await $("#status").getText()).toContain("undo");
  });

  it("hovering an entity shows a details tooltip without selecting it", async () => {
    const vp = await $("#viewport");
    const before = await $("#status").getText();
    // Settle the cursor over the viewport centre (the debounced peek fires, then entity_details).
    await browser.action("pointer", { parameters: { pointerType: "mouse" } })
      .move({ origin: vp, x: 5, y: 5 })
      .move({ origin: vp })
      .perform();
    await browser.waitUntil(
      async () => (await $("#tooltip").getCSSProperty("display")).value !== "none",
      { timeout: 10000, timeoutMsg: "hover tooltip never appeared" }
    );
    expect(await $("#tooltip").getText()).toMatch(/Transform|Health|Renderable/);
    // hover did NOT change the selection (status unchanged — no "picked" fired by hovering).
    expect(await $("#status").getText()).toBe(before);
  });

  it("right-DRAG still orbits and does NOT open the menu (disambiguation)", async () => {
    await $("#ctxmenu") && (await browser.keys(["Escape"])); // ensure menu closed
    const vp = await $("#viewport");
    await browser.action("pointer", { parameters: { pointerType: "mouse" } })
      .move({ origin: vp })
      .down({ button: 2 })
      .move({ origin: vp, x: 60, y: 30 }) // drag well past the movement threshold
      .move({ origin: vp, x: 90, y: 50 })
      .up({ button: 2 })
      .perform();
    // a drag past the threshold is an orbit, not a click → the menu must stay closed.
    await browser.pause(300);
    expect(await ctxVisible()).toBe(false);
  });
});
