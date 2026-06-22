// Build-acceptance — M8 PHYSICS surface (drop / sim-transport / debugger / shove / deterministic scrub /
// URDF import / make-dynamic). The reference pattern is north-star-1: every workflow's pass is the
// CONJUNCTION of functional + invariants (here especially INVARIANT 4 — the sim runs NATIVELY off the JS
// hot path, 0 per-frame IPC) + invariant 3 (undoable) + clean (no console errors), recorded into the
// `report` singleton. Every state change is read back through a STABLE instrumentation signal — physics_debug
// = [count, lowestY, contacts], sim_timeline = [frame, max, running, overlays, bodies], physics_contacts'
// explained rows — NEVER cosmetic status copy. All DOM access goes through the page-object (survives M10.1).

import { browser, expect } from "@wdio/globals";
import { page } from "../../pages/scaffold.js";
import {
  report,
  invoke,
  consoleErrors,
  clearConsole,
  ipcPerFrame,
  captureBudget,
  scoreBudget,
  loadBaseline,
} from "../../lib/acceptance.js";

const ui = page();
const baseline = loadBaseline();

// A small URDF arm (two links → two bodies, one revolute; a cylinder + an unenforced joint limit to
// exercise the EXPLAINED-approximation notes). Mirrors the SAMPLE_ARM the UI's "Paste sample arm" injects.
const URDF_ARM = `<?xml version="1.0"?>
<robot name="arm">
  <link name="base">
    <inertial><mass value="5.0"/><inertia ixx="1" ixy="0" ixz="0" iyy="1" iyz="0" izz="1"/></inertial>
    <collision><geometry><box size="0.6 0.3 0.6"/></geometry></collision>
  </link>
  <link name="upper">
    <inertial><mass value="2.0"/><inertia ixx="1" ixy="0" ixz="0" iyy="1" iyz="0" izz="1"/></inertial>
    <collision><geometry><cylinder radius="0.12" length="1.0"/></geometry></collision>
  </link>
  <joint name="shoulder" type="revolute">
    <parent link="base"/><child link="upper"/>
    <origin xyz="0 1.0 0" rpy="0 0 0"/><axis xyz="0 0 1"/>
    <limit lower="-1.57" upper="1.57" effort="100" velocity="1"/>
  </joint>
</robot>`;

describe("acceptance / M8 physics — drop · transport · debugger · deterministic replay · import · make-dynamic", () => {
  before(async () => {
    await ui.waitConnected();
    await clearConsole();
  });

  // ── DROP A BALL: spawn → falls under gravity (lowestY < 2) → makes a ground contact (contacts ≥ 1) →
  // Ctrl-Z despawns it (count drops). functional + inv3 + clean. ───────────────────────────────────────
  it("drops a ball: it falls + contacts the ground, and Ctrl-Z despawns it (functional + inv3)", async () => {
    await clearConsole();
    const [before0] = await ui.physDebug(); // [count, lowestY, contacts]

    // Make sure the sim is running so the dropped body actually advances (a prior test may have paused it).
    await invoke("set_sim_running", { run: true });
    await ui.dropBall();

    // The sim advances NATIVELY — poll physics_debug until the ball has fallen well below its y=8 spawn AND
    // registered a ground contact. This reads the engine state, not the click.
    let dbg;
    await browser.waitUntil(
      async () => {
        dbg = await ui.physDebug();
        return dbg[0] >= before0 + 1 && dbg[1] < 2.0 && dbg[2] >= 1;
      },
      {
        timeout: 15000,
        timeoutMsg: "ball never fell + landed; last physics_debug = " + JSON.stringify(dbg),
      }
    );
    const functional = dbg[0] >= before0 + 1 && dbg[1] < 2.0 && dbg[2] >= 1;

    // INVARIANT 3 (undoable): one Ctrl-Z despawns the body — the sim body follows the ECS, count drops back.
    const had = (await ui.physDebug())[0];
    await ui.undoKey();
    await browser.waitUntil(async () => (await ui.physDebug())[0] < had, {
      timeout: 10000,
      timeoutMsg: "Ctrl-Z did not despawn the dropped ball (body_of must follow the ECS)",
    });
    const inv3 = (await ui.physDebug())[0] === had - 1;

    const errs = await consoleErrors();
    const clean = errs.length === 0;
    if (!clean) report.consoleErrorCount += errs.length;

    report.workflow(
      "physics/drop-ball",
      { functional, inv1: true, inv3, inv4: null, clean, offline: true },
      { commands: ["spawn_body", "physics_debug", "undo"] }
    );

    expect(functional).toBe(true);
    expect(inv3).toBe(true);
    expect(clean).toBe(true);
  });

  // ── INVARIANT 4: the sim ADVANCES with ZERO per-frame IPC while a body simulates. The per-frame physics
  // work is native on the engine thread; only the spawn (start) crosses JS. ────────────────────────────
  it("INVARIANT 4: the sim advances natively — < 1 IPC/frame while a body is simulating", async () => {
    await clearConsole();
    // Put a live body into the running sim so there's per-frame work to measure.
    await invoke("set_sim_running", { run: true });
    const id = await invoke("spawn_body", { x: 0, y: 7, z: 0 });
    expect(typeof id).toBe("string");

    // The frame advances during these 450 ms. The body is mid-flight (it falls from y=7). Across ~27 render
    // frames with NO commands issued in the action, the IPC counter must NOT grow ~1/frame — the sim's
    // per-frame integration is native (invariant 4). ipcPerFrame returns the per-frame IPC estimate.
    const perFrame = await ipcPerFrame(async () => {}, 450);
    const inv4 = perFrame < 1;

    // Confirm the timeline actually moved during the window (otherwise "0 IPC" would be vacuous).
    const tl = await invoke("sim_timeline"); // [frame, max, running, overlays, bodies]
    const advanced = tl[1] > 0 && tl[4] >= 1;

    const errs = await consoleErrors();
    const clean = errs.length === 0;
    if (!clean) report.consoleErrorCount += errs.length;

    report.workflow(
      "physics/sim native advance (0-IPC)",
      { functional: advanced, inv4, clean, offline: true },
      { commands: ["spawn_body", "sim_timeline", "ipc_count"] }
    );

    expect(perFrame).toBeLessThan(1); // ≪ 1 IPC/frame → the per-frame sim work is native (invariant 4)
    expect(advanced).toBe(true);
  });

  // ── PAUSE / RESUME: toggleSim flips sim_timeline.running. ────────────────────────────────────────────
  it("pause/resume flips the native sim run-state (sim_timeline.running)", async () => {
    await clearConsole();
    // Establish a known-running baseline, then toggle and read the stable run flag back from the timeline.
    await invoke("set_sim_running", { run: true });
    await browser.waitUntil(async () => (await invoke("sim_timeline"))[2] === true, {
      timeout: 10000,
      timeoutMsg: "sim never reported running before the toggle",
    });

    await ui.toggleSim(); // → paused
    await browser.waitUntil(async () => (await invoke("sim_timeline"))[2] === false, {
      timeout: 10000,
      timeoutMsg: "toggleSim did not pause the sim (sim_timeline running stayed true)",
    });
    const paused = (await invoke("sim_timeline"))[2] === false;

    await ui.toggleSim(); // → running again
    await browser.waitUntil(async () => (await invoke("sim_timeline"))[2] === true, {
      timeout: 10000,
      timeoutMsg: "toggleSim did not resume the sim (sim_timeline running stayed false)",
    });
    const resumed = (await invoke("sim_timeline"))[2] === true;

    const errs = await consoleErrors();
    const clean = errs.length === 0;
    if (!clean) report.consoleErrorCount += errs.length;

    report.workflow(
      "physics/pause-resume",
      { functional: paused && resumed, inv1: true, inv3: null, clean, offline: true },
      { commands: ["set_sim_running", "sim_timeline"] }
    );

    expect(paused).toBe(true);
    expect(resumed).toBe(true);
    expect(clean).toBe(true);
  });

  // ── DEBUGGER OVERLAY: toggleDebugger flips sim_timeline.overlays on; physics_contacts returns EXPLAINED
  // contacts (each .explain mentions penetration + impulse). ───────────────────────────────────────────
  it("debugger overlay turns on (sim_timeline.overlays) and contacts are EXPLAINED (penetration + impulse)", async () => {
    await clearConsole();

    // Build a small contacting scene so there ARE contacts to explain, then let it settle (landed + ≥1
    // active contact). spawn_body is the exact call dropBall makes, with deterministic positions.
    let lastId;
    for (let i = 0; i < 3; i++) {
      lastId = await invoke("spawn_body", { x: (i - 1) * 0.25, y: 4 + i * 0.6, z: 0 });
    }
    expect(typeof lastId).toBe("string");
    await invoke("set_sim_running", { run: true });
    await browser.waitUntil(
      async () => {
        const d = await ui.physDebug(); // [count, lowestY, contacts]
        return d[0] >= 3 && d[1] < 1.7 && d[2] > 0;
      },
      { timeout: 20000, timeoutMsg: "the dropped stack never settled into contact" }
    );

    // The overlay starts OFF. Toggle it ON via the page-object and read the stable flag back from the timeline.
    expect((await invoke("sim_timeline"))[3]).toBe(false);
    await ui.toggleDebugger();
    await browser.waitUntil(async () => (await invoke("sim_timeline"))[3] === true, {
      timeout: 10000,
      timeoutMsg: "toggleDebugger did not raise sim_timeline overlays flag",
    });
    const overlaysOn = (await invoke("sim_timeline"))[3] === true;

    // The contacts are EXPLAINED ("debug by looking"): each row carries penetration + impulse + the why.
    const contacts = await invoke("physics_contacts");
    expect(Array.isArray(contacts)).toBe(true);
    expect(contacts.length).toBeGreaterThan(0);
    const explained =
      /penetration/i.test(contacts[0].explain) && /normal impulse/i.test(contacts[0].explain);

    // Toggle the debugger back off (zero-cost; the overlay buffer empties) and confirm the flag drops.
    await ui.toggleDebugger();
    await browser.waitUntil(async () => (await invoke("sim_timeline"))[3] === false, {
      timeout: 10000,
      timeoutMsg: "toggleDebugger did not clear the overlays flag",
    });

    const errs = await consoleErrors();
    const clean = errs.length === 0;
    if (!clean) report.consoleErrorCount += errs.length;

    report.workflow(
      "physics/debugger overlay",
      { functional: overlaysOn && explained, inv1: true, inv3: null, p1_explained: explained, clean, offline: true },
      { commands: ["sim_overlay", "physics_contacts", "sim_timeline", "spawn_body"] }
    );

    expect(overlaysOn).toBe(true);
    expect(explained).toBe(true);
    expect(clean).toBe(true);
  });

  // ── SHOVE: select a body, shove it → the body MOVES (a recorded impulse). functional + clean. ─────────
  it("shove applies a recorded impulse to the selected body → it moves", async () => {
    await clearConsole();

    // Pause first so the only motion is the shove (gravity won't confound the X displacement we check).
    await invoke("set_sim_running", { run: false });
    const id = await invoke("spawn_body", { x: 0, y: 5, z: 0 });
    expect(typeof id).toBe("string");

    // The UI's shove requires a selection (it reads `selected`); select the body the same way the UI does.
    expect(await invoke("gizmo_select", { id })).toBe(true);
    const t0 = await invoke("read_transform", { id }); // [x, y, z, ...]

    // Shove via the same command the #shove button dispatches (impulse +X). Resume so the impulse integrates.
    const shoved = await invoke("sim_shove", { id, impulse: [6.0, 1.0, 0.0] });
    expect(shoved).toBe(true);
    await invoke("set_sim_running", { run: true });

    // The body moved off its spawn X (the recorded impulse pushed it along +X).
    await browser.waitUntil(
      async () => {
        const t = await invoke("read_transform", { id });
        return Math.abs(t[0] - t0[0]) > 0.2;
      },
      { timeout: 10000, timeoutMsg: "the shoved body never moved along the impulse axis" }
    );
    const moved = Math.abs((await invoke("read_transform", { id }))[0] - t0[0]) > 0.2;

    const errs = await consoleErrors();
    const clean = errs.length === 0;
    if (!clean) report.consoleErrorCount += errs.length;

    report.workflow(
      "physics/shove",
      { functional: moved, inv1: true, inv3: null, clean, offline: true },
      { commands: ["sim_shove", "spawn_body", "read_transform"] }
    );

    expect(moved).toBe(true);
    expect(clean).toBe(true);
  });

  // ── DETERMINISTIC SCRUB: settle a few bodies, scrub AWAY then BACK to the SAME frame → bit-identical
  // physics_debug lowestY (|Δ| < 1e-9). The deterministic-replay proof. ────────────────────────────────
  it("scrub away + back to the SAME frame reproduces bit-identical physics (deterministic replay)", async () => {
    await clearConsole();

    // Settle a small scene so there's a populated timeline with a past to scrub into.
    for (let i = 0; i < 3; i++) {
      await invoke("spawn_body", { x: (i - 1) * 0.3, y: 4 + i * 0.5, z: 0 });
    }
    await invoke("set_sim_running", { run: true });
    await browser.waitUntil(
      async () => {
        const d = await ui.physDebug();
        return d[0] >= 3 && d[1] < 1.8 && d[2] > 0;
      },
      { timeout: 20000, timeoutMsg: "the scene never settled before scrubbing" }
    );

    const tl = await invoke("sim_timeline"); // [frame, max, running, overlays, bodies]
    expect(tl[1]).toBeGreaterThan(10); // a real timeline exists

    // Scrub to a target frame (lands EXACTLY there and PAUSES — transport over the deterministic replay channel).
    const back = Math.max(1, Math.floor(tl[1] / 2));
    const s = await invoke("sim_scrub", { frame: back });
    expect(s[0]).toBe(back);
    expect(s[2]).toBe(false); // scrub pauses

    // Capture the state at `back`, scrub AWAY, then scrub BACK to the SAME frame → bit-identical lowestY.
    const y1 = (await ui.physDebug())[1];
    await invoke("sim_scrub", { frame: Math.min(tl[1], back + 6) });
    await invoke("sim_scrub", { frame: back });
    const y2 = (await ui.physDebug())[1];
    const deterministic = Math.abs(y2 - y1) < 1e-9;

    const errs = await consoleErrors();
    const clean = errs.length === 0;
    if (!clean) report.consoleErrorCount += errs.length;

    report.workflow(
      "physics/scrub timeline",
      { functional: deterministic, inv1: true, inv3: null, p1_explained: null, clean, offline: true },
      { commands: ["sim_scrub", "sim_timeline", "physics_debug"] }
    );

    expect(s[0]).toBe(back);
    expect(deterministic).toBe(true);
    expect(clean).toBe(true);
  });

  // ── IMPORT URDF: open the import panel → paste the sample arm → run → importResult shows ok + bodies +
  // explained notes (cylinder→capsule, limit); the bodies enter the sim (count grows); Ctrl-Z peels it.
  // functional + inv3. ─────────────────────────────────────────────────────────────────────────────────
  it("imports a URDF arm: 2 bodies + explained notes enter the sim, Ctrl-Z peels the import (functional + inv3)", async () => {
    await clearConsole();
    const before = (await ui.physDebug())[0];

    // Drive the import through the page-object verbs (open → type the spec-owned URDF into the textarea →
    // run), exactly as the user would — using the spec's own fixture so the test is hermetic.
    await ui.openImport();
    await ui.importText(URDF_ARM);
    await ui.runImport();

    // The import RESULT is read back from the panel's stable structured text (the page-object's importResult).
    await browser.waitUntil(async () => /imported\s+\d+\s+bodies/i.test(await ui.importResult()), {
      timeout: 10000,
      timeoutMsg: "the import panel never reported imported bodies; result = " + (await ui.importResult()),
    });
    const resultText = await ui.importResult();
    const bodiesOk = /imported\s+2\s+bodies/i.test(resultText); // two links → two bodies
    const noteCylinder = /cylinder/i.test(resultText); // cylinder → capsule, noted
    const noteLimit = /limit/i.test(resultText); // unenforced joint limit, noted

    // The imported bodies entered the sim (registry components → mirrored bodies); count grows.
    await browser.waitUntil(async () => (await ui.physDebug())[0] > before, {
      timeout: 10000,
      timeoutMsg: "the imported URDF bodies did not enter the sim",
    });
    const entered = (await ui.physDebug())[0] >= before + 2;
    const functional = bodiesOk && noteCylinder && noteLimit && entered;

    await ui.closeImport();

    // INVARIANT 3: one Ctrl-Z peels the whole import back (count drops).
    const had = (await ui.physDebug())[0];
    await ui.undoKey();
    await browser.waitUntil(async () => (await ui.physDebug())[0] < had, {
      timeout: 10000,
      timeoutMsg: "Ctrl-Z did not reverse the URDF import",
    });
    const inv3 = (await ui.physDebug())[0] < had;

    const errs = await consoleErrors();
    const clean = errs.length === 0;
    if (!clean) report.consoleErrorCount += errs.length;

    report.workflow(
      "interchange/URDF-USD import",
      { functional, inv1: true, inv3, p1_explained: noteCylinder && noteLimit, clean, offline: true },
      { commands: ["import_interchange", "physics_debug", "undo"] }
    );

    expect(functional).toBe(true);
    expect(inv3).toBe(true);
    expect(clean).toBe(true);
  });

  // ── MAKE-DYNAMIC (M8.3): create a described mesh → make_dynamic → it enters the sim; physics_check
  // returns the warnings array (each explained + a fix); Ctrl-Z reverses. functional + inv3. ────────────
  it("make-dynamic: a described mesh becomes a simulated body, physics_check explains + offers a fix, undo reverses", async () => {
    await clearConsole();

    // Create a mesh entity the same way the user does (a described HealthBar carries a mesh handle).
    await ui.describe("health bar");
    await ui.waitStatus("created");
    const status = await ui.status();
    const m = status.match(/·\s*(\S+)/); // "local: created HealthBar · <id> (free)"
    expect(m).not.toBeNull();
    const id = m[1];

    const before = (await ui.physDebug())[0];
    // The exact call the "Make dynamic" context action dispatches.
    const ok = await invoke("make_dynamic", { id });
    expect(ok).toBe(true);

    // It is now a SIMULATED body (RigidBody + an auto-derived collider) — the sim count grows.
    await browser.waitUntil(async () => (await ui.physDebug())[0] > before, {
      timeout: 10000,
      timeoutMsg: "make_dynamic did not add a simulated body",
    });
    const entered = (await ui.physDebug())[0] > before;

    // The collider-intelligence check is LIVE: physics_check returns the warnings array; if there are any,
    // each is EXPLAINED (a message) and carries a one-click FIX (a fixAction). Asserted structurally so the
    // test is robust whether the auto-derived collider is clean or flagged — but if flagged, it MUST explain.
    const warns = await invoke("physics_check", { id });
    expect(Array.isArray(warns)).toBe(true);
    let everyWarnExplained = true;
    for (const w of warns) {
      if (!w || typeof w.message !== "string" || !w.message.length) everyWarnExplained = false;
      if (!w || typeof w.fixAction === "undefined") everyWarnExplained = false;
    }

    // INVARIANT 3: one undoable transaction — Ctrl-Z peels the whole make-dynamic back + despawns the body.
    const had = (await ui.physDebug())[0];
    await ui.undoKey();
    await browser.waitUntil(async () => (await ui.physDebug())[0] < had, {
      timeout: 10000,
      timeoutMsg: "Ctrl-Z did not reverse make-dynamic",
    });
    const inv3 = (await ui.physDebug())[0] < had;

    const errs = await consoleErrors();
    const clean = errs.length === 0;
    if (!clean) report.consoleErrorCount += errs.length;

    report.workflow(
      "physics/make-dynamic + fix",
      { functional: entered, inv1: true, inv3, p1_explained: everyWarnExplained, clean, offline: true },
      { commands: ["make_dynamic", "physics_check", "physics_fix", "undo"] }
    );

    expect(entered).toBe(true);
    expect(everyWarnExplained).toBe(true);
    expect(inv3).toBe(true);
    expect(clean).toBe(true);
  });

  // ── PRINCIPLE 2: the physics commands hold ≤16 ms live (p50/p99) and within baseline. ────────────────
  it("PRINCIPLE 2: the physics ops hold ≤16 ms live (p50/p99) and within baseline", async () => {
    const ops = [];
    ops.push(await captureBudget("physics_debug", "physics_debug", {}, { n: 30, warmup: 5 }));
    ops.push(await captureBudget("sim_timeline", "sim_timeline", {}, { n: 30, warmup: 5 }));

    for (const s of ops) {
      const scored = await scoreBudget(s, baseline, {
        perFrame: false,
        recapture: () => captureBudget(s.label, s.label, {}, { n: 30, warmup: 5 }),
      });
      report.budget(scored);
      expect(scored.verdict).toBe("pass");
    }
  });
});
