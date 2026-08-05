// LIVE M11.2 texture visual — import a full-PBR demo tile (solid base · metallic-roughness split ·
// rippled normal map) and capture the composited window from a few angles. The normal map should read as
// a quilted relief on the flat quad under the scene light; the MR map should split it into a smooth-metal
// half (reflects the IBL sky) and a rough-dielectric half (matte). Proof that MR + normal maps drive the
// shading, not just base-color.

import { browser } from "@wdio/globals";
import { execSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import path from "node:path";

const SHOT_DIR = process.env.MTK_SHOT_DIR;
const SHOT_PS1 = process.env.MTK_SHOT_PS1;
const GLB = process.env.MTK_TEX_GLB;
const PROC = "metrocalk-editor-shell";
mkdirSync(SHOT_DIR, { recursive: true });

const invoke = (cmd, args = {}) =>
  browser.execute(async (c, a) => window.__TAURI__.core.invoke(c, a), cmd, args);

let opN = 0;
const setField = async (id, field, value) => {
  opN += 1;
  const tx = {
    clientOpId: `tex-op-${opN}`,
    label: `set Transform.${field}`,
    patches: [],
    intent: { kind: "setField", id, component: "Transform", field, value },
  };
  await browser.execute(async (t) => window.__TAURI__.core.invoke("submit_edit", { tx: t }), tx);
};

const shot = async (label) => {
  await browser.pause(600);
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

describe("LIVE M11.2 — MR + normal map visual", () => {
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
    // Clear the first-run demo character so the imported tile renders alone (and frame_all frames it large).
    const dc = await invoke("demo_character").catch(() => null);
    if (Array.isArray(dc)) {
      const [root, parts] = dc;
      for (const pid of parts || []) await invoke("remove_entity", { id: pid }).catch(() => {});
      if (root) await invoke("remove_entity", { id: root }).catch(() => {});
    }
    await browser.pause(300);
  });

  it("imports the PBR tile and renders the rippled normal relief + metallic split", async () => {
    const before = await countEntities();
    const id = await invoke("import_asset", { path: GLB });
    if (typeof id !== "string") throw new Error("import_asset returned " + JSON.stringify(id));
    await browser.waitUntil(async () => (await countEntities()) > before, {
      timeout: 15000,
      timeoutMsg: "tile did not place into the scene",
    });

    // Front-on first (camera straight at the +Z face): base + relief + metallic split.
    await invoke("view_preset", { preset: "front" });
    await invoke("frame_all");
    await shot("00_front");

    // Perspective + framed: the relief catches the angled key light + the metal half reflects the sky.
    await invoke("view_preset", { preset: "persp" });
    await invoke("frame_all");
    await shot("01_persp");

    // Tilt the tile ~40° (yaw) so the light grazes the ripples — strongest relief.
    await setField(id, "qw", 0.94);
    await setField(id, "qy", 0.34);
    await invoke("frame_all");
    await shot("02_yaw40");

    // Pitch it back so the surface faces up into the key light — relief + metal/dielectric contrast.
    await setField(id, "qw", 0.92);
    await setField(id, "qy", 0.0);
    await setField(id, "qx", 0.38);
    await invoke("frame_all");
    await shot("03_pitch45");

    console.log(`  imported tile id=${id}`);
  });
});
