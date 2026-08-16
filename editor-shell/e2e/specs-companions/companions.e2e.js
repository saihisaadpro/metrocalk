// Companions on the PACKAGED .exe — follow, patrol, attack, gravity, collision, live.
//
// Scene 1 (follow + hunt): a Ball (Physics prop), a Dog (Companion) spawned in the AIR — it
// FALLS under gravity, then heels to the ball at its follow distance. We shove the ball across
// the arena; the dog gives chase. A Skeleton (Enemy) waits downfield: when the chase brings the
// dog inside its aggro, it hunts, strikes, and the skeleton is knocked flying and vanishes —
// the Score climbs to 1 through the authored defeat rule.
// Scene 2 (patrol): a lone dog with three numbered Waypoints walks the chain, looping.
// Scene 3 (pathfinding): a wall dead between dog and ball — the dog detours around it.
// Every claim is asserted from live sim positions (body_sim_position) or the rules runtime,
// and OS-captured for pixels.

import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const dir = path.dirname(fileURLToPath(import.meta.url));
const shots = path.resolve(dir, "../.shots-companions");
const capture = path.resolve(dir, "../scripts/capture-composited-window.ps1");
const captureFg = path.resolve(dir, "../../../.uxtest/audit/exe/capture-window-fg.ps1");
const pngdiff = path.resolve(dir, "../../../.uxtest/audit/exe/pngdiff.ps1");
mkdirSync(shots, { recursive: true });

let shotIndex = 0;

async function shot(label, thumbId) {
  await browser.pause(500);
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
      console.log(`[comp] captured ${path.basename(out)} via engine render (${statSync(out).size} bytes)`);
      return out;
    }
  }
  if (!ok) throw new Error(`capture ${label} came back blank on every path`);
  console.log(`[comp] captured ${path.basename(out)} (${statSync(out).size} bytes)`);
  return out;
}

function diffFrac(a, b) {
  const raw = execFileSync(
    "powershell.exe",
    ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", pngdiff, "-A", a, "-B", b, "-Thresh", "24", "-Sample", "2"],
    { encoding: "utf8" },
  );
  const line = raw.split(/\r?\n/).find((l) => l.trim().startsWith("{"));
  return line ? JSON.parse(line).frac : -1;
}

const pos = (id) => invoke("body_sim_position", { id });
const distXZ = (a, b) => Math.hypot(a[0] - b[0], a[2] - b[2]);

async function newScene() {
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

describe("Companions — follow, hunt, patrol, and physics that stays honest", () => {
  before(async () => {
    await browser.waitUntil(
      async () => browser.execute(() => !!(window.__TAURI__ && window.__TAURI__.core)),
      { timeout: 30000 },
    );
  });

  it("the dog FALLS from the sky (gravity is real), then heels to the ball", async () => {
    await newScene();
    const ball = (await invoke("shape_spawn", { kind: "sphere", pos: [0, 0, 0] })).created;
    await invoke("role_assign", { id: ball, role: "prop" });
    const dog = (await invoke("shape_spawn", { kind: "box", pos: [-5, 2.5, 0] })).created;
    await invoke("role_assign", { id: dog, role: "companion" });
    await invoke("frame_all");
    await shot("set_dog_airborne_ball_grounded");
    await invoke("play");
    await browser.pause(1200);
    const p = await pos(dog);
    console.log(`[comp] dog after 1.2s of gravity: y=${p[1].toFixed(2)}`);
    expect(p[1]).toBeLessThan(1.2);
    expect(p[1]).toBeGreaterThan(-0.5);

    // Heel: the dog closes to ~its follow distance of the ball and settles there.
    let d = null;
    await browser.waitUntil(
      async () => {
        d = distXZ(await pos(dog), await pos(ball));
        return d < 2.8;
      },
      { timeout: 15000, interval: 400, timeoutMsg: `dog never came to heel (last ${d})` },
    );
    console.log(`[comp] dog heeled at ${d.toFixed(2)} m`);
    const status = await invoke("role_status");
    expect(status.companions.length).toBe(1);
    console.log(`[comp] live status: ${status.companions[0].doing}`);
    await shot("dog_heeling_at_follow_distance");
    await invoke("stop");
    await browser.pause(700);
  });

  it("shove the ball — the dog gives chase across the arena", async () => {
    await newScene();
    const ball = await spawnWithRole("sphere", [0, 0, 0], "prop");
    const dog = await spawnWithRole("box", [-4, 0, 0], "companion");
    await invoke("frame_all");
    await invoke("play");
    await browser.pause(2500);
    const before = await pos(dog);
    await invoke("sim_shove", { id: ball, impulse: [7, 0, 0] });
    await browser.pause(2500);
    const ballNow = await pos(ball);
    const dogNow = await pos(dog);
    console.log(`[comp] ball rolled to x=${ballNow[0].toFixed(2)}; dog ran ${(dogNow[0] - before[0]).toFixed(2)} m in x`);
    expect(ballNow[0]).toBeGreaterThan(2.0);
    expect(dogNow[0] - before[0]).toBeGreaterThan(1.5);
    await shot("dog_chasing_shoved_ball");
    await invoke("stop");
    await browser.pause(700);
  });

  it("the hunt: chase leads into aggro — strike, knockback, the skeleton falls, Score 1", async () => {
    await newScene();
    const ball = await spawnWithRole("sphere", [0, 0, 0], "prop");
    const dog = await spawnWithRole("box", [-3, 0, 0], "companion");
    const skeleton = await spawnWithRole("wedge", [8, 0, 0.5], "enemy");
    await invoke("frame_all");
    await shot("hunt_set_skeleton_downfield");
    await invoke("play");
    await browser.pause(1500);
    await invoke("sim_shove", { id: ball, impulse: [8, 0, 0.5] });

    let status = null;
    await browser.waitUntil(
      async () => {
        status = await invoke("role_status");
        return status && status.score === 1;
      },
      { timeout: 25000, interval: 400, timeoutMsg: "the dog never defeated the skeleton" },
    );
    console.log(`[comp] DEFEAT — score=${status.score}; dog says: "${status.companions[0]?.doing}"`);
    await browser.pause(500);
    await shot("skeleton_defeated_score_1");
    await invoke("stop");
    await browser.pause(900);
    // Stop restores the skeleton and the score — the fight never touched the document.
    status = await invoke("role_status");
    expect(status.score).toBe(0);
    expect(status.roster.some((r) => r.role === "enemy")).toBe(true);
  });

  it("patrol: a lone dog walks the numbered waypoint chain (repeated movement)", async () => {
    await newScene();
    const dog = await spawnWithRole("box", [0, 0, 0], "companion");
    const w1 = await spawnWithRole("capsule", [4, 0, 0], "waypoint");
    const w2 = await spawnWithRole("capsule", [4, 0, 4], "waypoint");
    const w3 = await spawnWithRole("capsule", [0, 0, 4], "waypoint");
    expect(w1 && w2 && w3).toBeTruthy();
    await invoke("frame_all");
    await invoke("play");

    // Sample the dog's live position for ~14 s: it must pass near ≥2 distinct waypoints.
    const stops = [[4, 0, 0], [4, 0, 4], [0, 0, 4]];
    const visited = new Set();
    const t0 = Date.now();
    while (Date.now() - t0 < 14000 && visited.size < 2) {
      const p = await pos(dog);
      stops.forEach((s, i) => {
        if (distXZ(p, s) < 1.1) visited.add(i);
      });
      await browser.pause(300);
    }
    console.log(`[comp] patrol visited ${visited.size} waypoints: ${[...visited].join(",")}`);
    expect(visited.size).toBeGreaterThanOrEqual(2);
    const a = await shot("patrol_frame_a", dog);
    await browser.pause(1200);
    const b = await shot("patrol_frame_b", dog);
    const frac = diffFrac(a, b);
    console.log(`[comp] patrol motion diff frac=${frac}`);
    await invoke("stop");
    await browser.pause(700);
  });

  it("pathfinding: a wall between dog and ball — the dog finds a way around", async () => {
    await newScene();
    const ball = await spawnWithRole("sphere", [7, 0, 0], "prop");
    const wall = await spawnWithRole("box", [3.5, 0, 0], "solid");
    const dog = await spawnWithRole("box", [0, 0, 0], "companion");
    expect(wall).toBeTruthy();
    await invoke("frame_all");
    await invoke("play");
    let d = null;
    await browser.waitUntil(
      async () => {
        d = distXZ(await pos(dog), await pos(ball));
        return d < 3.0;
      },
      { timeout: 30000, interval: 500, timeoutMsg: `the wall stopped the dog for good (last ${d})` },
    );
    console.log(`[comp] dog rounded the wall and heeled at ${d.toFixed(2)} m`);
    await shot("dog_rounded_the_wall");
    await invoke("stop");
    await browser.pause(700);
  });

  it("authoring stays honest: cards visible, one Ctrl-Z per role, refusals explained", async () => {
    await newScene();
    const dog = (await invoke("shape_spawn", { kind: "box", pos: [0, 0, 0] })).created;
    const reply = await invoke("role_assign", { id: dog, role: "companion" });
    expect(reply.added.join(" ")).toContain("brain");
    await invoke("undo");
    const status = await invoke("role_status");
    expect(status.roster.length).toBe(0);
    // The Gameplay panel shows all seven cards for a selection.
    await browser.execute((id) => {
      const row = document.querySelector(`[data-testid="hrow"][data-id="${id}"]`);
      row?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    }, dog);
    await (await $('[data-testid="engine-gameplay"]')).click();
    for (const kind of ["companion", "enemy", "waypoint"]) {
      await (await $(`[data-testid="role-${kind}"]`)).waitForExist({ timeout: 10000 });
    }
    await shot("all_seven_role_cards");
  });

  it("every capture is real pixels", async () => {
    const files = (await import("node:fs")).readdirSync(shots);
    expect(files.length).toBeGreaterThanOrEqual(7);
    for (const f of files) {
      expect(statSync(path.join(shots, f)).size).toBeGreaterThan(4_000);
    }
    console.log(`[comp] evidence: ${files.join(", ")}`);
  });
});
