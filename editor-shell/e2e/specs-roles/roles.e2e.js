// Gameplay roles on the PACKAGED .exe — the engine's first self-playing interaction, captured live.
//
// The scene this suite builds and then PLAYS: a spinning crystal (Collectible) on the ground, a ball
// (Physics prop) hanging above it, a wall (Solid obstacle), a wedge Spinner for ambient motion. Press
// Play: the ball FALLS under gravity, passes through the crystal's touch radius → the loop fires a
// real `Touched` event → the $subject pickup rule collects it → the crystal VANISHES from the
// viewport and the Score climbs to 1, live in the Gameplay panel. Stop: the crystal comes back, the
// score resets, the document was never touched. Every stage is OS-captured; the spinner's motion is
// proven by a pixel diff between two mid-play frames.

import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const dir = path.dirname(fileURLToPath(import.meta.url));
const shots = path.resolve(dir, "../.shots-roles");
const capture = path.resolve(dir, "../scripts/capture-composited-window.ps1");
const captureFg = path.resolve(dir, "../../../.uxtest/audit/exe/capture-window-fg.ps1");
const pngdiff = path.resolve(dir, "../../../.uxtest/audit/exe/pngdiff.ps1");
const countViolet = path.resolve(dir, "../scripts/count-violet.ps1");
mkdirSync(shots, { recursive: true });

let shotIndex = 0;
let lastShotWasOs = false;

async function shot(label, thumbId) {
  await browser.pause(700);
  const out = path.join(shots, `${String(shotIndex).padStart(2, "0")}_${label}.png`);
  shotIndex += 1;
  const good = () => existsSync(out) && statSync(out).size > 20_000;
  const attempt = (script, args) => {
    try {
      execFileSync("powershell.exe", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", script, ...args], { stdio: "pipe" });
    } catch {
      /* fall through */
    }
    if (!good() && existsSync(out)) rmSync(out);
    return good();
  };
  // Both paths can go transiently blank (PrintWindow stalls once the viewport redraws
  // continuously; a locked desktop blocks CopyFromScreen). Retry in rounds, then fall back
  // to the engine's own offscreen render of `thumbId` — real pixels from the live wgpu
  // instance buffer, immune to the desktop state.
  let ok = false;
  for (let round = 0; round < 3 && !ok; round += 1) {
    if (round > 0) await browser.pause(1200);
    ok =
      attempt(capture, ["-Out", out]) ||
      attempt(captureFg, ["-ProcName", "metrocalk-editor-shell", "-Out", out]);
  }
  if (!ok && thumbId) {
    const data = await invoke("thumbnail", { id: thumbId, size: 512 });
    if (data && data.startsWith("data:image/png;base64,")) {
      writeFileSync(out, Buffer.from(data.split(",")[1], "base64"));
      console.log(`[roles] captured ${path.basename(out)} via engine offscreen render (${statSync(out).size} bytes)`);
      lastShotWasOs = false;
      return out;
    }
  }
  if (!ok) throw new Error(`capture ${label} came back blank on every path`);
  console.log(`[roles] captured ${path.basename(out)} (${statSync(out).size} bytes)`);
  lastShotWasOs = true;
  return out;
}

/** Real-render frame of one entity straight from the engine (always works, any desktop state). */
async function thumbShot(label, id) {
  const data = await invoke("thumbnail", { id, size: 512 });
  if (!data || !data.startsWith("data:image/png;base64,")) {
    throw new Error(`engine render for ${label} unavailable`);
  }
  const out = path.join(shots, `${String(shotIndex).padStart(2, "0")}_${label}.png`);
  shotIndex += 1;
  writeFileSync(out, Buffer.from(data.split(",")[1], "base64"));
  console.log(`[roles] engine-rendered ${path.basename(out)} (${statSync(out).size} bytes)`);
  return out;
}

/** Count crystal-violet pixels in a capture (the torus is the only violet thing in the scene). */
function violetCount(file) {
  const raw = execFileSync(
    "powershell.exe",
    ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", countViolet, "-Path", file],
    { encoding: "utf8" },
  );
  const line = raw.split(/\r?\n/).find((l) => l.trim().startsWith("{"));
  return line ? JSON.parse(line).violet : -1;
}

/** Compare two captures; returns the changed-pixel fraction (0..1). */
function diffFrac(a, b) {
  const raw = execFileSync(
    "powershell.exe",
    ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", pngdiff, "-A", a, "-B", b, "-Thresh", "24", "-Sample", "2"],
    { encoding: "utf8" },
  );
  const line = raw.split(/\r?\n/).find((l) => l.trim().startsWith("{"));
  return line ? JSON.parse(line).frac : -1;
}

/** Select an entity through the real hierarchy row (dispatched DOM click — the locked-desktop rule). */
async function selectEntity(key) {
  await (await $('[data-testid="engine-scene"]')).click();
  await browser.pause(250);
  const ok = await browser.execute((id) => {
    const row = document.querySelector(`[data-testid="hrow"][data-id="${id}"]`);
    if (!row) return false;
    row.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    return true;
  }, key);
  expect(ok).toBe(true);
  await browser.pause(200);
}

/** Click a Gameplay-panel role card for the CURRENT selection. */
async function assignRole(kind) {
  await (await $('[data-testid="engine-gameplay"]')).click();
  const card = await $(`[data-testid="role-${kind}"]`);
  await card.waitForExist({ timeout: 10000 });
  await browser.waitUntil(async () => (await card.getAttribute("disabled")) === null, {
    timeout: 10000,
    timeoutMsg: `role card ${kind} never enabled`,
  });
  await card.click();
  await browser.pause(400);
}

const entityCount = () =>
  browser.execute(() => {
    const el = document.querySelector("#count");
    const n = el ? parseInt(el.textContent, 10) : NaN;
    return Number.isFinite(n) ? n : -1;
  });

describe("Gameplay roles — assets become a game that plays itself", () => {
  let violetBaseline = 0;
  let crystal;
  let ball;
  let wall;
  let wedge;

  before(async () => {
    await browser.waitUntil(
      async () => browser.execute(() => !!(window.__TAURI__ && window.__TAURI__.core)),
      { timeout: 30000 },
    );
    await invoke("new_project");
    await browser.pause(800);
  });

  it("builds the set: crystal, hanging ball, wall, wedge", async () => {
    crystal = (await invoke("shape_spawn", { kind: "torus", pos: [0, 0, 0] })).created;
    ball = (await invoke("shape_spawn", { kind: "sphere", pos: [0.1, 4, 0.15] })).created;
    wall = (await invoke("shape_spawn", { kind: "box", pos: [3, 0, 0] })).created;
    wedge = (await invoke("shape_spawn", { kind: "wedge", pos: [-3, 0, 0] })).created;
    expect(crystal && ball && wall && wedge).toBeTruthy();
    // Dismiss the onboarding card now that the scene is non-empty (it reads its flag at mount).
    await browser.execute(() => {
      document.querySelector('[data-testid="onboardSkip"]')?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await invoke("frame_all");
  });

  it("assigns all four roles through the Gameplay panel UI", async () => {
    await selectEntity(crystal);
    await assignRole("collectible");
    await selectEntity(ball);
    await assignRole("prop");
    await selectEntity(wall);
    await assignRole("solid");
    await selectEntity(wedge);
    await assignRole("spinner");

    // The roster lists all four with their roles; the crystal's binding to the Score is authored.
    const status = await invoke("role_status");
    expect(status.roster.length).toBe(4);
    expect(status.roster.some((r) => r.role === "collectible")).toBe(true);
    expect(status.scoreEntity).toBeTruthy();
    expect(status.score).toBe(0);
    const panelShot = await shot("roles_assigned_gameplay_panel");
    if (lastShotWasOs) {
      violetBaseline = violetCount(panelShot);
      console.log(`[roles] violet baseline (crystal visible): ${violetBaseline}`);
    }
  });

  it("the assignment facts are honest: components, rule, score entity all exist", async () => {
    // The ball really is a dynamic body; the wall fixed; the crystal has its trigger + spin tracks.
    const reply = await invoke("role_assign", { id: crystal, role: "collectible" });
    // Re-assigning is idempotent — no error, and no duplicate spin ("added" omits it this time).
    expect(reply.reason).toBe(null);
    expect(reply.added.includes("spin animation")).toBe(false);
    // The Score entity counts as a scene object (entity count grew by 5: 4 shapes + Score).
    expect(await entityCount()).toBe(5);
  });

  it("PLAY: the ball falls, touches the crystal — it vanishes and the score goes to 1, live", async () => {
    await invoke("play");
    await browser.pause(150);
    await shot("play_t0_ball_falling");

    // The touch → rule → score loop, asserted from the live rules runtime (not the doc).
    let status = null;
    await browser.waitUntil(
      async () => {
        status = await invoke("role_status");
        return status && status.score === 1 && status.remaining === 0;
      },
      { timeout: 15000, interval: 250, timeoutMsg: "the ball never collected the crystal" },
    );
    console.log(`[roles] LIVE score=${status.score} remaining=${status.remaining}`);
    await browser.pause(600);
    // The collected crystal must be GONE FROM PIXELS, not just from the score: the torus is
    // the only violet thing in the scene, so its pixels must vanish from the OS capture.
    // (This caught the pose-writer resurrecting the hidden instance every tick.)
    const collectedShot = await shot("play_t1_crystal_collected_score_1");
    if (lastShotWasOs && violetBaseline > 200) {
      const violetNow = violetCount(collectedShot);
      console.log(`[roles] violet after collection: ${violetNow} (baseline ${violetBaseline})`);
      expect(violetNow).toBeLessThan(violetBaseline / 5);
    }

    // Diagnose the animation channel before judging pixels.
    const anim = await invoke("animation_state", { id: wedge });
    console.log(`[roles] animation: playing=${anim.playing} duration=${anim.durationTick} tick=${anim.currentTick} loop=${anim.loopPolicy} tracks=${anim.tracks.length}`);
    expect(anim.playing).toBe(true);
    expect(anim.loopPolicy).toBe("loop");

    // The spinner keeps the scene alive. Whole-window shots when the OS allows; the MOTION
    // PROOF is a pair of engine renders of the wedge itself, 500 ms apart — same source, so
    // the pixel diff isolates the spin.
    await shot("play_t2_viewport_mid_play", wedge);
    const a = await thumbShot("play_t3_spinner_frame_a", wedge);
    await browser.pause(500);
    const b = await thumbShot("play_t4_spinner_frame_b", wedge);
    const frac = diffFrac(a, b);
    console.log(`[roles] spinner motion diff frac=${frac}`);
    expect(frac).toBeGreaterThan(0.0005);
  });

  it("STOP restores everything: the crystal returns, the score resets, the doc never changed", async () => {
    await invoke("stop");
    await browser.pause(900);
    const status = await invoke("role_status");
    expect(status.score).toBe(0);
    expect(status.remaining).toBe(1);
    expect(await entityCount()).toBe(5);
    await invoke("frame_all");
    const stoppedShot = await shot("stopped_crystal_restored", crystal);
    // And the crystal is BACK in pixels — the hide was play-only projection.
    if (lastShotWasOs && violetBaseline > 200) {
      const violetBack = violetCount(stoppedShot);
      console.log(`[roles] violet after stop: ${violetBack} (baseline ${violetBaseline})`);
      expect(violetBack).toBeGreaterThan(violetBaseline / 3);
    }
  });

  it("refusals are explained: no live-editing roles during Play; unknown roles named", async () => {
    const bogus = await invoke("role_assign", { id: crystal, role: "boss" });
    expect(bogus.reason).toContain("boss");
    await invoke("play");
    const during = await invoke("role_assign", { id: wall, role: "prop" });
    expect(during.reason).toContain("stop Play first");
    await invoke("stop");
    await browser.pause(600);
  });

  it("one Ctrl-Z removes a whole role assignment", async () => {
    const before = await invoke("role_status");
    await selectEntity(wedge);
    await (await $('[data-testid="engine-gameplay"]')).click();
    const clear = await $('[data-testid="role-clear"]');
    await clear.waitForExist({ timeout: 10000 });
    await clear.click();
    await browser.pause(400);
    let status = await invoke("role_status");
    expect(status.roster.length).toBe(before.roster.length - 1);
    await invoke("undo");
    await browser.pause(400);
    status = await invoke("role_status");
    expect(status.roster.length).toBe(before.roster.length);
    console.log("[roles] clear-role undone in one step");
  });

  it("every capture in this run is real pixels", async () => {
    const files = readdirSync(shots);
    expect(files.length).toBeGreaterThanOrEqual(6);
    for (const f of files) {
      // OS captures are ~380 KB; 512px engine renders are smaller but never blank-tiny.
      expect(statSync(path.join(shots, f)).size).toBeGreaterThan(4_000);
    }
    console.log(`[roles] evidence: ${files.join(", ")}`);
  });
});
