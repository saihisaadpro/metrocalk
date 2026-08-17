#!/usr/bin/env node
//! Build the harness, open every scene in a real Chromium, capture it, and FAIL if the capture is
//! not evidence.
//!
//! WHY THIS IS A GATE AND NOT A SCREENSHOT SCRIPT. `mesh_frame_bench.rs` is this repository's
//! standing lesson: it compiled cleanly through four renderer generations while every contract inside
//! it rotted, and `clippy --all-targets` called it healthy the whole time. A CI job that only *built*
//! this harness would repeat that mistake exactly — the bundle would keep building long after a panel
//! stopped mounting. So each scene must clear three bars that a build cannot:
//!
//!   1. the page raised no `pageerror` and logged no `console.error`;
//!   2. the scene's root actually mounted (`[data-testid="shot-frame"]`, or a scene that declares it
//!      renders nothing);
//!   3. the captured PNG is not a single flat colour — a blank frame is the exact shape of the
//!      green-tests-but-black-viewport trap, and it is cheap to detect at rest.
//!
//! Bar 3 needs the pixels, not the DOM, which is why this decodes the PNG rather than trusting that
//! `screenshot()` returned bytes. It is deliberately a crude check: distinct colours, not a diff
//! against a golden. A golden-image baseline would make every legitimate style change a red gate and
//! would be waived within a month; "did anything render at all" never needs waiving.
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
const arg = (flag, dflt) => {
  const i = argv.indexOf(flag);
  return i >= 0 ? argv[i + 1] : dflt;
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

// The scene list comes from the built bundle's source, read as text, so this driver never imports
// TSX. One regex over one file, and a mismatch is a failure rather than an empty run.
const src = readFileSync(resolve(HERE, "scenes.tsx"), "utf8");
const ids = [...src.matchAll(/^\s*id:\s*"([a-z0-9-]+)",$/gm)].map((m) => m[1]);
if (ids.length === 0) {
  console.error("FAIL  no scenes found in scenes.tsx — the driver would have reported success over an empty run");
  process.exit(1);
}
const scenes = only ? ids.filter((s) => s === only) : ids;
if (scenes.length === 0) {
  console.error(`FAIL  --scene ${only} matches none of: ${ids.join(", ")}`);
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

let failed = 0;
for (const id of scenes) {
  const page = await browser.newPage();
  await page.setViewport({ width: 620, height: 900, deviceScaleFactor: 2 });
  const problems = [];
  page.on("pageerror", (e) => problems.push(`pageerror: ${e}`));
  page.on("console", (m) => m.type() === "error" && problems.push(`console.error: ${m.text()}`));

  const url = `${pathToFileURL(resolve(dist, "harness.html")).href}?scene=${id}`;
  await page.goto(url, { waitUntil: "networkidle0" });
  await new Promise((r) => setTimeout(r, 250)); // the panels fetch their report in an effect

  const path = resolve(outDir, `${id}.png`);
  await page.screenshot({ path, fullPage: true });
  const colours = distinctColours(readFileSync(path));
  const mounted = await page.$('[data-testid="shot-frame"]').then((h) => !!h);
  // A scene whose whole assertion is "this renders nothing" is the one legitimate flat frame. It is
  // named in the id rather than inferred, so the empty-by-design case can never launder a broken one.
  const empty_by_design = id.endsWith("-empty");

  const bad = [];
  if (problems.length) bad.push(...problems);
  if (!mounted) bad.push("the scene root never mounted");
  if (colours === null) bad.push("the capture is not an 8-bit RGB/RGBA PNG, so it was not checked");
  else if (colours < 3 && !empty_by_design) bad.push(`the capture is effectively blank (${colours} distinct colour(s))`);
  else if (colours >= 3 && empty_by_design) bad.push(`a *-empty scene rendered ${colours} colours — it is meant to render nothing`);

  if (bad.length) {
    failed++;
    console.error(`FAIL  ${id}`);
    for (const b of bad) console.error(`        ${b}`);
  } else {
    console.log(`ok    ${id}.png  (${colours === null ? "?" : colours} colours)`);
  }
  await page.close();
}

await browser.close();
console.log(
  failed
    ? `\n${failed} of ${scenes.length} scene(s) failed — a capture that is blank, that threw, or that never mounted is not evidence.`
    : `\n${scenes.length} scene(s) captured to ${outDir}; each one mounted, logged no error, and produced pixels.`,
);
process.exit(failed ? 1 : 0);
