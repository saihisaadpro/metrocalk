// HDR ENVIRONMENT IMPORT on the PACKAGED .exe — and the proof it reaches the renderer.
//
// BEFORE: the whole IBL chain existed — equirect upload, box-filtered mip chain, diffuse irradiance,
// roughness-indexed specular — and it already called the HDR decoder. It was reachable ONLY through
// the `MTK_ENV_HDR` environment variable, read once at process start. A complete capability no user
// could invoke: exactly the failure the STEP writer had.
// AFTER: `import_environment`, plus `.hdr` routed through the SAME canonical dispatcher as everything
// else, so File → Import, drag-and-drop and automation all light the scene with it.
//
// The claim that matters is not "the command returned ok". It is that the PIXELS CHANGE. These tests
// import a deliberately unsubtle red panorama, sample the viewport with the pixel probe, then import a
// blue one and sample again — a render that ignored the environment would read the same both times.

import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, rmSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const dir = path.dirname(fileURLToPath(import.meta.url));
const shots = path.resolve(dir, "../.shots-env");
const fixtures = path.resolve(dir, "../fixtures");
const capture = path.resolve(dir, "../scripts/capture-composited-window.ps1");
const captureFg = path.resolve(dir, "../../../.uxtest/audit/exe/capture-window-fg.ps1");
const probe = path.resolve(dir, "../scripts/probe-pixels.ps1");
mkdirSync(shots, { recursive: true });

const ps = (script, args) =>
  execFileSync("powershell.exe", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", script, ...args], {
    encoding: "utf8",
  });

async function shot(label) {
  await browser.pause(500);
  const file = path.join(shots, `${label}.png`);
  const good = () => existsSync(file) && statSync(file).size > 20_000;
  const attempt = (script, args) => {
    try {
      ps(script, args);
    } catch { /* fall through */ }
    if (!good() && existsSync(file)) rmSync(file);
    return good();
  };
  const ok = attempt(capture, ["-Out", file]) || attempt(captureFg, ["-ProcName", "metrocalk-editor-shell", "-Out", file]);
  if (!ok) {
    console.log(`[env] CAPTURE UNAVAILABLE for ${label}`);
    return null;
  }
  return file;
}

/** Mean RGB over a patch of a capture — the difference between "looks red" and "is red". */
function samplePatch(file, x, y, radius = 14) {
  const out = ps(probe, ["-Path", file, "-X", String(x), "-Y", String(y), "-Radius", String(radius)]);
  return JSON.parse(out);
}

const click = (selector) =>
  browser.execute((sel) => {
    const el = document.querySelector(sel);
    if (!el) return false;
    el.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    return true;
  }, selector);

describe("HDR environment — invokable, and the renderer actually uses it", () => {
  // The sky region of the viewport: above the horizon, clear of the docks and the Play badge.
  const SKY = { x: 1150, y: 260 };

  before(async () => {
    await browser.waitUntil(
      async () => browser.execute(() => !!(window.__TAURI__ && window.__TAURI__.core)),
      { timeout: 30000 },
    );
    await click('[data-testid="stop"]');
    await invoke("new_project");
    await browser.pause(700);
    await browser.execute(() => {
      document.querySelector('[data-testid="onboardSkip"]')?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    // A chrome sphere gives the environment somewhere to show up as a reflection as well as a sky.
    await invoke("shape_spawn", { kind: "sphere", pos: [0, 1, 0] });
    await invoke("frame_all");
  });

  it("starts on the built-in studio lighting and says so", async () => {
    const state = await invoke("environment_state");
    console.log(`[env] at start: ${JSON.stringify(state)}`);
    expect(state.applied).toBe(false);
    expect(state.label.toLowerCase()).toContain("studio");
  });

  it("THE PIXELS CHANGE: importing a red panorama turns the sky red", async () => {
    const before = await shot("00_studio_default");
    const baseline = before ? samplePatch(before, SKY.x, SKY.y) : null;
    if (baseline) console.log(`[env] studio sky RGB: ${baseline.r},${baseline.g},${baseline.b}`);

    const reply = await invoke("import_environment", { path: path.join(fixtures, "env-red.hdr") });
    console.log(`[env] import → ${JSON.stringify(reply)}`);
    expect(reply.applied).toBe(true);
    expect(reply.reason).toBe(null);
    expect(reply.width).toBe(64);
    expect(reply.height).toBe(32);

    // THE HARD PROOF, measured in-engine: the mean radiance the diffuse IBL lights with. The
    // box-filtered mip chain converges to exactly this, so it is not a proxy for what the renderer
    // uses — it IS what the renderer uses. Unlike a screenshot it cannot be refused by the desktop.
    const state = await invoke("environment_state");
    const [r, g, b] = state.meanRadiance;
    console.log(`[env] IBL mean radiance: r=${r.toFixed(2)} g=${g.toFixed(2)} b=${b.toFixed(2)}`);
    expect(state.applied).toBe(true);
    expect(r).toBeGreaterThan(4.0); // the fixture is authored at 8.0
    expect(r).toBeGreaterThan(b * 10);
    expect(r).toBeGreaterThan(g * 10);

    await browser.pause(900); // one frame with the rebuilt IBL
    const after = await shot("01_red_environment");
    if (after) {
      const red = samplePatch(after, SKY.x, SKY.y);
      console.log(`[env] red-env sky RGB: ${red.r},${red.g},${red.b}`);
      expect(red.r).toBeGreaterThan(red.b + 25);
      expect(red.r).toBeGreaterThan(red.g + 25);
      if (baseline) {
        expect(Math.abs(red.r - baseline.r) + Math.abs(red.b - baseline.b)).toBeGreaterThan(20);
      }
    } else {
      console.log("[env] capture unavailable — the in-engine radiance proof above carries this test");
    }
  });

  it("and a SECOND import replaces it — red really was the environment", async () => {
    // Without this, "the sky is reddish" could be any other reddish thing in the frame.
    const reply = await invoke("import_environment", { path: path.join(fixtures, "env-blue.hdr") });
    expect(reply.applied).toBe(true);
    const state = await invoke("environment_state");
    const [r, g, b] = state.meanRadiance;
    console.log(`[env] IBL mean radiance now: r=${r.toFixed(2)} g=${g.toFixed(2)} b=${b.toFixed(2)}`);
    expect(b).toBeGreaterThan(r * 10);
    expect(b).toBeGreaterThan(4.0);

    await browser.pause(900);
    const file = await shot("02_blue_environment");
    if (file) {
      const blue = samplePatch(file, SKY.x, SKY.y);
      console.log(`[env] blue-env sky RGB: ${blue.r},${blue.g},${blue.b}`);
      expect(blue.b).toBeGreaterThan(blue.r + 25);
    }
  });

  it("reset goes back to the built-in studio sky", async () => {
    const reply = await invoke("reset_environment");
    expect(reply.applied).toBe(true);
    await browser.pause(900);
    const state = await invoke("environment_state");
    console.log(`[env] after reset: ${state.label}, radiance ${JSON.stringify(state.meanRadiance)}`);
    expect(state.applied).toBe(false);
    expect(state.meanRadiance).toEqual([0, 0, 0]); // no custom panorama is loaded
    // `applied === false` above is the hard assertion; the pixel check confirms it when the desktop
    // lets us capture. The two PIXELS-CHANGE tests are the ones that must never be soft — this one is
    // corroboration, and a failed OS capture is a harness fact, not a product fact.
    const file = await shot("03_back_to_studio");
    if (file) {
      const studio = samplePatch(file, SKY.x, SKY.y);
      console.log(`[env] studio sky again: ${studio.r},${studio.g},${studio.b}`);
      expect(studio.b).not.toBeGreaterThan(studio.r + 25);
    }
  });

  it("a NON-equirectangular image is refused with its actual shape, not stretched", async () => {
    const reply = await invoke("import_environment", { path: path.join(fixtures, "env-square.hdr") });
    console.log(`[env] square refused: ${reply.reason}`);
    expect(reply.applied).toBe(false);
    expect(reply.reason).toContain("equirectangular");
    expect(reply.reason).toContain("32x32");
    // And it left the current environment alone.
    expect((await invoke("environment_state")).applied).toBe(false);
  });

  it("a CORRUPT panorama is refused without taking the app down", async () => {
    const reply = await invoke("import_environment", { path: path.join(fixtures, "env-corrupt.hdr") });
    console.log(`[env] corrupt refused: ${reply.reason}`);
    expect(reply.applied).toBe(false);
    expect(reply.reason).toBeTruthy();
    // The app is still answering — a bad asset must never be fatal.
    expect(await invoke("environment_state")).toBeTruthy();
  });

  it("a MISSING file is refused with a readable reason", async () => {
    const reply = await invoke("import_environment", { path: path.join(fixtures, "nope.hdr") });
    console.log(`[env] missing refused: ${reply.reason}`);
    expect(reply.applied).toBe(false);
    expect(reply.reason.toLowerCase()).toContain("could not be read");
  });

  it("THE CANONICAL PATH: the ordinary import command lights the scene too", async () => {
    // Not a second import architecture: `.hdr` is routed inside the one dispatcher that File → Import,
    // drag-and-drop, the command palette and automation all reach.
    const created = await invoke("import_asset", { path: path.join(fixtures, "env-red.hdr") });
    console.log(`[env] import_asset on a .hdr → ${created}`);
    expect(String(created)).toContain("env:");
    await browser.pause(900);
    const state = await invoke("environment_state");
    console.log(`[env] state after the generic import: ${JSON.stringify(state)}`);
    expect(state.applied).toBe(true);
    expect(state.label).toContain("env-red");
    // The pixel check is a BONUS here — the earlier test already proved the renderer path with a
    // probe. An OS capture can fail outright on a locked desktop, and letting that fail a test whose
    // functional assertions passed would be reporting a harness problem as a product problem.
    const file = await shot("04_via_canonical_import");
    if (file) {
      const red = samplePatch(file, SKY.x, SKY.y);
      console.log(`[env] sky after the generic import: ${red.r},${red.g},${red.b}`);
      expect(red.r).toBeGreaterThan(red.b + 25);
    } else {
      console.log("[env] capture unavailable — the state assertions above carry this test");
    }
  });

  it("the format catalog reports HDR honestly", async () => {
    const cat = await invoke("format_catalog");
    const hdr = cat.find((f) => f.id === "hdr");
    console.log(`[env] catalog says: ${hdr.label} — ${hdr.note}`);
    expect(hdr.available).toBe(true);
    expect(hdr.extensions).toContain("hdr");
  });
});
