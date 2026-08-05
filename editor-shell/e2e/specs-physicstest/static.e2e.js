// LIVE M11.1 — collider-on-import via make_static: import a cube, raise it, make it a STATIC obstacle,
// drop a ball straight down onto it, and confirm the ball RESTS ON the imported mesh (sim Y well above the
// ground rest) rather than falling through. Proof the convex-hull collider derived from an imported mesh is
// real in the sim, not just authored components.

import { browser } from "@wdio/globals";
import { execSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import path from "node:path";

const SHOT_DIR = process.env.MTK_SHOT_DIR;
const SHOT_PS1 = process.env.MTK_SHOT_PS1;
const GLB = process.env.MTK_PHYS_GLB;
const PROC = "metrocalk-editor-shell";
mkdirSync(SHOT_DIR, { recursive: true });

const invoke = (cmd, args = {}) =>
  browser.execute(async (c, a) => window.__TAURI__.core.invoke(c, a), cmd, args);

let opN = 0;
const setField = async (id, field, value) => {
  opN += 1;
  const tx = {
    clientOpId: `phys-op-${opN}`,
    label: `set Transform.${field}`,
    patches: [],
    intent: { kind: "setField", id, component: "Transform", field, value },
  };
  await browser.execute(async (t) => window.__TAURI__.core.invoke("submit_edit", { tx: t }), tx);
};

const shot = async (label) => {
  await browser.pause(500);
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

describe("LIVE M11.1 — imported mesh as a STATIC collider (make_static)", () => {
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

  it("a ball dropped onto a make_static'd imported cube rests on it (not through it)", async () => {
    const before = await countEntities();
    const cube = await invoke("import_asset", { path: GLB });
    if (typeof cube !== "string") throw new Error("import_asset returned " + JSON.stringify(cube));
    await browser.waitUntil(async () => (await countEntities()) > before, {
      timeout: 15000,
      timeoutMsg: "cube did not place",
    });
    // Raise the cube to y=2.5 + centre it, so a ball can rest on it well ABOVE the ground rest (~0.5).
    await setField(cube, "x", 0);
    await setField(cube, "z", 0);
    await setField(cube, "y", 2.5);

    // Make the imported cube a STATIC obstacle (fixed body + convex-hull collider).
    const made = await invoke("make_static", { id: cube });
    expect(made).toBe(true);

    // Drop a ball straight down onto the cube's centre.
    const ball = await invoke("spawn_body", { x: 0, y: 6, z: 0 });
    if (typeof ball !== "string") throw new Error("spawn_body returned " + JSON.stringify(ball));

    // Let it fall + settle, polling the sim position until it stops moving (or a timeout).
    let pos = [0, 99, 0];
    let last = 99;
    for (let i = 0; i < 30; i++) {
      await browser.pause(150);
      pos = await invoke("body_sim_position", { id: ball });
      if (Array.isArray(pos) && Math.abs(pos[1] - last) < 0.01 && i > 6) break;
      last = Array.isArray(pos) ? pos[1] : 99;
    }
    await invoke("view_preset", { preset: "side" });
    await invoke("frame_all");
    await shot("00_ball_on_cube");

    console.log(`  ball rested at ${JSON.stringify(pos)} (cube top ~3.0; ground rest ~0.5)`);
    // The ball rests ON the cube (centre y=2.5, top ~3.0, + ball radius) — well above the ground rest.
    // If the collider were missing/ignored it would fall through to ~0.5.
    expect(pos[1]).toBeGreaterThan(1.5);
  });
});
