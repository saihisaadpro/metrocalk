// LIVE camera look-through (M11.4, ADR-043): author a scene Camera entity at a distinct pose, then LOOK
// THROUGH it — the rendered viewport must jump from the editor fly-cam to the camera's view, and revert
// when toggled off. The viewport is native wgpu under the transparent WebView2, so an OS-level PrintWindow
// captures the composite after each state. Drives `add_camera` / `look_through_camera` / `scene_camera_debug`
// — the same commands a future React "Cameras" panel will feed.

import { browser } from "@wdio/globals";
import { execSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import path from "node:path";

const SHOT_DIR = process.env.MTK_SHOT_DIR;
const SHOT_PS1 = process.env.MTK_SHOT_PS1; // capture.ps1 (-Out, -ProcName)
const PROC = "metrocalk-editor-shell";
mkdirSync(SHOT_DIR, { recursive: true });

const invoke = (cmd, args = {}) =>
  browser.execute(async (c, a) => window.__TAURI__.core.invoke(c, a), cmd, args);

const shot = async (label) => {
  await browser.pause(550); // let the render loop pick up the new state + present a few frames
  const out = path.join(SHOT_DIR, `${label}.png`);
  try {
    execSync(`powershell -ExecutionPolicy Bypass -File "${SHOT_PS1}" -Out "${out}" -ProcName "${PROC}"`, {
      stdio: "ignore",
    });
    console.log("  shot", out);
  } catch (e) {
    console.error("shot failed", label, String(e));
  }
};

const countEntities = async () => {
  try {
    const el = await browser.$("#count");
    const m = (await el.getText()).match(/(\d+)\s+entities/);
    return m ? Number(m[1]) : NaN;
  } catch {
    return NaN;
  }
};

describe("LIVE camera look-through (M11.4 visual verification)", () => {
  before(async () => {
    await browser.waitUntil(async () => Number.isFinite(await countEntities()), {
      timeout: 30000,
      timeoutMsg: "editor never connected (#count empty)",
    });
    // Dismiss the first-run onboarding overlay (it blocks the lower viewport).
    await browser.execute(() => {
      try {
        localStorage.setItem("mtk.onboarded.v1", "1");
      } catch (e) {
        void e;
      }
      const b = [...document.querySelectorAll("button")].find((x) => /skip/i.test(x.textContent || ""));
      if (b) b.click();
    });
    await browser.pause(300);
  });

  it("add a scene camera at a distinct pose → look through it → the view changes, then reverts", async () => {
    // Known fly-cam baseline: standard perspective, whole scene framed.
    await invoke("view_preset", { preset: "persp" });
    await invoke("frame_all");
    await shot("00_flycam_baseline");

    // Author a scene camera at a LOW, off-centre pose — maximally different from the elevated framed persp.
    const before = await countEntities();
    const id = await invoke("add_camera", { x: 28, y: 2, z: 0, fov: 55, active: true });
    if (typeof id !== "string") throw new Error("add_camera returned " + JSON.stringify(id));
    const grew = (await countEntities()) > before;

    // The add is one authored entity; the VIEW is unchanged until we look through (proves the camera is a
    // scene object, not the editor cam).
    const [count, activePresent, fov] = await invoke("scene_camera_debug");
    await shot("01_camera_added_view_unchanged");

    // Look through → the viewport jumps to the camera's pose + fov.
    const found = await invoke("look_through_camera", { on: true });
    await shot("02_looking_through_camera");

    // Toggle off → back to the editor fly-cam (same as the baseline).
    const back = await invoke("look_through_camera", { on: false });
    await shot("03_back_to_flycam");

    console.log(
      `  signals: grew=${grew} count=${count} activePresent=${activePresent} fov=${fov} found=${found} back=${back}`
    );

    expect(grew).toBe(true); // one new authored camera entity
    expect(count).toBeGreaterThanOrEqual(1);
    expect(activePresent).toBe(true);
    expect(Math.abs(fov - 55) < 0.01).toBe(true);
    expect(found).toBe(true); // an active camera was found to look through
    expect(back).toBe(true);
  });

  it("Play renders through the active scene camera; Stop restores the editor fly-cam", async () => {
    // Known editor view first (the prior test left look-through off, but be explicit + self-contained).
    await invoke("look_through_camera", { on: false });
    await invoke("view_preset", { preset: "persp" });
    await invoke("frame_all");
    await shot("04_play_pre_flycam");

    // The active camera authored in the prior test persists this session — confirm it's there.
    const [, activePresent] = await invoke("scene_camera_debug");
    expect(activePresent).toBe(true);

    // Play → the viewport must switch to the active scene camera's view (Play-renders-active, ADR-043).
    const p = await invoke("play");
    await shot("05_playing_renders_through_camera");
    expect(p.playing).toBe(true);

    // Stop → non-destructive restore of the editor fly-cam view.
    const s = await invoke("stop");
    await shot("06_stopped_back_to_flycam");
    expect(s.playing).toBe(false);

    console.log(`  play=${JSON.stringify(p)} stop=${JSON.stringify(s)}`);
  });
});
