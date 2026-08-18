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

// ── the layout invariants ─────────────────────────────────────────────────────────────────────────

/** Runs IN THE PAGE. Three geometric properties every panel owes, checked against measured boxes.
 *
 *  WHY THIS EXISTS AT ALL. `editor/src` has 44 panels, 41 of them with a vitest suite, and 412 tests
 *  green — and not one of those tests has ever measured a box. jsdom implements no layout: every
 *  `getBoundingClientRect()` returns zeros, so overflow, clipping, wrapping and overlap are not
 *  things the suite gets wrong, they are things it CANNOT express. The repository's single
 *  `getBoundingClientRect` outside this file is a `mockReturnValue`.
 *
 *  That blind spot has already cost twice, both times found by a human squinting at a PNG: a remove
 *  button that wrapped onto its own line, and a provenance line that ran past its panel at 360 px
 *  (the ADR-120 defect class). "A human looked once" is the same non-gate `mesh_frame_bench.rs`
 *  stood for — so the part of that judgement which is geometric, and therefore mechanical, is
 *  written down here and fails the run.
 *
 *  These are UNIVERSAL, not per-scene. A scene's `expect` says what that panel shows; these say what
 *  every panel owes regardless, which is what makes the next scene cost one entry instead of three
 *  hand-written assertions. There is deliberately no waiver mechanism yet: no legitimate violation
 *  has appeared, and an escape hatch built before its first real case is an escape hatch that gets
 *  used for the first real defect. */
function layoutInvariants() {
  const frame = document.querySelector('[data-testid="shot-frame"]');
  if (!frame) return [];
  const out = [];
  const TOL = 1; // sub-pixel rounding; fractional layout is not a defect

  // A `data-testid` is the best name an element has, right up until a LIST uses one — and then it is
  // the worst, because every row answers to it. R3's first real firing read
  // "[data-testid=\"candidate\"] and [data-testid=\"candidate\"] overlap by 536×14px", which names the
  // defect class and not one of the two things involved in it. A gate whose entire product is a
  // sentence a human acts on cannot afford an ambiguous subject: the reader has to re-derive by hand
  // exactly what the check already knew. So a testid that is not unique inside the frame is qualified
  // with the row's own text — the thing that actually distinguishes it on screen.
  const txtOf = (el) => (el.textContent ?? "").replace(/\s+/g, " ").trim().slice(0, 36);
  const name = (el) => {
    const tid = el.getAttribute("data-testid");
    if (tid) {
      const sel = `[data-testid="${tid}"]`;
      if (frame.querySelectorAll(sel).length < 2) return sel;
      const txt = txtOf(el);
      return txt ? `${sel} "${txt}"` : sel;
    }
    const cls =
      typeof el.className === "string" && el.className.trim()
        ? "." + el.className.trim().split(/\s+/).slice(0, 2).join(".")
        : "";
    const txt = txtOf(el);
    return `<${el.tagName.toLowerCase()}${cls}>` + (txt ? ` "${txt}"` : "");
  };
  const visible = (el, r) => {
    if (r.width < 1 && r.height < 1) return false;
    const cs = getComputedStyle(el);
    return cs.visibility !== "hidden" && cs.display !== "none" && cs.opacity !== "0";
  };

  const box = frame.getBoundingClientRect();
  const fs = getComputedStyle(frame);
  const inner = {
    left: box.left + parseFloat(fs.paddingLeft || 0) + parseFloat(fs.borderLeftWidth || 0),
    right: box.right - parseFloat(fs.paddingRight || 0) - parseFloat(fs.borderRightWidth || 0),
  };
  const els = [...frame.querySelectorAll("*")];

  // R1 — CONTENT ESCAPES ITS PANEL. An element painted outside the frame's content box has left the
  // surface it belongs to: at best it is clipped by whatever is downstream, at worst it lands on a
  // neighbour. An ancestor that scrolls or clips horizontally means the overflow is deliberate and
  // bounded, so the walk up stops there — R2 judges whether that clipping is honest.
  for (const el of els) {
    const r = el.getBoundingClientRect();
    if (!visible(el, r)) continue;
    if (getComputedStyle(el).position === "fixed") continue; // out of the frame's flow by design
    let clipped = false;
    for (let p = el.parentElement; p && p !== frame; p = p.parentElement) {
      const ox = getComputedStyle(p).overflowX;
      if (ox === "hidden" || ox === "clip" || ox === "auto" || ox === "scroll") {
        clipped = true;
        break;
      }
    }
    if (clipped) continue;
    if (r.right > inner.right + TOL) {
      out.push(
        `${name(el)} is painted ${Math.round(r.right - inner.right)}px past the panel's right edge ` +
          `— nothing between it and the frame clips or scrolls, so it escapes the panel`,
      );
    } else if (r.left < inner.left - TOL) {
      out.push(
        `${name(el)} starts ${Math.round(inner.left - r.left)}px left of the panel's content box`,
      );
    }
  }

  // R2 — TRUNCATED WITH NOTHING TO SAY SO. `overflow: hidden` without `text-overflow: ellipsis` and
  // without a scrollbar removes content with no mark at all: the reader cannot tell a name ends at
  // "Overhead Cra" from a name that is spelled that way. Honest truncation ellipsises; deliberate
  // panning scrolls. This is the one that makes a narrow-width scene worth capturing.
  for (const el of els) {
    const r = el.getBoundingClientRect();
    if (!visible(el, r)) continue;
    if (el.scrollWidth <= el.clientWidth + TOL) continue;
    const cs = getComputedStyle(el);
    if (cs.overflowX === "auto" || cs.overflowX === "scroll") continue; // the user can pan to it
    if (cs.overflowX !== "hidden" && cs.overflowX !== "clip") continue; // it spills; that is R1's
    if (cs.textOverflow === "ellipsis") continue;
    // Only where TEXT is what is lost. A clipped decorative box says nothing to a reader.
    const own = [...el.childNodes].some((n) => n.nodeType === 3 && n.textContent.trim());
    if (!own) continue;
    out.push(
      `${name(el)} hides ${el.scrollWidth - el.clientWidth}px of its own text with ` +
        `overflow-x: ${cs.overflowX} and text-overflow: ${cs.textOverflow} — the reader is given no ` +
        "sign that anything was cut",
    );
  }

  // R4 — TEXT PAINTED OUTSIDE ITS OWN BOX. R1 and R2 between them cover content that leaves the
  // PANEL and content that is clipped away; this is the third outcome, and it is the one that was
  // missed. Diagnostics' explanatory span is `flex: 1, minWidth: 0` — "take the leftovers, and I am
  // content at zero" — and next to a button that could not shrink the leftovers were zero. It
  // measured 0 px wide, `overflow: visible`, and painted its whole sentence straight through the
  // button on top of it. Nothing escaped the panel, so R1 was silent; nothing was clipped, so R2 was
  // silent; the two overlapping things were a span and a button, so R3 was silent. A human reading
  // the capture caught it, which is precisely the dependency this file exists to remove.
  //
  // Restricted to box-generating elements with their own text: an inline element's box legitimately
  // reports its line box, and a clipped decorative div is R2's business.
  for (const el of els) {
    const r = el.getBoundingClientRect();
    if (!visible(el, r)) continue;
    const cs = getComputedStyle(el);
    if (cs.overflowX !== "visible") continue; // clipped or scrolled — R2 judges those
    if (!/^(block|flex|grid|list-item|flow-root)$/.test(cs.display)) continue;
    if (el.scrollWidth <= el.clientWidth + TOL) continue;
    if (![...el.childNodes].some((n) => n.nodeType === 3 && n.textContent.trim())) continue;
    out.push(
      `${name(el)} is ${Math.round(r.width)}px wide and its content needs ${el.scrollWidth}px, with ` +
        "overflow-x: visible — the text is painted outside its own box, over whatever is beside it",
    );
  }

  // R3 — TWO CONTROLS ON THE SAME PIXELS. The Progress-Evaluation1 wallet-overlap class. Whichever
  // one is on top steals the click, and which that is depends on paint order, not on intent.
  const CONTROLS = 'button, a[href], input, select, textarea, [role="button"], [tabindex]:not([tabindex="-1"])';
  const controls = [...frame.querySelectorAll(CONTROLS)]
    .map((el) => [el, el.getBoundingClientRect()])
    .filter(([el, r]) => visible(el, r));
  for (let i = 0; i < controls.length; i++) {
    for (let j = i + 1; j < controls.length; j++) {
      const [ea, a] = controls[i];
      const [eb, b] = controls[j];
      if (ea.contains(eb) || eb.contains(ea)) continue; // nesting is composition, not collision
      const w = Math.min(a.right, b.right) - Math.max(a.left, b.left);
      const h = Math.min(a.bottom, b.bottom) - Math.max(a.top, b.top);
      if (w > TOL && h > TOL) {
        out.push(
          `${name(ea)} and ${name(eb)} overlap by ${Math.round(w)}×${Math.round(h)}px — one of them ` +
            "is taking the other's clicks, and which depends on paint order",
        );
      }
    }
  }
  return out;
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
    for (const [sa, sb] of e.same_line ?? []) {
      const a = frame.querySelector(sa);
      const b = frame.querySelector(sb);
      if (!a || !b) {
        out.push(`same_line needs both \`${sa}\` and \`${sb}\`; ${a ? sb : sa} is not there`);
        continue;
      }
      const ra = a.getBoundingClientRect();
      const rb = b.getBoundingClientRect();
      if (Math.min(ra.bottom, rb.bottom) - Math.max(ra.top, rb.top) <= 1) {
        out.push(
          `\`${sa}\` and \`${sb}\` are on different lines (${Math.round(ra.top)}–${Math.round(ra.bottom)} ` +
            `vs ${Math.round(rb.top)}–${Math.round(rb.bottom)}) — the row wrapped`,
        );
      }
    }
    return out;
  }, expect);

  // Universal, and evaluated for every scene whether or not it says anything about layout — the
  // point is that the next panel added here inherits them for nothing.
  const layout = await page.evaluate(layoutInvariants);

  const bad = [...problems, ...claim, ...layout];
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
