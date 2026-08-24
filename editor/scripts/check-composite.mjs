#!/usr/bin/env node

/**
 * THE NATIVE COMPOSITE, MEASURED IN PIXELS.
 *
 * ADR-008 and ADR-090 §5 state one rule about the packaged app: `html`, `body`, `#root` and the
 * native app root must NOT paint, so the wgpu scene composites up through the viewport, and every
 * surface that is not the stage must therefore paint its OWN opaque background. The rule is stated
 * in prose, in two ADRs, and in half a dozen comments beside the `background:` declarations that
 * honour it — and until this file, nothing compared any of those statements to what a browser
 * actually paints.
 *
 * That is the `<test_and_ci_discipline>` 6 shape exactly, and it has the failure mode that shape
 * always has: **every existing gate is silent about it.** `vitest` runs in jsdom, which has no
 * pixels. `check-palette-contrast.mjs` reads the token file, not the page. The `shots` invariants
 * R1–R9 are geometric — a dock that stopped painting its background is exactly where it should be,
 * the right size, not clipped, not overlapping, and perfectly legible in the capture, because the
 * capture is taken over an opaque dev body. The only place the defect appears is the .exe, on a
 * machine with a display, after a release build — which is where the black-viewport bug was found
 * the first time.
 *
 * WHAT IT DOES. It opens the real shell (the `shots` harness's `shell-wide` scene), paints magenta
 * behind everything, and forces transparent the exactly two elements that read
 * `native ? "transparent" : <a colour>` in `App.tsx` — `.mtk-editor-root` and `#viewport`. That IS
 * the `native === true` branch and nothing more: no other element is touched, so what the page does
 * next is what the packaged app does. Then it samples: magenta over chrome is the 3D scene bleeding
 * into a panel; no magenta over the stage is the viewport painted shut.
 *
 * THE GUTTERS ARE WHY IT EXISTS NOW. The shell's dock tracks paint the workspace ground and float
 * their panels inside it, so there are surfaces — the gaps between the rail, the docks and the
 * stage — that belong to no panel and are painted only by a track's padding. Nothing that existed
 * before this was responsible for them, which makes them the most likely thing in the shell to
 * become a hole by omission, and the least likely to be noticed.
 *
 * `--self-test` replays two regressions that have actually shipped in this repository's lineage: a
 * shell surface that stops painting, and an opaque root that occludes the wgpu layer. A gate that
 * stops catching those fails on itself.
 */

import { existsSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import zlib from "node:zlib";

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url));
const EDITOR_ROOT = resolve(SCRIPT_DIR, "..");
const DEFAULT_HARNESS = resolve(EDITOR_ROOT, "scripts/shots/dist/harness.html");

const argv = process.argv.slice(2);
const selfTest = argv.includes("--self-test");
const harnessArg = argv.indexOf("--harness");
const HARNESS = harnessArg >= 0 ? resolve(argv[harnessArg + 1]) : DEFAULT_HARNESS;

/** The `native === true` branch of the shell, and nothing else. */
const NATIVE = `
  html { background: #ff00ff !important; }
  body, #root, [data-testid='shot-frame'], .mtk-editor-root, #viewport { background: transparent !important; }
`;

// ── PNG, decoded by hand ──────────────────────────────────────────────────────────────────────────
// Same reasoning as `shots/shoot.mjs`: a dependency whose only job is to answer one question about
// one screenshot is a dependency that will be out of date the first time anyone looks at it.
function decode(png) {
  let i = 8;
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
    } else if (type === "IDAT") idat.push(body);
    else if (type === "IEND") break;
    i += 12 + len;
  }
  const channels = { 2: 3, 6: 4 }[colourType];
  if (!channels || bitDepth !== 8 || !width || !height) throw new Error("not an 8-bit RGB/RGBA PNG");
  const raw = zlib.inflateSync(Buffer.concat(idat));
  const stride = width * channels;
  const out = Buffer.alloc(stride * height);
  const prev = Buffer.alloc(stride);
  const line = Buffer.alloc(stride);
  let p = 0;
  for (let y = 0; y < height; y++) {
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
    line.copy(out, y * stride);
    line.copy(prev);
  }
  return {
    width,
    height,
    at(x, y) {
      const o = y * stride + x * channels;
      return [out[o], out[o + 1], out[o + 2]];
    },
  };
}

/** Generous, because the question is "did the 3D show through", not "which magenta". */
const isWgpu = ([r, g, b]) => r > 200 && g < 70 && b > 200;

/** The shell surfaces that must be opaque, named the way a reader would name them. */
const OPAQUE = [
  ["engines rail card", "[data-testid='engine-rail']"],
  ["left dock panel", "[data-testid='hierarchy']"],
  ["inspector dock", "[data-testid='inspector-dock']"],
  ["editor header", "[data-testid='editor-header']"],
  ["status bar", "[data-testid='status']"],
  ["bottom dock", "[data-testid='bottom-dock']"],
];

/**
 * One run of the page. `extraCss` is the mutation under `--self-test`, injected AFTER `NATIVE` so it
 * wins at equal specificity — the point of a mutation is that it is allowed to break the thing.
 */
async function run(browser, extraCss) {
  const page = await browser.newPage();
  await page.setViewport({ width: 1440, height: 900, deviceScaleFactor: 1 });
  const noise = [];
  page.on("pageerror", (e) => noise.push(`pageerror: ${e}`));
  page.on("console", (m) => m.type() === "error" && noise.push(`console.error: ${m.text()}`));

  await page.goto(`${pathToFileURL(HARNESS).href}?scene=shell-wide`, { waitUntil: "networkidle0" });
  await page.waitForSelector("[data-testid='viewport']", { timeout: 20_000 });
  await page.addStyleTag({ content: NATIVE });
  if (extraCss) await page.addStyleTag({ content: extraCss });
  await new Promise((r) => setTimeout(r, 300)); // let the style settle before the camera

  const img = decode(await page.screenshot({ type: "png" }));
  const box = (sel) =>
    page.$eval(sel, (el) => {
      const r = el.getBoundingClientRect();
      return { x: r.x, y: r.y, w: r.width, h: r.height };
    });

  // A 5×5 grid INSET from the edges. A rounded corner is meant to be a hole — the bottom dock's two
  // top corners are the stage showing through, deliberately — so sampling the corners would report
  // the design as the defect. Antialiasing along a border is excluded for the same reason.
  const scan = (b) => {
    let wgpu = 0;
    let total = 0;
    for (let i = 1; i <= 5; i++) {
      for (let j = 1; j <= 5; j++) {
        const x = Math.round(b.x + (b.w * i) / 6);
        const y = Math.round(b.y + (b.h * j) / 6);
        if (x < 0 || y < 0 || x >= img.width || y >= img.height) continue;
        total++;
        if (isWgpu(img.at(x, y))) wgpu++;
      }
    }
    return { wgpu, total };
  };

  const findings = [];
  const lines = [];
  for (const [name, sel] of OPAQUE) {
    const { wgpu, total } = scan(await box(sel));
    if (wgpu > 0) findings.push(`${name}: ${wgpu} of ${total} sample points are the wgpu layer — this surface does not paint, so the 3D scene shows through a panel`);
    lines.push(`  ${wgpu === 0 ? "ok  " : "LEAK"}  ${name.padEnd(18)} ${wgpu}/${total}`);
  }

  // THE STAGE IS THE HOLE, and it has to still be one. Sampled over a grid rather than at its
  // centre: the centre of this scene holds the viewport's own placeholder line and the first-run
  // card, so a single centre sample measures a label rather than the composite.
  const stage = await box("[data-testid='viewport']");
  {
    const { wgpu, total } = scan(stage);
    const want = Math.round(total * 0.6);
    if (wgpu < want) findings.push(`the stage: only ${wgpu} of ${total} sample points are the wgpu layer — something above the viewport is painting, which is the black-viewport regression ADR-008 exists to prevent`);
    lines.push(`  ${wgpu >= want ? "ok  " : "SHUT"}  ${"the stage".padEnd(18)} ${wgpu}/${total}`);
  }

  // THE GUTTERS: ground painted by a track's padding and by nothing else.
  const rail = await box("[data-testid='engine-rail']");
  const hierarchy = await box("[data-testid='hierarchy']");
  for (const [name, x] of [
    ["window|rail", Math.round(rail.x / 2)],
    ["rail|dock", Math.round((rail.x + rail.w + hierarchy.x) / 2)],
    ["dock|stage", Math.round((hierarchy.x + hierarchy.w + stage.x) / 2)],
  ]) {
    let wgpu = 0;
    let total = 0;
    for (let j = 1; j <= 9; j++) {
      total++;
      if (isWgpu(img.at(x, Math.round(rail.y + (rail.h * j) / 10)))) wgpu++;
    }
    if (wgpu > 0) findings.push(`gutter ${name}: ${wgpu} of ${total} sample points are the wgpu layer — the shell ground has a hole in it between two panels`);
    lines.push(`  ${wgpu === 0 ? "ok  " : "LEAK"}  ${`gutter ${name}`.padEnd(18)} ${wgpu}/${total}`);
  }

  if (noise.length) findings.push(`the page reported ${noise.length} error(s): ${noise.join(" · ")}`);
  await page.close();
  return { findings, lines };
}

// ── the run ───────────────────────────────────────────────────────────────────────────────────────

if (!existsSync(HARNESS)) {
  console.error(`FAIL  no harness at ${HARNESS}`);
  console.error("      This gate reads the bundle the shots harness builds. Run `pnpm shots` first,");
  console.error("      or point it at another build with --harness <path/to/harness.html>.");
  process.exit(1);
}

const chromium = (await import("@sparticuz/chromium")).default;
const puppeteer = (await import("puppeteer-core")).default;
const browser = await puppeteer.launch({
  args: [...chromium.args, "--font-render-hinting=none", "--allow-file-access-from-files"],
  executablePath: await chromium.executablePath(),
  headless: true,
});

let failed = false;
try {
  if (selfTest) {
    // Two regressions this repository has actually shipped, in one form or another. Each must be
    // CAUGHT; a mutation that passes means the gate has stopped measuring what it claims to.
    const mutations = [
      ["a shell surface stops painting", ".mtk-shell-track, .mtk-shell-card { background: transparent !important; }"],
      ["an opaque root occludes the wgpu layer", ".mtk-editor-root { background: #ffffff !important; }"],
    ];
    for (const [what, css] of mutations) {
      const { findings } = await run(browser, css);
      if (findings.length === 0) {
        console.error(`FAIL  self-test: "${what}" produced NO findings — this gate no longer measures the composite`);
        failed = true;
      } else {
        console.log(`ok    self-test: "${what}" produced ${findings.length} finding(s)`);
      }
    }
  }

  const { findings, lines } = await run(browser, null);
  console.log(lines.join("\n"));
  if (findings.length) {
    failed = true;
    console.error(`\nFAIL  the native composite is broken in ${findings.length} place(s):`);
    for (const f of findings) console.error(`  - ${f}`);
  } else {
    console.log("\ncomposite: every chrome surface is opaque, every gutter is opaque, and the stage is still a hole.");
  }
} finally {
  await browser.close();
}

process.exit(failed ? 1 : 0);
