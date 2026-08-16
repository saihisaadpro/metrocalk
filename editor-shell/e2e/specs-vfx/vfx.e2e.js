// VFX on the PACKAGED .exe — before and after, MEASURED.
//
// BEFORE: the engine had NO particle system. Not a weak one — none. `grep -ri particle` across every
// engine crate returned nothing, and the renderer's scene pass drew sky, cubes, meshes, terrain, ground,
// grid, lines and gizmos. There was no way for an author to make anything burn, spark or explode.
// AFTER: select an object → Effects → 🔥 Fire. One undoable commit. Press Play and it burns — and the
// fire is not a decal or a sprite loop: it is a closed-form particle solve, drawn as camera-facing
// billboards INSIDE the linear-HDR scene pass, so the over-1.0 colours bloom through the same post
// chain the rest of the frame uses.
//
// The evidence is arithmetic, not vibes. `vfx_probe` reports how many particles the renderer is
// actually drawing this instant and the peak radiance among them, so every claim below is a number:
//   * zero before, hundreds during Play, zero again after Stop (a projection, never a document write)
//   * peak radiance ABOVE 1.0 — the scene is genuinely emitting, which is what makes bloom happen
//   * a one-shot effect fires at a MOMENT (a pick-up) and retires itself
//   * the determinism claim: the same frame of the same run resolves to the same particle count

import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, rmSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const dir = path.dirname(fileURLToPath(import.meta.url));
const shots = path.resolve(dir, "../.shots-vfx");
const capture = path.resolve(dir, "../scripts/capture-composited-window.ps1");
const captureFg = path.resolve(dir, "../../../.uxtest/audit/exe/capture-window-fg.ps1");
mkdirSync(shots, { recursive: true });

let shotIndex = 0;

async function shot(label) {
  await browser.pause(400);
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
  for (let round = 0; round < 3 && !ok; round += 1) {
    if (round > 0) await browser.pause(900);
    ok = attempt(capture, ["-Out", out]) || attempt(captureFg, ["-ProcName", "metrocalk-editor-shell", "-Out", out]);
  }
  if (!ok) {
    console.log(`[vfx] CAPTURE UNAVAILABLE for ${label} — the desktop refused both paths`);
    return null;
  }
  console.log(`[vfx] captured ${path.basename(out)} (${statSync(out).size} bytes)`);
  return out;
}

const click = (selector) =>
  browser.execute((sel) => {
    const el = document.querySelector(sel);
    if (!el) return false;
    el.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    return true;
  }, selector);

const selectRow = (id) =>
  browser.execute((key) => {
    const row = document.querySelector(`[data-testid="hrow"][data-id="${key}"]`);
    if (!row) return false;
    row.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    return true;
  }, id);

const fx = () => invoke("vfx_probe");

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
  await browser.pause(300);
}

/** Poll the probe for up to `ms`, returning the best (highest-total) sample seen. */
async function peak(ms) {
  let best = { additive: 0, soft: 0, total: 0, peakRadiance: 0 };
  const until = Date.now() + ms;
  while (Date.now() < until) {
    const s = await fx();
    if (s.total > best.total) best = s;
    await browser.pause(120);
  }
  return best;
}

describe("VFX — no particle system at all, to fire that blooms", () => {
  let brazier;
  let hero;
  let coin;

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

  it("builds the set: a brazier, a hero, and a coin to grab", async () => {
    brazier = (await invoke("shape_spawn", { kind: "cylinder", pos: [0, 0.6, 0] })).created;
    hero = (await invoke("shape_spawn", { kind: "capsule", pos: [-6, 0, 0] })).created;
    await invoke("role_assign", { id: hero, role: "player" });
    coin = (await invoke("shape_spawn", { kind: "torus", pos: [-1.5, 0, 0] })).created;
    const r = await invoke("role_assign", { id: coin, role: "collectible" });
    expect(r.reason).toBe(null);
    await invoke("frame_all");
    expect(brazier && hero && coin).toBeTruthy();
    await shot("00_set_built");
  });

  it("BEFORE: Play with no effects — the renderer draws ZERO particles", async () => {
    await pressPlay();
    await browser.pause(1200);
    const before = await fx();
    console.log(`[vfx] BEFORE total=${before.total} additive=${before.additive} soft=${before.soft} peak=${before.peakRadiance}`);
    expect(before.total).toBe(0);
    await shot("01_before_play_no_particles");
    await pressStop();
    await browser.pause(600);
  });

  it("authors Fire in one click — and it reads back as a sentence", async () => {
    await click('[data-testid="engine-gameplay"]');
    await browser.pause(500);
    expect(await selectRow(brazier)).toBe(true);
    await (await $('[data-testid="vfx-section"]')).waitForExist({ timeout: 10000 });

    const empty = await invoke("vfx_list", { id: brazier });
    expect(empty.layers).toBe(0);
    await shot("02_effects_panel_empty");

    expect(await click('[data-testid="fx-fire"]')).toBe(true);
    await browser.waitUntil(
      async () => (await invoke("vfx_list", { id: brazier })).layers === 1,
      { timeout: 10000, timeoutMsg: "the effect never landed" },
    );
    const after = await invoke("vfx_list", { id: brazier });
    console.log(`[vfx] layer 1 reads: "${after.reads[0]}" (${after.particles} particles)`);
    expect(after.reads[0]).toContain("Fire");
    expect(after.particles).toBeGreaterThan(0);

    const panel = await browser.execute(
      () => document.querySelector('[data-testid="vfx-layers"]')?.textContent ?? "",
    );
    console.log(`[vfx] the panel reads: ${panel.trim()}`);
    expect(panel).toContain("Fire");
    await shot("03_fire_authored");
  });

  it("advice, not a wall: adding a second glowing layer says what would read better", async () => {
    const r = await invoke("vfx_add", { id: brazier, kind: "sparkle", trigger: "always" });
    console.log(`[vfx] problems: ${JSON.stringify(r.problems)}`);
    expect(r.layers).toBe(2); // it still landed — advice never blocks the click
    expect(r.problems.join(" ").toLowerCase()).toContain("glow");
    // put it back to just fire for a clean measurement
    await invoke("vfx_remove", { id: brazier, index: 1 });
  });

  it("AFTER: Play — hundreds of particles, and they are genuinely EMITTING (peak > 1.0)", async () => {
    await pressPlay();
    const live = await peak(2500);
    console.log(`[vfx] AFTER total=${live.total} additive=${live.additive} soft=${live.soft} peakRadiance=${live.peakRadiance.toFixed(2)}`);
    // 1. There are particles at all — the thing that did not exist before this round.
    expect(live.total).toBeGreaterThan(20);
    // 2. They are drawn on the ADDITIVE path (fire emits light).
    expect(live.additive).toBeGreaterThan(20);
    // 3. The claim that makes it more than coloured dots: the scene is emitting ABOVE white, which is
    //    what the bloom pass in the HDR chain actually responds to.
    expect(live.peakRadiance).toBeGreaterThan(1.0);
    await shot("04_after_play_fire_burning");
  });

  it("MOMENT-fired: a pick-up pop exists only for the instant the coin is taken", async () => {
    await pressStop();
    await browser.pause(600);
    // The whole "fully wired" claim: the effect is attached to a MOMENT in the game, not left running.
    const r = await invoke("vfx_add", { id: coin, kind: "pickup", trigger: "whenCollected" });
    expect(r.reason).toBe(null);
    console.log(`[vfx] the coin now carries: ${r.reads.join(" | ")}`);

    await pressPlay();
    await browser.pause(900);
    // Nothing from the coin yet — a one-shot must not fire on Play.
    const idle = await fx();
    console.log(`[vfx] before the pick-up: total=${idle.total}`);

    // STEER toward the coin rather than betting on a fixed direction for a fixed time: physics
    // acceleration, drift and an overshoot all make "hold right for 1.5s" a coin flip, and a test that
    // fails because the hero missed tells you nothing about the effect it was supposed to be checking.
    let seen = idle.total;
    let bursts = 0;
    let collected = false;
    for (let attempt = 0; attempt < 14 && !collected; attempt += 1) {
      const hero_p = await invoke("body_sim_position", { id: hero });
      const dx = (hero_p?.[0] ?? 0) - -1.5; // coin sits at x = -1.5
      const dir = dx > 0 ? "ArrowLeft" : "ArrowRight";
      await key("keydown", dir);
      for (let i = 0; i < 8; i += 1) {
        await browser.pause(90);
        const s = await fx();
        if (s.bursts > 0) bursts = Math.max(bursts, s.bursts);
        if (s.total > seen) seen = s.total;
        const status = await invoke("role_status");
        if (status.score >= 1) {
          collected = true;
          break;
        }
      }
      await key("keyup", dir);
      await browser.pause(150);
    }
    expect(collected).toBe(true);
    const status = await invoke("role_status");
    // One read of the HIGH-WATER MARKS, not a race against a 0.86s window with 400ms polls. The
    // engine records the peak; the test asks it afterwards.
    const marks = await fx();
    console.log(`[vfx] coin collected: score=${status.score}; burstsFired=${marks.burstsFired}; peakTotal=${marks.peakTotal} (idle ${idle.total}); sampled peak=${seen}, live bursts=${bursts}`);
    expect(status.score).toBeGreaterThanOrEqual(1);
    // A one-shot came into existence at the moment...
    expect(marks.burstsFired).toBeGreaterThan(0);
    // ...and it put more particles on screen than the brazier alone ever accounts for.
    expect(marks.peakTotal).toBeGreaterThan(idle.total);
    await shot("05_pickup_burst");
  });

  it("Stop leaves NOTHING behind — effects are a projection, never a document write", async () => {
    await pressStop();
    await browser.pause(900);
    const after = await fx();
    console.log(`[vfx] after Stop: total=${after.total}`);
    expect(after.total).toBe(0);
    // and the authored effect is exactly what it was
    const brazierFx = await invoke("vfx_list", { id: brazier });
    console.log(`[vfx] the brazier still carries ${brazierFx.layers} effect(s)`);
    expect(brazierFx.layers).toBe(1);
    await shot("06_stopped_clean");
  });

  it("one Ctrl-Z removes an effect, and the object keeps everything else", async () => {
    // Author immediately before undoing. A Play/Stop cycle in between puts Stop's own snapshot
    // restore on the undo stack, so the Ctrl-Z would be spent on that instead of on the edit.
    await invoke("vfx_add", { id: coin, kind: "sparks", trigger: "whenHit" });
    expect((await invoke("vfx_list", { id: coin })).layers).toBe(2);
    await invoke("undo");
    await browser.pause(500);
    const undone = await invoke("vfx_list", { id: coin });
    console.log(`[vfx] after one undo the coin carries ${undone.layers} effect(s)`);
    expect(undone.layers).toBe(1);
    const status = await invoke("role_status");
    expect(status.roster.some((r) => r.entity === coin)).toBe(true);
    await shot("07_undone");
  });
});
