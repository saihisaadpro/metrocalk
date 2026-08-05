// LIVE M11.4 marker icons — a light + a camera + a real cube. The light should render as a warm wireframe
// burst, the camera as a cyan wireframe frustum (NOT solid placeholder cubes); the cube is real geometry.

import { browser } from "@wdio/globals";
import { execSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import path from "node:path";

const SHOT_DIR = process.env.MTK_SHOT_DIR;
const SHOT_PS1 = process.env.MTK_SHOT_PS1;
const FIX = process.env.MTK_FIX_DIR;
const PROC = "metrocalk-editor-shell";
mkdirSync(SHOT_DIR, { recursive: true });

const invoke = (cmd, args = {}) =>
  browser.execute(async (c, a) => window.__TAURI__.core.invoke(c, a), cmd, args);

const shot = async (label) => {
  await browser.pause(700);
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

describe("LIVE M11.4 — light/camera marker icons", () => {
  before(async () => {
    await browser.waitUntil(async () => Number.isFinite(await countEntities()), {
      timeout: 30000,
      timeoutMsg: "editor never connected",
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
    const dc = await invoke("demo_character").catch(() => null);
    if (Array.isArray(dc)) {
      const [root, parts] = dc;
      for (const pid of parts || []) await invoke("remove_entity", { id: pid }).catch(() => {});
      if (root) await invoke("remove_entity", { id: root }).catch(() => {});
    }
    await browser.pause(1500);
  });

  it("a light renders as a burst icon and a camera as a frustum icon, not solid cubes", async () => {
    // Light (left) + camera (right) flanking the origin, front view — both glyphs clearly in frame. No real
    // mesh here, so the ONLY things on screen are the two icons: a solid marker cube would be unmistakable.
    await invoke("add_light", { kind: "point", x: -1.6, y: 1.2, z: 0, r: 1, g: 0.9, b: 0.6, intensity: 4 });
    await invoke("add_camera", { x: 1.6, y: 1.2, z: 0, fov: 50, active: false });
    await invoke("view_preset", { preset: "front" });
    await invoke("frame_all");
    const [, , , ] = [0, 0, 0, 0];
    const sc = await invoke("scene_camera_debug").catch(() => null);
    console.log("  scene_camera_debug:", JSON.stringify(sc), "count:", await countEntities());
    await shot("00_markers");
  });
});
