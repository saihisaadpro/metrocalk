// ADR-190 step 2 — A SECOND LAUNCH OF THE SAME `.exe`, and the five answers are still there.
//
// THIS IS THE HALF NO UNIT TEST CAN REACH, and it is the half ADR-182 named when it wrote the closing
// gate: "the delivery, the rate, the size and the name surviving a reopen AND a restart — session-scoped
// memory would satisfy a reopen and fail the thing that matters." A reopen is a React remount. This is a
// different PROCESS: `beforeSession` deliberately does not clean `metrocalk-scene.jsonl`, so the only
// road from step 1's gestures to this window is `Record::CinemaRender`, replayed at startup into a
// document built from nothing.
//
// The assertions are made on the DIALOG'S OWN CONTROLS, not on `cinema_list`. The engine holding the
// value and the dialog opening on it are two claims, and only the second one is what the author sees:
// the failure this closes is a form that had the answer available and painted a constant anyway.

import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const dir = path.dirname(fileURLToPath(import.meta.url));
const handoff = path.resolve(dir, "../.shots-rendersettings");

const click = (selector) =>
  browser.execute((sel) => {
    const el = document.querySelector(sel);
    if (!el) return false;
    el.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    return true;
  }, selector);

const clickTabNamed = (containerSel, word) =>
  browser.execute(
    (sel, name) => {
      const root = document.querySelector(sel);
      if (!root) return false;
      const tab = [...root.querySelectorAll('[role="tab"]')].find((t) => t.textContent?.trim() === name);
      if (!tab) return false;
      tab.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      return true;
    },
    containerSel,
    word,
  );

const selectRow = (id) =>
  browser.execute((key) => {
    const row = document.querySelector(`[data-testid="hrow"][data-id="${key}"]`);
    if (!row) return false;
    row.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    return true;
  }, id);

const text = (selector) =>
  browser.execute((sel) => document.querySelector(sel)?.textContent ?? null, selector);

const note = (line) => console.log(line);

describe("ADR-190 · and it is still there after the editor is restarted", () => {
  let subject;
  let authored;

  before(async () => {
    ({ subject, authored } = JSON.parse(readFileSync(path.join(handoff, "authored.json"), "utf8")));
    await browser.waitUntil(
      async () => browser.execute(() => !!(window.__TAURI__ && window.__TAURI__.core)),
      { timeout: 30000 },
    );
    await browser.waitUntil(
      async () => /\d+\s+entities/.test(await browser.execute(() => document.querySelector("#count")?.textContent ?? "")),
      { timeout: 60000, timeoutMsg: "the relaunched editor never connected" },
    );
    await browser.execute(() => {
      document.querySelector('[data-testid="onboardSkip"]')?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
  });

  it("the replayed document still holds what the last session authored", async () => {
    const cut = await invoke("cinema_list", { id: subject });
    note(`[restart] the replayed cut: ${cut.shots} shots, delivering ${cut.render.format} · ${cut.render.fps} fps · ${cut.render.height} · name ${JSON.stringify(cut.render.name)} · into ${cut.render.folder}`);
    expect(cut.shots).toBe(2);
    expect(cut.render).toEqual(authored);
  });

  it("the Render dialog opens ON those answers, from a cold start", async () => {
    await click('[data-testid="engine-gameplay"]');
    await browser.pause(500);
    expect(await selectRow(subject)).toBe(true);

    if (!(await browser.execute(() => !!document.querySelector("#bottom-workspaces-animation-tab")))) {
      await click('[data-testid="bottom-dock-toggle"]');
      await browser.pause(400);
    }
    expect(await click("#bottom-workspaces-animation-tab")).toBe(true);
    await browser.pause(600);
    expect(await clickTabNamed("#bottom-workspaces-animation-panel", "Cutscene")).toBe(true);
    await (await $('[data-testid="cutscene-timeline"]')).waitForExist({ timeout: 15000 });

    expect(await click('[data-testid="cutscene-render"]')).toBe(true);
    await (await $('[data-testid="render-dialog"]')).waitForExist({ timeout: 10000 });

    const opened = await browser.execute(() => ({
      format: document.querySelector('[data-testid="render-format"]')?.value ?? null,
      fps: document.querySelector('[data-testid="render-fps"]')?.value ?? null,
      size: document.querySelector('[data-testid="render-size"]')?.value ?? null,
      stem: document.querySelector('[data-testid="render-stem"]')?.value ?? null,
      where: document.querySelector('[data-testid="render-folder"]')?.textContent ?? null,
    }));
    note(`[restart] the dialog opens on: ${opened.format} · ${opened.fps} fps · ${opened.size} · name ${JSON.stringify(opened.stem)} · into ${opened.where}`);

    // THE FOUR THE GATE NAMED. Before ADR-190 every one of these read back a constant — `movie`, `24`,
    // `1080`, and the object's own name — no matter what the last session had chosen.
    expect(opened.format).toBe(authored.format);
    expect(opened.fps).toBe(String(authored.fps));
    expect(opened.size).toBe(String(authored.height));
    expect(opened.stem).toBe(authored.name);
    // ...and the fifth, which was never a field at all: the destination was asked for by the operating
    // system AFTER the click, and the only surface that ever named it was the ledger at the end.
    expect(opened.where).toContain("delivery");

    // AND THE COST SENTENCE IS COMPUTED FROM THEM. 60 fps against 24 is not a cosmetic difference: a
    // dialog that painted the stored answers and then planned against the defaults would pass every
    // assertion above and render the wrong film.
    const plan = await invoke("cinema_render_plan", {
      id: subject,
      fps: authored.fps,
      shot: null,
      height: authored.height,
      format: authored.format,
    });
    expect(plan.reason).toBe(null);
    const cost = await text('[data-testid="render-cost"]');
    note(`[restart] the dialog's cost line: ${cost?.replace(/\s+/g, " ").trim()}`);
    expect(cost).toContain(`${plan.frames} frames`);
    expect(cost).toContain(`${authored.fps} fps`);
    expect(cost).toContain(`${plan.width}x${plan.height}`);
  });
});
