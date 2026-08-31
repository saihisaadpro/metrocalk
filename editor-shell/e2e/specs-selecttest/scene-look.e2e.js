// **The scene's look, on the packaged `.exe` and against the real renderer.**
//
// `specs-interchange/environment.e2e.js` already proves the ENGINE half: an imported panorama reaches
// the renderer and the pixels change. What that spec cannot say — because it invokes the command
// directly — is whether a person can get to it. This is the other half.
//
// THREE CLAIMS, and none of them is "the panel renders":
//
//  1. **The panel and the engine agree.** Whatever `environment_state` says on the real core is what
//     the section shows. A jsdom test necessarily photographs the mock.
//  2. **The exposure slider reaches the RENDERER.** Moved through the UI, read back from
//     `colour_status` — which reads the render state rather than restating it, so a slider that
//     updated only its own React state fails here and looks perfect in vitest.
//  3. **The sky survives a save and a reopen.** Import, save, reopen, and ask what it is lit by. This
//     is the whole point of the `.view.json` sidecar and it is the one claim no unit test can reach:
//     it spans two processes' worth of state through a file on disk.
//
// WHAT THIS SPEC DELIBERATELY DOES NOT DO: click `Use a sky image…`. That button opens a NATIVE modal
// file dialog, which on an unattended run would block the WebDriver session until the timeout with no
// way to dismiss it. The button's wiring is covered by vitest (it calls `importEnvironment()` with no
// argument, which is precisely what opens the picker); what needs the real engine is everything after
// a path exists, and that is what is driven here.

import { existsSync, mkdirSync, readFileSync, rmSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const dir = path.dirname(fileURLToPath(import.meta.url));
const fixtures = path.resolve(dir, "../fixtures");
const shots = process.env.MTK_SHOT_DIR || path.resolve(dir, "../.shots-selecttest");
mkdirSync(shots, { recursive: true });

// A project path this spec owns, beside the fixtures rather than in the app's own directory: the
// sidecar is written as `<project>.view.json`, and the assertion reads that exact file.
const projectFile = path.join(shots, "scene-look.mtk");
const sidecar = `${projectFile}.view.json`;

const sleep = (ms) => browser.pause(ms);

/** DOM screenshot, deliberately: this feature is entirely DOM. Nothing here is the render, so an OS
 *  window capture would be a stronger-looking instrument answering a question nobody asked — and
 *  `capture-window-fg.ps1` photographs the FOREGROUND window, which on an unattended box is whatever
 *  else has focus. `browser.saveScreenshot` cannot be misaimed. */
const shot = async (label) => {
  await sleep(500);
  const out = path.join(shots, `${label}.png`);
  await browser.saveScreenshot(out);
  console.log("  shot", out);
};

/** Open the Scene workspace and expand the Lighting & sky section, then return its readings. */
async function openLookSection() {
  await browser.execute(() => {
    document.querySelector('[data-testid="engine-tab-scene"], #engine-tab-scene')?.click();
  });
  await sleep(300);
  // The section is closed by default (`unmountOnClose`), which is the whole reason the hierarchy keeps
  // the dock's fill. Opening it is the gesture a person makes.
  await browser.execute(() => {
    const header = document.querySelector('#scene-look button, [id="scene-look"] button');
    if (header && header.getAttribute("aria-expanded") === "false") header.click();
  });
  await browser.waitUntil(
    async () => (await browser.execute(() => !!document.querySelector('[data-testid="look-section"]'))) === true,
    { timeout: 15000, timeoutMsg: "the Lighting & sky section never mounted" },
  );
  await sleep(400);
}

const panelReading = () =>
  browser.execute(() => {
    const label = document.querySelector('[data-testid="look-env-label"]');
    const measure = document.querySelector('[data-testid="look-brightness"]');
    const slider = document.querySelector('[data-testid="look-exposure"]');
    return {
      label: label?.textContent ?? null,
      measure: measure?.textContent ?? null,
      hasReset: !!document.querySelector('[data-testid="look-env-reset"]'),
      sliderValue: slider ? Number(slider.value) : null,
      sliderMax: slider ? Number(slider.max) : null,
      lights: document.querySelector('[data-testid="look-lights"]')?.textContent ?? null,
    };
  });

/** Open a project the way a person does: File → Recent → the row for this path.
 *
 *  Not `invoke("open_project")`. That command restores the engine state and says nothing to the
 *  editor, so a spec using it can prove the sidecar works and learn nothing about whether the editor
 *  noticed — which is exactly the difference between the two things this test is about. */
async function openRecent(file) {
  await browser.execute(() => document.querySelector('[data-testid="fileMenu"]')?.click());
  await sleep(600);
  const found = await browser.execute((target) => {
    const rows = Array.from(document.querySelectorAll(".fileRecentItem"));
    const row = rows.find((r) => (r.getAttribute("data-path") ?? "").replace(/\\/g, "/") === target.replace(/\\/g, "/"));
    if (!row) return rows.map((r) => r.getAttribute("data-path"));
    row.click();
    return true;
  }, file);
  if (found !== true) throw new Error(`no Recent row for ${file}; the menu offers ${JSON.stringify(found)}`);
  await sleep(800);
  // A dirty document would put the unsaved-changes guard in the way. Nothing here dirties the
  // document (the sky and the exposure are render-only, ADR-021) so this should not fire — but a
  // spec that hangs on a modal it did not expect reports the wrong failure.
  await browser.execute(() => document.querySelector('[data-testid="guardDiscard"]')?.click());
}

describe("the scene's look is reachable, and it reports the real renderer", () => {
  before(async () => {
    await browser.waitUntil(
      async () => (await browser.execute(() => !!(window.__TAURI__ && window.__TAURI__.core))) === true,
      { timeout: 30000, timeoutMsg: "TAURI bridge never appeared" },
    );
    await browser.execute(() => {
      try {
        localStorage.setItem("mtk.onboarded.v1", "1");
        // The section persists its own open state; clear it so this run starts from the shipped
        // default (closed) rather than from whatever a previous run left behind.
        localStorage.removeItem("mtk.scene-look");
      } catch (e) {
        void e;
      }
    });
    await browser.refresh();
    await sleep(1500);
    for (const f of [projectFile, sidecar]) {
      try {
        rmSync(f, { force: true });
      } catch {
        /* nothing to clean */
      }
    }
    await invoke("reset_environment");
  });

  it("the section is reachable from the Scene workspace and agrees with the engine", async () => {
    await openLookSection();
    const [ui, engine] = [await panelReading(), await invoke("environment_state")];
    console.log("  panel:", JSON.stringify(ui));
    console.log("  engine:", JSON.stringify(engine));

    if (ui.label !== engine.label) {
      throw new Error(`the panel says ${JSON.stringify(ui.label)} and the engine says ${JSON.stringify(engine.label)}`);
    }
    // The built-in sky: no way back to offer, and nothing measured to report.
    if (engine.applied) throw new Error("this spec starts from the built-in sky; the engine has a panorama");
    if (ui.hasReset) throw new Error("a reset control is offered with nothing to reset from");
    if (ui.measure !== null) throw new Error(`a measurement is shown for the built-in sky: ${ui.measure}`);
    if (ui.lights === null) throw new Error("the scene's own light count is not reported");
    await shot("10_look_studio");
  });

  it("the exposure slider reaches the RENDERER, not just its own React state", async () => {
    const before = (await invoke("colour_status")).exposure;
    const ui = await panelReading();
    console.log(`  engine exposure ${before}, slider at ${ui.sliderValue} of ${ui.sliderMax}`);

    // Two stops up from wherever it is, through the control itself. React's onChange needs a native
    // value set for a controlled input, which is why this goes through the prototype setter rather
    // than assigning `.value`.
    const target = Math.min(ui.sliderMax, ui.sliderValue + 6);
    await browser.execute((v) => {
      const el = document.querySelector('[data-testid="look-exposure"]');
      const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value").set;
      setter.call(el, String(v));
      el.dispatchEvent(new Event("input", { bubbles: true }));
      el.dispatchEvent(new Event("change", { bubbles: true }));
    }, target);
    await sleep(800);

    const after = (await invoke("colour_status")).exposure;
    console.log(`  engine exposure after: ${after}`);
    if (!(after > before)) {
      throw new Error(`the slider moved to index ${target} and the renderer's exposure stayed at ${after}`);
    }
    // And the label reports the value in stops, not the raw multiplier.
    const text = await browser.execute(() => document.querySelector('[data-testid="look-section"]')?.textContent ?? "");
    if (!/stops|default/.test(text)) throw new Error(`no stop reading on the panel: ${text.slice(0, 200)}`);
    await shot("11_look_exposure");
  });

  it("a chosen sky SURVIVES a save and a reopen — the sidecar, end to end", async () => {
    const hdr = path.join(fixtures, "env-red.hdr");
    const imported = await invoke("import_environment", { path: hdr });
    console.log("  imported:", JSON.stringify(imported));
    if (!imported.applied) throw new Error(`the fixture panorama did not load: ${imported.message}`);
    if (imported.path !== hdr && !imported.path?.endsWith("env-red.hdr")) {
      throw new Error(`the reply does not name the file it read: ${imported.path}`);
    }

    const saved = await invoke("save_project", { path: projectFile });
    console.log("  saved to:", saved?.path ?? null);
    if (!existsSync(sidecar)) throw new Error(`no view sidecar beside the project: ${sidecar}`);
    const written = JSON.parse(readFileSync(sidecar, "utf8"));
    console.log("  sidecar:", JSON.stringify(written));
    if (!String(written.environment ?? "").endsWith("env-red.hdr")) {
      throw new Error(`the sidecar does not record the panorama: ${JSON.stringify(written)}`);
    }
    // The presentation keys stay at the TOP level — an older build reads this file too.
    if (written.exposure === undefined) throw new Error(`the presentation was nested, not flattened: ${JSON.stringify(written)}`);

    // Drop the sky, prove it is gone, then reopen and prove it came back. Without the middle step this
    // would pass on a build that simply never cleared it.
    await invoke("reset_environment");
    if ((await invoke("environment_state")).applied) throw new Error("reset_environment left a panorama in force");

    // THROUGH THE UI, and the first version of this spec is why. It called `invoke("open_project")`,
    // which restores the engine's sky perfectly and never tells the editor the document changed — so
    // it proved the sidecar and then asked the panel about a gesture the panel never saw. `Recent` is
    // the route a person actually takes, and it is the one that advances the project session.
    await openRecent(projectFile);
    await sleep(2000);
    const restored = await invoke("environment_state");
    console.log("  after reopen:", JSON.stringify(restored));
    if (!restored.applied) throw new Error("reopening the project did not restore its sky");
    if (!restored.label.includes("env-red")) throw new Error(`restored the wrong sky: ${restored.label}`);

    // And the PANEL says so — `sessionId` advances on open, which is what re-reads it.
    await openLookSection();
    await browser.waitUntil(
      async () => (await panelReading()).hasReset === true,
      { timeout: 15000, timeoutMsg: "the panel still reports the built-in sky after the project was reopened" },
    );
    const ui = await panelReading();
    console.log("  panel after reopen:", JSON.stringify(ui));
    if (!ui.label.includes("env-red")) throw new Error(`the panel names ${ui.label} after reopening`);
    if (!ui.measure) throw new Error("no measurement line for a loaded panorama");
    await shot("12_look_restored");
  });

  it("the command palette can light the scene without knowing where the section is", async () => {
    // The section is closed by default, so the palette is the other way in — and it must reach the
    // same engine state, not a parallel one.
    await invoke("import_environment", { path: path.join(fixtures, "env-blue.hdr") });
    if (!(await invoke("environment_state")).applied) throw new Error("the blue fixture did not load");

    await browser.keys(["Control", "k"]);
    await sleep(500);
    await browser.execute(() => {
      const box = document.querySelector('.mtk-command-palette input[role="combobox"]');
      if (box) {
        const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value").set;
        setter.call(box, "studio sky");
        box.dispatchEvent(new Event("input", { bubbles: true }));
      }
    });
    await sleep(500);
    await shot("13_look_palette");
    // Keyed on the command ID, never on the label: a spec that matches user-facing copy breaks
    // silently the day the copy improves (`<test_and_ci_discipline>` 3). The label is still READ, to
    // report what was clicked, but it is not the thing being matched.
    const clicked = await browser.execute(() => {
      const row = document.querySelector('[data-command-id="look-environment-reset"]');
      if (!row) return null;
      row.click();
      return row.textContent;
    });
    console.log("  palette row:", JSON.stringify(clicked));
    if (!clicked) throw new Error("the palette has no row for the built-in studio sky");
    await sleep(1200);

    const after = await invoke("environment_state");
    console.log("  after the palette row:", JSON.stringify(after));
    if (after.applied) throw new Error(`the palette row did not reach the engine: still lit by ${after.label}`);
  });
});
