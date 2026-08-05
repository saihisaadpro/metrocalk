// LIVE M11.4 bloom — set up a bright chrome scene (high exposure + a strong light) and capture it so the run
// under MTK_BLOOM=on (glow on bright highlights) vs =off can be compared. Label suffixed by MTK_BLOOM.

import { browser } from "@wdio/globals";
import { execSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import path from "node:path";

const SHOT_DIR = process.env.MTK_SHOT_DIR;
const SHOT_PS1 = process.env.MTK_SHOT_PS1;
const FIX = process.env.MTK_FIX_DIR;
const B = process.env.MTK_BLOOM || "default";
const PROC = "metrocalk-editor-shell";
mkdirSync(SHOT_DIR, { recursive: true });

const invoke = (cmd, args = {}) =>
  browser.execute(async (c, a) => window.__TAURI__.core.invoke(c, a), cmd, args);

const shot = async (label) => {
  await browser.pause(700);
  const out = path.join(SHOT_DIR, `${label}_bloom${B}.png`);
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

describe("LIVE M11.4 — bloom post-processing", () => {
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
    await browser.pause(1500); // let the window reach full size before any PrintWindow capture
  });

  it(`captures a bright chrome scene (bloom = ${B})`, async () => {
    const cube = await invoke("import_asset", { path: path.join(FIX, "cube.glb") });
    if (typeof cube === "string") await invoke("ai_edit", { id: cube, material: "chrome" });
    // A strong light + high exposure so highlights cross the bloom threshold.
    await invoke("add_light", { kind: "directional", x: 4, y: 8, z: 3, r: 1, g: 1, b: 1, intensity: 5 });
    await invoke("set_exposure", { exposure: 3.0 });
    await invoke("view_preset", { preset: "persp" });
    await invoke("frame_all");
    await shot("00_chrome");
  });
});
