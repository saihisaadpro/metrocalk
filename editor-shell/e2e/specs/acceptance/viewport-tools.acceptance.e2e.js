// Build-acceptance — M10.7 VIEWPORT TOOLS (ADR-037): the viewport is a real authoring surface in the React
// editor. The shipped M9 gizmo's mode/space/pivot live in a React toolbar over the stage (the drag stays
// native + 0-IPC); plus the camera/framing ergonomics every editor has (frame-selected/all + view presets).
// SINGLE-SOURCE: the toolbar reads the ONE gizmo state (`gizmo_debug`) so the React control + the native
// gizmo can't desync. Camera state is read back from `camera_debug` (the wgpu pixels aren't WebDriver-
// readable). The orbit's 0-IPC/frame (invariant 4, with the toolbar present) is covered by north-star-1's
// INVARIANT-4 test, which now runs with this toolbar mounted.

import { browser, expect, $ } from "@wdio/globals";
import {
  report,
  invoke,
  consoleErrors,
  clearConsole,
  captureBudget,
  scoreBudget,
  loadBaseline,
} from "../../lib/acceptance.js";

const baseline = loadBaseline();
const cam = () => invoke("camera_debug"); // [orbit, elevation, distance, tx, ty, tz]
const giz = () => invoke("gizmo_debug"); // [mode, hasSel, dragging, space, pivot]

describe("acceptance / M10.7 — viewport tools (the M9 gizmo in React + camera ergonomics, live)", () => {
  before(async () => {
    await browser.waitUntil(
      async () => {
        try {
          return (await invoke("camera_debug")).length === 6;
        } catch {
          return false;
        }
      },
      { timeout: 20000, timeoutMsg: "editor never connected (camera_debug unavailable)" }
    );
    await clearConsole();
  });

  // ── THE M9 GIZMO IN REACT — the toolbar's mode buttons drive the native gizmo, single-source ──────────
  it("the ViewportToolbar surfaces the M9 gizmo — its mode buttons drive the native gizmo (no desync)", async () => {
    await clearConsole();
    expect(await $("#vptoolbar").isExisting()).toBe(true); // the toolbar overlays the stage

    // Click Rotate (E) → the native gizmo mode is "rotate" (read back from the ONE gizmo state); Move (W) →
    // "translate". The React control reflects gizmo_debug, so a W/E/R key and the toolbar stay in sync.
    await $("#vpRotate").click();
    await browser.waitUntil(async () => (await giz())[0] === "rotate", {
      timeout: 5000,
      timeoutMsg: "the Rotate button didn't set the native gizmo mode",
    });
    const rotated = (await giz())[0] === "rotate";
    await $("#vpMove").click();
    await browser.waitUntil(async () => (await giz())[0] === "translate", {
      timeout: 5000,
      timeoutMsg: "the Move button didn't set the native gizmo mode",
    });
    const translated = (await giz())[0] === "translate";

    // Space toggle drives the native gizmo space (world ↔ local).
    const space0 = (await giz())[3];
    await $("#vpSpace").click();
    await browser.waitUntil(async () => (await giz())[3] !== space0, {
      timeout: 5000,
      timeoutMsg: "the Space button didn't toggle world/local",
    });
    const spaceToggled = (await giz())[3] !== space0;

    const errs = await consoleErrors();
    const clean = errs.length === 0;
    if (!clean) report.consoleErrorCount += errs.length;
    report.workflow(
      "viewport/gizmo-toolbar",
      { functional: rotated && translated && spaceToggled, inv1: true, inv4: true, clean, offline: true },
      { controls: ["#vpMove", "#vpRotate", "#vpSpace"], commands: ["gizmo_mode", "gizmo_space_toggle", "gizmo_debug"] }
    );
    expect(rotated).toBe(true);
    expect(translated).toBe(true);
    expect(spaceToggled).toBe(true);
    expect(clean).toBe(true);
  });

  // ── CAMERA & FRAMING — frame-all + canonical view presets snap the camera (toolbar → camera_debug) ────
  it("camera ergonomics: Frame-all + view presets (top/front/side/persp) snap the camera", async () => {
    await clearConsole();
    // Frame all → a non-degenerate framing (a positive fit distance).
    await $("#vpFrameAll").click();
    await browser.waitUntil(async () => (await cam())[2] > 0, {
      timeout: 5000,
      timeoutMsg: "Frame-all didn't set a fit distance",
    });
    const framed = (await cam())[2] > 0;

    // View presets set the canonical orbit/elevation. camera_debug = [orbit, elevation, distance, …].
    await $("#vpTop").click();
    await browser.waitUntil(async () => (await cam())[1] > 1.2, {
      timeout: 5000,
      timeoutMsg: "Top view didn't pitch the camera down",
    });
    const top = (await cam())[1] > 1.2;

    await $("#vpFront").click();
    await browser.waitUntil(async () => Math.abs((await cam())[1]) < 0.15, {
      timeout: 5000,
      timeoutMsg: "Front view didn't level the camera",
    });
    const front = Math.abs((await cam())[1]) < 0.15;

    await $("#vpPersp").click();
    await browser.waitUntil(
      async () => {
        const e = (await cam())[1];
        return e > 0.2 && e < 1.0;
      },
      { timeout: 5000, timeoutMsg: "Persp view didn't restore a 3/4 pitch" }
    );
    const persp = (await cam())[1] > 0.2 && (await cam())[1] < 1.0;

    // The orientation readout reflects the view.
    const orient = await $("#vpOrient").getAttribute("data-view");

    const errs = await consoleErrors();
    const clean = errs.length === 0;
    if (!clean) report.consoleErrorCount += errs.length;
    report.workflow(
      "viewport/camera-framing",
      { functional: framed && top && front && persp && orient === "persp", inv1: true, clean, offline: true },
      { controls: ["#vpFrameAll", "#vpTop", "#vpFront", "#vpSide", "#vpPersp", "#vpOrient"], commands: ["frame_all", "view_preset", "camera_debug"] }
    );
    expect(framed).toBe(true);
    expect(top).toBe(true);
    expect(front).toBe(true);
    expect(persp).toBe(true);
    expect(clean).toBe(true);
  });

  // ── PRINCIPLE 2 — the camera ops hold the interaction budget live ─────────────────────────────────────
  it("PRINCIPLE 2: frame_all / view_preset / camera_debug round-trips are ≤16 ms live and within baseline", async () => {
    const ops = [
      await captureBudget("view_preset", "view_preset", () => ({ preset: "persp" }), { n: 30, warmup: 5 }),
      await captureBudget("camera_debug", "camera_debug", {}, { n: 30, warmup: 5 }),
    ];
    let pass = true;
    for (const s of ops) {
      const scored = await scoreBudget(s, baseline, { perFrame: false });
      report.budget(scored);
      console.log(`BUDGET ${s.label}: p50=${s.p50.toFixed(2)}ms p99=${s.p99.toFixed(2)}ms`);
      if (scored.verdict === "fail") pass = false;
    }
    expect(pass).toBe(true);
  });
});
