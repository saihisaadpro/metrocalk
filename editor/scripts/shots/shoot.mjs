#!/usr/bin/env node
//! Build the harness, open every scene in a real Chromium, capture it, and FAIL if the capture is
//! not evidence.
//!
//! WHY THIS IS A GATE AND NOT A SCREENSHOT SCRIPT. `mesh_frame_bench.rs` is this repository's
//! standing lesson: it compiled cleanly through four renderer generations while every contract inside
//! it rotted, and `clippy --all-targets` called it healthy the whole time. A CI job that only *built*
//! this harness would repeat that mistake exactly — the bundle would keep building long after a panel
//! stopped mounting.
//!
//! THE FIRST VERSION OF THIS FILE MADE THE SAME MISTAKE ONE LEVEL UP, and an adversarial review
//! proved it in three lines. Its bars were: no page error · a mounted root · a PNG with more than two
//! distinct colours. Delete **every import row** from the panel — the entire subject of all four
//! scenes — and it stayed **green**: the header and the filter chips still painted, and the "root" it
//! checked (`[data-testid="shot-frame"]`) is emitted by the harness wrapper, not by `scene.render()`,
//! so it is true whenever the bundle mounts at all. Re-introducing this session's own defect
//! (rendering a literal `null · null · null`) was green too, while the report claimed of that very
//! capture that "the string 'null' appears nowhere" — a human having read a PNG once.
//!
//! So the bars are now, in order of strength:
//!
//!   1. **the scene's own claim**, declared as `expect` beside the scene and evaluated in the page —
//!      selectors that must be present with a minimum count, selectors that must be absent, text that
//!      must and must not appear. A scene with no `expect` fails the run before it opens;
//!   2. the page raised no `pageerror` and logged no `console.error`;
//!   3. the captured PNG painted something at all. This is the weakest bar and is kept only because
//!      it costs nothing and catches a renderer that dies after the DOM is built. It is deliberately
//!      not a golden-image diff: that would make every legitimate style change a red gate and would
//!      be waived within a month.
//!
//! There is no "this scene is allowed to be blank" exemption keyed on the filename any more. The
//! first version had one and it laundered a broken scene: a client that REJECTS and a panel that
//! chooses to render nothing produce the same two-colour frame, and the id-suffix check could not
//! tell them apart. A scene that must be empty now says so with `absent` plus a sentinel its own
//! `render()` only emits after the reply resolves.
//!
//! Usage:  node scripts/shots/shoot.mjs [--out DIR] [--scene ID] [--no-build]
//! Needs:  npm i -D @sparticuz/chromium puppeteer-core   (npm-delivered; no CDN download)

import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, readdirSync, existsSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import zlib from "node:zlib";

const HERE = dirname(fileURLToPath(import.meta.url));
const EDITOR = resolve(HERE, "..", "..");
const require = createRequire(import.meta.url);

const argv = process.argv.slice(2);
/** A flag whose value is missing is an ERROR, not a default. `--out` as the last argument used to
 *  crash inside `resolve(undefined)`, and `--scene` as the last argument silently captured EVERY
 *  scene — the exact opposite of the "an unknown scene fails" property this driver advertises. */
const arg = (flag, dflt) => {
  const i = argv.indexOf(flag);
  if (i < 0) return dflt;
  const v = argv[i + 1];
  if (v === undefined || v.startsWith("--")) {
    console.error(`FAIL  ${flag} needs a value`);
    process.exit(2);
  }
  return v;
};
const outDir = resolve(arg("--out", resolve(HERE, "shots")));
const only = arg("--scene", null);

// ── the pixel check ───────────────────────────────────────────────────────────────────────────────

/** How many distinct RGB values a PNG contains, capped — enough to tell "it rendered" from "it did
 *  not" without pulling in an image library. Decodes IDAT by hand: a dependency whose only job is to
 *  answer one boolean is a dependency that will be out of date the first time anyone looks. */
function distinctColours(png, cap = 64) {
  let i = 8; // skip the signature
  let width = 0;
  let height = 0;
  let bitDepth = 0;
  let colourType = 0;
  const idat = [];
  while (i < png.length) {
    const len = png.readUInt32BE(i);
    const type = png.toString("ascii", i + 4, i + 8);
    const body = png.subarray(i + 8, i + 8 + len);
    if (type === "IHDR") {
      width = body.readUInt32BE(0);
      height = body.readUInt32BE(4);
      bitDepth = body[8];
      colourType = body[9];
    } else if (type === "IDAT") {
      idat.push(body);
    } else if (type === "IEND") {
      break;
    }
    i += 12 + len;
  }
  // Only the shape puppeteer emits is handled. Anything else is reported as unknown rather than
  // guessed at — a confident wrong answer from a health check is worse than an admitted unknown.
  const channels = { 2: 3, 6: 4 }[colourType];
  if (!channels || bitDepth !== 8 || !width || !height) return null;

  const raw = zlib.inflateSync(Buffer.concat(idat));
  const stride = width * channels;
  const seen = new Set();
  const prev = Buffer.alloc(stride);
  const line = Buffer.alloc(stride);
  let p = 0;
  for (let y = 0; y < height && seen.size < cap; y++) {
    const filter = raw[p++];
    raw.copy(line, 0, p, p + stride);
    p += stride;
    for (let x = 0; x < stride; x++) {
      const a = x >= channels ? line[x - channels] : 0;
      const b = prev[x];
      const c = x >= channels ? prev[x - channels] : 0;
      let add = 0;
      if (filter === 1) add = a;
      else if (filter === 2) add = b;
      else if (filter === 3) add = (a + b) >> 1;
      else if (filter === 4) {
        const pa = Math.abs(b - c);
        const pb = Math.abs(a - c);
        const pc = Math.abs(a + b - 2 * c);
        add = pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
      }
      line[x] = (line[x] + add) & 0xff;
    }
    for (let x = 0; x + channels <= stride; x += channels * 4) {
      seen.add((line[x] << 16) | (line[x + 1] << 8) | line[x + 2]);
      if (seen.size >= cap) break;
    }
    line.copy(prev);
  }
  return seen.size;
}

// ── run ───────────────────────────────────────────────────────────────────────────────────────────

if (!argv.includes("--no-build")) {
  console.log("building the harness…");
  execFileSync(
    process.platform === "win32" ? "npx.cmd" : "npx",
    ["vite", "build", "--config", resolve(HERE, "vite.config.ts"), "--logLevel", "warn"],
    { cwd: EDITOR, stdio: "inherit" },
  );
}

const dist = resolve(HERE, "dist");
if (!existsSync(resolve(dist, "harness.html"))) {
  console.error(`FAIL  the harness did not build: no ${resolve(dist, "harness.html")}`);
  process.exit(1);
}

const chromium = (await import("@sparticuz/chromium")).default;
const puppeteer = (await import("puppeteer-core")).default;
mkdirSync(outDir, { recursive: true });

const browser = await puppeteer.launch({
  args: [...chromium.args, "--font-render-hinting=none", "--allow-file-access-from-files"],
  executablePath: await chromium.executablePath(),
  headless: true,
});
const href = pathToFileURL(resolve(dist, "harness.html")).href;

// The registry comes off `window.__MTK_SHOTS__` in the BUILT bundle — the same objects the page
// renders. The first version regexed `scenes.tsx` for `id:` lines, which is a second statement of
// the scene list that nothing compares to the first: reformat the file and the driver silently
// captures fewer scenes, reporting success over the ones it stopped opening.
const boot = await browser.newPage();
await boot.goto(href, { waitUntil: "networkidle0" });
const registry = await boot.evaluate(() => window.__MTK_SHOTS__ ?? []);
await boot.close();
if (registry.length === 0) {
  console.error("FAIL  the built bundle exposes no scenes — this run would have reported success over nothing");
  process.exit(1);
}
const missing = registry.filter((s) => !s.expect || Object.keys(s.expect).length === 0);
if (missing.length) {
  console.error(`FAIL  scene(s) with no \`expect\`: ${missing.map((s) => s.id).join(", ")}`);
  console.error("        a capture that asserts nothing is a screenshot, not a gate");
  process.exit(1);
}
const scenes = only ? registry.filter((s) => s.id === only) : registry;
if (scenes.length === 0) {
  console.error(`FAIL  --scene ${only} matches none of: ${registry.map((s) => s.id).join(", ")}`);
  process.exit(1);
}

let failed = 0;
for (const scene of scenes) {
  const { id, looking_for, expect } = scene;
  const page = await browser.newPage();
  await page.setViewport({ width: 620, height: 900, deviceScaleFactor: 2 });
  const problems = [];
  page.on("pageerror", (e) => problems.push(`pageerror: ${e}`));
  page.on("console", (m) => m.type() === "error" && problems.push(`console.error: ${m.text()}`));

  await page.goto(`${href}?scene=${id}`, { waitUntil: "networkidle0" });
  await new Promise((r) => setTimeout(r, 250)); // the panels fetch their report in an effect

  const path = resolve(outDir, `${id}.png`);
  await page.screenshot({ path, fullPage: true });
  const colours = distinctColours(readFileSync(path));

  // The scene's OWN claim, evaluated in the page. `[data-testid="shot-frame"]` is emitted by the
  // harness wrapper, not by `scene.render()`, so it was never evidence that the scene rendered —
  // it is true whenever the bundle mounts at all.
  const claim = await page.evaluate((e) => {
    const out = [];
    const frame = document.querySelector('[data-testid="shot-frame"]');
    if (!frame) return ["the harness itself never mounted"];
    const text = frame.textContent ?? "";
    for (const [sel, atLeast] of e.present ?? []) {
      const n = frame.querySelectorAll(sel).length;
      if (n < atLeast) out.push(`expected at least ${atLeast} \`${sel}\`, found ${n}`);
    }
    for (const sel of e.absent ?? []) {
      const n = frame.querySelectorAll(sel).length;
      if (n) out.push(`expected no \`${sel}\`, found ${n}`);
    }
    for (const s of e.text_present ?? []) {
      if (!text.includes(s)) out.push(`the rendered text does not contain ${JSON.stringify(s)}`);
    }
    for (const s of e.text_absent ?? []) {
      if (text.includes(s)) out.push(`the rendered text contains ${JSON.stringify(s)}`);
    }
    return out;
  }, expect);

  const bad = [...problems, ...claim];
  if (colours === null) bad.push("the capture is not an 8-bit RGB/RGBA PNG, so it was not checked");
  // The pixel bar is now the WEAKEST of the three, kept only because it costs nothing and catches a
  // renderer that dies after the DOM is built. A scene that must be blank says so through `absent`
  // + a sentinel, not through its filename — the id-suffix exemption the first version used let a
  // rejecting client photograph as "renders nothing".
  else if (colours < 2) bad.push(`the capture has ${colours} distinct colour(s) — nothing painted at all`);

  if (bad.length) {
    failed++;
    console.error(`FAIL  ${id}`);
    console.error(`        looking for: ${looking_for}`);
    for (const b of bad) console.error(`        ${b}`);
  } else {
    console.log(`ok    ${id}.png  (${colours} colours)  ${looking_for}`);
  }
  await page.close();
}

await browser.close();
console.log(
  failed
    ? `\n${failed} of ${scenes.length} scene(s) failed — a capture that threw, that painted nothing, or that does not show what the scene claims is not evidence.`
    : `\n${scenes.length} scene(s) captured to ${outDir}; each one rendered what it claims to render.`,
);
process.exit(failed ? 1 : 0);
