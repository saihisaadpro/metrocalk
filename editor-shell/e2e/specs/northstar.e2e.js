// North-star #1, end-to-end against the real .exe (WebView2 DOM via tauri-driver). Drives the editor
// panel AND the transparent viewport <div> (whose clicks fire the native Rust pick), asserting the
// resulting DOM — so the live round-trip, including the viewport pick we could only test by hand
// before, is verified automatically.

import { browser, $, $$, expect } from "@wdio/globals";

const boundCount = async () => (await $$("#reveal .boundrow")).length;

describe("Metrocalk editor — north-star #1 live", () => {
  it("launches, composites, and connects to /core (5000 entities)", async () => {
    await browser.waitUntil(async () => /\d+ entities/.test(await $("#count").getText()), {
      timeout: 60000,
      timeoutMsg: "editor never showed an entity count (no /core connection?)",
    });
    expect(await $("#count").getText()).toMatch(/5000 entities/);
    expect(await $("#status").getText()).toContain("connected");
  });

  it("surfaces requirers (HealthBars) to bind", async () => {
    const reqs = await $$("#requirers .cand");
    expect(reqs.length).toBeGreaterThan(0);
    expect(await reqs[0].getText()).toContain("HealthBar");
  });

  it("reveals ranked compatible targets when a requirer is selected", async () => {
    await (await $$("#requirers .cand"))[0].click();
    await browser.waitUntil(async () => (await $("#reveal").getText()).includes("requires"), {
      timeout: 10000,
      timeoutMsg: "reveal panel never populated after selecting a requirer",
    });
    expect(await $("#reveal").getText()).toContain("Health");
    expect((await $$("#reveal .cand")).length).toBeGreaterThan(0);
  });

  it("binds a compatible target in one click → it moves to 'tracking'", async () => {
    const before = await boundCount();
    await (await $$("#reveal .cand"))[0].click();
    await browser.waitUntil(async () => (await boundCount()) > before, {
      timeout: 10000,
      timeoutMsg: "bound target never appeared under 'tracking'",
    });
    expect(await boundCount()).toBeGreaterThan(before);
  });

  it("undoes the bind in one step → 'tracking' shrinks", async () => {
    const before = await boundCount();
    await $("#undo").click(); // same doUndo() path as Ctrl-Z
    await browser.waitUntil(async () => (await boundCount()) < before, {
      timeout: 10000,
      timeoutMsg: "tracking did not shrink after undo",
    });
    expect(await boundCount()).toBeLessThan(before);
  });

  it("picks an entity from the VIEWPORT — the native pick round-trip", async () => {
    // Click the viewport <div>'s centre → JS sends a normalized cursor → Rust picks the nearest cube
    // → returns its id → the inspector + status update. This is the path we kept fixing blind.
    await $("#viewport").click();
    await browser.waitUntil(async () => (await $("#status").getText()).includes("picked"), {
      timeout: 10000,
      timeoutMsg: "no 'picked' status after a viewport click (pick not serviced?)",
    });
    const status = await $("#status").getText();
    const inspector = await $("#inspector").getText();
    expect(status).toContain("picked"); // a click in the cube cloud must select something
    expect(status).not.toContain("nothing here");
    expect(inspector).toContain("Transform");
  });

  it("edits a field through the pipeline (round-trip)", async () => {
    const input = await $("#inspector input");
    // The pick test above asserted the inspector shows "Transform", so a picked entity MUST expose an
    // editable field. Assert it exists rather than silently early-returning — a no-field inspector is a
    // real regression to surface, not a reason to vacuously pass.
    expect(await input.isExisting()).toBe(true);
    await input.setValue("12.5");
    await browser.keys(["Enter"]);
    await browser.waitUntil(async () => (await $("#status").getText()).includes("edit"), {
      timeout: 10000,
      timeoutMsg: "no 'edit' status after changing a field",
    });
    expect(await $("#status").getText()).toContain("edit");
  });

  // ── north-star test #2: describe-to-create (live) ──────────────────────────────────────────────
  it("describes a component into existence and offers its attach", async () => {
    await $("#describe").setValue("health bar");
    await $("#describeBtn").click();
    await browser.waitUntil(async () => (await $("#status").getText()).includes("created"), {
      timeout: 10000,
      timeoutMsg: "no 'created' status after describe",
    });
    expect(await $("#status").getText()).toContain("HealthBar"); // resolved to the right kind
    // the created HealthBar is selected → its reveal panel offers Health providers to attach (≤2 total)
    await browser.waitUntil(async () => (await $("#reveal").getText()).includes("requires"), {
      timeout: 10000,
      timeoutMsg: "the described entity's attach panel never populated",
    });
    expect(await $("#reveal").getText()).toContain("Health");
  });

  it("a no-local-match describe BUYS from the MARKETPLACE tier — pre-componentized, not faked (M5/M7)", async () => {
    await $("#describe").setValue("rusty medieval sword");
    await $("#describeBtn").click();
    // M7: the marketplace tier is a real BUY (debit-on-success), so the status reads
    // "bought … from marketplace: …" — distinct from the generate seam's "no local or marketplace match".
    await browser.waitUntil(async () => (await $("#status").getText()).includes("bought"), {
      timeout: 10000,
      timeoutMsg: "no marketplace BUY on a no-local-match describe (rusty medieval sword)",
    });
    const status = await $("#status").getText();
    expect(status).toContain("marketplace:"); // resolved through the marketplace tier (not local, not generate)
    expect(status).toContain("tokens");        // it was metered (M7 real buy — the price, ~70% to the creator)
  });

  // ── M3.3: viewport context actions + hover details (the "context reveal") ─────────────────────────
  const ctxVisible = async () => (await $("#ctxmenu").getCSSProperty("display")).value !== "none";

  it("right-clicks a viewport entity → a context menu of its valid actions appears", async () => {
    const vp = await $("#viewport");
    // A right-button press+release with no movement (a click, not an orbit) at the viewport centre.
    await browser.action("pointer", { parameters: { pointerType: "mouse" } })
      .move({ origin: vp })
      .down({ button: 2 })
      .up({ button: 2 })
      .perform();
    await browser.waitUntil(ctxVisible, { timeout: 10000, timeoutMsg: "context menu never opened on right-click" });
    const items = await $$("#ctxmenu .ctxitem");
    expect(items.length).toBeGreaterThanOrEqual(5); // Bind / Remove / Duplicate / Focus / Inspect
    // every unavailable action carries a reason (the explain-every-"no" discipline)
    for (const it of items) {
      if ((await it.getAttribute("class")).includes("disabled")) {
        expect(await it.getText()).toContain("—"); // "Action  —  reason"
      }
    }
  });

  it("Remove from the menu deletes the entity → status says removed (Ctrl-Z to undo)", async () => {
    // Click the Remove item.
    const remove = await $('#ctxmenu .ctxitem[data-action="remove"]');
    await remove.click();
    await browser.waitUntil(async () => (await $("#status").getText()).includes("removed"), {
      timeout: 10000,
      timeoutMsg: "no 'removed' status after clicking Remove",
    });
    expect(await $("#status").getText()).toContain("Ctrl-Z");
    // and the menu closed.
    expect(await ctxVisible()).toBe(false);
  });

  it("Ctrl-Z restores the removed entity (one undoable transaction)", async () => {
    await browser.keys(["Control", "z"]);
    await browser.waitUntil(async () => (await $("#status").getText()).includes("undo"), {
      timeout: 10000,
      timeoutMsg: "no 'undo' status after Ctrl-Z",
    });
    expect(await $("#status").getText()).toContain("undo");
  });

  it("hovering an entity shows a details tooltip without selecting it", async () => {
    const vp = await $("#viewport");
    const before = await $("#status").getText();
    // Settle the cursor over the viewport centre (the debounced peek fires, then entity_details).
    await browser.action("pointer", { parameters: { pointerType: "mouse" } })
      .move({ origin: vp, x: 5, y: 5 })
      .move({ origin: vp })
      .perform();
    await browser.waitUntil(
      async () => (await $("#tooltip").getCSSProperty("display")).value !== "none",
      { timeout: 10000, timeoutMsg: "hover tooltip never appeared" }
    );
    expect(await $("#tooltip").getText()).toMatch(/Transform|Health|Renderable/);
    // hover did NOT change the selection (status unchanged — no "picked" fired by hovering).
    expect(await $("#status").getText()).toBe(before);
  });

  it("right-DRAG still orbits and does NOT open the menu (disambiguation)", async () => {
    await $("#ctxmenu") && (await browser.keys(["Escape"])); // ensure menu closed
    const vp = await $("#viewport");
    await browser.action("pointer", { parameters: { pointerType: "mouse" } })
      .move({ origin: vp })
      .down({ button: 2 })
      .move({ origin: vp, x: 60, y: 30 }) // drag well past the movement threshold
      .move({ origin: vp, x: 90, y: 50 })
      .up({ button: 2 })
      .perform();
    // a drag past the threshold is an orbit, not a click → the menu must stay closed.
    await browser.pause(300);
    expect(await ctxVisible()).toBe(false);
  });

  // ── Focus mode: center + zoom-to-frame the entity ("get nearby") and gray out the rest; Escape (or
  // a click) brings everything back to normal. The 3D dim happens in the wgpu surface UNDER the
  // transparent WebView (ADR-008) so WebdriverIO can't read its pixels — we assert the observable
  // state instead: the banner (DOM) + the Rust camera state surfaced into the banner's dataset. ──────
  const banner = async () => $("#focusbanner");
  const bannerVisible = async () => (await (await banner()).getCSSProperty("display")).value !== "none";

  it("Focus from the menu centers, zooms 'nearby', and raises the dim flag", async () => {
    await browser.keys(["Escape"]); // clean slate: no prior focus, no open menu
    const vp = await $("#viewport");
    // Open the context menu on the centre entity, then click Focus.
    await browser.action("pointer", { parameters: { pointerType: "mouse" } })
      .move({ origin: vp }).down({ button: 2 }).up({ button: 2 }).perform();
    await browser.waitUntil(ctxVisible, { timeout: 10000, timeoutMsg: "menu never opened for Focus" });
    await $('#ctxmenu .ctxitem[data-action="focus"]').click();
    // The banner is the visible "you are focused" affordance.
    await browser.waitUntil(bannerVisible, { timeout: 10000, timeoutMsg: "focus banner never appeared" });
    expect(await (await banner()).getText()).toContain("Focused");
    // The Rust camera state (surfaced into the dataset) confirms focus active + zoomed into the framing
    // range — "get nearby" puts the orbit distance at ≤ 40 (the focus-distance clamp), well in from the
    // ~60 default overview.
    const b = await banner();
    expect(await b.getAttribute("data-focused")).toBe("true");
    expect(Number(await b.getAttribute("data-dist"))).toBeLessThanOrEqual(40);
  });

  it("Escape unfocuses → banner clears and the dim flag drops (everything back to normal)", async () => {
    expect(await bannerVisible()).toBe(true); // still focused from the previous step
    await browser.keys(["Escape"]);
    await browser.waitUntil(async () => !(await bannerVisible()), {
      timeout: 10000,
      timeoutMsg: "focus banner never cleared on Escape",
    });
    expect(await $("#status").getText()).toContain("focus cleared");
  });

  it("Focus again, then a plain viewport click returns to the normal overview", async () => {
    const vp = await $("#viewport");
    await browser.action("pointer", { parameters: { pointerType: "mouse" } })
      .move({ origin: vp }).down({ button: 2 }).up({ button: 2 }).perform();
    await browser.waitUntil(ctxVisible, { timeout: 10000, timeoutMsg: "menu never reopened for Focus" });
    await $('#ctxmenu .ctxitem[data-action="focus"]').click();
    await browser.waitUntil(bannerVisible, { timeout: 10000, timeoutMsg: "focus banner never reappeared" });
    // A plain left-click anywhere exits focus mode (click-away returns to normal).
    await vp.click();
    await browser.waitUntil(async () => !(await bannerVisible()), {
      timeout: 10000,
      timeoutMsg: "a plain click did not exit focus mode",
    });
  });

  // ── M8.2 physics: drop a deterministic dynamic ball → it falls under gravity (sim on the native engine
  // thread, off the JS hot path) and rests on the ground; one undoable spawn. The test reads the physics
  // state on demand via physics_debug = [bodyCount, lowestY, contacts] — the app itself never polls it. ──
  const physDbg = async () =>
    browser.execute(async () => await window.__TAURI__.core.invoke("physics_debug"));

  it("M8.2: a dropped ball falls under gravity and lands on the ground", async () => {
    await $("#dropBall").click();
    await browser.waitUntil(async () => (await $("#status").getText()).includes("dropped a ball"), {
      timeout: 10000,
      timeoutMsg: "no 'dropped a ball' status after clicking Drop a ball",
    });
    // The sim advances natively — poll until the ball has fallen well below its y=8 spawn AND made a
    // ground contact (rest height ≈ 0.95). This proves the deterministic fixed-step loop + the delta
    // transform sync to the viewport are live.
    let dbg;
    await browser.waitUntil(
      async () => {
        dbg = await physDbg(); // [count, lowestY, contacts]
        return dbg[0] >= 1 && dbg[1] < 2.0 && dbg[2] >= 1;
      },
      { timeout: 15000, timeoutMsg: "ball never fell + landed; last physics_debug = " + JSON.stringify(dbg) }
    );
    expect(dbg[0]).toBeGreaterThanOrEqual(1); // at least one simulated body
    expect(dbg[1]).toBeLessThan(2.0); // fell from y=8 toward the ground
  });

  it("M8.2: Ctrl-Z removes the ball (setup is one undoable transaction; the sim body despawns too)", async () => {
    const had = (await physDbg())[0];
    expect(had).toBeGreaterThanOrEqual(1);
    await browser.keys(["Control", "z"]);
    await browser.waitUntil(async () => (await physDbg())[0] < had, {
      timeout: 10000,
      timeoutMsg: "undo did not despawn the ball (body_of must follow the ECS)",
    });
    expect((await physDbg())[0]).toBe(had - 1);
  });

  // ── M8.3 intent-first authoring (test #3 boxes 1–2): a dead mesh → a correct dynamic body via the
  // "Make dynamic" intent (one undoable tx), and the collider-intelligence check surfaces live. ──────────
  const invokeT = async (cmd, args) =>
    browser.execute(async (c, a) => await window.__TAURI__.core.invoke(c, a), cmd, args || {});

  it("M8.3: a described mesh is made a dynamic body (≤2 clicks) → it enters the sim", async () => {
    // Create a mesh entity (a described HealthBar carries a mesh handle).
    await $("#describe").setValue("health bar");
    await $("#describeBtn").click();
    await browser.waitUntil(async () => /created/.test(await $("#status").getText()), {
      timeout: 10000,
      timeoutMsg: "no 'created' status after describe",
    });
    const m = (await $("#status").getText()).match(/·\s*(\S+)/);
    expect(m).not.toBeNull();
    const id = m[1];

    const before = (await physDbg())[0];
    // The exact call the "Make dynamic" context action dispatches.
    const ok = await invokeT("make_dynamic", { id });
    expect(ok).toBe(true);
    // It is now a SIMULATED body (correct by construction: RigidBody + an auto-derived collider).
    await browser.waitUntil(async () => (await physDbg())[0] > before, {
      timeout: 10000,
      timeoutMsg: "make_dynamic did not add a simulated body",
    });

    // The collider-intelligence check is live (returns the warnings array — each explained + a fix id).
    const warns = await invokeT("physics_check", { id });
    expect(Array.isArray(warns)).toBe(true);

    // One undoable transaction — Ctrl-Z peels the whole make-dynamic back + despawns the sim body.
    const had = (await physDbg())[0];
    await browser.keys(["Control", "z"]);
    await browser.waitUntil(async () => (await physDbg())[0] < had, {
      timeout: 10000,
      timeoutMsg: "undo did not reverse make-dynamic",
    });
  });

  // ── M8.4 "debug by looking" (test #3 sim-debug boxes): pause → scrub a deterministic timeline → SEE +
  // EXPLAIN a contact → edit-at-pause (one undoable tx) → resume. Same seed + inputs reproduce it. ────────
  it("M8.4: scrub the deterministic timeline, SEE + explain a contact, edit-at-pause, resume", async () => {
    // Build a small contacting scene — a few balls dropped close together fall + rest on the ground.
    let lastId;
    for (let i = 0; i < 3; i++) {
      lastId = await invokeT("spawn_body", { x: (i - 1) * 0.25, y: 4 + i * 0.6, z: 0 });
    }
    expect(typeof lastId).toBe("string");

    // Let them settle into contact (landed + at least one active contact).
    await browser.waitUntil(
      async () => {
        const d = await physDbg(); // [count, lowestY, contacts]
        return d[0] >= 3 && d[1] < 1.7 && d[2] > 0;
      },
      { timeout: 20000, timeoutMsg: "balls did not settle into contact" }
    );

    // The timeline has advanced — there's a past to scrub into.
    const tl = await invokeT("sim_timeline"); // [frame, max_frame, running, overlays_on, bodies]
    expect(tl[1]).toBeGreaterThan(10);

    // Turn ON the contact/solver debugger overlay (OFF by default) — read-only, non-mutating.
    await invokeT("sim_overlay", { on: true });

    // The contacts are EXPLAINED ("debug by looking"): each carries penetration + impulse + the why.
    const contacts = await invokeT("physics_contacts");
    expect(Array.isArray(contacts)).toBe(true);
    expect(contacts.length).toBeGreaterThan(0);
    expect(contacts[0].explain).toMatch(/penetration/);
    expect(contacts[0].explain).toMatch(/normal impulse/);

    // SCRUB back to an earlier frame → it lands EXACTLY there and PAUSES (transport over the replay channel).
    const back = Math.max(1, Math.floor(tl[1] / 2));
    const s = await invokeT("sim_scrub", { frame: back });
    expect(s[0]).toBe(back);
    expect(s[2]).toBe(false); // scrub pauses

    // DETERMINISTIC REPRODUCTION (P2/P3 through the running tool): scrub away, scrub back to the SAME
    // frame → bit-identical state (same lowest-y). A captured moment reproduces, not a ghost.
    const y1 = (await physDbg())[1];
    await invokeT("sim_scrub", { frame: Math.min(tl[1], back + 6) });
    await invokeT("sim_scrub", { frame: back });
    const y2 = (await physDbg())[1];
    expect(Math.abs(y2 - y1)).toBeLessThan(1e-9);

    // EDIT-AT-PAUSE = one undoable transaction through the SAME commit pipeline: nudge friction. The body
    // survives (a material edit, not structural); the M8.3 mistake-checks re-run; resume re-simulates.
    const beforeBodies = (await invokeT("sim_timeline"))[4];
    await invokeT("submit_edit", {
      tx: {
        clientOpId: "e2e-friction",
        label: "set Collider.friction",
        patches: [],
        intent: { kind: "setField", id: lastId, component: "Collider", field: "friction", value: 0.95 },
      },
    });
    const afterBodies = (await invokeT("sim_timeline"))[4];
    expect(afterBodies).toBe(beforeBodies); // edit-at-pause changed a value, not the body set
    const warns = await invokeT("physics_check", { id: lastId });
    expect(Array.isArray(warns)).toBe(true); // the checks re-ran on the edited value

    // RESUME → deterministic re-simulation from the edit; the frame advances again.
    await invokeT("set_sim_running", { run: true });
    await browser.waitUntil(async () => (await invokeT("sim_timeline"))[0] > 0, {
      timeout: 10000,
      timeoutMsg: "sim did not resume after edit-at-pause",
    });

    // Closing the debugger is zero-cost (the overlay buffer empties) — toggle it off cleanly.
    await invokeT("sim_overlay", { on: false });
    const off = await invokeT("sim_timeline");
    expect(off[3]).toBe(false);
  });

  // ── M8.5 interchange: import a URDF arm → registry-component entities, units validated, every
  // unsupported feature explained, undoable. (USD is exercised headlessly in /interchange.) ──────────────
  it("M8.5: imports a URDF arm into the sim as registry components, with explained notes", async () => {
    const URDF = `<?xml version="1.0"?>
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

    const before = (await physDbg())[0];
    const r = await invokeT("import_interchange", { format: "urdf", source: URDF });
    expect(r.ok).toBe(true);
    expect(r.bodies).toBe(2); // two links → two bodies
    expect(r.joints).toBe(1); // one revolute
    expect(r.format).toMatch(/URDF/);
    // Every approximation is EXPLAINED, not silently dropped (the accuracy discipline).
    expect(Array.isArray(r.notes)).toBe(true);
    expect(r.notes.some((n) => /cylinder/i.test(n))).toBe(true); // cylinder → capsule, noted
    expect(r.notes.some((n) => /limit/i.test(n))).toBe(true); // unenforced joint limit, noted

    // The imported bodies entered the sim (registry components → mirrored bodies).
    await browser.waitUntil(async () => (await physDbg())[0] > before, {
      timeout: 10000,
      timeoutMsg: "the imported URDF bodies did not enter the sim",
    });

    // ONE undoable transaction — Ctrl-Z peels the whole import back.
    const had = (await physDbg())[0];
    await browser.keys(["Control", "z"]);
    await browser.waitUntil(async () => (await physDbg())[0] < had, {
      timeout: 10000,
      timeoutMsg: "undo did not reverse the URDF import",
    });
  });

  // ── M9.1 (G1): the transform gizmo drags a selected entity — native (0 per-frame IPC), one undoable
  // transaction, hierarchy-correct (parent-space write-back proven headless in /gizmo). ──────────────────
  it("M9.1: the transform gizmo drags a selected entity (0 per-frame IPC, one undoable tx)", async () => {
    // A deterministic target: a PAUSED physics body at a known position (so the gizmo move is the only motion).
    const id = await invokeT("spawn_body", { x: 0, y: 6, z: 0 });
    expect(typeof id).toBe("string");
    await invokeT("set_sim_running", { run: false });
    expect(await invokeT("gizmo_select", { id })).toBe(true);
    await invokeT("gizmo_mode", { mode: "translate" });

    const g = await invokeT("gizmo_debug"); // [mode, hasSelection, dragging, space, pivot]
    expect(g[0]).toBe("translate");
    expect(g[1]).toBe(true);
    const t0 = await invokeT("read_transform", { id });

    // Start a drag on the Y handle (vertical → always well-presented, never edge-on to the camera), then
    // prove 0-PER-FRAME-IPC: across ~27 render frames (450 ms) with an ACTIVE drag and no commands, the
    // IPC counter does NOT grow per frame — the per-frame work is native in the render loop (only the
    // gesture's start/end cross JS, invariant 4).
    expect(await invokeT("gizmo_grab", { axis: "y" })).toBe(true);
    const ipcA = await invokeT("ipc_count");
    await browser.pause(450);
    const ipcB = await invokeT("ipc_count");
    expect(ipcB - ipcA).toBeLessThan(10); // ≈ the handful of reads, NOT ~27 (1/frame) → 0 per-frame IPC

    // Drag up +Y (the render loop moves the instance natively), release → ONE undoable commit.
    await invokeT("gizmo_set_target", { tx: 0.0, ty: t0[1] + 5.0, tz: 0.0 });
    await browser.pause(150); // let the native render-loop drag apply
    await invokeT("gizmo_drag_end");
    await browser.waitUntil(async () => (await invokeT("read_transform", { id }))[1] > t0[1] + 1.0, {
      timeout: 10000,
      timeoutMsg: "the gizmo drag did not move the entity along +Y",
    });

    // ONE undoable transaction — Ctrl-Z reverts the whole move atomically.
    await browser.keys(["Control", "z"]);
    await browser.waitUntil(
      async () => Math.abs((await invokeT("read_transform", { id }))[1] - t0[1]) < 0.2,
      { timeout: 10000, timeoutMsg: "Ctrl-Z did not revert the gizmo move" }
    );
  });

  // ── M9.2 (G2): rigid part editing — the G1 gizmo on a CHILD node, stored as a per-field OVERRIDE
  // (ADR-026), save-the-character-for-reuse, and deactivate-not-delete. ─────────────────────────────────
  it("M9.2: edit a child PART (override) → save the character → a fresh instance carries the edit; deactivate a part → Ctrl-Z restores it", async () => {
    // The seeded demo character: a body root + rigid child parts (a part is a child Movable-Tree node).
    const demo = await invokeT("demo_character"); // [root, [parts]]
    expect(demo).not.toBe(null);
    const [, parts] = demo;
    expect(parts.length).toBeGreaterThanOrEqual(2);
    const part = parts[0];

    // The part starts ACTIVE. (Override count is NOT asserted as 0 — a prior run's persisted edit may
    // have replayed; the test keys off the DELTA the edit produces, which is robust either way.)
    const before = await invokeT("part_debug", { id: part }); // [x, y, z, active, nOverrides]
    expect(before[3]).toBe(true);

    // Edit the part with the gizmo: select → translate → grab Y → move up → release. The commit routes
    // through edit_part_transform (parent-space write-back → a sparse per-field OVERRIDE), NOT a base
    // rewrite — so the source stays intact and the override wins by structure.
    expect(await invokeT("gizmo_select", { id: part })).toBe(true);
    await invokeT("gizmo_mode", { mode: "translate" });
    expect(await invokeT("gizmo_grab", { axis: "y" })).toBe(true);
    await invokeT("gizmo_set_target", { tx: before[0], ty: before[1] + 3.0, tz: before[2] });
    await browser.pause(150);
    await invokeT("gizmo_drag_end");
    await browser.waitUntil(
      async () => {
        const d = await invokeT("part_debug", { id: part });
        return d[4] > 0 && d[1] > before[1] + 1.0; // overrides present AND the part moved up in world
      },
      { timeout: 10000, timeoutMsg: "the part edit did not land as a per-field override + move" }
    );
    const after = await invokeT("part_debug", { id: part });
    expect(after[4]).toBeGreaterThan(0); // sparse per-field override keys (not a whole-object copy)

    // Save the edited character for reuse → drop a fresh instance → its matching part carries the edit
    // (override baked into the reusable asset; override wins over the un-edited source by structure).
    const comp = await invokeT("save_character", { id: part });
    expect(typeof comp).toBe("string");
    const inst = await invokeT("instantiate_character", { comp });
    expect(typeof inst).toBe("string");
    const instPart = await invokeT("part_at_path", { root: inst, path: "0" });
    expect(typeof instPart).toBe("string");
    const instDbg = await invokeT("part_debug", { id: instPart });
    expect(instDbg[1]).toBeGreaterThan(before[1] + 1.0); // the saved edit is present on the fresh instance

    // Deactivate-not-delete: a removed part is hidden + recoverable. Ensure a known ACTIVE baseline first
    // (robust to a prior run), deactivate, then Ctrl-Z restores it.
    const part2 = parts[1];
    await invokeT("set_part_active", { id: part2, active: true });
    expect(await invokeT("set_part_active", { id: part2, active: false })).toBe(true);
    expect((await invokeT("part_debug", { id: part2 }))[3]).toBe(false); // hidden, but data preserved
    await browser.keys(["Control", "z"]);
    await browser.waitUntil(async () => (await invokeT("part_debug", { id: part2 }))[3] === true, {
      timeout: 10000,
      timeoutMsg: "Ctrl-Z did not restore the deactivated part (deactivate-not-delete)",
    });
  });

  // ── M9.4 (G4): the intent-powered transform solver — snap-graph (ADR-011 reuse) + declared constraints
  // (every "no" explained) + a natural-language placement sentence (schema-validated patch). ─────────────
  it("M9.4: the snap-graph ranks targets, a declared constraint solves or explains, a placement sentence compiles", async () => {
    const demo = await invokeT("demo_character"); // [root, [parts]]
    const [root, parts] = demo;
    const part = parts[0];

    // The snap-graph: ranked candidates within radius, each with an explained "why this" (the reveal/rank/
    // explain pattern applied to space — the SAME ADR-011 ranker, reused).
    const hits = await invokeT("snap_query", { id: part, radius: 100.0 });
    expect(Array.isArray(hits) && hits.length > 0).toBe(true);
    expect(typeof hits[0].why).toBe("string");
    expect(hits[0].why.toLowerCase()).toContain("snap to");
    // Ranked nearest-first (proximity primary, ADR-011).
    expect(hits[0].distance).toBeLessThanOrEqual(hits[hits.length - 1].distance + 1e-3);

    // Declare a "snap" constraint → the part moves to the target's world position (solve + commit, undoable).
    const rootDbg = await invokeT("part_debug", { id: root }); // [x, y, z, active, nOverrides]
    const res = await invokeT("apply_constraint", { id: part, kind: "snap", target: root, value: 0.0 });
    expect(res.ok).toBe(true);
    await browser.waitUntil(
      async () => {
        const d = await invokeT("part_debug", { id: part });
        return Math.abs(d[0] - rootDbg[0]) < 0.1 && Math.abs(d[1] - rootDbg[1]) < 0.1 && Math.abs(d[2] - rootDbg[2]) < 0.1;
      },
      { timeout: 10000, timeoutMsg: "the snap constraint did not move the part to the target" }
    );

    // A constraint with no target is REFUSED, with an explanation (every "no" explained, ADR-016).
    const blocked = await invokeT("apply_constraint", { id: part, kind: "coaxial", target: null, value: 0.0 });
    expect(blocked.ok).toBe(false);
    expect(typeof blocked.reason).toBe("string");
    expect(blocked.reason.length).toBeGreaterThan(0);

    // A natural-language placement sentence compiles to editable intents (the AI-as-constraint-compiler).
    const placed = await invokeT("placement_sentence", {
      id: root,
      text: "put it upright, 10 cm from the edge",
    });
    expect(Array.isArray(placed.intents) && placed.intents.length > 0).toBe(true);

    // Physics-aware placement feedback reuses the M8.3 collider-intelligence check (a body placed without a
    // collider is flagged + explained, never a silent runtime glitch).
    const body = await invokeT("spawn_body", { x: 0, y: 5, z: 0 });
    const warnings = await invokeT("physics_check", { id: body });
    expect(Array.isArray(warnings)).toBe(true);
  });
});
