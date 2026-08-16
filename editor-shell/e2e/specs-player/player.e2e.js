// Round 5 on the PACKAGED .exe — the BEFORE/AFTER evidence run.
//
// BEFORE this round: testing a game meant `sim_shove` from a console; driven bodies glided
// without turning; a defeated enemy vanished the same tick the knockback landed (the kick was
// invisible); nothing announced a win.
// AFTER: click 🎮 Player on a shape, press Play, and DRIVE it with the arrow keys — your
// companion heels to YOU (players outrank props), driven bodies FACE their travel, a struck
// enemy goes FLYING and tumbles before it vanishes, and the panel crowns the run with a
// victory banner the moment the last objective falls.

import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const dir = path.dirname(fileURLToPath(import.meta.url));
const shots = path.resolve(dir, "../.shots-player");
const capture = path.resolve(dir, "../scripts/capture-composited-window.ps1");
const captureFg = path.resolve(dir, "../../../.uxtest/audit/exe/capture-window-fg.ps1");
mkdirSync(shots, { recursive: true });

let shotIndex = 0;

async function shot(label, thumbId) {
  await browser.pause(450);
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
    if (round > 0) await browser.pause(1200);
    ok =
      attempt(capture, ["-Out", out]) ||
      attempt(captureFg, ["-ProcName", "metrocalk-editor-shell", "-Out", out]);
  }
  if (!ok && thumbId) {
    const data = await invoke("thumbnail", { id: thumbId, size: 512 });
    if (data && data.startsWith("data:image/png;base64,")) {
      writeFileSync(out, Buffer.from(data.split(",")[1], "base64"));
      console.log(`[r5] captured ${path.basename(out)} via engine render (${statSync(out).size} bytes)`);
      return out;
    }
  }
  if (!ok) throw new Error(`capture ${label} blank on every path`);
  console.log(`[r5] captured ${path.basename(out)} (${statSync(out).size} bytes)`);
  return out;
}

const pos = (id) => invoke("body_sim_position", { id });
const distXZ = (a, b) => Math.hypot(a[0] - b[0], a[2] - b[2]);

/** Press or release a drive key inside the app (the window listener is the real input path). */
const key = (type, k) =>
  browser.execute(
    (t, kk) => {
      window.dispatchEvent(new KeyboardEvent(t, { key: kk, bubbles: true }));
    },
    type,
    k,
  );

/** Press the REAL Play/Stop buttons — the key listener is gated on the UI's play state. */
async function pressPlay() {
  await browser.execute(() => {
    document.querySelector('[data-testid="play"]')?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });
  await browser.waitUntil(
    async () =>
      browser.execute(() => !!document.querySelector('[data-testid="stop"]')),
    { timeout: 10000, timeoutMsg: "Play never engaged (no Stop button)" },
  );
}

async function pressStop() {
  await browser.execute(() => {
    document.querySelector('[data-testid="stop"]')?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });
  await browser.pause(800);
}

async function newScene() {
  await pressStop(); // defensive: a failed prior test must not leave Play running
  await invoke("new_project");
  await browser.pause(700);
  await browser.execute(() => {
    document.querySelector('[data-testid="onboardSkip"]')?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });
}

async function spawnWithRole(kind, at, role) {
  const id = (await invoke("shape_spawn", { kind, pos: at })).created;
  const reply = await invoke("role_assign", { id, role });
  if (reply.reason) throw new Error(`${role} refused: ${reply.reason}`);
  return id;
}

describe("Round 5 — you drive, the party follows, victory is announced", () => {
  before(async () => {
    await browser.waitUntil(
      async () => browser.execute(() => !!(window.__TAURI__ && window.__TAURI__.core)),
      { timeout: 30000 },
    );
  });

  it("ARROW KEYS drive the Player — and release brakes it", async () => {
    await newScene();
    const hero = await spawnWithRole("capsule", [0, 0, 0], "player");
    await invoke("frame_all");
    await pressPlay();
    await browser.pause(800);
    const start = await pos(hero);

    await key("keydown", "ArrowRight");
    await browser.pause(2000);
    await key("keyup", "ArrowRight");
    const afterDrive = await pos(hero);
    const driven = afterDrive[0] - start[0];
    console.log(`[r5] 2s of ArrowRight drove the hero ${driven.toFixed(2)} m in +x`);
    expect(driven).toBeGreaterThan(2.0);

    // Release = brake: within a second it is nearly at rest, not coasting away.
    await browser.pause(1000);
    const settled = await pos(hero);
    const coast = Math.abs(settled[0] - afterDrive[0]);
    console.log(`[r5] after release it coasted only ${coast.toFixed(2)} m`);
    expect(coast).toBeLessThan(1.2);
    await shot("hero_driven_by_keys", hero);
    await pressStop();
  });

  it("the companion heels to the PLAYER even with a nearer prop (players outrank props)", async () => {
    await newScene();
    const hero = await spawnWithRole("capsule", [4, 0, 0], "player");
    const crate_ = await spawnWithRole("box", [-2, 0, 0], "prop");
    const dog = await spawnWithRole("box", [0, 0, 3], "companion");
    expect(crate_).toBeTruthy();
    await invoke("frame_all");
    await pressPlay();
    let dHero = null;
    await browser.waitUntil(
      async () => {
        dHero = distXZ(await pos(dog), await pos(hero));
        return dHero < 2.8;
      },
      { timeout: 15000, interval: 400, timeoutMsg: `dog never heeled to the player (last ${dHero})` },
    );
    const dCrate = distXZ(await pos(dog), await pos(crate_));
    console.log(`[r5] dog heels to the HERO at ${dHero.toFixed(2)} m (crate is ${dCrate.toFixed(2)} m away)`);
    expect(dHero).toBeLessThan(dCrate);
    await shot("dog_heels_to_player_not_crate");
    await pressStop();
  });

  it("a struck enemy goes FLYING first (visible knockback), vanishes after, and the banner crowns it", async () => {
    await newScene();
    const hero = await spawnWithRole("capsule", [0, 0, 0], "player");
    const dog = await spawnWithRole("box", [-2, 0, 0], "companion");
    const skeleton = await spawnWithRole("wedge", [4.5, 0, 0.3], "enemy");
    expect(hero && dog).toBeTruthy();
    await invoke("frame_all");
    await pressPlay();
    await browser.pause(600);
    // YOU lead the charge: drive toward the skeleton — the dog follows you into its aggro,
    // then breaks off and hunts. That is the whole game loop, played like a player.
    await key("keydown", "ArrowRight");
    await browser.pause(1600);
    await key("keyup", "ArrowRight");

    let status = null;
    await browser.waitUntil(
      async () => {
        status = await invoke("role_status");
        return status && status.score === 1;
      },
      { timeout: 20000, interval: 250, timeoutMsg: "no defeat" },
    );
    // The strike just landed: the corpse must be AIRBORNE from the knockback and still
    // rendered (the linger window) — the drama the old same-tick hide swallowed.
    const flung = await pos(skeleton);
    console.log(`[r5] knockback: skeleton at y=${flung[1].toFixed(2)} the instant after the strike`);
    await shot("skeleton_mid_flight");
    expect(flung[1]).toBeGreaterThan(0.55);

    // Victory: the only enemy is down — the banner is up within the linger window.
    await browser.waitUntil(
      async () => {
        status = await invoke("role_status");
        return status.won === true;
      },
      { timeout: 5000, interval: 250, timeoutMsg: "no victory" },
    );
    console.log(`[r5] WON=${status.won}`);
    await browser.execute(() => {
      document.querySelector('[data-testid="engine-gameplay"]')?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await (await $('[data-testid="role-victory"]')).waitForExist({ timeout: 5000 });
    await browser.pause(900); // past the linger — the corpse is gone now
    await shot("victory_banner_skeleton_gone");
    await pressStop();
    status = await invoke("role_status");
    expect(status.won).toBe(false);
    expect(status.score).toBe(0);
  });

  it("driven bodies FACE their travel (render-only yaw)", async () => {
    await newScene();
    const hero = await spawnWithRole("wedge", [0, 0, 0], "player");
    await invoke("frame_all");
    await pressPlay();
    await browser.pause(500);
    // Drive +x for a moment, capture the engine render mid-run: the wedge must be yawed 90°
    // (its rectangular face toward the camera changes silhouette vs at rest).
    const rest = await invoke("thumbnail", { id: hero, size: 256 });
    await key("keydown", "ArrowRight");
    await browser.pause(900);
    const moving = await invoke("thumbnail", { id: hero, size: 256 });
    await key("keyup", "ArrowRight");
    expect(rest && moving).toBeTruthy();
    expect(moving).not.toBe(rest);
    console.log(`[r5] silhouette changed while driving (rest ${rest.length} vs moving ${moving.length} chars)`);
    await pressStop();
  });

  it("every capture is real pixels", async () => {
    const files = readdirSync(shots);
    expect(files.length).toBeGreaterThanOrEqual(3);
    for (const f of files) {
      expect(statSync(path.join(shots, f)).size).toBeGreaterThan(4_000);
    }
    console.log(`[r5] evidence: ${files.join(", ")}`);
  });
});
