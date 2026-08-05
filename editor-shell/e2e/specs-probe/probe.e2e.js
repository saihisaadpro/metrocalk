// Diagnostic probe for the M15.9 empty-viewport-after-rigging defect: reproduce the mechanism spec
// step by step, and after each step dump camera_debug + a viewport_peek ray-pick + an OS capture, to
// find WHERE meshes stop rendering (instance data vs draw).
import { browser } from "@wdio/globals";
import { execSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const dir = path.dirname(fileURLToPath(import.meta.url));
const SHOT_DIR = path.resolve(dir, "../.shots-probe");
const SHOT_PS1 = path.resolve(dir, "../../../.uxtest/audit/exe/capture-window-fg.ps1");
const PROC = "metrocalk-editor-shell";
const QUARTET = path.resolve(dir, "../samples/analytic_quartet.stp");
mkdirSync(SHOT_DIR, { recursive: true });

const shot = async (label) => {
  await browser.pause(600);
  try {
    execSync(`powershell -ExecutionPolicy Bypass -File "${SHOT_PS1}" -ProcName "${PROC}" -Out "${path.join(SHOT_DIR, `${label}.png`)}"`, { stdio: "ignore" });
  } catch (e) {
    console.error("shot failed", label, String(e));
  }
};
const invoke = (cmd, args) =>
  browser.execute(async (c, a) => { try { return await window.__TAURI__.core.invoke(c, a || {}); } catch (e) { return { error: String(e) }; } }, cmd, args);
const kick = (cmd, args) =>
  browser.execute((c, a) => { window.__TAURI__.core.invoke(c, a || {}).catch(() => {}); return true; }, cmd, args);
const entityCount = () =>
  browser.execute(() => { const el = document.getElementById("count"); const m = el && el.textContent ? el.textContent.match(/\d+/) : null; return m ? parseInt(m[0], 10) : 0; });

const probe = async (label) => {
  const cam = await invoke("camera_debug");
  // Ray-pick a 3×3 grid of NDC points across the middle of the viewport.
  const hits = {};
  for (const [nx, ny] of [[0, 0], [-0.3, 0], [0.3, 0], [0, -0.3], [0, 0.3]]) {
    const h = await invoke("viewport_peek", { x: nx, y: ny });
    if (h) hits[`${nx},${ny}`] = h;
  }
  console.log(`PROBE ${label}: cam=${JSON.stringify(cam)} hits=${JSON.stringify(hits)}`);
  await shot(label);
};

describe("probe: where meshes stop rendering after rigging", () => {
  before(async () => {
    await browser.waitUntil(
      async () => (await browser.execute(() => !!(window.__TAURI__ && window.__TAURI__.core))) === true,
      { timeout: 30000, timeoutMsg: "TAURI bridge never appeared" },
    );
    await browser.pause(1200);
    await browser.execute(() => {
      for (const b of document.querySelectorAll("button")) {
        if (/got it/i.test(b.textContent || "")) { b.click(); return true; }
      }
      return false;
    });
    await browser.execute(async () => { try { await window.__TAURI__.core.invoke("new_project"); } catch (e) { void e; } });
    await browser.pause(600);
  });

  it("probes each rigging step", async () => {
    await kick("import_asset", { path: QUARTET });
    await browser.waitUntil(async () => (await entityCount()) >= 4, { timeout: 60000, interval: 1000 });
    await invoke("frame_all");
    await probe("p00_imported");

    const ids = await browser.execute(() =>
      [...document.querySelectorAll('[data-testid="hrow"]')].map((r) => r.getAttribute("data-id")).filter(Boolean),
    );
    console.log("entity ids:", JSON.stringify(ids));
    const sphere = ids[1] || "1_0";

    await invoke("set_joint", { id: sphere, revolute: false, axis: [1, 0, 0], pivot: [0.02, 0, 0.005], min: -1, max: 1, source: "manual" });
    await probe("p01_set_joint");

    await invoke("joint_key", { id: sphere, t: 0 });
    await probe("p02_key0");

    await invoke("joint_value", { id: sphere, value: 0.06, commit: true });
    await probe("p03_value_commit");

    await invoke("joint_key", { id: sphere, t: 2 });
    await probe("p04_key2");

    await invoke("joint_scrub", { t: 1.0 });
    await probe("p05_scrub_noframe");

    await invoke("frame_all");
    await probe("p06_scrub_framed");

    await invoke("joint_scrub", { t: -1.0 });
    await probe("p07_scrub_off");
  });
});
