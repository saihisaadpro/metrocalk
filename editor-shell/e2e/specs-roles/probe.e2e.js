// Isolation probe: what does each roles-related entity actually render as?
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const dir = path.dirname(fileURLToPath(import.meta.url));
const shots = path.resolve(dir, "../.shots-roles-probe");
const capture = path.resolve(dir, "../scripts/capture-composited-window.ps1");
mkdirSync(shots, { recursive: true });

let n = 0;
async function shot(label) {
  await browser.pause(700);
  const out = path.join(shots, `${String(n).padStart(2, "0")}_${label}.png`);
  n += 1;
  try {
    execFileSync("powershell.exe", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", capture, "-Out", out], { stdio: "pipe" });
  } catch { /* ignore */ }
  console.log(`[probe] ${path.basename(out)} ${existsSync(out) ? statSync(out).size : 0} bytes`);
}

describe("roles render probe", () => {
  before(async () => {
    await browser.waitUntil(
      async () => browser.execute(() => !!(window.__TAURI__ && window.__TAURI__.core)),
      { timeout: 30000 },
    );
    await invoke("new_project");
    await browser.pause(800);
  });

  it("isolates each render", async () => {
    const wedge = (await invoke("shape_spawn", { kind: "wedge", pos: [0, 0, 0] })).created;
    await browser.execute(() => {
      document.querySelector('[data-testid="onboardSkip"]')?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await invoke("frame_all");
    await shot("wedge_plain");

    let reply = await invoke("role_assign", { id: wedge, role: "spinner" });
    console.log(`[probe] spinner assign: ${JSON.stringify(reply)}`);
    await invoke("frame_all");
    await shot("wedge_as_spinner");

    const details = await invoke("entity_details", { id: wedge });
    console.log(`[probe] wedge details: ${JSON.stringify(details).slice(0, 1200)}`);

    const torus = (await invoke("shape_spawn", { kind: "torus", pos: [3, 0, 0] })).created;
    reply = await invoke("role_assign", { id: torus, role: "collectible" });
    console.log(`[probe] collectible assign: ${JSON.stringify(reply)}`);
    await invoke("frame_all");
    await shot("plus_torus_collectible_and_score");

    const status = await invoke("role_status");
    console.log(`[probe] status: ${JSON.stringify(status)}`);
  });

  it("replicates the main-suite order and closes in on the wedge", async () => {
    await invoke("new_project");
    await browser.pause(600);
    const crystal = (await invoke("shape_spawn", { kind: "torus", pos: [0, 0, 0] })).created;
    const ball = (await invoke("shape_spawn", { kind: "sphere", pos: [0.1, 4, 0.15] })).created;
    const wall = (await invoke("shape_spawn", { kind: "box", pos: [3, 0, 0] })).created;
    const wedge = (await invoke("shape_spawn", { kind: "wedge", pos: [-3, 0, 0] })).created;
    for (const [id, role] of [[crystal, "collectible"], [ball, "prop"], [wall, "solid"], [wedge, "spinner"]]) {
      const r = await invoke("role_assign", { id, role });
      console.log(`[probe] ${role}: reason=${r.reason}`);
    }
    await invoke("frame_all");
    await shot("main_suite_replica_full");
    await invoke("focus_entity", { id: wedge });
    await invoke("zoom", { delta: -4 });
    await shot("wedge_closeup");
    await invoke("focus_entity", { id: wall });
    await invoke("zoom", { delta: -4 });
    await shot("wall_closeup");
    const det = await invoke("entity_details", { id: wedge });
    console.log(`[probe] replica wedge: ${JSON.stringify(det).slice(0, 400)}`);

    // Bisect the amber-quad trigger: selection vs the Gameplay tab.
    await invoke("frame_all");
    await (await $('[data-testid="engine-scene"]')).click();
    await browser.pause(300);
    await browser.execute((id) => {
      const row = document.querySelector(`[data-testid="hrow"][data-id="${id}"]`);
      row?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    }, wedge);
    await browser.pause(400);
    await shot("wedge_selected_scene_tab");
    await (await $('[data-testid="engine-gameplay"]')).click();
    await browser.pause(500);
    await shot("wedge_selected_gameplay_tab");
    await browser.execute((id) => {
      const row = document.querySelector(`[data-testid="hrow"][data-id="${id}"]`);
      row?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    }, crystal);
    await browser.pause(400);
    await shot("crystal_selected_gameplay_tab");
  });
});
