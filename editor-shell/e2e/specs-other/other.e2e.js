// `$other` on the PACKAGED .exe — a rule can finally talk about WHO touched it.
//
// BEFORE: every touch was anonymous. A rule knew *what* was touched (`$subject`) but had no way to
// name the toucher, so "only the player may take this" was not merely unwired — it was inexpressible.
// A companion wandering past collected your quest item exactly like you did.
// AFTER: the touch bridge names the toucher as `$other`, and the clause card "🎮 The PLAYER touched
// it" reads it. The companion walks straight through the coin and the panel explains why; the player
// walks into the same coin and takes it.
//
// The scene is built so the COMPANION reaches the coin first (it starts beside it and heels toward
// the player), which is precisely the situation that used to steal the pickup.

import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readdirSync, rmSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const dir = path.dirname(fileURLToPath(import.meta.url));
const shots = path.resolve(dir, "../.shots-other");
const capture = path.resolve(dir, "../scripts/capture-composited-window.ps1");
const captureFg = path.resolve(dir, "../../../.uxtest/audit/exe/capture-window-fg.ps1");
mkdirSync(shots, { recursive: true });

let shotIndex = 0;

async function shot(label) {
  await browser.pause(600);
  const out = path.join(shots, `${String(shotIndex).padStart(2, "0")}_${label}.png`);
  shotIndex += 1;
  const good = () => existsSync(out) && statSync(out).size > 20_000;
  const attempt = (script, args) => {
    try {
      execFileSync("powershell.exe", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", script, ...args], { stdio: "pipe" });
    } catch { /* fall through */ }
    if (!good() && existsSync(out)) rmSync(out);
    return good();
  };
  let ok = false;
  for (let round = 0; round < 4 && !ok; round += 1) {
    if (round > 0) await browser.pause(1200);
    ok =
      attempt(capture, ["-Out", out]) ||
      attempt(captureFg, ["-ProcName", "metrocalk-editor-shell", "-Out", out]);
  }
  if (!ok) {
    console.log(`[other] CAPTURE UNAVAILABLE for ${label} — the desktop refused both paths`);
    return null;
  }
  console.log(`[other] captured ${path.basename(out)} (${statSync(out).size} bytes)`);
  return out;
}

const click = (selector) =>
  browser.execute((sel) => {
    const el = document.querySelector(sel);
    if (!el) return false;
    el.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    return true;
  }, selector);

const setValue = (testid, value) =>
  browser.execute(
    (tid, v) => {
      const el = document.querySelector(`[data-testid="${tid}"]`);
      if (!el) return false;
      const proto = el.tagName === "SELECT" ? window.HTMLSelectElement.prototype : window.HTMLInputElement.prototype;
      Object.getOwnPropertyDescriptor(proto, "value").set.call(el, String(v));
      el.dispatchEvent(new Event("change", { bubbles: true }));
      el.dispatchEvent(new Event("input", { bubbles: true }));
      el.dispatchEvent(new FocusEvent("focusout", { bubbles: true }));
      return true;
    },
    testid,
    value,
  );

const selectRow = (id) =>
  browser.execute((key) => {
    const row = document.querySelector(`[data-testid="hrow"][data-id="${key}"]`);
    if (!row) return false;
    row.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    return true;
  }, id);

async function pressPlay() {
  await click('[data-testid="play"]');
  await browser.waitUntil(
    async () => browser.execute(() => !!document.querySelector('[data-testid="stop"]')),
    { timeout: 10000, timeoutMsg: "Play never engaged" },
  );
}
async function pressStop() {
  await click('[data-testid="stop"]');
  await browser.pause(800);
}
const key = (type, k) =>
  browser.execute((t, kk) => { window.dispatchEvent(new KeyboardEvent(t, { key: kk, bubbles: true })); }, type, k);

async function walk(direction, ms) {
  await key("keydown", direction);
  await browser.pause(ms);
  await key("keyup", direction);
  await browser.pause(400);
}

async function newScene() {
  await pressStop();
  await invoke("new_project");
  await browser.pause(700);
  await browser.execute(() => {
    document.querySelector('[data-testid="onboardSkip"]')?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });
}

describe("$other — a rule that knows who touched it", () => {
  let hero;
  let dog;
  let coin;

  before(async () => {
    await browser.waitUntil(
      async () => browser.execute(() => !!(window.__TAURI__ && window.__TAURI__.core)),
      { timeout: 30000 },
    );
  });

  it("BEFORE: with no clause, whoever reaches the coin first takes it — even the dog", async () => {
    await newScene();
    hero = (await invoke("shape_spawn", { kind: "capsule", pos: [0, 0, 0] })).created;
    await invoke("role_assign", { id: hero, role: "player" });
    // The dog starts ON the far side of the coin, so heeling to the hero drags it straight through.
    coin = (await invoke("shape_spawn", { kind: "torus", pos: [5, 0, 0] })).created;
    await invoke("role_assign", { id: coin, role: "collectible" });
    dog = (await invoke("shape_spawn", { kind: "box", pos: [9, 0, 0] })).created;
    await invoke("role_assign", { id: dog, role: "companion" });
    await invoke("frame_all");
    await shot("scene");

    await pressPlay();
    let status = null;
    await browser.waitUntil(
      async () => {
        status = await invoke("role_status");
        return status.score >= 1;
      },
      { timeout: 20000, interval: 300, timeoutMsg: "nobody collected the coin" },
    );
    // The hero never moved — so the dog took it. That is the old behaviour, captured.
    console.log(`[other] BEFORE: score ${status.score} with the hero standing still — the DOG took it`);
    expect(status.score).toBe(1);
    await shot("before_dog_stole_the_coin");
    await pressStop();
  });

  it("AFTER: add '🎮 The PLAYER touched it' — one clause, read back as a sentence", async () => {
    await newScene();
    hero = (await invoke("shape_spawn", { kind: "capsule", pos: [0, 0, 0] })).created;
    await invoke("role_assign", { id: hero, role: "player" });
    coin = (await invoke("shape_spawn", { kind: "torus", pos: [5, 0, 0] })).created;
    await invoke("role_assign", { id: coin, role: "collectible" });
    dog = (await invoke("shape_spawn", { kind: "box", pos: [9, 0, 0] })).created;
    await invoke("role_assign", { id: dog, role: "companion" });
    await invoke("frame_all");

    await click('[data-testid="engine-scene"]');
    await browser.pause(300);
    expect(await selectRow(coin)).toBe(true);
    await (await $('[data-testid="onlyif-block"]')).waitForExist({ timeout: 10000 });
    expect(await setValue("onlyif-pick", "touched_by_player")).toBe(true);
    await browser.pause(300);
    expect(await click('[data-testid="onlyif-add"]')).toBe(true);
    await browser.waitUntil(
      async () => (await invoke("condition_list", { id: coin })).all.length === 1,
      { timeout: 10000, timeoutMsg: "the clause never landed" },
    );
    const list = await invoke("condition_list", { id: coin });
    console.log(`[other] SENTENCE: "${list.sentence}"`);
    expect(list.all[0].reads).toBe("the player touched it");
    expect(list.sentence).toContain("the player touched it");
    await shot("after_clause_added");
  });

  it("the dog now walks straight through it — and the panel says why", async () => {
    await pressPlay();
    let status = null;
    await browser.waitUntil(
      async () => {
        status = await invoke("role_status");
        return !!status.blocked;
      },
      { timeout: 20000, interval: 300, timeoutMsg: "the dog never even tried" },
    );
    console.log(`[other] BLOCKED: "${status.blocked.why}"`);
    expect(status.score).toBe(0);
    expect(status.blocked.why).toContain("player touched it");

    await click('[data-testid="engine-gameplay"]');
    await browser.pause(700);
    const panel = await browser.execute(
      () => document.querySelector('[data-testid="roles-blocked"]')?.textContent ?? "",
    );
    console.log(`[other] gameplay panel: ${panel}`);
    expect(panel).toContain("player touched it");
    await shot("after_dog_refused_with_reason");
  });

  it("YOU walk into the same coin and take it", async () => {
    let status = null;
    for (let attempt = 0; attempt < 8; attempt += 1) {
      await walk("ArrowRight", 1600);
      status = await invoke("role_status");
      if (status.score >= 1) break;
    }
    console.log(`[other] AFTER: the player took it — score ${status.score}`);
    expect(status.score).toBe(1);
    await shot("after_player_collected");
    await pressStop();
    await browser.pause(600);
    expect((await invoke("role_status")).score).toBe(0);
  });

  it("any captures taken are real pixels", async () => {
    const files = readdirSync(shots);
    if (files.length === 0) {
      console.log("[other] no captures this run — the desktop refused OS capture");
      return;
    }
    for (const f of files) {
      expect(statSync(path.join(shots, f)).size).toBeGreaterThan(20_000);
    }
    console.log(`[other] evidence: ${files.join(", ")}`);
  });
});
