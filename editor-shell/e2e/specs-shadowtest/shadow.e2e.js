// LIVE M11.3 shadow quality-profile (Off tier) — capture a small shadow-casting scene so the run under
// MTK_SHADOW_QUALITY=high (ground shadows present) vs =off (no shadows, fully lit) can be compared. The
// label is suffixed by the active quality so the two runs don't clobber each other.

import { browser } from "@wdio/globals";
import { execSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import path from "node:path";

const SHOT_DIR = process.env.MTK_SHOT_DIR;
const SHOT_PS1 = process.env.MTK_SHOT_PS1;
const Q = process.env.MTK_SHADOW_QUALITY || "default";
const PROC = "metrocalk-editor-shell";
mkdirSync(SHOT_DIR, { recursive: true });

const invoke = (cmd, args = {}) =>
  browser.execute(async (c, a) => window.__TAURI__.core.invoke(c, a), cmd, args);

const shot = async (label) => {
  await browser.pause(700);
  const out = path.join(SHOT_DIR, `${label}_${Q}.png`);
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

describe("LIVE M11.3 — shadow quality profile (Off = min-spec)", () => {
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
    const dc = await invoke("demo_character").catch(() => null);
    if (Array.isArray(dc)) {
      const [root, parts] = dc;
      for (const pid of parts || []) await invoke("remove_entity", { id: pid }).catch(() => {});
      if (root) await invoke("remove_entity", { id: root }).catch(() => {});
    }
    await browser.pause(300);
  });

  it(`captures the seeded casters + ground (shadow quality = ${Q})`, async () => {
    await invoke("view_preset", { preset: "persp" });
    await invoke("frame_all");
    await shot("00_scene");
  });
});
