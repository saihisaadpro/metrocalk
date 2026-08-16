// The Behaviour block on the PACKAGED .exe — click an asset, its behaviour is RIGHT THERE.
//
// BEFORE: assigning a role meant selecting the object, finding the Engines rail, switching to
// the Gameplay tab, and scrolling to the Roles section — four hops from the click.
// AFTER: click the asset; the Inspector's ✨ Behaviour block is already showing the full role
// catalog with one-tap cards, the held role, a clear button, and jump links into Animate and
// Gameplay. Assignment happens where the selection happens.

import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readdirSync, rmSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const dir = path.dirname(fileURLToPath(import.meta.url));
const shots = path.resolve(dir, "../.shots-behaviour");
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
    // The desktop can refuse OS captures entirely (locked, display off). The functional
    // assertions are the truth of this suite — log the gap loudly and keep going.
    console.log(`[bhv] CAPTURE UNAVAILABLE for ${label} — desktop refused both paths`);
    return null;
  }
  console.log(`[bhv] captured ${path.basename(out)} (${statSync(out).size} bytes)`);
  return out;
}

const click = (selector) =>
  browser.execute((sel) => {
    const el = document.querySelector(sel);
    if (!el) return false;
    el.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    return true;
  }, selector);

/** Commit a value through a NumericField with dispatched events (locked-desktop safe). */
const setNumeric = (testid, value) =>
  browser.execute(
    (tid, v) => {
      const input = document.querySelector(`[data-testid="${tid}"]`);
      if (!input) return false;
      const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value").set;
      setter.call(input, String(v));
      input.dispatchEvent(new Event("input", { bubbles: true }));
      input.dispatchEvent(new FocusEvent("focusout", { bubbles: true }));
      return true;
    },
    testid,
    value,
  );

const pos = (id) => invoke("body_sim_position", { id });
const distXZ = (a, b) => Math.hypot(a[0] - b[0], a[2] - b[2]);

async function pressPlay() {
  await click('[data-testid="play"]');
  await browser.waitUntil(
    async () => browser.execute(() => !!document.querySelector('[data-testid="stop"]')),
    { timeout: 10000, timeoutMsg: "Play never engaged" },
  );
}

describe("Behaviour in the Inspector — assignment lives where the click lands", () => {
  before(async () => {
    await browser.waitUntil(
      async () => browser.execute(() => !!(window.__TAURI__ && window.__TAURI__.core)),
      { timeout: 30000 },
    );
    await invoke("new_project");
    await browser.pause(700);
  });

  it("select an asset — the Behaviour block appears in the Inspector with the full catalog", async () => {
    const crystal = (await invoke("shape_spawn", { kind: "torus", pos: [0, 0, 0] })).created;
    await browser.execute(() => {
      document.querySelector('[data-testid="onboardSkip"]')?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await invoke("frame_all");
    // Select through the hierarchy row, like a click in the outliner.
    const ok = await browser.execute((id) => {
      const row = document.querySelector(`[data-testid="hrow"][data-id="${id}"]`);
      if (!row) return false;
      row.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      return true;
    }, crystal);
    expect(ok).toBe(true);

    const section = await $('[data-testid="behaviour-section"]');
    await section.waitForExist({ timeout: 10000 });
    const state = await $('[data-testid="behaviour-state"]');
    expect(await state.getText()).toContain("doesn't do anything yet");
    for (const kind of ["collectible", "companion", "enemy", "player"]) {
      await (await $(`[data-testid="behaviour-${kind}"]`)).waitForExist({ timeout: 5000 });
    }
    await shot("behaviour_block_on_selection");
  });

  it("one tap in the Inspector assigns the role — no tab hunting", async () => {
    expect(await click('[data-testid="behaviour-collectible"]')).toBe(true);
    await browser.waitUntil(
      async () => (await (await $('[data-testid="behaviour-state"]')).getText()).includes("Collectible"),
      { timeout: 10000, timeoutMsg: "the held-role line never updated" },
    );
    // The engine really has it (not just UI state).
    const status = await invoke("role_status");
    expect(status.roster.some((r) => r.role === "collectible")).toBe(true);
    await shot("assigned_from_inspector");
  });

  it("the jump links land in the deeper workspaces", async () => {
    expect(await click('[data-testid="behaviour-jump-gameplay"]')).toBe(true);
    await (await $('[data-testid="roles-section"]')).waitForExist({ timeout: 10000 });
    console.log("[bhv] Gameplay jump landed on the full Roles panel");
    await shot("jump_landed_in_gameplay");
    expect(await click('[data-testid="behaviour-jump-animate"]')).toBe(true);
    await browser.pause(600);
    await shot("jump_landed_in_animate");
  });

  it("the data is REAL: raise Follow distance in the panel and the dog heels farther", async () => {
    await invoke("new_project");
    await browser.pause(700);
    await browser.execute(() => {
      document.querySelector('[data-testid="onboardSkip"]')?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    const hero = (await invoke("shape_spawn", { kind: "capsule", pos: [0, 0, 0] })).created;
    await invoke("role_assign", { id: hero, role: "player" });
    const dog = (await invoke("shape_spawn", { kind: "box", pos: [-4, 0, 0] })).created;
    await invoke("role_assign", { id: dog, role: "companion" });
    await invoke("frame_all");

    // Select the dog in the outliner — the Behaviour block shows its tuning knobs.
    await click('[data-testid="engine-scene"]');
    await browser.pause(300);
    await browser.execute((id) => {
      document.querySelector(`[data-testid="hrow"][data-id="${id}"]`)?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    }, dog);
    await (await $('[data-testid="behaviour-tuning"]')).waitForExist({ timeout: 10000 });
    await shot("tuning_knobs_on_companion");

    // Raise Follow distance 2.0 → 4.5 through the friendly field (one undoable edit).
    expect(await setNumeric("tune-follow", 4.5)).toBe(true);
    await browser.pause(600);

    await pressPlay();
    let d = null;
    await browser.waitUntil(
      async () => {
        d = distXZ(await pos(dog), await pos(hero));
        return d < 5.6;
      },
      { timeout: 15000, interval: 400, timeoutMsg: `dog never approached (last ${d})` },
    );
    await browser.pause(2500); // settle at the stand-off
    d = distXZ(await pos(dog), await pos(hero));
    console.log(`[bhv] with Follow distance 4.5 the dog settles at ${d.toFixed(2)} m (default 2.0 settled ~2.7)`);
    expect(d).toBeGreaterThan(3.4);
    expect(d).toBeLessThan(5.8);
    await shot("dog_heels_at_tuned_distance");
    await click('[data-testid="stop"]');
    await browser.pause(700);
  });

  it("any captures taken are real pixels", async () => {
    const files = readdirSync(shots);
    if (files.length === 0) {
      console.log("[bhv] no captures this run — the desktop refused OS capture (functional assertions carried the suite)");
      return;
    }
    for (const f of files) {
      expect(statSync(path.join(shots, f)).size).toBeGreaterThan(20_000);
    }
    console.log(`[bhv] evidence: ${files.join(", ")}`);
  });
});
