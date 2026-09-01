// ADR-197 — the engine saying that it could not find anywhere to film a CARD shot from, on the
// packaged `.exe`.
//
// ADR-195 closed the half of this that is about a camera the AUTHOR placed. This is the other half,
// and it is the larger one: for a card shot the ENGINE chooses the placement, walking a fifty-four
// rung ladder of framings and yaws and judging each at five points along its move, and when nothing
// on that ladder is acceptable it films the least bad one. That is the right decision — a shot has to
// be filmed from somewhere — and it was completely silent. Nine of the production factory film's
// thirty shots were filmed at a placement the engine itself scored `acceptable: false`, and those are
// exactly its remaining illegible seconds.
//
// WHY THIS FILE CANNOT BE PASSED BY RELABELLING ANYTHING. Every claim is read from either
// `cinema_list().problems` — the sentence the panel draws — or `camera_probe`, which reports where
// the renderer's own camera actually is. The four questions, in the order a person would ask them:
//
//   1. Does an ordinary card shot in an open world say anything?     It must NOT. The negative
//      control, first, because every message below has to be caused by the world objecting rather
//      than by a card shot existing.
//   2. Enclose the subject: does the engine say so?                  In the author's words, naming
//      the shot, with no mechanism named and no percentage of a thing they cannot see.
//   3. Does it still film the shot, from the least bad place?         It must. This is a NOTE, not a
//      refusal: the camera still moves, the cutscene still plays, nothing is disabled.
//   4. Take the obstruction away: does it go quiet?                   Or the warning is about a card
//      shot existing, not about the world.
//
// ...and the fifth, which is the one that makes the other four evidence about the product rather than
// about this session: the warning is DERIVED, so it must come back by itself after Save/New/Open
// without anything having been written into the document.
//
// Local-only, for the standing reason the rest of this directory is: a display, a WebView2-matched
// `msedgedriver`, and a GPU the wgpu surface can be created on.

import { existsSync, mkdirSync, rmSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const dir = path.dirname(fileURLToPath(import.meta.url));
const evidence = path.resolve(dir, "../.shots-nowhere");
mkdirSync(evidence, { recursive: true });

const captureFrame = async (name) => {
  const reply = await invoke("viewport_capture", { path: path.join(evidence, name) });
  expect(reply.reason).toBeFalsy();
  return reply;
};

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

/** The warning SENTENCES off a cinema reply.
 *
 *  ADR-200 made each entry `{ shot, message }`, because a panel drawing a bare string could name the
 *  shot in prose and act on nothing. This spec asserts the prose here and the field where it matters. */
const said_of = (reply) => reply.problems.map((p) => p.message);

const panelProblems = () =>
  browser.execute(() =>
    [...document.querySelectorAll('[data-testid="cutscene-problem"]')].map((n) => n.textContent ?? ""),
  );

let opN = 0;
/** Move an object the way the editor does — one ordinary `setField` transaction. */
const moveTo = async (id, x, y, z) => {
  for (const [field, value] of [["x", x], ["y", y], ["z", z]]) {
    opN += 1;
    const tx = {
      clientOpId: `nw-op-${opN}`,
      label: `set Transform.${field}`,
      patches: [],
      intent: { kind: "setField", id, component: "Transform", field, value },
    };
    await browser.execute(async (t) => window.__TAURI__.core.invoke("submit_edit", { tx: t }), tx);
  }
  await browser.pause(250);
};

/** The pose the RUNTIME films this shot from, at its opening instant — the same solver Play runs,
 *  which is the only reason these numbers are evidence about the film rather than about a panel. */
const filmedPose = async (id, capture = null) => {
  await invoke("cinema_preview", { id, seconds: 0, active: true });
  await browser.pause(450);
  const probe = await invoke("camera_probe");
  expect(probe.cinematic).toBe(true);
  if (capture) await captureFrame(capture);
  await invoke("cinema_preview", { id, seconds: 0, active: false });
  await browser.pause(250);
  return probe;
};

const finite = (v) => v.every((n) => Number.isFinite(n));
const dist = (a, b) => Math.hypot(a[0] - b[0], a[1] - b[1], a[2] - b[2]);
const note = (line) => console.log(line);
const xyz = (v) => v.map((n) => n.toFixed(2)).join(", ");

describe("ADR-197 · nowhere good to film from", () => {
  let subject;
  /** The shell that swallows every placement the ladder can reach. Spawned in test 2, gone in test 4. */
  let shell;
  const HOME = [0, 1.6, 0];
  /** How big the shell has to be, and the number is load-bearing.
   *
   *  The ladder widens as well as swinging, and `ExtremeWide` on a metre-scale subject solves a
   *  stand-off around 30 m. A shell that only swallowed the authored framing would be escaped by a
   *  wider candidate, the planner would come back with an ACCEPTABLE placement, and this spec would
   *  fail while the engine did exactly the right thing. 120 m puts every rung inside it. */
  const SHELL_M = 120;

  before(async () => {
    await browser.waitUntil(
      async () => browser.execute(() => !!(window.__TAURI__ && window.__TAURI__.core)),
      { timeout: 30000 },
    );
    await click('[data-testid="stop"]');
    await browser.pause(500);
    await browser.execute(() => {
      document
        .querySelector('[data-testid="onboardSkip"]')
        ?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
  });

  it("says nothing about a card shot in an open world", async () => {
    // THE NEGATIVE CONTROL, and it is the whole reason the rest of this file is evidence. A close
    // shot on a lone capsule is negotiated against a world with nothing in it, so the ladder's first
    // rung is accepted and there is nothing to say.
    subject = (await invoke("shape_spawn", { kind: "capsule", pos: HOME })).created;
    await invoke("frame_all");
    expect(subject).toBeTruthy();

    await click('[data-testid="engine-gameplay"]');
    await browser.pause(500);
    expect(await selectRow(subject)).toBe(true);
    await (await $('[data-testid="cinema-section"]')).waitForExist({ timeout: 10000 });
    // Through the panel's own catalogue, not through a command: this is the gesture an author makes.
    expect(await click('[data-testid="shot-hero"]')).toBe(true);
    await browser.waitUntil(async () => (await invoke("cinema_list", { id: subject })).shots === 1, {
      timeout: 15000,
      timeoutMsg: "the shot never landed",
    });

    const said = said_of(await invoke("cinema_list", { id: subject }));
    note(`[open world] ${JSON.stringify(said)}`);
    expect(said.some((p) => /nowhere good to film from/.test(p))).toBe(
      false,
    );

    // Open the panel that draws these, so the later tests can read the SURFACE and not only the reply.
    if (!(await browser.execute(() => !!document.querySelector("#bottom-workspaces-animation-tab")))) {
      await click('[data-testid="bottom-dock-toggle"]');
      await browser.pause(400);
    }
    expect(await click("#bottom-workspaces-animation-tab")).toBe(true);
    await browser.pause(600);
    expect(await clickTabNamed("#bottom-workspaces-animation-panel", "Cutscene")).toBe(true);
    await (await $('[data-testid="cutscene-timeline"]')).waitForExist({ timeout: 15000 });

    await invoke("view_preset", { preset: "iso" });
    await browser.pause(300);
    await invoke("focus_entity", { id: subject });
    await browser.pause(400);
    await captureFrame("1-open-world.png");
  });

  it("says so when there is nowhere on the ladder to stand", async () => {
    // A shell around the subject, large enough that every rung of the ladder — six framings crossed
    // with nine yaws, judged at five progresses — puts the lens inside it. It is a SEPARATE entity,
    // so it is never part of the subject and never excluded from the obstruction test; that is the
    // difference between "the camera is close to what it is filming" and "the camera is in a wall".
    //
    // A shape rests on the ground in its own local frame, so the block's origin sits half its height
    // below the subject for the subject to be at its centre.
    shell = (await invoke("shape_spawn", { kind: "box", pos: [0, 0, 0] })).created;
    await invoke("shape_update", { id: shell, params: { width: SHELL_M, height: SHELL_M, depth: SHELL_M } });
    await moveTo(shell, HOME[0], HOME[1] - SHELL_M / 2, HOME[2]);
    await browser.pause(600);

    const reply = await invoke("cinema_list", { id: subject });
    const said = said_of(reply);
    note(`[enclosed] ${JSON.stringify(said)}`);
    const stuck = said.find((p) => /nowhere good to film from/.test(p));
    expect(stuck).toBeTruthy();
    // ADR-200 — AND THE SHOT NUMBER IS A FIELD, not only a word in the prose. This is what a control
    // beside the sentence acts on; without it the panel holds a string and can offer nothing.
    const entry = reply.problems.find((p) => /nowhere good to film from/.test(p.message));
    expect(entry.shot).toBe(0);

    // THE AUTHOR'S WORDS. It names the shot, says what is wrong, and points at the one control that
    // is left — never at the size or the angle, which is what the engine has already tried on every
    // rung of the ladder on the author's behalf.
    expect(stuck).toMatch(/^shot 1 /);
    expect(stuck).toMatch(/Shoot from this view/);
    // ADR-200 — and the FIRST thing it offers is a place, not an instruction to go and find one.
    expect(stuck).toMatch(/Take me there/);
    expect(stuck).not.toMatch(/change the size or the angle/);
    // ...and no engine vocabulary reaches the panel.
    expect(said.join(" ")).not.toMatch(/eye_inside|vantage|Vantage|acceptable|clear:|crowded/);

    // THE SURFACE, not only the reply: the sentence is drawn in the same warning list as the
    // continuity notes, which is the "one place the author looks" claim made checkable.
    await browser.pause(600);
    const drawn = await panelProblems();
    note(`[panel] ${JSON.stringify(drawn)}`);
    expect(drawn.some((p) => /nowhere good to film from/.test(p))).toBe(true);
    await captureFrame("2-nowhere-to-film.png");
  });

  it("is a note and not a refusal — the shot is still filmed, from the least bad place", async () => {
    // The engine's decision has not changed and must not: a shot has to be filmed from somewhere, so
    // the camera still moves to a real, finite pose and the cutscene still plays. A warning that
    // silently stopped filming the shot would be a far worse failure than the silence it replaced.
    const pose = await filmedPose(subject, "3-filmed-anyway.png");
    note(`[filmed] eye ${xyz(pose.eye)} -> ${xyz(pose.look_at)}`);
    expect(finite(pose.eye)).toBe(true);
    expect(finite(pose.look_at)).toBe(true);
    // Aimed at the thing it is about, however bad the view of it is. A loose bound on purpose: this
    // is a sanity check that the camera is still pointed at the subject, not a claim about framing,
    // and a spawned shape rests on the ground so its centre is not exactly `HOME`.
    expect(dist(pose.look_at, HOME)).toBeLessThan(5);
    // The shot is still in the document, at its authored length, with nothing removed or disabled.
    const cut = await invoke("cinema_list", { id: subject });
    expect(cut.shots).toBe(1);
    expect(cut.rows[0].camera).toBeFalsy();
    expect(cut.reason).toBeFalsy();
  });

  it("takes the author to the placement it gave up on, and stores it from there", async () => {
    // ADR-200 — THE ADVICE, EXECUTED. The sentence above says *press "Take me there" to stand where
    // it tried, then frame it yourself with "Shoot from this view"*. This is that sentence carried
    // out through the two commands the two buttons send, on the packaged binary, against the
    // 15,711-part-scale machinery that produced the warning — which is the only way to know the
    // advice composes rather than merely reading well.
    const there = await invoke("cinema_stand_at_shot", { id: subject, index: 0 });
    note(`[stood] ${JSON.stringify(there)}`);
    expect(there.reason).toBeFalsy();
    expect(there.moved).toBe(true);
    // The engine took the author to the placement it SETTLED on, and says so: unacceptable, and a
    // whole ladder's worth of rejected candidates behind it. A "take me there" that landed on the
    // authored placement would be pointing at a frame the search never chose.
    expect(there.acceptable).toBe(false);
    expect(there.steps).toBeGreaterThan(0);
    expect(there.placed).toBe(false);
    expect(finite(there.stood)).toBe(true);
    expect(dist(there.lookAt, HOME)).toBeLessThan(5);

    // THE VIEWPORT ITSELF, not the reply's own claim about it. `camera_probe` reads the renderer's
    // camera, and `cinematic` is false because this is the author's own orbit camera — the whole
    // difference from a preview, which takes the camera away again on the next tick.
    const probe = await invoke("camera_probe");
    note(`[probe] eye ${xyz(probe.eye)} -> ${xyz(probe.lookAt)} cinematic=${probe.cinematic}`);
    expect(probe.cinematic).toBe(false);
    expect(dist(probe.eye, there.stood)).toBeLessThan(0.01);
    expect(dist(probe.lookAt, there.lookAt)).toBeLessThan(0.01);
    await captureFrame("5-take-me-there.png");

    // ...and "Shoot from this view" from here stores exactly this pose. That is the round trip the
    // warning's advice describes: the author's manual search now STARTS at the engine's answer
    // instead of at wherever the viewport happened to be.
    const stored = await invoke("cinema_set_shot_camera", { id: subject, index: 0 });
    expect(stored.reason).toBeFalsy();
    const camera = stored.rows[0].camera;
    expect(camera).toBeTruthy();
    expect(dist(camera.eye, there.stood)).toBeLessThan(0.01);
    expect(dist(camera.lookAt, there.lookAt)).toBeLessThan(0.01);

    // Give the shot back to its card, so the tests after this one are still about a CARD shot —
    // which is the only kind `card_shot_problems` speaks about at all.
    await invoke("cinema_clear_shot_camera", { id: subject, index: 0 });
    await browser.pause(400);
    expect((await invoke("cinema_list", { id: subject })).rows[0].camera).toBeFalsy();
  });

  it("goes quiet when the obstruction goes away", async () => {
    // ...so the warning is a statement about the world, not about the shot having been made from a
    // card. Same shot, same subject, same panel — one entity removed.
    await invoke("remove_entity", { id: shell });
    await browser.pause(700);
    const cleared = said_of(await invoke("cinema_list", { id: subject }));
    note(`[cleared] ${JSON.stringify(cleared)}`);
    expect(cleared.some((p) => /nowhere good to film from/.test(p))).toBe(false);
    await browser.pause(500);
    expect((await panelProblems()).some((p) => /nowhere good to film from/.test(p))).toBe(false);
  });

  it("comes back by itself after Save, New and Open — it is derived, not stored", async () => {
    // THE PROPERTY THAT MAKES IT A CAPABILITY RATHER THAN A SESSION. Nothing about this warning is
    // written into the document: it is the world's answer about the shot, recomputed. So a reopened
    // project must produce it again from the geometry alone — and if it did NOT come back, the
    // silence would look exactly like a scene with nothing wrong in it.
    const again = (await invoke("shape_spawn", { kind: "box", pos: [0, 0, 0] })).created;
    await invoke("shape_update", { id: again, params: { width: SHELL_M, height: SHELL_M, depth: SHELL_M } });
    await moveTo(again, HOME[0], HOME[1] - SHELL_M / 2, HOME[2]);
    await browser.pause(600);
    expect(
      said_of(await invoke("cinema_list", { id: subject })).some((p) =>
        /nowhere good to film from/.test(p),
      ),
    ).toBe(true);

    // Beside this spec's own evidence, NOT beside the .exe: `CARGO_TARGET_DIR` can put the binary
    // anywhere, and a save into a directory that does not exist fails as `os error 3`.
    const file = path.join(evidence, "nowhere-to-film.mtk");
    if (existsSync(file)) rmSync(file, { force: true });
    await invoke("save_project", { path: file });
    await browser.waitUntil(async () => existsSync(file), {
      timeout: 20000,
      timeoutMsg: "no .mtk was written",
    });
    // NEW FIRST. Re-opening over a live document could pass on an engine that saved nothing at all.
    await invoke("new_project");
    await browser.pause(900);
    expect((await invoke("cinema_list", { id: subject })).shots).toBe(0);
    await invoke("open_project", { path: file });
    await browser.pause(1500);
    expect((await invoke("cinema_list", { id: subject })).shots).toBe(1);

    const reopened = said_of(await invoke("cinema_list", { id: subject }));
    note(`[reopened] ${JSON.stringify(reopened)}`);
    expect(reopened.some((p) => /nowhere good to film from/.test(p))).toBe(true);
    await captureFrame("4-after-reopen.png");
  });
});
