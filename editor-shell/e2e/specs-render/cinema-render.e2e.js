// ADR-175 — A PICTURE COMES OUT OF THE ENGINE, on the packaged `.exe`.
//
// BEFORE: this shell wrote no image file anywhere. `render_thumbnail` has read the renderer's own
// frames back to PNG since M14.2 (square, 256px, one entity, for the asset browser), and `solve_shot`
// has posed the camera at any instant since cutscenes shipped — and there was no command, no menu item
// and no dialog that produced a single file. Every still of this project's own benchmark film was an
// operating-system screenshot taken by a harness outside the engine, which is evidence that the
// renderer works and none at all that Metrocalk can deliver a picture.
//
// AFTER: `viewport_capture` writes the frame on the stage, and `cinema_render_start` writes a cutscene
// out as a numbered sequence — both by reading back the SAME swapchain texture the viewer is looking
// at, between the frame's submit and its present. So the file is the picture, by construction.
//
// EVERY CLAIM HERE IS A PROPERTY OF THE FILES. How many exist, what shape they are (read out of each
// PNG's own IHDR, not out of the reply that claims it), and whether consecutive frames differ — which
// is the only way to tell a rendered camera MOVE from 60 copies of one frame. The UI is driven the way
// a user drives it, with ONE documented exception: the folder. `cinema_render_start` opens a native
// folder picker when it is not given one, and a WebDriver session cannot dismiss an OS dialog — so the
// spec supplies the folder the picker would have returned and clicks everything else.

import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, readdirSync, rmSync, existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const dir = path.dirname(fileURLToPath(import.meta.url));
const shots = path.resolve(dir, "../.shots-render");
mkdirSync(shots, { recursive: true });

/** A PNG's real pixel size, from its own IHDR — never from the reply that claims it.
 *
 *  Eight bytes of signature, then the IHDR chunk: 4 length, 4 type, then width and height as
 *  big-endian u32. Reading it here is what makes "the file is 2.39:1" a measurement of the FILE
 *  rather than an echo of the number the engine sent back about it. */
function pngSize(file) {
  const buf = readFileSync(file);
  const signature = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
  if (!buf.subarray(0, 8).equals(signature)) throw new Error(`${file} is not a PNG`);
  if (buf.toString("ascii", 12, 16) !== "IHDR") throw new Error(`${file} has no IHDR`);
  return { width: buf.readUInt32BE(16), height: buf.readUInt32BE(20), bytes: buf.length };
}

const sha = (file) => createHash("sha256").update(readFileSync(file)).digest("hex").slice(0, 12);

const frames = (folder) =>
  existsSync(folder) ? readdirSync(folder).filter((f) => f.endsWith(".png")).sort() : [];

const click = (selector) =>
  browser.execute((sel) => {
    const el = document.querySelector(sel);
    if (!el) return false;
    el.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    return true;
  }, selector);

/** Click a control by its VISIBLE WORD inside a container — the dock's tab strip has no test id, and
 *  inventing one for a test would be a hook nothing else uses. */
const clickTabNamed = (containerSel, word) =>
  browser.execute(
    (sel, name) => {
      const root = document.querySelector(sel);
      if (!root) return false;
      const tab = [...root.querySelectorAll('[role="tab"]')].find((t) => t.textContent?.trim() === name);
      if (!tab) return false;
      tab.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      return true;
    },
    containerSel,
    word,
  );

const selectRow = (id) =>
  browser.execute((key) => {
    const row = document.querySelector(`[data-testid="hrow"][data-id="${key}"]`);
    if (!row) return false;
    row.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    return true;
  }, id);

const text = (selector) =>
  browser.execute((sel) => document.querySelector(sel)?.textContent ?? null, selector);

const exists = (selector) => browser.execute((sel) => !!document.querySelector(sel), selector);

const setSelect = (selector, value) =>
  browser.execute(
    (sel, v) => {
      const el = document.querySelector(sel);
      if (!el) return false;
      const setter = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value")?.set;
      setter?.call(el, v);
      el.dispatchEvent(new Event("change", { bubbles: true }));
      return true;
    },
    selector,
    value,
  );

/** Run a render to completion through the same three commands the dialog calls, and hand back the
 *  ledger. The folder is the one argument the picker would have supplied. */
async function render(entity, { fps, shot, stem, folder }) {
  const started = await invoke("cinema_render_start", { id: entity, fps, shot, stem, folder });
  if (started.reason) throw new Error(`the render was refused: ${started.reason}`);
  let last = started;
  await browser.waitUntil(
    async () => {
      last = await invoke("cinema_render_status");
      return last.done === true;
    },
    { timeout: 240000, interval: 500, timeoutMsg: `the render never finished (${last.written}/${last.frames})` },
  );
  return last;
}

describe("Rendering a cutscene to files — the engine delivers a picture", () => {
  let statue;
  let plan;

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
  });

  it("builds a set worth filming, and authors a cut through the shot cards", async () => {
    statue = (await invoke("shape_spawn", { kind: "capsule", pos: [0, 1.6, 0] })).created;
    await invoke("shape_spawn", { kind: "cylinder", pos: [0, 0.2, 0] });
    for (const p of [[7, 0.5, 6], [-8, 0.5, 5], [6, 0.5, -7], [-6, 0.5, -6]]) {
      await invoke("shape_spawn", { kind: "box", pos: p });
    }
    await invoke("frame_all");
    expect(statue).toBeTruthy();

    await click('[data-testid="engine-gameplay"]');
    await browser.pause(500);
    expect(await selectRow(statue)).toBe(true);
    await (await $('[data-testid="cinema-section"]')).waitForExist({ timeout: 10000 });
    for (const card of ["establish", "hero"]) {
      expect(await click(`[data-testid="shot-${card}"]`)).toBe(true);
      await browser.pause(500);
    }
    await browser.waitUntil(async () => (await invoke("cinema_list", { id: statue })).shots === 2, {
      timeout: 15000,
      timeoutMsg: "the two shots never landed",
    });

    // Open the timeline the way a user does: the bottom dock, its Animate workspace, its Cutscene tab.
    if (!(await browser.execute(() => !!document.querySelector("#bottom-workspaces-animation-tab")))) {
      await click('[data-testid="bottom-dock-toggle"]');
      await browser.pause(400);
    }
    expect(await click("#bottom-workspaces-animation-tab")).toBe(true);
    await browser.pause(600);
    expect(await clickTabNamed("#bottom-workspaces-animation-panel", "Cutscene")).toBe(true);
    await (await $('[data-testid="cutscene-timeline"]')).waitForExist({ timeout: 15000 });
  });

  it("the Render control is on the panel, and its dialog states the ENGINE's plan", async () => {
    expect(await exists('[data-testid="cutscene-render"]')).toBe(true);
    expect(await click('[data-testid="cutscene-render"]')).toBe(true);
    await (await $('[data-testid="render-dialog"]')).waitForExist({ timeout: 10000 });

    // The cost, before the click, and it is the engine's own arithmetic — asserted by asking
    // `cinema_render_plan` the same question the dialog asked and comparing the two.
    plan = await invoke("cinema_render_plan", { id: statue, fps: 24, shot: null });
    expect(plan.reason).toBe(null);
    expect(plan.frames).toBeGreaterThan(0);
    const cost = await text('[data-testid="render-cost"]');
    console.log(`[render] the dialog says: ${cost?.replace(/\s+/g, " ").trim()}`);
    expect(cost).toContain(`${plan.frames} frames`);
    // ...and the button that pays it says the same number, which is `<ux_quality>` 3 in one assertion.
    const button = await text('[data-testid="render-start"]');
    expect(button).toContain(`${plan.frames}`);

    // The one limit, said BEFORE the click rather than discovered after it.
    const dialog = await text('[data-testid="render-dialog"]');
    expect(dialog).toContain("Frame size");
    expect(await click('[data-testid="render-cancel"]')).toBe(true);
    await browser.pause(300);
    expect(await exists('[data-testid="render-dialog"]')).toBe(false);
  });

  it("writes one PNG per planned frame, and every one of them is a real image", async () => {
    const folder = path.join(shots, "viewport-cut");
    const ledger = await render(statue, { fps: 24, shot: 0, stem: "hero", folder });
    console.log(`[render] ${ledger.message}`);
    expect(ledger.reason).toBe(null);
    expect(ledger.failures).toEqual([]);

    const files = frames(folder);
    console.log(`[render] ${files.length} file(s) in ${folder}: ${files.slice(0, 2).join(", ")} …`);
    // THE COUNT IS THE PLAN'S. A render that quietly wrote fewer files and still said "done" is the
    // failure this assertion exists for, and it is the one a ledger alone cannot catch.
    expect(files.length).toBe(ledger.frames);
    expect(ledger.written).toBe(ledger.frames);

    // Every file is a decodable PNG of the size the ledger claims — the reply checked against the
    // bytes, rather than against itself.
    for (const f of files) {
      const size = pngSize(path.join(folder, f));
      expect(size.width).toBe(ledger.width);
      expect(size.height).toBe(ledger.height);
      expect(size.bytes).toBeGreaterThan(1000);
    }
    console.log(`[render] every frame is ${ledger.width}x${ledger.height}, ${ledger.bytes} bytes total`);

    // AND THE CAMERA MOVED. A hero shot pushes in, so the first frame and the last cannot be the same
    // picture — 60 copies of one frame would satisfy every count above.
    const first = sha(path.join(folder, files[0]));
    const last = sha(path.join(folder, files[files.length - 1]));
    console.log(`[render] first ${files[0]} ${first} · last ${files[files.length - 1]} ${last}`);
    expect(first).not.toBe(last);
  });

  it("the file is the frame the shot was COMPOSED for, not the shape of the window", async () => {
    // The delivery frame is set through its own control, the way an author sets it.
    expect(await clickTabNamed("#bottom-workspaces-animation-panel", "Cutscene")).toBe(true);
    await (await $('[data-testid="cutscene-delivery"]')).waitForExist({ timeout: 10000 });
    expect(await setSelect('[data-testid="cutscene-delivery"]', "scope")).toBe(true);
    await browser.waitUntil(
      async () => (await invoke("cinema_list", { id: statue })).delivery === "scope",
      { timeout: 15000, timeoutMsg: "the delivery frame never became scope" },
    );

    const folder = path.join(shots, "scope-cut");
    const ledger = await render(statue, { fps: 24, shot: 0, stem: "scope", folder });
    expect(ledger.failures).toEqual([]);
    const files = frames(folder);
    expect(files.length).toBe(ledger.frames);

    const { width, height } = pngSize(path.join(folder, files[0]));
    const aspect = width / height;
    console.log(`[render] scope frames are ${width}x${height} = ${aspect.toFixed(3)}:1 (2.39 wanted)`);
    // Rounded to whole pixels on both axes, so the tolerance is a pixel's worth of ratio, not a wish.
    expect(Math.abs(aspect - 2.39)).toBeLessThan(0.02);
  });

  it("saves the frame on the stage as one PNG, with no cutscene involved at all", async () => {
    const file = path.join(shots, "stage-frame.png");
    if (existsSync(file)) rmSync(file);
    const saved = await invoke("viewport_capture", { path: file });
    console.log(`[render] ${saved.message}`);
    expect(saved.reason).toBe(null);
    const size = pngSize(file);
    expect(size.width).toBe(saved.width);
    expect(size.height).toBe(saved.height);
    expect(size.bytes).toBeGreaterThan(1000);
  });

  it("refuses what it cannot render, in a sentence, and writes nothing", async () => {
    const empty = (await invoke("shape_spawn", { kind: "box", pos: [0, 40, 0] })).created;
    const refused = await invoke("cinema_render_plan", { id: empty, fps: 24, shot: null });
    expect(refused.reason).toContain("no shots");
    const folder = path.join(shots, "never-written");
    const started = await invoke("cinema_render_start", {
      id: empty,
      fps: 24,
      shot: null,
      stem: "nothing",
      folder,
    });
    expect(started.reason).toContain("no shots");
    // NOTHING ON DISK. A refusal that had already created its output folder would be a refusal that
    // half-happened, and the next run would find it and count it.
    expect(frames(folder).length).toBe(0);

    // A rate the engine does not offer is refused rather than clamped: a sequence quietly timed
    // against 24 fps when the author chose 23 is a film that plays at the wrong speed.
    const badRate = await invoke("cinema_render_plan", { id: statue, fps: 23, shot: null });
    expect(badRate.reason).toContain("23");
  });
});
