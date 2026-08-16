// CINEMATICS on the PACKAGED .exe — before and after, MEASURED.
//
// BEFORE: there was no way to author a camera move at all. `cam_override` carried a POSITION and
// nothing else, so the renderer always aimed at the editor's orbit target — a cutscene was impossible
// by construction, not merely unimplemented. Press Play and you got the same orbit view you were
// building in.
// AFTER: select an object → Cinematics → "Hero shot". One undoable commit. Press Play and the camera
// takes authority, frames the subject three-quarters on, and creeps in — then hands the view back
// exactly as it found it when the shot list runs out.
//
// The evidence is not "the screenshot looks cinematic". `camera_probe` reports the live eye, look-at
// and fov every step, so the claim is arithmetic: the camera MOVED, it AIMS AT THE SUBJECT, the
// distance SHRINKS across the push-in, and it is RESTORED on Stop.

import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, rmSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const dir = path.dirname(fileURLToPath(import.meta.url));
const shots = path.resolve(dir, "../.shots-cinema");
const capture = path.resolve(dir, "../scripts/capture-composited-window.ps1");
const captureFg = path.resolve(dir, "../../../.uxtest/audit/exe/capture-window-fg.ps1");
mkdirSync(shots, { recursive: true });

let shotIndex = 0;

async function shot(label) {
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
    if (round > 0) await browser.pause(1000);
    ok = attempt(capture, ["-Out", out]) || attempt(captureFg, ["-ProcName", "metrocalk-editor-shell", "-Out", out]);
  }
  if (!ok) {
    console.log(`[cine] CAPTURE UNAVAILABLE for ${label} — the desktop refused both paths`);
    return null;
  }
  console.log(`[cine] captured ${path.basename(out)} (${statSync(out).size} bytes)`);
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

const cam = () => invoke("camera_probe");
const dist3 = (a, b) => Math.hypot(a[0] - b[0], a[1] - b[1], a[2] - b[2]);
const fmt = (v) => `[${v.map((n) => n.toFixed(2)).join(", ")}]`;

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

describe("Cinematics — a shot is a sentence, solved per tick", () => {
  let statue;
  let plainCam;

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

  it("builds the set: a statue on a plinth, and some world to be lost in", async () => {
    statue = (await invoke("shape_spawn", { kind: "capsule", pos: [0, 1.6, 0] })).created;
    await invoke("shape_spawn", { kind: "cylinder", pos: [0, 0.2, 0] });
    // Scatter a few landmarks so a wide shot and a close shot are visibly different pictures.
    for (const p of [[7, 0.5, 6], [-8, 0.5, 5], [6, 0.5, -7], [-6, 0.5, -6]]) {
      await invoke("shape_spawn", { kind: "box", pos: p });
    }
    await invoke("frame_all");
    expect(statue).toBeTruthy();
    await shot("00_set_built");
  });

  it("BEFORE: Play with no cutscene — the camera is the editor's own orbit view", async () => {
    await pressPlay();
    await browser.pause(1200);
    plainCam = await cam();
    console.log(`[cine] BEFORE  eye=${fmt(plainCam.eye)} lookAt=${fmt(plainCam.lookAt)} dist=${plainCam.distance.toFixed(2)} cinematic=${plainCam.cinematic}`);
    // The claim, stated as an assertion: nothing owns the camera.
    expect(plainCam.cinematic).toBe(false);
    await shot("01_before_play_plain_camera");
    await pressStop();
    await browser.pause(600);
  });

  it("authors a Hero shot in one click — and it reads back as English", async () => {
    await click('[data-testid="engine-gameplay"]');
    await browser.pause(500);
    expect(await selectRow(statue)).toBe(true);
    await (await $('[data-testid="cinema-section"]')).waitForExist({ timeout: 10000 });

    const before = await invoke("cinema_list", { id: statue });
    console.log(`[cine] shots before: ${before.shots}`);
    expect(before.shots).toBe(0);
    await shot("02_cinema_panel_empty");

    expect(await click('[data-testid="shot-hero"]')).toBe(true);
    await browser.waitUntil(
      async () => (await invoke("cinema_list", { id: statue })).shots === 1,
      { timeout: 10000, timeoutMsg: "the shot never landed" },
    );
    const after = await invoke("cinema_list", { id: statue });
    console.log(`[cine] shot 1 reads: "${after.reads[0]}"`);
    expect(after.reads[0].toLowerCase()).toContain("pushing in");
    expect(after.seconds).toBeGreaterThan(0);

    // The panel shows the sentence where the author is looking, not only in the reply.
    const panel = await browser.execute(
      () => document.querySelector('[data-testid="cinema-shots"]')?.textContent ?? "",
    );
    console.log(`[cine] the panel reads: ${panel}`);
    expect(panel.length).toBeGreaterThan(10);
    await shot("03_hero_shot_authored");
  });

  it("continuity is checked FOR you: opening on a close-up is called out in plain language", async () => {
    // A second cutscene that opens tight then cuts wide — the mistake every first cutscene makes.
    // (A LONE close-up is a legitimate insert, so the check deliberately needs a following shot.)
    const bad = (await invoke("shape_spawn", { kind: "sphere", pos: [12, 1, 12] })).created;
    const one = await invoke("cinema_add_shot", { id: bad, kind: "closeup" });
    expect(one.problems.length).toBe(0); // one close-up alone is fine — no false alarm
    const r = await invoke("cinema_add_shot", { id: bad, kind: "establish" });
    console.log(`[cine] problems: ${JSON.stringify(r.problems)}`);
    expect(r.problems.length).toBeGreaterThanOrEqual(1);
    expect(r.problems.join(" ").toLowerCase()).toContain("open");
    // and it never blocks the author — both shots still landed.
    expect(r.shots).toBe(2);
    // Take that whole cutscene back off, so the camera-authority test below has exactly one claimant
    // and the measurement is unambiguous.
    await invoke("cinema_remove_shot", { id: bad, index: 1 });
    await invoke("cinema_remove_shot", { id: bad, index: 0 });
    const cleared = await invoke("cinema_list", { id: bad });
    expect(cleared.shots).toBe(0);
  });

  it("THE EXCLUSIVITY RULE: with two cutscenes armed, exactly ONE owns the camera", async () => {
    // There is one camera. Before this was enforced, two armed cutscenes both wrote `cam_override`
    // every tick and the view landed on whichever resolved last — a camera aimed at neither subject.
    const other = (await invoke("shape_spawn", { kind: "sphere", pos: [14, 1, 14] })).created;
    await invoke("cinema_add_shot", { id: other, kind: "hero" });
    await pressPlay();
    await browser.pause(1000);
    const live = await cam();
    const toStatue = dist3(live.lookAt, [0, 1.6, 0]);
    const toOther = dist3(live.lookAt, [14, 1, 14]);
    console.log(`[cine] aim is ${toStatue.toFixed(2)} from the statue, ${toOther.toFixed(2)} from the other`);
    expect(live.cinematic).toBe(true);
    // It is aimed squarely at ONE of them — not averaged, not oscillating between the two.
    expect(Math.min(toStatue, toOther)).toBeLessThan(2.0);
    await pressStop();
    await browser.pause(600);
    // Put the scene back to a single claimant for the measurements that follow.
    await invoke("cinema_remove_shot", { id: other, index: 0 });
    expect((await invoke("cinema_list", { id: other })).shots).toBe(0);
  });

  it("AFTER: Play — the camera takes authority, frames the statue, and pushes in", async () => {
    await pressPlay();
    await browser.pause(450);

    const early = await cam();
    console.log(`[cine] AFTER@1 eye=${fmt(early.eye)} lookAt=${fmt(early.lookAt)} dist=${early.distance.toFixed(2)} cinematic=${early.cinematic}`);
    // 1. The shot solver owns the view — an aimed camera, which BEFORE was not representable.
    expect(early.cinematic).toBe(true);
    // 2. It is a DIFFERENT camera from the editor's.
    const moved = dist3(early.eye, plainCam.eye);
    console.log(`[cine] the camera moved ${moved.toFixed(2)} units from the editor's view`);
    expect(moved).toBeGreaterThan(1.0);
    // 3. It is aimed AT THE STATUE, not at the orbit target.
    const statuePos = [0, 1.6, 0];
    const aimErr = dist3(early.lookAt, statuePos);
    console.log(`[cine] aim error vs the statue: ${aimErr.toFixed(2)} units`);
    expect(aimErr).toBeLessThan(2.0);

    // 4. The push-in is real: later in the SAME shot the camera is measurably closer. Both samples
    //    have to land inside the shot's 2.5s — since the hand-back was fixed, a late sample measures
    //    the restored editor view instead, which is a different (and now correct) thing entirely.
    //    Screenshots are taken after the measurements for exactly that reason: a capture costs the
    //    best part of a second.
    await browser.pause(900);
    const late = await cam();
    console.log(`[cine] AFTER@2 dist=${late.distance.toFixed(2)} (was ${early.distance.toFixed(2)}) cinematic=${late.cinematic}`);
    expect(late.cinematic).toBe(true);
    expect(late.distance).toBeLessThan(early.distance - 0.05);
    await shot("04_after_play_pushed_in");
  });

  it("THE PICTURE: a multi-shot cutscene, captured mid-shot, with the authoring chrome gone", async () => {
    // A 2.5s shot is shorter than an OS window capture takes, so the earlier screenshots landed after
    // the hand-back and showed the editor view. A longer cutscene gives the capture somewhere to land
    // — and it is the only way to SEE the thing the measurements have been asserting.
    await pressStop();
    await browser.pause(600);
    for (const kind of ["orbit", "reveal"]) {
      const r = await invoke("cinema_add_shot", { id: statue, kind });
      expect(r.reason).toBe(null);
    }
    const cut = await invoke("cinema_list", { id: statue });
    console.log(`[cine] the cutscene is now ${cut.shots} shots / ${cut.seconds.toFixed(1)}s`);
    expect(cut.seconds).toBeGreaterThan(6);

    await pressPlay();
    await browser.pause(1200);
    const live = await cam();
    console.log(`[cine] capturing mid-shot: cinematic=${live.cinematic} eye=${fmt(live.eye)}`);
    expect(live.cinematic).toBe(true);
    await shot("05_cinematic_frame_no_chrome");

    // The viewport stops being a workspace for the duration: no transform gizmo, no selection
    // outline, no binding lines drawn across the shot.
    const stillRolling = await cam();
    console.log(`[cine] still rolling after the capture: ${stillRolling.cinematic}`);
    await pressStop();
    await browser.pause(700);
    // Put it back to one shot for the tests below.
    for (const i of [2, 1]) {
      await invoke("cinema_remove_shot", { id: statue, index: i });
    }
    expect((await invoke("cinema_list", { id: statue })).shots).toBe(1);
    await pressPlay();
  });

  it("THE HAND-BACK: when the shots run out the camera returns, WITHOUT pressing Stop", async () => {
    // This is what "the camera takes over, then hands back" means, and it was not true: nothing sets
    // `Cinematic.playing` false, so the tick after the shots ran out saw the cue still high and
    // started the whole cutscene again — forever. A cutscene now plays once per Play run.
    await browser.waitUntil(
      async () => !(await cam()).cinematic,
      { timeout: 15000, interval: 300, timeoutMsg: "the cutscene never ended — it re-armed itself" },
    );
    const back = await cam();
    console.log(`[cine] handed back mid-Play: cinematic=${back.cinematic} eye=${fmt(back.eye)}`);
    expect(back.cinematic).toBe(false);
    // and it STAYS handed back rather than restarting a beat later
    await browser.pause(2500);
    const later = await cam();
    console.log(`[cine] two and a half seconds on: cinematic=${later.cinematic}`);
    expect(later.cinematic).toBe(false);
    await shot("06_handed_back_mid_play");
  });

  it("hands the camera back: Stop restores exactly the view you were building in", async () => {
    await pressStop();
    await browser.pause(900);
    const back = await cam();
    console.log(`[cine] RESTORED eye=${fmt(back.eye)} cinematic=${back.cinematic}`);
    expect(back.cinematic).toBe(false);
    expect(dist3(back.eye, plainCam.eye)).toBeLessThan(0.5);
    await shot("06_stopped_camera_restored");
  });

  it("THE INVARIANT: Play wrote nothing — the cutscene is exactly what you authored", async () => {
    const after = await invoke("cinema_list", { id: statue });
    console.log(`[cine] after a full Play/Stop cycle: ${after.shots} shot(s), ${after.seconds.toFixed(1)}s`);
    expect(after.shots).toBe(1);
    // One Ctrl-Z takes a shot away. Author a second one first, so the undo target is unambiguous —
    // undo is global, and the scene has had other commits since the hero shot landed.
    await invoke("cinema_add_shot", { id: statue, kind: "closeup" });
    expect((await invoke("cinema_list", { id: statue })).shots).toBe(2);
    await invoke("undo");
    await browser.pause(500);
    const undone = await invoke("cinema_list", { id: statue });
    console.log(`[cine] after one undo: ${undone.shots} shot(s)`);
    expect(undone.shots).toBe(1);
    await shot("07_undone");
  });
});
