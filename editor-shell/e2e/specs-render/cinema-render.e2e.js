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

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, readdirSync, rmSync, existsSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const dir = path.dirname(fileURLToPath(import.meta.url));
const shots = path.resolve(dir, "../.shots-render");
const captureWindowScript = path.resolve(dir, "../scripts/capture-composited-window.ps1");
mkdirSync(shots, { recursive: true });

/** The OS-composited pixels of the packaged window — the WebView2 panels AND the wgpu surface beneath
 *  them. A WebDriver screenshot cannot see the 3D at all (the stage is a transparent hole), so the one
 *  claim in this file that is about what the AUTHOR sees has to be taken this way. `PrintWindow` reads
 *  the window's own presentation, so it steals no foreground and disturbs no render. */
function captureWindow(out) {
  if (existsSync(out)) rmSync(out);
  execFileSync(
    "powershell.exe",
    ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", captureWindowScript, "-Out", out],
    { stdio: "pipe" },
  );
  // Never leave a blank behind looking like evidence: the failure mode this harness has actually seen
  // is a 158-byte 16x16 PNG, not an exception.
  if (!existsSync(out) || statSync(out).size < 20_000) {
    throw new Error(`the window capture came back blank (${existsSync(out) ? statSync(out).size : 0} bytes)`);
  }
  return out;
}

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
async function render(entity, { fps, shot, stem, folder, height = null }) {
  const started = await invoke("cinema_render_start", { id: entity, fps, shot, stem, folder, height });
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
    plan = await invoke("cinema_render_plan", { id: statue, fps: 24, shot: null, height: null });
    expect(plan.reason).toBe(null);
    expect(plan.frames).toBeGreaterThan(0);
    const cost = await text('[data-testid="render-cost"]');
    console.log(`[render] the dialog says: ${cost?.replace(/\s+/g, " ").trim()}`);
    expect(cost).toContain(`${plan.frames} frames`);
    // ...and the button that pays it says the same number, which is `<ux_quality>` 3 in one assertion.
    const button = await text('[data-testid="render-start"]');
    expect(button).toContain(`${plan.frames}`);

    // ADR-177 — THE SIZE IS A CHOICE, and the dialog opens on a delivery format rather than on the
    // window. Until this pass the same place said the opposite: "each frame is written at the size of
    // the composed picture on screen", because it was.
    const dialog = await text('[data-testid="render-dialog"]');
    expect(dialog).toContain("Frame size");
    expect(await exists('[data-testid="render-size"]')).toBe(true);
    const opened = await browser.execute(
      () => document.querySelector('[data-testid="render-size"]')?.value ?? null,
    );
    console.log(`[render] the size picker opens on: ${opened}`);
    expect(opened).toBe("1080");
    // ...and the cost sentence carries the pixels the engine planned, not a multiplication done in the
    // dialog. Asked of the engine with the same height and compared.
    const sized = await invoke("cinema_render_plan", { id: statue, fps: 24, shot: null, height: 1080 });
    expect(sized.reason).toBe(null);
    expect(sized.height).toBe(1080);
    console.log(`[render] 1080 planned as ${sized.width}x${sized.height}`);
    expect(cost).toContain(`${sized.width}x${sized.height}`);
    expect(await click('[data-testid="render-cancel"]')).toBe(true);
    await browser.pause(300);
    expect(await exists('[data-testid="render-dialog"]')).toBe(false);
  });

  it("writes one PNG per planned frame, and every one of them is a real image", async () => {
    const folder = path.join(shots, "viewport-cut");
    // Shot 2 is the HERO card — a full three-quarter that pushes in, so the camera is moving through
    // every frame of it. Shot 1 (the establishing wide) also moves, but a push-in against a subject
    // that fills the frame is the largest per-frame difference this fixture can produce, which is what
    // the distinct-hash assertion below is measuring.
    const ledger = await render(statue, { fps: 24, shot: 1, stem: "hero", folder });
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

    // AND THE CAMERA MOVED. A hero shot pushes in, so the frames cannot all be the same picture —
    // N copies of one frame satisfy every count above, and so does a sequence of blank ones. Distinct
    // hashes across the whole run is the claim; first-vs-last alone would pass on a two-state flicker.
    const hashes = new Set(files.map((f) => sha(path.join(folder, f))));
    console.log(`[render] ${hashes.size} distinct frame(s) of ${files.length}`);
    expect(hashes.size).toBeGreaterThan(files.length / 2);

    // AND IT GAVE THE CAMERA BACK. A render that left the viewport inside a shot would look like a
    // successful render and leave the author unable to see their own scene.
    const after = await invoke("camera_probe");
    expect(after.cinematic).toBe(false);
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
    const ledger = await render(statue, { fps: 24, shot: 1, stem: "scope", folder });
    expect(ledger.failures).toEqual([]);
    const files = frames(folder);
    expect(files.length).toBe(ledger.frames);

    const { width, height } = pngSize(path.join(folder, files[0]));
    const aspect = width / height;
    console.log(`[render] scope frames are ${width}x${height} = ${aspect.toFixed(3)}:1 (2.39 wanted)`);
    // Rounded to whole pixels on both axes, so the tolerance is a pixel's worth of ratio, not a wish.
    expect(Math.abs(aspect - 2.39)).toBeLessThan(0.02);
  });

  it("ADR-177 — renders 1080 lines out of a window that is nowhere near 1080 tall", async () => {
    // THE CLOSING GATE OF ADR-175's FIRST OWED ITEM. Every render before this one came off the
    // viewport's own swapchain, so a frame was exactly as tall as whatever the docks had left of the
    // window — on this WebDriver session, around three hundred lines. The delivery is now drawn into
    // targets of its own, so the two numbers below are unrelated by construction, and this asserts
    // that they are.
    const stage = await browser.execute(() => ({
      inner: window.innerHeight,
      screenY: window.screen?.height ?? 0,
    }));
    const folder = path.join(shots, "hd-cut");
    const ledger = await render(statue, { fps: 24, shot: 1, stem: "hd", folder, height: 1080 });
    console.log(`[render] window is ${stage.inner}px tall; ${ledger.message}`);
    expect(ledger.reason).toBe(null);
    expect(ledger.failures).toEqual([]);
    expect(ledger.offscreen).toBe(true);

    const files = frames(folder);
    expect(files.length).toBe(ledger.frames);
    // Read out of every PNG's own IHDR. `every` rather than the first: a render that produced one
    // correct frame and then fell back to the window would pass a first-file check.
    for (const f of files) {
      const size = pngSize(path.join(folder, f));
      expect(size.height).toBe(1080);
      expect(size.width).toBe(ledger.width);
      expect(size.bytes).toBeGreaterThan(1000);
    }
    console.log(`[render] all ${files.length} frames are ${ledger.width}x1080 from a ${stage.inner}px window`);
    // THE POINT, IN ONE ASSERTION: the file is taller than the window it was rendered from.
    expect(stage.inner).toBeLessThan(1080);
    // And the shape is still the delivery's — the cut is in scope by now, so 1080 lines is 2582 wide.
    const shape = pngSize(path.join(folder, files[0]));
    expect(Math.abs(shape.width / shape.height - 2.39)).toBeLessThan(0.02);

    // A MOVE, STILL FILMED. The bigger target is the same shot at more pixels, not a still.
    const hashes = new Set(files.map((f) => sha(path.join(folder, f))));
    console.log(`[render] ${hashes.size} distinct frame(s) of ${files.length} at 1080`);
    expect(hashes.size).toBeGreaterThan(files.length / 2);

    // ...and the viewport is handed back, at its own size: a render that left the loop drawing into
    // an offscreen texture would leave the editor showing a frozen picture with no way to unfreeze it.
    const after = await invoke("camera_probe");
    expect(after.cinematic).toBe(false);
    const still = await invoke("viewport_capture", { path: path.join(shots, "after-hd.png") });
    expect(still.reason).toBe(null);
    console.log(`[render] the stage is back at ${still.width}x${still.height}`);
    expect(still.height).toBeLessThan(1080);
  });

  it("ADR-177 — the stage shows the render while it runs, at the window's own size", async () => {
    // THE ONE CLAIM HERE THAT IS ABOUT WHAT THE AUTHOR SEES. A render at a chosen size draws into
    // targets of its own, so the swapchain gets nothing unless the loop puts it there — and a viewport
    // frozen on the last editor frame for the length of a render would be a progress bar and no
    // picture. The final resolve therefore runs a SECOND time, into the composed hole, through a
    // viewport transform that FITS the 2582x1440 frame inside a window that is nothing like that size.
    const folder = path.join(shots, "preview-cut");
    const started = await invoke("cinema_render_start", {
      id: statue,
      fps: 24,
      shot: null,
      stem: "preview",
      folder,
      height: 1440,
    });
    expect(started.reason).toBe(null);
    expect(started.offscreen).toBe(true);
    // Catch it MID-JOB — a capture after it finished would photograph the editor, which proves nothing.
    let live = started;
    await browser.waitUntil(
      async () => {
        live = await invoke("cinema_render_status");
        return live.written >= 3 || live.done === true;
      },
      { timeout: 120000, interval: 250, timeoutMsg: "the render never wrote a frame" },
    );
    expect(live.done).toBe(false);
    const out = captureWindow(path.join(shots, "render-in-progress.png"));
    const size = pngSize(out);
    const running = await invoke("cinema_render_status");
    console.log(
      `[render] window is ${size.width}x${size.height} (${size.bytes} bytes) while ${running.width}x${running.height} frames are being written, ${running.written} so far`,
    );
    // The WINDOW is its own size, not the render's — so the picture in it is FITTED, not the file.
    expect(running.height).toBe(1440);
    expect(size.height).toBeLessThan(1440);
    // …and it is a picture, not a flat fill. A blank window compresses to a few KB; this is the whole
    // editor with a lit 3D frame inside it.
    expect(size.bytes).toBeGreaterThan(50_000);

    // Stopping keeps what was written and hands the viewport back its own size.
    const stopped = await invoke("cinema_render_cancel");
    expect(stopped.done).toBe(true);
    expect(frames(folder).length).toBe(stopped.written);
    const after = await invoke("camera_probe");
    expect(after.cinematic).toBe(false);
  });

  it("ADR-175 item 6 — re-rendering an unchanged cut writes the same bytes", async () => {
    // OBSERVED IN THE ADR-175 RUN, ASSERTED HERE. Two independent `.exe` builds produced byte-
    // identical sequences, which is a real property of a pure shot solver over a deterministic
    // renderer — and it was a line in a log rather than a gate. A second render of the same cut into
    // a second folder, compared file for file, is what makes it one.
    const first = path.join(shots, "repeat-a");
    const second = path.join(shots, "repeat-b");
    const a = await render(statue, { fps: 24, shot: 1, stem: "take", folder: first, height: 720 });
    const b = await render(statue, { fps: 24, shot: 1, stem: "take", folder: second, height: 720 });
    expect(a.failures).toEqual([]);
    expect(b.failures).toEqual([]);
    const fa = frames(first);
    const fb = frames(second);
    expect(fb).toEqual(fa);
    // SIZE FIRST, so a frame drawn at the wrong size is NAMED rather than reported as "one hash
    // differed". This is how the ADR-177 race showed up on 2026-08-31: `take.0000.png` alone differed
    // between two runs, because the render loop could read the output size in a different critical
    // section from the camera generation and draw the first frame at the WINDOW's size while carrying
    // an epoch that let it answer the request. 59 of 60 identical and one silently a different shape.
    const shapes = [...fa.map((f) => pngSize(path.join(first, f))), ...fb.map((f) => pngSize(path.join(second, f)))];
    const odd = shapes.filter((s) => s.width !== a.width || s.height !== a.height);
    console.log(`[render] ${shapes.length} files, ${odd.length} not ${a.width}x${a.height}`);
    expect(odd).toEqual([]);
    const differ = fa.filter((f) => sha(path.join(first, f)) !== sha(path.join(second, f)));
    console.log(`[render] ${fa.length} frames re-rendered, ${differ.length} differed`);
    expect(differ).toEqual([]);
    expect(b.bytes).toBe(a.bytes);
  });

  it("refuses a size this build does not render at, by name rather than by clamping", async () => {
    // The same shape of refusal as an unoffered frame rate, and for the same reason: a render that
    // quietly became 1080 would deliver a master at a size nobody chose.
    const odd = await invoke("cinema_render_plan", { id: statue, fps: 24, shot: null, height: 900 });
    expect(odd.reason).toContain("900");
    expect(odd.reason).toContain("2160");
    const folder = path.join(shots, "never-900");
    const started = await invoke("cinema_render_start", {
      id: statue,
      fps: 24,
      shot: null,
      stem: "nine",
      folder,
      height: 900,
    });
    expect(started.reason).toContain("900");
    expect(frames(folder).length).toBe(0);
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
