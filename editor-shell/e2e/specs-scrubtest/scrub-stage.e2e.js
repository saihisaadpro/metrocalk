// M14.3 — the ONE owed Accepted-tier gate (ADR-059): a NumericField *scrub* in the inspector VISIBLY
// transforms the selected entity ON THE STAGE, as ONE coalesced, undoable, optimistic transaction, on the
// REAL /core. The proof is REAL PIXELS of the composited native wgpu viewport (CopyFromScreen of the .exe
// window — a DOM/WebDriver screenshot sees only the transparent overlay and is INVALID here).
//
// Experimental design (a controlled diff, so movement can't be confused with temporal/AA noise):
//   A   = before (selected, settled)
//   A2  = control (NO edit between A and A2)        → diff(A,A2) = the noise floor
//   ── scrub Transform.y via a real synthetic drag on the NumericField (mousedown → window mousemoves → up)
//   B   = after the scrub                           → diff(A,B)  MUST be >> the floor  (the cube moved)
//   ── Ctrl+Z (one undo)
//   C   = after undo                                → diff(B,C)  >> floor (moved back) AND diff(A,C) ≈ floor
// Plus the structured signals: read_transform before/after/reverted (the tx reached the real core + one undo
// restored it = ONE coalesced tx); data-scrubbing 1→0 (the drag engaged); ipc_count proves the whole drag
// streamed NO per-move IPC and committed exactly ONCE (invariant 4 under scrub).
//
// Y axis: world-up projects to strong screen-vertical motion for ANY camera azimuth (orbit is about Y), so
// the move is visible regardless of what `view_preset persp` sets — robust where X (≈ along the view) is not.

import { browser } from "@wdio/globals";
import { execSync } from "node:child_process";
import { mkdirSync, rmSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const E2E = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const SHOT_PS1 = process.env.MTK_SHOT_PS1 || path.resolve(E2E, "../../.uxtest/audit/exe/capture-window-fg.ps1");
const PNGDIFF_PS1 = process.env.MTK_PNGDIFF_PS1 || path.resolve(E2E, "../../.uxtest/audit/exe/pngdiff.ps1");
const SHOT_DIR = process.env.MTK_SHOT_DIR || path.resolve(E2E, ".shots-scrub");
const PROC = "metrocalk-editor-shell";

try {
  rmSync(SHOT_DIR, { recursive: true, force: true });
} catch {
  /* fresh */
}
mkdirSync(SHOT_DIR, { recursive: true });

const invoke = (cmd, args = {}) =>
  browser.execute(async (c, a) => window.__TAURI__.core.invoke(c, a), cmd, args);

const shot = async (label) => {
  await browser.pause(550); // let the render loop present the new state for a few frames
  const out = path.join(SHOT_DIR, `${label}.png`);
  execSync(`powershell -ExecutionPolicy Bypass -File "${SHOT_PS1}" -Out "${out}" -ProcName "${PROC}"`, {
    stdio: "ignore",
  });
  return out;
};

// Crop the diff to the central wgpu STAGE (exclude the chrome: left tree, right inspector/AI-card/diagnostics,
// the status bar) so the diff measures the VIEWPORT — what "moves on the stage" means — not panel drift (the
// inspector group order / value text / async diagnostics all change over a long run; that is not the stage).
const VIEWPORT = { l: 0.21, t: 0.19, r: 0.73, b: 0.96 };
const diff = (a, b) => {
  const raw = execSync(
    `powershell -ExecutionPolicy Bypass -File "${PNGDIFF_PS1}" -A "${a}" -B "${b}" -CropL ${VIEWPORT.l} -CropT ${VIEWPORT.t} -CropR ${VIEWPORT.r} -CropB ${VIEWPORT.b}`,
    { encoding: "utf8" },
  );
  const line = raw.trim().split(/\r?\n/).filter(Boolean).pop();
  return JSON.parse(line);
};

// Readiness = the hierarchy has streamed in real rows (proves the projection connected to /core AND that
// there are selectable rows). The M14.1 redesign moved the old `#count` into `#status` ("connected · N
// entities"); hrow presence is the robust, selector-stable signal the scrub actually needs.
const hrowCount = () => browser.execute(() => document.querySelectorAll('[data-testid="hrow"]').length);
const statusText = () => browser.execute(() => document.querySelector("#status")?.textContent || "");

// Drive a REAL drag-scrub, SPLIT so the spec can read the LIVE mid-drag state in a later tick: `data-scrubbing`
// is React state — it only flips to "1" AFTER React flushes, which never happens inside the same execute as
// the synchronous moves. dragStart = mousedown + N window mousemoves (NO release); dragRelease = window
// mouseup → the SINGLE coalesced onCommit → handleChange → client.setField → submit_edit (the ADR-010 tx).
const dragStart = (testid, dxPixels, moves) =>
  browser.execute(
    (sel, dx, n) => {
      const el = document.querySelector(`[data-testid="${sel}"]`);
      if (!el) return { ok: false, reason: "field not found" };
      const r = el.getBoundingClientRect();
      const x0 = Math.round(r.left + r.width / 2);
      const y0 = Math.round(r.top + r.height / 2);
      window.__sx = x0;
      window.__sy = y0;
      window.__sdx = dx;
      el.dispatchEvent(new MouseEvent("mousedown", { clientX: x0, clientY: y0, button: 0, bubbles: true }));
      for (let i = 1; i <= n; i++) {
        const cx = x0 + Math.round((dx * i) / n);
        window.dispatchEvent(new MouseEvent("mousemove", { clientX: cx, clientY: y0, bubbles: true }));
      }
      return { ok: true };
    },
    testid,
    dxPixels,
    moves,
  );
const dragRelease = () =>
  browser.execute(() => {
    window.dispatchEvent(
      new MouseEvent("mouseup", { clientX: window.__sx + window.__sdx, clientY: window.__sy, bubbles: true }),
    );
    return true;
  });
const attr = (testid, name) =>
  browser.execute((sel, n) => document.querySelector(`[data-testid="${sel}"]`)?.getAttribute(n), testid, name);

describe("M14.3 — a NumericField scrub VISIBLY moves the entity on the native stage (ADR-059)", () => {
  const AXIS = "num-Transform.y";
  const IDX = 1; // read_transform index for the scrubbed axis (y)
  let id, v0, d, s, dx, sign;
  const frames = {};
  const diffs = {};
  let scrubRes, txAfter, txReverted, ipcDelta, scrubLatencyMs, commitBench;

  before(async () => {
    await browser.waitUntil(async () => (await hrowCount()) > 0, {
      timeout: 60000,
      interval: 1000,
      timeoutMsg: "editor never connected to the real /core (no hierarchy rows streamed in)",
    });
    console.log(`  connected · ${await hrowCount()} hierarchy rows · status="${await statusText()}"`);
    // Dismiss the first-run onboarding overlay (it blocks the lower viewport / the inspector).
    await browser.execute(() => {
      try {
        localStorage.setItem("mtk.onboarded.v1", "1");
      } catch (e) {
        void e;
      }
      const b = [...document.querySelectorAll("button")].find((x) => /skip/i.test(x.textContent || ""));
      if (b) b.click();
      // Collect any runtime errors for the no-console-errors gate.
      window.__scrubErrors = [];
      window.addEventListener("error", (e) => window.__scrubErrors.push(String((e && e.message) || e)));
      window.addEventListener("unhandledrejection", (e) => window.__scrubErrors.push("rej:" + String(e.reason)));
      const ce = console.error.bind(console);
      console.error = (...a) => {
        window.__scrubErrors.push(a.map(String).join(" "));
        ce(...a);
      };
    });
    // Clear the composed demo character so the moving cube is unambiguous against a clean grid.
    const dc = await invoke("demo_character").catch(() => null);
    if (Array.isArray(dc)) {
      const [root, parts] = dc;
      for (const pid of parts || []) await invoke("remove_entity", { id: pid }).catch(() => {});
      if (root) await invoke("remove_entity", { id: root }).catch(() => {});
    }
    // A known wide framing (the fbx-campaign-proven recipe: persp + frame_all → a translate visibly shifts
    // against the grid). Keep the camera FIXED across all four captures (only frame_all here, never between).
    await invoke("view_preset", { preset: "persp" }).catch(() => {});
    await invoke("frame_all").catch(() => {});
    await browser.pause(400);
  });

  it("setup: select a real /core entity → the inspector renders its real Transform.y field", async () => {
    // Pick the first hierarchy row's REAL entity id (the real /core scene, not MockCore).
    id = await browser.execute(() => document.querySelector('[data-testid="hrow"]')?.getAttribute("data-id"));
    expect(typeof id).toBe("string");
    // Selecting via the hierarchy row drives the REACT store (selectedId) → the inspector; gizmo_select would
    // set only the ENGINE selection (the M14.3 inspector reads the React store — the m14-thumbnails finding).
    await browser.execute((i) => document.querySelector(`[data-testid="hrow"][data-id="${i}"]`)?.click(), id);
    await browser.pause(400);
    const present = await browser.execute((sel) => !!document.querySelector(`[data-testid="${sel}"]`), AXIS);
    expect(present).toBe(true); // the inspector rendered the REAL Transform.y NumericField for the selection

    const t0 = await invoke("read_transform", { id }); // [x,y,z,qx,qy,qz,qw,scale]
    v0 = Number(t0[IDX]);
    const cam = await invoke("camera_debug").catch(() => null);
    d = Array.isArray(cam) && cam[2] ? Number(cam[2]) : 12; // framed camera distance
    s = Math.max(1.5, +(d * 0.18).toFixed(3)); // the fbx-proven visible-but-in-frame world step
    sign = v0 > 0 ? -1 : 1; // move toward centre so the cube stays well inside the frame
    dx = sign * Math.round(s * 10); // 0.1 value/px (float step) → |dx*0.1| ≈ s ; |dx| ≫ the 3px threshold
    console.log(`  id=${id} v0=${v0.toFixed(3)} d=${d.toFixed(2)} s=${s} dx=${dx}`);
  });

  it("captures the BEFORE frame + a CONTROL frame (the no-edit noise floor)", async () => {
    frames.A = await shot("00_before");
    frames.A2 = await shot("01_control_noedit");
    diffs.control = diff(frames.A, frames.A2);
    console.log(`  control diff: ${JSON.stringify(diffs.control)}`);
    // A static scene must read near-identical frame-to-frame, or the movement test is meaningless.
    expect(diffs.control.frac).toBeLessThan(0.01);
  });

  it("a scrub-drag is LIVE then commits ONE coalesced tx at release — no per-move commit/IPC (invariant 4)", async () => {
    const vStart = Number((await invoke("read_transform", { id }))[IDX]);
    const ipc0 = await invoke("ipc_count");
    const t = Date.now();
    const started = await dragStart(AXIS, dx, 40); // 40 moves — a per-move-commit bug would fire ~N txs
    expect(started.ok).toBe(true);
    await browser.pause(150); // let React flush the scrubbing state so data-scrubbing repaints
    const scrubbingMid = await attr(AXIS, "data-scrubbing");
    const vMid = Number((await invoke("read_transform", { id }))[IDX]);
    await dragRelease();
    scrubLatencyMs = Date.now() - t;
    await browser.pause(250); // let the commit reconcile + the engine apply
    const ipc1 = await invoke("ipc_count");
    const scrubbingAfter = await attr(AXIS, "data-scrubbing");
    txAfter = await invoke("read_transform", { id });
    ipcDelta = ipc1 - ipc0;
    const moved = Number(txAfter[IDX]) - vStart;
    scrubRes = { scrubbingMid, scrubbingAfter, vStart, vMid, vAfter: Number(txAfter[IDX]), moved };
    console.log(`  scrub: ${JSON.stringify(scrubRes)} ipcDelta=${ipcDelta} latency=${scrubLatencyMs}ms`);

    expect(scrubbingMid).toBe("1"); // the drag engaged LIVE (a real scrub, not a click)
    expect(scrubbingAfter).toBe("0"); // released cleanly
    expect(Math.abs(vMid - vStart)).toBeLessThan(0.001); // NO per-move commit — the core didn't move mid-drag
    // ONE commit at release moved the REAL core by ~the scrub delta, in the dragged direction (optimistic tx).
    expect(Math.sign(moved)).toBe(sign);
    expect(Math.abs(moved - sign * s)).toBeLessThan(Math.max(0.6, s * 0.25));
    // 40 moves did NOT flood commits (one tx, not N): a per-move-commit bug pushes ipcDelta toward ~40.
    // Tolerates a stray deferred IPC; the authoritative coalescing proof is the single-undo-full-revert below.
    expect(ipcDelta).toBeLessThan(8);
  });

  it("the scrub VISIBLY moved the entity on the stage (real composited pixels) — the Accepted-tier proof", async () => {
    frames.B = await shot("02_after_scrub");
    diffs.move = diff(frames.A, frames.B);
    console.log(`  MOVE diff: ${JSON.stringify(diffs.move)} vs control ${JSON.stringify(diffs.control)}`);
    // THE GATE: the before→after stage pixels differ FAR more than the no-edit control → the cube visibly
    // transformed on the native viewport (not a re-framed thumbnail, not a DOM screenshot).
    expect(diffs.move.changed).toBeGreaterThan(diffs.control.changed * 6 + 300);
    expect(diffs.move.frac).toBeGreaterThan(0.004);
  });

  it("ONE undo restores BOTH the value AND the stage pixels (the coalesced-tx + reconcile proof)", async () => {
    await browser.execute(() => document.activeElement && document.activeElement.blur());
    await browser.keys(["Control", "z"]); // one undo step — not the input's text undo
    await browser.pause(400);
    txReverted = await invoke("read_transform", { id });
    console.log(`  after undo read_transform.y = ${Number(txReverted[IDX]).toFixed(3)} (want ≈${v0.toFixed(3)})`);
    expect(Math.abs(Number(txReverted[IDX]) - v0)).toBeLessThan(0.15); // one undo reverted the whole scrub

    frames.C = await shot("03_after_undo");
    diffs.restore = diff(frames.B, frames.C); // B→C: the undo visibly moved the cube/gizmo back
    diffs.returned = diff(frames.A, frames.C); // A→C: the stage returned to the start (≈ control floor)
    console.log(`  restore diff: ${JSON.stringify(diffs.restore)}`);
    console.log(`  returned diff: ${JSON.stringify(diffs.returned)}`);
    expect(diffs.restore.changed).toBeGreaterThan(diffs.control.changed * 5 + 300); // undo VISIBLY moved it back
    // The stage returned toward the start: A→C is far smaller than the move A→B (the cube/gizmo is back where
    // it began). Relative (not an absolute floor) so it's robust to viewport temporal noise over the run.
    expect(diffs.returned.changed).toBeLessThan(diffs.move.changed * 0.5);
  });

  it("commit latency holds under repeated rapid scrub commits (the under-scrub budget)", async () => {
    // Fire a burst of commits on the SAME field (what a fast scrub coalesces into one of, but here unthrottled)
    // and time the submit_edit end-to-end via the real .exe IPC. End-to-end includes the WebDriver round-trip,
    // so it is an UPPER bound vs the ~1.5 ms headless commit; the point is scrubbing never STALLS the pipeline.
    const base = Number((await invoke("read_transform", { id }))[IDX]);
    const samples = [];
    for (let k = 0; k < 24; k++) {
      const val = base + (k % 2 === 0 ? 0.4 : -0.4);
      const tx = {
        clientOpId: `scrub-bench-${k}`,
        label: "set Transform.y",
        patches: [{ op: "replace", path: `/entities/${id}/components/Transform/y`, value: val }],
        intent: { kind: "setField", id, component: "Transform", field: "y", value: val },
      };
      const t = Date.now();
      await browser.execute(async (x) => window.__TAURI__.core.invoke("submit_edit", { tx: x }), tx);
      samples.push(Date.now() - t);
    }
    // Leave the field where it started (the last write put it at base; restore exactly).
    await browser.execute(async (i, v) => window.__TAURI__.core.invoke("submit_edit", {
      tx: { clientOpId: "scrub-bench-restore", label: "set Transform.y", patches: [{ op: "replace", path: `/entities/${i}/components/Transform/y`, value: v }], intent: { kind: "setField", id: i, component: "Transform", field: "y", value: v } },
    }), id, base);
    samples.sort((a, b) => a - b);
    const at = (q) => samples[Math.min(samples.length - 1, Math.floor(samples.length * q))];
    commitBench = { n: samples.length, p50: at(0.5), p99: at(0.99), max: samples[samples.length - 1] };
    console.log(`  commit-under-scrub (end-to-end .exe IPC): ${JSON.stringify(commitBench)}`);
    expect(commitBench.p99).toBeLessThan(80); // generous ceiling — scrubbing must never stall the commit pipe
  });

  it("no runtime / console errors across the scrub run", async () => {
    const errs = await browser.execute(() => window.__scrubErrors || []);
    if (errs.length) console.log("  errors:", JSON.stringify(errs));
    expect(errs).toEqual([]);
  });

  after(() => {
    const summary = {
      id,
      axis: AXIS,
      v0,
      cameraDistance: d,
      step_s: s,
      dxPixels: dx,
      readTransform: { before: v0, after: txAfter && Number(txAfter[IDX]), reverted: txReverted && Number(txReverted[IDX]) },
      scrub: scrubRes,
      ipcDelta,
      scrubLatencyMs,
      commitBench,
      diffs,
      frames,
    };
    // eslint-disable-next-line no-console
    console.log("\nM14.3 SCRUB-STAGE SUMMARY\n" + JSON.stringify(summary, null, 2));
  });
});
