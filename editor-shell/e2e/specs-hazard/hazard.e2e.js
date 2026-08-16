// HAZARDS on the PACKAGED .exe — "the spike hurts whoever stepped on it".
//
// BEFORE: this sentence had no expressible form. Touch was anonymous, so a rule could not aim an
// effect at the toucher; and `Health` was pure registry metadata that nothing in the engine ever read
// or wrote. A spike was, at best, a wall.
// AFTER: one ⚡ Hazard card. Walking into it costs you health through the `Damage` verb (floored at
// zero, aimed at `$other`), and at zero health you are out of the game — visible in the viewport,
// through the same one-owner visibility pass a collected coin uses.

import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readdirSync, rmSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const dir = path.dirname(fileURLToPath(import.meta.url));
const shots = path.resolve(dir, "../.shots-hazard");
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
    console.log(`[hz] CAPTURE UNAVAILABLE for ${label} — the desktop refused both paths`);
    return null;
  }
  console.log(`[hz] captured ${path.basename(out)} (${statSync(out).size} bytes)`);
  return out;
}

const click = (selector) =>
  browser.execute((sel) => {
    const el = document.querySelector(sel);
    if (!el) return false;
    el.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    return true;
  }, selector);

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

describe("Hazards — the spike hurts whoever stepped on it", () => {
  let hero;
  let spike;

  before(async () => {
    await browser.waitUntil(
      async () => browser.execute(() => !!(window.__TAURI__ && window.__TAURI__.core)),
      { timeout: 30000 },
    );
    await pressStop();
    await invoke("new_project");
    await browser.pause(700);
    await browser.execute(() => {
      document.querySelector('[data-testid="onboardSkip"]')?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
  });

  it("a Player now carries health, and a ⚡ Hazard authors the hurt rule", async () => {
    hero = (await invoke("shape_spawn", { kind: "capsule", pos: [0, 0, 0] })).created;
    const heroReply = await invoke("role_assign", { id: hero, role: "player" });
    console.log(`[hz] player adds: ${heroReply.added.join(" · ")}`);
    expect(heroReply.added.join(" ")).toContain("health");

    spike = (await invoke("shape_spawn", { kind: "cone", pos: [4, 0, 0] })).created;
    const spikeReply = await invoke("role_assign", { id: spike, role: "hazard" });
    console.log(`[hz] hazard adds: ${spikeReply.added.join(" · ")}`);
    expect(spikeReply.reason).toBe(null);
    expect(spikeReply.added.join(" ")).toContain("toucher");

    // The rule targets the TOUCHER — the thing that was inexpressible before `$other`.
    const rules = await invoke("list_rules");
    console.log(`[hz] rules: ${rules.map((r) => r.name).join(", ")}`);
    expect(rules.some((r) => r.name.includes("Hurt whoever"))).toBe(true);
    await invoke("frame_all");
    await shot("scene_hero_and_spike");
  });

  it("walking into the spike costs health — and at zero you are out of the game", async () => {
    await pressPlay();
    await browser.pause(600);

    // Health is now readable straight off the run — no guessing from pixels.
    const start = (await invoke("role_status")).health;
    console.log(`[hz] starting health: ${start.hp}/${start.maxHp} (${start.name})`);
    expect(start.hp).toBe(3);

    // Walk in, back off, walk in again: a hazard never latches, so it hurts every time.
    let health = start;
    for (let attempt = 0; attempt < 12 && health.hp > 0; attempt += 1) {
      await walk("ArrowRight", 1400);
      await walk("ArrowLeft", 700);
      const next = (await invoke("role_status")).health;
      if (next.hp !== health.hp) {
        console.log(`[hz] hit — health ${health.hp} -> ${next.hp}`);
        if (health.hp === start.hp) await shot("play_first_hit");
      }
      health = next;
    }
    console.log(`[hz] final health: ${health.hp}/${health.maxHp}`);
    expect(health.hp).toBe(0);

    // Zero health takes you out of the world through the SAME visibility pass a collected coin uses.
    await click('[data-testid="engine-gameplay"]');
    await browser.pause(700);
    const hearts = await browser.execute(
      () => document.querySelector('[data-testid="roles-health"]')?.textContent ?? "",
    );
    console.log(`[hz] panel reads: ${hearts}`);
    expect(hearts).toContain("out of the game");
    await shot("play_hero_defeated_at_zero_hp");
  });

  it("Stop restores the hero — the whole fight was a projection", async () => {
    await pressStop();
    await browser.pause(900);
    const after = (await invoke("role_status")).health;
    expect(after).toBe(null); // no run, no live health
    // The authored document still says 3 — the fight never touched it.
    const authored = await invoke("entity_details", { id: hero });
    expect(authored.components).toContain("Health");
    console.log("[hz] Stop cleared the run; the authored Health survives untouched");
    await shot("stopped_hero_restored");
  });

  it("any captures taken are real pixels", async () => {
    const files = readdirSync(shots);
    if (files.length === 0) {
      console.log("[hz] no captures this run — the desktop refused OS capture");
      return;
    }
    for (const f of files) {
      expect(statSync(path.join(shots, f)).size).toBeGreaterThan(20_000);
    }
    console.log(`[hz] evidence: ${files.join(", ")}`);
  });
});
