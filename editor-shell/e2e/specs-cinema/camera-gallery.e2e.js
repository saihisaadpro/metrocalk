// THE CAMERA GALLERY — every shot card, authored and MEASURED on the packaged .exe.
//
// The point is not that a cutscene runs (the cinema spec proves that). It is that the catalogue is a
// real vocabulary rather than one shot with twelve names: each card must put the camera somewhere
// genuinely different, aim it at the subject, and move it the way its own label promises. All of that
// is read off `camera_probe` while the shot is live, so every claim below is a number.
//
// Each card also leaves a screenshot behind, so the gallery is a contact sheet you can look at.

import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, rmSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const dir = path.dirname(fileURLToPath(import.meta.url));
const shots = path.resolve(dir, "../.shots-camera");
const capture = path.resolve(dir, "../scripts/capture-composited-window.ps1");
const captureFg = path.resolve(dir, "../../../.uxtest/audit/exe/capture-window-fg.ps1");
mkdirSync(shots, { recursive: true });

async function shot(label) {
  const out = path.join(shots, `${label}.png`);
  const good = () => existsSync(out) && statSync(out).size > 20_000;
  const attempt = (script, args) => {
    try {
      execFileSync("powershell.exe", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", script, ...args], { stdio: "pipe" });
    } catch { /* fall through */ }
    if (!good() && existsSync(out)) rmSync(out);
    return good();
  };
  const ok = attempt(capture, ["-Out", out]) || attempt(captureFg, ["-ProcName", "metrocalk-editor-shell", "-Out", out]);
  if (!ok) console.log(`[cam] CAPTURE UNAVAILABLE for ${label}`);
  return ok;
}

const click = (selector) =>
  browser.execute((sel) => {
    const el = document.querySelector(sel);
    if (!el) return false;
    el.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    return true;
  }, selector);

const cam = () => invoke("camera_probe");
const dist3 = (a, b) => Math.hypot(a[0] - b[0], a[1] - b[1], a[2] - b[2]);
const fmt = (v) => `[${v.map((n) => n.toFixed(1)).join(",")}]`;

async function pressPlay() {
  await click('[data-testid="play"]');
  await browser.waitUntil(
    async () => browser.execute(() => !!document.querySelector('[data-testid="stop"]')),
    { timeout: 10000, timeoutMsg: "Play never engaged" },
  );
}
async function pressStop() {
  await click('[data-testid="stop"]');
  await browser.pause(500);
}

/** Author exactly one shot, roll it, and sample the live camera twice inside the take.
 *
 * MEASURING and CAPTURING are separate passes on purpose. An OS window capture costs the better part
 * of two seconds, so taking one between the two samples put the second sample AFTER the shot had ended
 * — every card then measured the restored editor view and "did it push in?" was unanswerable. The
 * measurement pass runs tight and untouched; the capture pass runs afterwards for the picture. */
async function takeShot(statue, kind, snap) {
  const add = await invoke("cinema_add_shot", { id: statue, kind });
  if (add.reason) throw new Error(`${kind} refused: ${add.reason}`);
  await pressPlay();
  await browser.pause(250);
  const early = await cam();
  await browser.pause(550);
  const late = await cam();
  if (snap) await shot(`${snap}_${kind}`);
  await pressStop();
  await browser.pause(400);
  await invoke("cinema_remove_shot", { id: statue, index: 0 });
  const left = await invoke("cinema_list", { id: statue });
  if (left.shots !== 0) throw new Error(`${kind} did not clean up: ${left.shots} left`);
  return { kind, early, late, reads: add.reads[0] };
}

describe("The camera gallery — every shot card, measured live", () => {
  let statue;
  const SUBJECT = [0, 1.6, 0];
  const takes = [];

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
    // A subject worth filming, and landmarks so a wide shot and a tight one are different pictures.
    statue = (await invoke("shape_spawn", { kind: "capsule", pos: SUBJECT })).created;
    await invoke("shape_spawn", { kind: "cylinder", pos: [0, 0.2, 0] });
    for (const p of [[8, 0.5, 7], [-9, 0.5, 6], [7, 0.5, -8], [-7, 0.5, -7], [0, 0.5, 12], [0, 0.5, -12]]) {
      await invoke("shape_spawn", { kind: "box", pos: p });
    }
    await invoke("frame_all");
  });

  it("offers a real vocabulary, not one shot with many names", async () => {
    const catalogue = await invoke("cinema_catalog");
    console.log(`[cam] ${catalogue.length} shot cards: ${catalogue.map((c) => c.kind).join(", ")}`);
    expect(catalogue.length).toBeGreaterThanOrEqual(14);
    // Every card explains what it costs before you click it.
    for (const c of catalogue) {
      expect(c.adds.length).toBeGreaterThan(10);
      expect(c.label.length).toBeGreaterThan(2);
    }
  });

  it("rolls all 15 cards live and captures each one", async () => {
    const catalogue = await invoke("cinema_catalog");
    let i = 0;
    for (const card of catalogue) {
      const take = await takeShot(statue, card.kind, String(i).padStart(2, "0"));
      takes.push(take);
      console.log(
        `[cam] ${card.kind.padEnd(13)} eye=${fmt(take.early.eye).padEnd(20)} d=${take.early.distance.toFixed(2).padStart(6)} → ${take.late.distance.toFixed(2).padStart(6)}  "${take.reads}"`,
      );
      i += 1;
    }
    expect(takes.length).toBeGreaterThanOrEqual(14);
  });

  it("EVERY card took the camera — none silently did nothing", async () => {
    const dead = takes.filter((t) => !t.early.cinematic || !t.late.cinematic);
    console.log(`[cam] cards that failed to take the camera: ${dead.map((d) => d.kind).join(", ") || "none"}`);
    expect(dead).toEqual([]);
  });

  it("EVERY card aimed at the subject — a shot that frames nothing is not a shot", async () => {
    const misses = takes
      .map((t) => ({ kind: t.kind, err: dist3(t.early.lookAt, SUBJECT) }))
      .filter((m) => m.err > 2.5);
    console.log(
      `[cam] aim errors: ${takes.map((t) => `${t.kind} ${dist3(t.early.lookAt, SUBJECT).toFixed(2)}`).join(" · ")}`,
    );
    expect(misses).toEqual([]);
  });

  it("EVERY card filmed from a DIFFERENT place — no two are the same shot", async () => {
    const clashes = [];
    for (let a = 0; a < takes.length; a += 1) {
      for (let b = a + 1; b < takes.length; b += 1) {
        const d = dist3(takes[a].early.eye, takes[b].early.eye);
        if (d < 0.3) clashes.push(`${takes[a].kind}≈${takes[b].kind} (${d.toFixed(2)})`);
      }
    }
    console.log(`[cam] indistinguishable pairs: ${clashes.join(", ") || "none"}`);
    expect(clashes).toEqual([]);
  });

  it("EVERY move moved the way its card promised", async () => {
    const by = Object.fromEntries(takes.map((t) => [t.kind, t]));
    const closes = (k) => by[k].late.distance < by[k].early.distance - 0.02;
    const backs = (k) => by[k].late.distance > by[k].early.distance + 0.02;
    const rises = (k) => by[k].late.eye[1] > by[k].early.eye[1] + 0.02;
    const drops = (k) => by[k].late.eye[1] < by[k].early.eye[1] - 0.02;
    const holds = (k) => dist3(by[k].late.eye, by[k].early.eye) < 0.02;
    const swings = (k) =>
      Math.hypot(by[k].late.eye[0] - by[k].early.eye[0], by[k].late.eye[2] - by[k].early.eye[2]) > 0.2;

    const failures = [];
    for (const k of ["hero", "looming", "confront"]) if (!closes(k)) failures.push(`${k} did not push in`);
    for (const k of ["establish", "pullback"]) if (!backs(k)) failures.push(`${k} did not pull out`);
    if (!rises("reveal")) failures.push("reveal did not crane up");
    for (const k of ["birdseye", "dropin"]) if (!drops(k)) failures.push(`${k} did not crane down`);
    for (const k of ["orbit", "sweep"]) if (!swings(k)) failures.push(`${k} did not orbit`);
    for (const k of ["closeup", "vista", "overshoulder", "detail"]) if (!holds(k)) failures.push(`${k} drifted`);

    console.log(`[cam] move failures: ${failures.join(" · ") || "none"}`);
    expect(failures).toEqual([]);
  });

  it("the size words are ordered — 'vista' really is further out than 'detail'", async () => {
    const by = Object.fromEntries(takes.map((t) => [t.kind, t.early.distance]));
    const ordered = ["vista", "establish", "hero", "orbit", "closeup", "detail"];
    console.log(`[cam] ${ordered.map((k) => `${k} ${by[k].toFixed(1)}`).join(" > ")}`);
    for (let i = 0; i + 1 < ordered.length; i += 1) {
      expect(by[ordered[i]]).toBeGreaterThan(by[ordered[i + 1]]);
    }
  });

  it("and the camera came all the way home afterwards", async () => {
    const back = await cam();
    console.log(`[cam] final: cinematic=${back.cinematic} eye=${fmt(back.eye)}`);
    expect(back.cinematic).toBe(false);
    expect((await invoke("cinema_list", { id: statue })).shots).toBe(0);
  });
});
