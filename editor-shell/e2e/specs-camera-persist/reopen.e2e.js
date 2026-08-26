// DID THE CAMERA COME BACK? (M11.4, ADR-154)
//
// A SECOND launch of the packaged `.exe`, on the session log the previous launch left behind — see
// `wdio.camera-persist.conf.js` for why this conf has no `onPrepare`. This spec CREATES NOTHING. If the
// camera is here, `Record::AddCamera` replayed; if its aim is here, the `look_at` field replayed with it;
// if its name is here, so did that. If any of them is missing, this fails, which is the only way to tell
// "the records exist" from "reopening works".
//
// The distinction matters more than it sounds. A camera used to carry a position and a field of view and
// no aim, so a reopened project showed every camera from a different distance at the same subject — the
// picture came back wrong while every count came back right.

import { browser } from "@wdio/globals";
import { execSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import path from "node:path";

const SHOT_DIR = process.env.MTK_SHOT_DIR;
const SHOT_PS1 = process.env.MTK_SHOT_PS1;
const PROC = "metrocalk-editor-shell";
if (SHOT_DIR) mkdirSync(SHOT_DIR, { recursive: true });

/** READ-ONLY, like `camera-ui.e2e.js`. This spec must not be able to create what it is looking for. */
const read = (cmd, args = {}) =>
  browser.execute(async (c, a) => window.__TAURI__.core.invoke(c, a), cmd, args);

const shot = async (label) => {
  if (!SHOT_DIR || !SHOT_PS1) return;
  await browser.pause(500);
  try {
    execSync(
      `powershell -ExecutionPolicy Bypass -File "${SHOT_PS1}" -Out "${path.join(SHOT_DIR, `${label}.png`)}" -ProcName "${PROC}"`,
      { stdio: "ignore" },
    );
    console.log("  shot", label);
  } catch (e) {
    console.error("shot failed", label, String(e));
  }
};

const countEntities = async () =>
  browser.execute(() => {
    const el = document.querySelector("#count");
    const m = (el?.textContent || "").match(/(\d+)\s+entities/);
    return m ? Number(m[1]) : Number.NaN;
  });

describe("a saved camera survives closing and reopening the app (ADR-154)", () => {
  before(async () => {
    await browser.waitUntil(async () => Number.isFinite(await countEntities()), {
      timeout: 30000,
      timeoutMsg: "editor never connected (#count empty)",
    });
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

  it("the camera authored in the previous launch is here, still active, with its lens", async () => {
    const [count, activePresent, fov] = await read("scene_camera_debug");
    console.log(`  after reopen: cameras=${count} active=${activePresent} fov=${fov}`);
    expect(count).toBeGreaterThanOrEqual(1);
    expect(activePresent).toBe(true);
    // The previous launch walked the lens down from 55, so a camera that came back at exactly 55 would
    // be one the replay re-authored from scratch rather than one whose edits replayed too.
    expect(fov).toBeLessThan(55);
  });

  it("and it still knows what it was pointed at — the field whose absence was the whole defect", async () => {
    const cameras = await read("scene_cameras");
    console.log("  cameras:", JSON.stringify(cameras));
    expect(Array.isArray(cameras)).toBe(true);
    expect(cameras.length).toBeGreaterThanOrEqual(1);
    const cam = cameras.find((c) => c.active) ?? cameras[0];
    expect(cam.name).toMatch(/^Camera \d+$/); // the name replayed, not a hex id
    expect(cam.lookAt).not.toBe(null); // the aim replayed
    // An aim of exactly the world origin on every axis is the unaimed fallback wearing the fix's clothes.
    const [ax, ay, az] = cam.lookAt;
    expect(Math.abs(ax) + Math.abs(ay) + Math.abs(az)).toBeGreaterThan(0);
    // And the clip planes came back solved from the stand-off, not as the legacy 0.1 / 500.
    expect(cam.near).toBeGreaterThan(0.1);
    expect(cam.far).toBeLessThan(500);
  });

  it("is reachable from the surfaces that draw it, not merely present in the document", async () => {
    // The store is filled by the View menu's mount-time read, so opening it is what proves the reopened
    // camera reaches the UI rather than only the engine.
    const view = await browser.$('[data-testid="vpView"]');
    await view.waitForExist({ timeout: 10000 });
    await view.click();
    await browser.pause(400);
    const rows = await browser.$$('[data-testid="vpCamera"]');
    expect(rows.length).toBeGreaterThanOrEqual(1);
    const label = await rows[0].getText();
    console.log("  menu row after reopen:", label.replace(/\s+/g, " "));
    expect(/mm/.test(label)).toBe(true);
    expect((await browser.$$('[data-testid="vpNoCameras"]')).length).toBe(0);
    await shot("reopen-01-camera-still-in-the-menu");
  });
});
