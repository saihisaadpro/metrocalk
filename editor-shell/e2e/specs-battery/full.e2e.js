// LIVE interaction BATTERY — 60+ real interactions captured as OS screenshots for M11 close-out QA.
// Each `step` runs an action (errors caught, never aborting the battery), pauses, then captures. A
// results.json records per-step ok/error + key state reads so failures are triageable without opening
// every image.

import { browser } from "@wdio/globals";
import { execSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";

const SHOT_DIR = process.env.MTK_SHOT_DIR;
const SHOT_PS1 = process.env.MTK_SHOT_PS1;
const FIX = process.env.MTK_FIX_DIR; // dir holding the demo .glb fixtures
const PROC = "metrocalk-editor-shell";
mkdirSync(SHOT_DIR, { recursive: true });

const invoke = (cmd, args = {}) =>
  browser.execute(async (c, a) => window.__TAURI__.core.invoke(c, a), cmd, args);

let opN = 0;
const setField = async (id, field, value) => {
  opN += 1;
  const tx = {
    clientOpId: `bat-op-${opN}`,
    label: `set Transform.${field}`,
    patches: [],
    intent: { kind: "setField", id, component: "Transform", field, value },
  };
  await browser.execute(async (t) => window.__TAURI__.core.invoke("submit_edit", { tx: t }), tx);
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

const results = [];
let n = 0;
const shot = (label) => {
  const file = `${String(n).padStart(2, "0")}_${label}.png`;
  const out = path.join(SHOT_DIR, file);
  try {
    execSync(`powershell -ExecutionPolicy Bypass -File "${SHOT_PS1}" -Out "${out}" -ProcName "${PROC}"`, {
      stdio: "ignore",
    });
  } catch (e) {
    console.error("shot failed", file, String(e));
  }
  return file;
};
const step = async (label, fn) => {
  let error = null;
  let ret;
  try {
    ret = fn ? await fn() : null;
  } catch (e) {
    error = String(e);
    console.error(`STEP ${n} ${label} FAILED:`, error);
  }
  await browser.pause(420);
  const file = shot(label);
  const count = await countEntities();
  results.push({ n, label, file, error, count, ret: typeof ret === "string" ? ret : undefined });
  n += 1;
  return ret;
};

const clearScene = async () => {
  await invoke("new_project").catch(() => {});
  await browser.pause(300);
  const dc = await invoke("demo_character").catch(() => null);
  if (Array.isArray(dc)) {
    const [root, parts] = dc;
    for (const pid of parts || []) await invoke("remove_entity", { id: pid }).catch(() => {});
    if (root) await invoke("remove_entity", { id: root }).catch(() => {});
  }
  await browser.pause(200);
};

const glb = (name) => path.join(FIX, name);

describe("LIVE interaction battery (M11 close-out QA)", () => {
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

  after(() => {
    writeFileSync(path.join(SHOT_DIR, "results.json"), JSON.stringify(results, null, 2));
    const fails = results.filter((r) => r.error);
    console.log(`\nBATTERY: ${results.length} steps, ${fails.length} errored.`);
    for (const f of fails) console.log(`  FAIL ${f.n} ${f.label}: ${f.error}`);
  });

  it("drives 60+ interactions and captures each", async () => {
    let a, b, c, mesh;

    // ── Phase A: authoring + hierarchy (clean scene) ───────────────────────────────
    await clearScene();
    await step("new_project_empty", async () => invoke("frame_all"));
    a = await step("create_box_a", async () => invoke("create_entity", { x: 0, y: 0.5, z: 0, name: "Box A" }));
    b = await step("create_box_b", async () => invoke("create_entity", { x: 3, y: 0.5, z: 0, name: "Box B" }));
    c = await step("create_box_c", async () => invoke("create_entity", { x: -3, y: 0.5, z: 0, name: "Box C" }));
    await step("frame_all_three", async () => invoke("frame_all"));
    const dup = await step("duplicate_a", async () => invoke("duplicate_entity", { id: a }));
    await step("select_a", async () => invoke("gizmo_select", { id: a }));
    await step("remove_dup", async () => invoke("remove_entity", { id: dup }));
    await step("undo_restores_dup", async () => invoke("undo"));
    await step("group_three", async () => invoke("group_entities", { ids: [a, b, c], name: "Group1" }));
    await step("rename_a", async () => invoke("rename_entity", { id: a, name: "Renamed A" }));

    // ── Phase B: transform via setField + gizmo modes ──────────────────────────────
    await step("select_b", async () => invoke("gizmo_select", { id: b }));
    await step("move_b_x", async () => setField(b, "x", 5));
    await step("move_b_y", async () => setField(b, "y", 2.5));
    await step("move_b_z", async () => setField(b, "z", 2));
    await step("gizmo_rotate", async () => invoke("gizmo_mode", { mode: "rotate" }));
    await step("rotate_b_yaw", async () => {
      await setField(b, "qw", 0.7071);
      await setField(b, "qy", 0.7071);
    });
    await step("gizmo_scale", async () => invoke("gizmo_mode", { mode: "scale" }));
    await step("scale_b_up", async () => setField(b, "scale", 2));
    await step("scale_b_reset", async () => setField(b, "scale", 1));
    await step("gizmo_move_snap_off", async () => {
      await invoke("gizmo_mode", { mode: "move" });
      await invoke("set_snap", { on: false });
    });

    // ── Phase C: camera presets + zoom + look-through ──────────────────────────────
    await step("view_top", async () => {
      await invoke("view_preset", { preset: "top" });
      await invoke("frame_all");
    });
    await step("view_front", async () => {
      await invoke("view_preset", { preset: "front" });
      await invoke("frame_all");
    });
    await step("view_side", async () => {
      await invoke("view_preset", { preset: "side" });
      await invoke("frame_all");
    });
    await step("view_persp", async () => {
      await invoke("view_preset", { preset: "persp" });
      await invoke("frame_all");
    });
    await step("zoom_out", async () => invoke("zoom", { delta: 25 }));
    await step("zoom_in", async () => invoke("zoom", { delta: -35 }));
    await step("reframe", async () => invoke("frame_all"));
    await step("add_camera", async () => invoke("add_camera", { x: 12, y: 7, z: 12, fov: 50, active: true }));
    await step("look_through_on", async () => invoke("look_through_camera", { on: true }));
    await step("look_through_off", async () => invoke("look_through_camera", { on: false }));

    // ── Phase D: exposure ──────────────────────────────────────────────────────────
    await step("exposure_dim", async () => invoke("set_exposure", { exposure: 0.4 }));
    await step("exposure_bright", async () => invoke("set_exposure", { exposure: 2.5 }));
    await step("exposure_default", async () => invoke("set_exposure", { exposure: 1.0 }));

    // ── Phase E: lighting ──────────────────────────────────────────────────────────
    await step("light_directional", async () =>
      invoke("add_light", { kind: "directional", x: 0, y: 10, z: 0, r: 1, g: 0.95, b: 0.9, intensity: 3 })
    );
    await step("light_point", async () =>
      invoke("add_light", { kind: "point", x: 4, y: 4, z: 4, r: 0.4, g: 0.7, b: 1, intensity: 5 })
    );
    await step("light_spot", async () =>
      invoke("add_light", { kind: "spot", x: -4, y: 6, z: 0, r: 1, g: 0.6, b: 0.3, intensity: 6 })
    );

    // ── Phase F: import + PBR materials (fresh scene) ──────────────────────────────
    await clearScene();
    await step("import_cube", async () => invoke("import_asset", { path: glb("cube.glb") }));
    mesh = results[results.length - 1].ret;
    await step("import_sphere", async () => invoke("import_asset", { path: glb("sphere.glb") }));
    await step("import_prop", async () => invoke("import_asset", { path: glb("prop.glb") }));
    await step("import_healthbar", async () => invoke("import_asset", { path: glb("healthbar.glb") }));
    await step("import_multimat", async () => invoke("import_asset", { path: glb("multi_material_quad.glb") }));
    await step("import_normalmap", async () => invoke("import_asset", { path: glb("normal_mapped_quad.glb") }));
    await step("frame_all_imports", async () => {
      await invoke("view_preset", { preset: "persp" });
      await invoke("frame_all");
    });
    if (typeof mesh === "string") {
      await step("material_chrome", async () => invoke("ai_edit", { id: mesh, material: "chrome" }));
      await step("material_gold", async () => invoke("ai_edit", { id: mesh, material: "gold" }));
      await step("material_copper", async () => invoke("ai_edit", { id: mesh, material: "copper" }));
      await step("material_rusty", async () => invoke("ai_edit", { id: mesh, material: "rusty" }));
      await step("material_plastic", async () => invoke("ai_edit", { id: mesh, material: "plastic" }));
    }

    // ── Phase G: physics (fresh scene) ─────────────────────────────────────────────
    await clearScene();
    const sc = await step("phys_import_cube", async () => invoke("import_asset", { path: glb("cube.glb") }));
    await step("phys_raise_cube", async () => {
      await setField(sc, "x", 0);
      await setField(sc, "z", 0);
      await setField(sc, "y", 2.5);
      await invoke("view_preset", { preset: "side" });
      await invoke("frame_all");
    });
    await step("phys_make_static", async () => invoke("make_static", { id: sc }));
    await step("phys_drop_ball", async () => invoke("spawn_body", { x: 0, y: 6, z: 0 }));
    await step("phys_ball_falling", async () => browser.pause(250));
    await step("phys_ball_rest", async () => browser.pause(1500));
    await step("phys_second_ball", async () => invoke("spawn_body", { x: 0.4, y: 7, z: 0 }));
    await step("phys_settle", async () => browser.pause(1500));
    const dyn = await step("phys_import_dyn", async () => invoke("import_asset", { path: glb("sphere.glb") }));
    await step("phys_make_dynamic", async () => {
      await setField(dyn, "y", 8);
      await invoke("make_dynamic", { id: dyn });
    });
    await step("phys_play", async () => invoke("play"));
    await step("phys_playing", async () => browser.pause(1200));
    await step("phys_stop", async () => invoke("stop"));

    // ── Phase H: describe-to-create + combined + focus ─────────────────────────────
    await clearScene();
    await step("describe_health_bar", async () => invoke("describe", { query: "a glowing health bar" }));
    await step("describe_frame", async () => invoke("frame_all"));
    const lit = await step("combined_import", async () => invoke("import_asset", { path: glb("cube.glb") }));
    await step("combined_lights", async () => {
      await invoke("add_light", { kind: "directional", x: 3, y: 8, z: 2, r: 1, g: 1, b: 1, intensity: 4 });
      await invoke("view_preset", { preset: "persp" });
      await invoke("frame_all");
    });
    await step("combined_chrome", async () => {
      if (typeof lit === "string") await invoke("ai_edit", { id: lit, material: "chrome" });
    });
    await step("focus_entity", async () => {
      if (typeof lit === "string") await invoke("focus_entity", { id: lit });
    });
    await step("unfocus", async () => invoke("unfocus"));
    await step("final_scene", async () => {
      await invoke("view_preset", { preset: "persp" });
      await invoke("frame_all");
    });

    expect(results.length).toBeGreaterThanOrEqual(60);
  });
});
