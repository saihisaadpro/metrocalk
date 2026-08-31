// ADR-192 — a camera the author PLACED, on the packaged `.exe`.
//
// Every shot before this one was a CARD: one of six sizes crossed with one of six angles, solved
// relative to the subject's own facing, negotiated against the scene and re-solved every tick. That
// vocabulary is why the first click looks good and it could not express the most basic gesture in
// cinematography — orbit until the frame is the one you want, then shoot THAT.
//
// THE ASSERTION THAT CANNOT BE PASSED BY RELABELLING IS THE CAMERA. Every step below reads
// `camera_probe`, which reports where the renderer's camera actually is, and compares three poses:
//
//   V — where the viewport was standing when the author pressed the button;
//   C — where this shot's CARD films from, measured by previewing it before the gesture;
//   P — where the cutscene runtime films the shot from AFTER the gesture.
//
// The claim is `P == V` and `P` far from `C`. A feature that stored a pose, captioned a row and
// changed no pixels would pass a DOM assertion and fail this one.

import { invoke } from "../pages/scaffold.js";

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

const text = (selector) =>
  browser.execute((sel) => document.querySelector(sel)?.textContent ?? null, selector);

const isDisabled = (selector) =>
  browser.execute((sel) => {
    const el = document.querySelector(sel);
    return el ? el.disabled === true || el.getAttribute("aria-disabled") === "true" : null;
  }, selector);

const dist = (a, b) => Math.hypot(a[0] - b[0], a[1] - b[1], a[2] - b[2]);
const note = (line) => console.log(line);

describe("ADR-192 · shoot from this view", () => {
  let subject;
  /** Where the shot's CARD films from — measured, not assumed. */
  let card;
  /** Where the viewport was standing when the button was pressed. */
  let view;

  before(async () => {
    await browser.waitUntil(
      async () => browser.execute(() => !!(window.__TAURI__ && window.__TAURI__.core)),
      { timeout: 30000 },
    );
    await click('[data-testid="stop"]');
    await browser.pause(500);
    await browser.execute(() => {
      document.querySelector('[data-testid="onboardSkip"]')?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
  });

  it("authors a shot from a card, and MEASURES where that card films from", async () => {
    subject = (await invoke("shape_spawn", { kind: "capsule", pos: [0, 1.6, 0] })).created;
    await invoke("shape_spawn", { kind: "cylinder", pos: [6, 0.2, -4] });
    await invoke("frame_all");
    expect(subject).toBeTruthy();

    await click('[data-testid="engine-gameplay"]');
    await browser.pause(500);
    expect(await selectRow(subject)).toBe(true);
    await (await $('[data-testid="cinema-section"]')).waitForExist({ timeout: 10000 });
    expect(await click('[data-testid="shot-hero"]')).toBe(true);
    await browser.waitUntil(async () => (await invoke("cinema_list", { id: subject })).shots === 1, {
      timeout: 15000,
      timeoutMsg: "the shot never landed",
    });

    // The card's pose, from the runtime that films it — `cinema_preview` is the SAME solver Play
    // uses, which is the only reason this number is evidence about the film and not about a preview.
    await invoke("cinema_preview", { id: subject, seconds: 0, active: true });
    await browser.pause(400);
    card = (await invoke("camera_probe")).eye;
    note(`[card] the hero card films from ${card.map((n) => n.toFixed(2)).join(", ")}`);
    await invoke("cinema_preview", { id: subject, seconds: 0, active: false });
    await browser.pause(300);
  });

  it("refuses to store a lens from a view that has none, and says which view", async () => {
    // `view_preset` switches the stage to a PARALLEL projection for the axis views, and a cutscene
    // camera is perspective by construction. A silent store here would hand back a picture with
    // vanishing points where the author was reading a plan.
    await invoke("view_preset", { preset: "top" });
    await browser.pause(300);
    const refused = await invoke("cinema_set_shot_camera", { id: subject, index: 0 });
    expect(refused.reason).toBeTruthy();
    expect(refused.reason).toMatch(/no lens/i);
    note(`[refusal] ${refused.reason}`);
    // Nothing was written.
    expect((await invoke("cinema_list", { id: subject })).rows[0].camera).toBe(null);
  });

  it("films the shot from EXACTLY the view the author was looking at", async () => {
    // Back to a perspective view, then somewhere the card would never choose: aimed at the OTHER
    // object entirely, six metres away from the subject this shot frames.
    await invoke("view_preset", { preset: "iso" });
    await browser.pause(300);
    await invoke("focus_entity", { id: subject });
    await browser.pause(300);
    const probe = await invoke("camera_probe");
    view = probe.eye;
    note(`[view] the stage is standing at ${view.map((n) => n.toFixed(2)).join(", ")}, ${probe.fovDeg}deg lens`);

    // ── the gesture, through the panel's own button ──────────────────────────────────────────────
    if (!(await browser.execute(() => !!document.querySelector("#bottom-workspaces-animation-tab")))) {
      await click('[data-testid="bottom-dock-toggle"]');
      await browser.pause(400);
    }
    expect(await click("#bottom-workspaces-animation-tab")).toBe(true);
    await browser.pause(600);
    expect(await clickTabNamed("#bottom-workspaces-animation-panel", "Cutscene")).toBe(true);
    await (await $('[data-testid="cutscene-timeline"]')).waitForExist({ timeout: 15000 });
    expect(await click('[data-testid="cutscene-clip"]')).toBe(true);
    await (await $('[data-testid="cutscene-shoot-here"]')).waitForExist({ timeout: 10000 });
    expect(await click('[data-testid="cutscene-shoot-here"]')).toBe(true);

    await browser.waitUntil(
      async () => !!(await invoke("cinema_list", { id: subject })).rows[0].camera,
      { timeout: 15000, timeoutMsg: "the pose never reached the document" },
    );

    // ── the stored pose IS the view ─────────────────────────────────────────────────────────────
    const stored = (await invoke("cinema_list", { id: subject })).rows[0].camera;
    expect(dist(stored.eye, view)).toBeLessThan(0.01);
    // The LENS the viewport draws through — 55 degrees, not the cutscene runtime's 50 and not the
    // bare 45 `camera_probe` reported for a projection that has never used one.
    expect(Math.abs(stored.fovDeg - probe.fovDeg)).toBeLessThan(0.01);

    // ── and the pose the RUNTIME films from is that one ──────────────────────────────────────────
    await invoke("cinema_preview", { id: subject, seconds: 0, active: true });
    await browser.pause(500);
    const filmed = await invoke("camera_probe");
    note(`[placed] the shot now films from ${filmed.eye.map((n) => n.toFixed(2)).join(", ")}`);
    expect(dist(filmed.eye, view)).toBeLessThan(0.05);
    expect(filmed.cinematic).toBe(true);

    // ── THE NEGATIVE CONTROL, and it is the whole test ──────────────────────────────────────────
    // Without this, a shot that ignored the stored pose and happened to solve near the viewport
    // would pass every line above.
    const moved = dist(filmed.eye, card);
    note(`[delta] the placed shot films ${moved.toFixed(2)} units from where its card would`);
    expect(moved).toBeGreaterThan(1.0);

    await invoke("cinema_preview", { id: subject, seconds: 0, active: false });
    await browser.pause(300);
  });

  it("says so in the panel, and stops the two controls that no longer decide anything", async () => {
    expect(await text('[data-testid="cutscene-shot-reads"]')).toMatch(/a placed shot of/);
    expect(await text('[data-testid="cutscene-placed-pose"]')).toMatch(/looking at/);
    expect(await isDisabled('[data-testid="cutscene-size"]')).toBe(true);
    expect(await isDisabled('[data-testid="cutscene-angle"]')).toBe(true);
    // ...while the move and its strength stay live: "put the camera here and push in" is a sentence
    // this engine can film, and that is what makes a placed camera a shot rather than an override.
    expect(await isDisabled('[data-testid="cutscene-motion"]')).toBe(false);
    expect(await isDisabled('[data-testid="cutscene-amount"]')).toBe(false);
    // The lane too — "Wide pushing in" over a hand-framed shot is the same untruth one line up.
    expect(await text('[data-testid="cutscene-clip"]')).toMatch(/Placed/);
  });

  it("still moves: a push-in from a placed camera closes its own stand-off", async () => {
    const stored = (await invoke("cinema_list", { id: subject })).rows[0].camera;
    const range = dist(stored.eye, stored.lookAt);
    await invoke("cinema_set_shot_framing", {
      id: subject,
      index: 0,
      edit: { motion: "push_in", amount: 0.5 },
    });
    await invoke("cinema_preview", { id: subject, seconds: 0, active: true });
    await browser.pause(400);
    const open = (await invoke("camera_probe")).distance;
    const cut = await invoke("cinema_list", { id: subject });
    await invoke("cinema_preview", { id: subject, seconds: cut.seconds * 0.999, active: true });
    await browser.pause(400);
    const end = (await invoke("camera_probe")).distance;
    note(`[move] a 0.5 push-in on a ${range.toFixed(2)}-unit stand-off ran ${open.toFixed(2)} -> ${end.toFixed(2)}`);
    expect(end).toBeLessThan(open * 0.75);
    await invoke("cinema_preview", { id: subject, seconds: 0, active: false });
    await browser.pause(300);
  });

  it("gives the shot back to its card, and films from the card again", async () => {
    expect(await click('[data-testid="cutscene-use-card"]')).toBe(true);
    await browser.waitUntil(
      async () => (await invoke("cinema_list", { id: subject })).rows[0].camera === null,
      { timeout: 15000, timeoutMsg: "the camera never cleared" },
    );
    // The card it goes back to is the one it was authored with — nothing was overwritten while the
    // camera was placed.
    const row = (await invoke("cinema_list", { id: subject })).rows[0];
    expect(row.angle).toBe("three_quarter");
    expect(await isDisabled('[data-testid="cutscene-size"]')).toBe(false);

    // AND THE PICTURE FOLLOWS. The move was changed to a push-in above, so this is compared at the
    // shot's opening instant, where a push-in has not yet travelled.
    await invoke("cinema_preview", { id: subject, seconds: 0, active: true });
    await browser.pause(500);
    const back = (await invoke("camera_probe")).eye;
    note(`[cleared] back to ${back.map((n) => n.toFixed(2)).join(", ")}, card was ${card.map((n) => n.toFixed(2)).join(", ")}`);
    expect(dist(back, card)).toBeLessThan(0.5);
    expect(dist(back, view)).toBeGreaterThan(1.0);
    await invoke("cinema_preview", { id: subject, seconds: 0, active: false });
  });
});
