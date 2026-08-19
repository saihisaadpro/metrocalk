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
/** Runs IN THE PAGE, once, before anything is measured. The rectangle an element ACTUALLY occupies on
 *  screen: its own box, intersected with every ancestor that clips in that axis, stopping at the
 *  frame. `getBoundingClientRect()` alone is the box an element *would* occupy — a scroll container
 *  reports it in full for a row panned a hundred pixels out of view — and every geometric judgement
 *  this file makes about "is it there" or "are they on the same pixels" is wrong if it reads that.
 *
 *  Installed on `window` rather than closed over because the two callers run in separate
 *  `page.evaluate` calls, which serialize the function and not its scope. One definition, two
 *  readers. */
function installClipRect() {
  window.__mtkClipRect = (el, frame) => {
    const r = el.getBoundingClientRect();
    const box = { left: r.left, right: r.right, top: r.top, bottom: r.bottom };
    for (let p = el.parentElement; p && p !== frame; p = p.parentElement) {
      const cs = getComputedStyle(p);
      const cx = cs.overflowX !== "visible";
      const cy = cs.overflowY !== "visible";
      if (!cx && !cy) continue;
      const pr = p.getBoundingClientRect();
      if (cx) {
        box.left = Math.max(box.left, pr.left);
        box.right = Math.min(box.right, pr.right);
      }
      if (cy) {
        box.top = Math.max(box.top, pr.top);
        box.bottom = Math.min(box.bottom, pr.bottom);
      }
    }
    // `width`/`height` are carried because `visible()` reads them. Returning a bare edge box would
    // leave `r.width` undefined, `undefined < 1` is false, and the zero-size guard would silently
    // stop guarding — a check that agrees with everything, which is the H2 shape from ADR-125.
    box.width = Math.max(0, box.right - box.left);
    box.height = Math.max(0, box.bottom - box.top);
    return box;
  };
}

function layoutInvariants(windowIsTheSubject) {
  const frame = document.querySelector('[data-testid="shot-frame"]');
  if (!frame) return [];
  const out = [];
  const TOL = 1; // sub-pixel rounding; fractional layout is not a defect

  /** How thick a scrollbar is IN THIS BROWSER, at this zoom, under this media query. Measured, never
   *  chosen: `(pointer: coarse)` widens these to 14px in `global.css` and a zoomed page changes it
   *  again, so a hard-coded 15 would be wrong in both — and it is **0** here, which is the point.
   *  Headless Chromium 149 draws OVERLAY scrollbars: `offsetWidth - clientWidth` is 0 for
   *  `overflow: scroll`, with and without `scrollbar-width: auto`, empty or full. Windows WebView2 —
   *  the browser the product actually ships in — draws classic ones that cost ~15px. So this number
   *  is a real variable between the harness and the target, and reading it is the only way the same
   *  rule means the same thing in both. */
  const scrollbarSize = () => {
    const probe = document.createElement("div");
    probe.style.cssText =
      "position:absolute;top:-9999px;width:100px;height:100px;overflow:scroll;scrollbar-width:auto";
    document.body.appendChild(probe);
    const px = probe.offsetWidth - probe.clientWidth;
    probe.remove();
    return px;
  };

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

  // R5 — CONTENT LEAVES THE WINDOW DOWNWARDS. R1 is the horizontal half of this rule and, until the
  // shell scenes, that was the only half there could be: the frame is `maxWidth`-capped, so the
  // frame's right edge is a real boundary — but its height is `minHeight: 100vh` and otherwise
  // AUTO, so vertically the frame GROWS to whatever is put in it. Ask R1's question against the
  // frame's bottom edge and the answer is always no, because the edge moved. The reference that
  // does not move is the WINDOW, and it is only the right reference for a scene that declared the
  // window to be its subject — a panel scene is a `maxWidth` box on a page that legitimately
  // scrolls, and judging it against `innerHeight` would fire on every panel taller than the shot.
  //
  // Why it is owed at all: `unclipped` catches the bottom dock leaving the screen ONLY because
  // `.mtk-stage-column` happens to be `overflow: hidden`, so the pixels are lost to an ancestor
  // that this harness can see. Delete that one declaration and the dock does not get clipped — it
  // simply paints past the bottom of the window, off the physical screen, and every invariant here
  // goes quiet. Measured under that mutation: **the dock's own tab strip 217px below a 480px
  // window**, silent. A defect that hides by being MORE off-screen is the worst possible shape for
  // a gate to be blind to.
  //
  // AND IT USED TO SKIP EVERY CLIPPED ELEMENT, WHICH IS THE GAP ADR-126 NAMED ON ITS WAY OUT. The
  // first version walked the ancestors and gave up at the first one that clipped vertically, on the
  // R1 reasoning that "the overflow is deliberate and bounded". That reasoning holds only while the
  // clipping ancestor is itself on screen. Put a scroll container half off the bottom of the window
  // and its content is clipped AND off-screen: R5 skipped it because something clipped it, and R7 is
  // silent because it judges a child against its parent's own rectangle and the child is comfortably
  // inside a parent that is itself in the wrong place. **Two invariants, both correct about their own
  // question, and the pixels are gone between them.**
  //
  // The repair is to stop asking a yes/no question about clipping and read the rectangle that already
  // answers it. `__mtkClipRect` is the element's box intersected with every clipping ancestor — the
  // definition R3, `unclipped` and both protected floors have used since ADR-126 — so a row scrolled
  // below the fold of an on-screen scroller comes back clamped to that scroller and does not fire,
  // while the same row inside a scroller hanging off the window keeps a bottom edge below it and
  // does. One definition, now five readers, and the last special case that had its own opinion about
  // where an element is has been removed.
  if (windowIsTheSubject) {
    for (const el of els) {
      if (!visible(el, el.getBoundingClientRect())) continue;
      if (getComputedStyle(el).position === "fixed") continue; // positioned against the window by design
      const r = window.__mtkClipRect(el, frame);
      if (r.height < TOL) continue; // clipped to nothing, or a zero-height marker — not content
      if (r.bottom > window.innerHeight + TOL) {
        out.push(
          `${name(el)} is painted ${Math.round(r.bottom - window.innerHeight)}px below the bottom of ` +
            `a ${window.innerHeight}px window — those pixels survive every clip between it and the ` +
            "frame, so they are off the screen and there is no way to reach them",
        );
      }
    }
  }

  // R7 — CUT OFF WITH NO WAY TO REACH IT. R2 asks whether an element hides its OWN TEXT; this asks
  // the same question one relationship out — whether a CONTAINER hides a CHILD — which is the form
  // the two largest findings of this session both took. `overflow: hidden` on a parent loses the
  // child outright; `overflow: auto` with `scrollbar-width: none` loses it just as completely while
  // reading, in the stylesheet, like the affordance that every other invariant here accepts as the
  // excuse for content leaving a box. R1 and R2 both stop walking at the first clipping ancestor on
  // the grounds that "the user can pan to it"; this is the check that the sentence is TRUE.
  //
  // The bar is deliberately the scrollbar and not the scroll position: content below the fold of a
  // real scroller is reachable and is not a defect, which is why a long list does not fire here.
  for (const el of els) {
    const r = el.getBoundingClientRect();
    if (!visible(el, r) || r.width < TOL || r.height < TOL) continue;
    if (getComputedStyle(el).position === "fixed") continue;
    // Per axis, only the FIRST ancestor that clips on that axis decides reachability — the same walk
    // R1 makes, for the same reason. Judging every clipping ancestor instead was measured at **344
    // findings across the 21 scenes**, essentially all of them an inner `.mtk-scroll` doing its job
    // while some outer `overflow: hidden` was blamed for the row it had scrolled past. If that inner
    // scroller is itself cut, it is reported on its own line, where the subject is right.
    for (const [axis, oKey, edge] of [
      ["horizontally", "overflowX", (a, b) => Math.max(b.left - a.left, a.right - b.right)],
      ["vertically", "overflowY", (a, b) => Math.max(b.top - a.top, a.bottom - b.bottom)],
    ]) {
      for (let p = el.parentElement; p && p !== frame; p = p.parentElement) {
        const cs = getComputedStyle(p);
        const ov = cs[oKey];
        if (ov === "visible") continue;
        const lost = edge(r, p.getBoundingClientRect());
        const scroller = ov === "auto" || ov === "scroll";
        if (lost > TOL && !(scroller && cs.scrollbarWidth !== "none")) {
          out.push(
            `${name(el)} is cut ${Math.round(lost)}px ${axis} by ${name(p)}, which is ` +
              (scroller
                ? `overflow: ${ov} with scrollbar-width: none — it scrolls, and nothing on screen says so`
                : `overflow: ${ov} — there is no scrollbar and no way to reach what was cut`),
          );
        }
        break;
      }
    }
  }

  // R6 — A SCROLL CONTAINER WITH NOTHING TO SCROLL IN. `overflow: auto` is the affordance R1 and R2
  // both accept as the excuse for content leaving the box: "the user can pan to it". Collapse that
  // container to zero and the excuse becomes false while the declaration that bought it stays in the
  // stylesheet — content allocated, laid out, reachable by neither eye, mouse nor scrollbar, because
  // a scrollbar needs a box to live in. Every other invariant here goes quiet, and two of them go
  // quiet *because of* the declaration that is now a lie.
  //
  // Found live, and not by reasoning: with the bottom dock correctly yielded to 80px at a 1440×480
  // window, `.mtk-workspace-panel__body.mtk-scroll` measured **clientHeight 0 against scrollHeight
  // 179** — the whole Model workspace — because `.mtk-workspace-panel__header` is `flex: none` with a
  // 52px minimum and takes a 38px content box entirely.
  //
  // THE THRESHOLD WAS ZERO, AND ZERO WAS TOO SMALL BY ONE PIXEL. The first version's reasoning was
  // right and is kept: "a box that can show NOTHING is not a matter of taste, and a threshold with a
  // number in it is the kind that gets argued down to nothing at its first red." What it missed is
  // that a box which can show ALMOST nothing is not a matter of taste either, and the shell produced
  // one immediately. With a workspace open below a 631px window the dock's content box came out at
  // **exactly 1px** holding 188px of workspace — and stayed 1px at every window height from 440 down
  // to 300. `clientHeight < 1` is false for 1, so R6 said nothing; R7 exempts a scroller that has not
  // hidden its scrollbar, and this one had not — it simply had nowhere to draw one. The dock reported
  // `is-open`, the tab lit up, and the editor did nothing.
  //
  // SO THE BAR IS RAISED FROM "NOTHING" TO "NOT ONE ROW", AND BOTH HALVES ARE MEASURED. An
  // `overflow: auto` exemption is bought with two promises: an affordance the user can find, and a
  // box with something visible in it. A container smaller than its own scrollbar cannot keep the
  // first; a container shorter than one line of its own text cannot keep the second. Neither number
  // is chosen — `scrollbarSize()` comes from the renderer and `line-height` from the element's own
  // computed style — and the bar is whichever binds, so the rule means the same thing on a classic-
  // scrollbar platform (WebView2, ~15px) and an overlay one (this harness, 0px), where the row is
  // what constrains instead.
  //
  // HONEST ABOUT WHAT THIS DID AND DID NOT CHANGE, because the first draft of it over-claimed and the
  // mutation said so. At a 440px window HEAD's content box came out at `clientHeight` **0** — one
  // border-box pixel, minus its 1px border — so the OLD `< 1` rule fired there too and R6 was never
  // blind to that frame. The band it genuinely could not see is `h` 444–458, where the box is 1–15px:
  // the arithmetic is `content = h - 443`, so those windows produce a box with a real positive height,
  // holding 188px of workspace, that can show neither a scrollbar nor a single row. Above 459 the box
  // is taller than a line and R6 is silent BY DESIGN — "37px of 188 is not enough" is a judgement
  // about a specific panel, which is a scene's `min_height` and not a universal invariant's business.
  const gutter = scrollbarSize();
  for (const el of els) {
    const cs = getComputedStyle(el);
    const scrolls = (v) => v === "auto" || v === "scroll";
    // `scrollbar-width: none` is R7's business — that container hides its affordance deliberately and
    // R7 says so by name. Judging it here too would report one defect twice under two rule numbers.
    if (cs.scrollbarWidth === "none") continue;
    const line = Math.round(Number.parseFloat(cs.lineHeight)) || Math.round(Number.parseFloat(cs.fontSize) * 1.2) || 1;
    const bar = Math.max(gutter, line);
    const y = scrolls(cs.overflowY) && el.scrollHeight > el.clientHeight + TOL && el.clientHeight < bar;
    const x = scrolls(cs.overflowX) && el.scrollWidth > el.clientWidth + TOL && el.clientWidth < bar;
    if (!y && !x) continue;
    if (cs.display === "none" || cs.visibility === "hidden") continue;
    out.push(
      `${name(el)} declares overflow-${y ? "y" : "x"}: ${y ? cs.overflowY : cs.overflowX} and holds ` +
        `${y ? el.scrollHeight : el.scrollWidth}px of content in a box ${y ? el.clientHeight : el.clientWidth}px ` +
        `${y ? "tall" : "wide"} — less than the ${bar}px it would take to show ` +
        (gutter >= line ? `this browser's ${gutter}px scrollbar` : `one ${line}px line of its own content`) +
        ", so it can show neither the affordance nor a row, and the content is there and unreachable",
    );
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
  //
  // ON WHAT "THE SAME PIXELS" MEANS, AND WHY THIS READS THE CLIPPED RECT. The first version compared
  // `getBoundingClientRect()` directly, which is the box an element WOULD occupy — a scroll container
  // reports it for rows that are scrolled a hundred pixels out of view, and an `overflow: hidden`
  // ancestor reports it for content that is not painted at all. So R3 could call two things a
  // collision when neither of them was on screen, and this session's own new scenes produced twelve
  // such reports in one run: an `animation-marker-name` at y=768 inside a scroll container ending at
  // y=648, in a 640px window, "overlapping" an Inspector row that was equally far below the fold.
  // Twelve confident sentences about pixels that do not exist. R1 has always walked the ancestors for
  // exactly this reason and skipped what is clipped; R3 never did, so its verdicts were only ever
  // reliable in a frame with no scrolling in it — which described the eight panel scenes, and stopped
  // describing anything the moment the whole shell was in frame.
  //
  // Intersecting with every clipping ancestor is strictly better than R1's binary skip: an element
  // half inside a scroll container still collides on the half you can see, and an element entirely
  // outside one collides with nothing, because there is nothing there.
  //
  // `window.__mtkClipRect` is installed once per page by `installClipRect` below and used by BOTH
  // this and the scene-declared `unclipped` claim. Defining it twice — once here and once in the
  // claim evaluator, which cannot see this closure — would be two statements of one geometry, which
  // is the drift every other gate in this repository exists to catch.
  const clipped = (el) => window.__mtkClipRect(el, frame);
  const CONTROLS = 'button, a[href], input, select, textarea, [role="button"], [tabindex]:not([tabindex="-1"])';
  const controls = [...frame.querySelectorAll(CONTROLS)]
    .map((el) => [el, clipped(el)])
    .filter(([el, r]) => visible(el, r) && r.width > TOL && r.height > TOL);
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

  // R8 — TEXT THAT CANNOT BE READ. R1–R7 answer one question in seven ways: is it WHERE it should be.
  // Every one of them measures a rectangle, and a rectangle in exactly the right place, unclipped,
  // uncovered and un-overlapped, is silent about whether there is anything legible inside it. That is
  // not a gap in the rules; it is the axis they are not written on — the same shape as ADR-126's
  // "every invariant asked a horizontal question", one dimension further out.
  //
  // WHAT THIS IS SCOPED TO, AND WHY THAT EXACT SCOPE. WCAG 2.2 SC 1.4.3 (Contrast Minimum, Level AA):
  // 4.5:1 for text, 3:1 for large-scale text. It is not a house style — EN 301 549 clause 11.1.4.3
  // binds non-web software to WCAG 2.1 SC 1.4.3, and Section 508 E207.2 binds it to WCAG 2.0's, and
  // the criterion's text is identical in 2.0/2.1/2.2. The Constitution's own Accessibility section
  // asks for "High readability" and a "High contrast mode"; this measures the first one.
  //
  // THE EXEMPTION IS THE STANDARD'S, NOT OURS — and it is what keeps this rule from being a style
  // opinion. SC 1.4.3 excepts "text ... that is part of an inactive user interface component", and
  // Understanding 1.4.3 names the case: "a disabled control in HTML". So an inactive control's own
  // label is skipped, and everything else must be readable. That draws the "essential information"
  // line MECHANICALLY, which a gate could never do by judging what matters: the residue after the
  // exemption is exactly the text a user is expected to read.
  //
  // TWO THINGS THE PALETTE CANNOT PREDICT, WHICH IS WHY THIS MEASURES THE PAGE AND NOT THE STYLESHEET:
  //   * `opacity` on an ancestor dims the text AFTER the colour is chosen and composes with it. The
  //     Reveal panel's rejection reason was `opacity: 0.75` over a token that clears AA on its own
  //     surface: 5.00:1 as authored, 3.07:1 as painted. A token audit calls that pair fine.
  //   * a background is a STACK. A translucent surface over a panel over the backdrop is none of the
  //     three, and only the composite is what the eye receives.
  //
  // Modelled from computed styles rather than sampled from the framebuffer, deliberately: SC 1.4.3 is
  // defined on specified colours, and Understanding 1.4.3 says as much when it warns that
  // anti-aliasing can leave text "a contrast ratio that nominally passes ... but has a much lower
  // contrast in practice". Sampling pixels would measure the renderer's gamma, not the author's choice.
  const srgb = (c) => {
    const s = c / 255;
    return s <= 0.04045 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
  };
  // WCAG 2.2 relative luminance + contrast ratio, verbatim from the normative definitions.
  const relLum = (c) => 0.2126 * srgb(c.r) + 0.7152 * srgb(c.g) + 0.0722 * srgb(c.b);
  const contrastOf = (a, b) => {
    const l1 = relLum(a);
    const l2 = relLum(b);
    return (Math.max(l1, l2) + 0.05) / (Math.min(l1, l2) + 0.05);
  };
  const rgbOf = (s) => {
    const m = /rgba?\(([^)]+)\)/.exec(s || "");
    if (!m) return null;
    const p = m[1].split(/[,\s/]+/).filter(Boolean).map(Number);
    if (p.length < 3 || p.some((n) => Number.isNaN(n))) return null;
    return { r: p[0], g: p[1], b: p[2], a: p.length > 3 ? p[3] : 1 };
  };
  const composite = (fg, bg) => ({
    r: fg.r * fg.a + bg.r * (1 - fg.a),
    g: fg.g * fg.a + bg.g * (1 - fg.a),
    b: fg.b * fg.a + bg.b * (1 - fg.a),
    a: 1,
  });
  // The painted background under an element: the first opaque `background-color` up the tree, with
  // every translucent one above it composited onto it in paint order. Falls back to white, which is
  // what a browser paints over nothing — and is the CONSERVATIVE choice for a light theme, since
  // white is the lightest possible backdrop and therefore the most forgiving to dark text.
  const paintedBg = (el) => {
    const layers = [];
    for (let p = el; p; p = p.parentElement) {
      const c = rgbOf(getComputedStyle(p).backgroundColor);
      if (!c || c.a === 0) continue;
      layers.push(c);
      if (c.a >= 1) break;
    }
    let acc = layers.length && layers[layers.length - 1].a >= 1 ? layers.pop() : { r: 255, g: 255, b: 255, a: 1 };
    for (let i = layers.length - 1; i >= 0; i--) acc = composite(layers[i], acc);
    return acc;
  };
  // `opacity` below 1 composites the whole subtree against what is behind it, so it multiplies down
  // the tree and belongs on the TEXT side of the comparison — not on the background, which is behind
  // the group being faded. Stops at the frame: the harness's own chrome is not the product's.
  const groupAlpha = (el) => {
    let a = 1;
    for (let p = el; p && p !== frame; p = p.parentElement) {
      const o = parseFloat(getComputedStyle(p).opacity);
      if (!Number.isNaN(o) && o < 1) a *= o;
    }
    return a;
  };
  // "Large scale" is normatively 18pt, or 14pt bold (WCAG 2.2 glossary). The px equivalents are
  // INFORMATIVE and W3C's own two statements disagree — Understanding 1.4.3 says "approximately
  // 18.5px and 24px" while the conversion it states in the same paragraph (1pt = 1.333px) gives
  // 18.66px. We take the arithmetic one, because it is derivable from the normative pt figure rather
  // than rounded from it, and being the LARGER of the two it is the stricter reading: fewer elements
  // qualify for the relaxed 3:1 bar.
  const LARGE_PX = 24;
  const LARGE_BOLD_PX = 14 * (4 / 3);
  // "An inactive user interface component" — the criterion's own exception, and the only one taken.
  const inactive = (el) =>
    !!el.closest('[disabled], [aria-disabled="true"], fieldset[disabled], [aria-hidden="true"]');
  const reported = new Set();
  for (const el of els) {
    // Only an element that owns a text node: a wrapper's `color` is inherited by children that will
    // be judged themselves, and judging the wrapper too would report one defect as many.
    if (![...el.childNodes].some((n) => n.nodeType === 3 && n.textContent.trim().length > 0)) continue;
    const r = clipped(el);
    if (!visible(el, r) || r.width < TOL || r.height < TOL) continue;
    if (inactive(el)) continue;
    const cs = getComputedStyle(el);
    const fg = rgbOf(cs.color);
    if (!fg) continue;
    const bg = paintedBg(el);
    const alpha = fg.a * groupAlpha(el);
    if (alpha <= 0) continue; // invisible, not unreadable — `visible()` and R1..R7 own that
    const painted = composite({ ...fg, a: alpha }, bg);
    const px = parseFloat(cs.fontSize);
    const bold = (parseInt(cs.fontWeight, 10) || 400) >= 700;
    const large = px >= LARGE_PX || (bold && px >= LARGE_BOLD_PX);
    const need = large ? 3 : 4.5;
    const ratio = contrastOf(painted, bg);
    // Understanding 1.4.3: "the computed values should not be rounded (e.g., 4.499:1 would not meet
    // the 4.5:1 threshold)". So the comparison is strict, and the reported number is truncated
    // DOWNWARDS — a report that rounds 4.497 to "4.50" argues against its own finding.
    if (ratio >= need) continue;
    const shown = (Math.floor(ratio * 100) / 100).toFixed(2);
    // One line per (colour, background, size) — the same token failing in forty rows is one defect,
    // and forty sentences about it is a report nobody finishes reading (the R7 calibration lesson).
    const key = `${cs.color}|${Math.round(bg.r)},${Math.round(bg.g)},${Math.round(bg.b)}|${alpha.toFixed(3)}|${px}|${bold}`;
    if (reported.has(key)) continue;
    reported.add(key);
    const dimmed = alpha < 1 ? `, dimmed to ${alpha.toFixed(2)} by an ancestor's opacity` : "";
    out.push(
      `${name(el)} is ${shown}:1 against what is painted behind it and needs ${need}:1 — ` +
        `${cs.color} on rgb(${Math.round(bg.r)}, ${Math.round(bg.g)}, ${Math.round(bg.b)})${dimmed} at ` +
        `${px}px/${cs.fontWeight}. It is not part of an inactive control, so it is text a reader is ` +
        "expected to read (WCAG 2.2 SC 1.4.3)",
    );
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
// `width` caps the frame inside the default window; `viewport` resizes the window and the frame
// spans it. A scene that sets both has written one number twice — and the frame's copy is the one
// that would quietly stop matching, photographing a box whose contents laid themselves out for a
// different width. Rejected at the registry, before anything is captured.
const doubled = registry.filter((s) => s.viewport && s.width !== undefined);
if (doubled.length) {
  console.error(`FAIL  scene(s) setting BOTH \`width\` and \`viewport\`: ${doubled.map((s) => s.id).join(", ")}`);
  console.error("        one of the two is the width the content lays out against; the other is a copy that will drift");
  process.exit(1);
}
const scenes = only ? registry.filter((s) => s.id === only) : registry;
if (scenes.length === 0) {
  console.error(`FAIL  --scene ${only} matches none of: ${registry.map((s) => s.id).join(", ")}`);
  process.exit(1);
}

let failed = 0;
for (const scene of scenes) {
  const { id, looking_for, expect, viewport, click } = scene;
  const page = await browser.newPage();
  // EVERY SCENE STARTS FROM A FIRST-RUN USER'S STATE. Chromium serves one storage area to every
  // `file://` page in a run, and the shell remembers real things there: which docks are pinned
  // (`metrocalk:shell-layout:v1:*`), which sections are open (`metrocalk:disclosure:*`), and whether
  // the first-run card has been dismissed (`mtk.onboarded.v1`).
  //
  // Measured, not assumed — the first version of this comment blamed a red on the leak and was
  // wrong, so the claim was tested. Open `shell-build`, click the onboarding card's Skip, then open
  // `shell-terrain`: WITHOUT this reset the second scene has no first-run card at all, because a
  // click in a previous scene dismissed it forever. Every later scene is then composed and measured
  // against a shell that is quietly missing one of its overlays, and whether it is missing depends
  // on which scenes ran first. That is a verdict decided by run order, which is the isolation rule
  // in `<test_and_ci_discipline>` 4. WITH the reset, both scenes show the card.
  //
  // `evaluateOnNewDocument` runs before the page's own scripts, so a scene that WANTS a remembered
  // state still writes it in `setup`, which runs later.
  await page.evaluateOnNewDocument(() => {
    try {
      localStorage.clear();
      sessionStorage.clear();
    } catch {
      /* a storage-less context is already isolated */
    }
  });
  // 620×900 is the default a panel scene is composed against; a responsive scene names its own,
  // because the thing under test reads the window rather than its container.
  await page.setViewport({ width: 620, height: 900, ...(viewport ?? {}), deviceScaleFactor: 2 });
  const problems = [];
  page.on("pageerror", (e) => problems.push(`pageerror: ${e}`));
  page.on("console", (m) => m.type() === "error" && problems.push(`console.error: ${m.text()}`));

  await page.goto(`${href}?scene=${id}`, { waitUntil: "networkidle0" });
  await new Promise((r) => setTimeout(r, 250)); // the panels fetch their report in an effect

  // Drive the scene to the state it claims to photograph. A selector that matches nothing is a
  // FAILURE: silently skipping it would capture the default state under a caption describing
  // another, and a caption nothing evaluates is what this harness exists to stop.
  for (const sel of click ?? []) {
    const el = await page.$(sel);
    if (!el) {
      problems.push(`click \`${sel}\` matched nothing — the scene never reached the state it claims`);
      break;
    }
    await el.click();
    await new Promise((r) => setTimeout(r, 300)); // the workspace is lazy-loaded behind Suspense
  }

  // MEASURE BEFORE CAPTURING, because capturing MOVES THINGS. `screenshot({ fullPage: true })`
  // resizes the page to the content box and puts it back, and a shell that lays itself out from
  // `window.innerWidth` reacts: `shell-wide` rendered its docks open at 1440, the capture briefly
  // narrowed the window under the 980 px breakpoint, the shell did exactly what it is supposed to do
  // and collapsed both docks to rails — and the assertions, which used to run after the capture,
  // then read a shell that had responded to the camera. Every measurement this gate has ever taken
  // was taken on the layout the capture left behind; the eight panel scenes never noticed only
  // because a panel does not listen for `resize`. The claim and the invariants now run on the layout
  // the SCENE produced, and the perturbation itself is fixed below.
  //
  // The scene's OWN claim, evaluated in the page. `[data-testid="shot-frame"]` is emitted by the
  // harness wrapper, not by `scene.render()`, so it was never evidence that the scene rendered —
  // it is true whenever the bundle mounts at all.
  await page.evaluate(installClipRect);
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
    // A PROTECTED FLOOR IS A MEASUREMENT — OF THE PIXELS THAT ARE ON SCREEN.
    //
    // `min_width` (ADR-125) exists because a grid-template STRING cannot say whether the tracks fit
    // in the window; `min_height` is the same claim on the axis that had never had even a string —
    // the dock's height is a `vh` clamp that has never known the header exists, and below it sat a
    // viewport at `min-height: 0`, so on a short window the stage paid the whole difference (282px
    // of stage at 1440×700, 78px at 480).
    //
    // Both read the CLIPPED rect, and that is not a detail. The first version of `min_height` read
    // `getBoundingClientRect()`, which is the box an element WOULD occupy — the same mistake R3 was
    // repaired for twenty lines below, made again in the same session by the check whose entire
    // subject is "is the stage actually there". `.mtk-stage-column` is `overflow: hidden`, so the
    // configuration that defeats it is not exotic: it is the NAIVE FIX FOR THE VERY DEFECT THIS
    // GATE WAS WRITTEN FOR. Put `minHeight: STAGE_MIN` on the viewport — where its layout already
    // lives, inline, which is why an `#viewport { min-height }` in the stylesheet does not even
    // apply — leave the dock unyielding, and the element reports 320px while the column clips it to
    // 78. **Measured: `shell-dock-short` went GREEN under exactly that mutation**, reporting a floor
    // held about pixels no user can see. Reading the clipped rect turns it red and says which of the
    // two numbers is the lie. A gate that a plausible wrong repair satisfies is worse than no gate:
    // it certifies the wrong repair.
    const floor = (axis, pairs) => {
      const tall = axis === "height";
      for (const [sel, px] of pairs ?? []) {
        const el = frame.querySelector(sel);
        if (!el) {
          out.push(`min_${axis} needs \`${sel}\`, which is not there`);
          continue;
        }
        const box = el.getBoundingClientRect();
        const seen = window.__mtkClipRect(el, frame);
        const claimed = Math.round(tall ? box.height : box.width);
        const onScreen = Math.round(tall ? seen.height : seen.width);
        if (onScreen >= px - 1) continue;
        out.push(
          `\`${sel}\` measures ${onScreen}px${tall ? " tall" : ""}, below its protected floor of ${px}px — ` +
            (claimed - onScreen > 1
              ? `it CLAIMS ${claimed}px and an ancestor clips the rest away, so the floor is held in the ` +
                "layout tree and not on the screen"
              : `something ${tall ? "above or below" : "beside"} it refused to yield first`),
        );
      }
    };
    floor("width", e.min_width);
    floor("height", e.min_height);

    // THE STAGE'S FLOOR, STATED SO THAT IT IS TRUE IN EVERY REGIME — INCLUDING THE ONE WHERE 320px
    // CANNOT EXIST. Below a ~443px window the chrome (81px), the stage's floor and the dock's 42px
    // bar want more room than there is. A flat `min_height: 320` on the viewport is then asserting
    // arithmetic, not a rule, and a gate that must be wrong somewhere is a gate that gets an
    // exemption bolted on at its first red — which is exactly the waiver mechanism ADR-124 refused
    // to build. So the claim is not weakened for that regime, it is stated completely: **the stage
    // gets its floor, or everything that is left, whichever is smaller** — the sentence
    // `dockGridColumns` already wrote down for the other axis ("if even the rails and the floor do
    // not fit, the stage absorbs the remainder").
    //
    // "What is left" is measured, never assumed — and it is what the siblings are ENTITLED to, not
    // what they took. The first version subtracted each sibling's measured height, and that is the
    // shape of a gate that certifies the wrong repair: a dock forced to `min-height: 230px` at a
    // 480px window leaves the stage 169px, and a cap computed from 230 then declares 169 to be all
    // the stage was owed. **A greedy panel would have justified its own greed, and the scene's other
    // new claim — a 188px content box — is satisfied by exactly the same wrong repair.** So the
    // subtraction is each sibling's own computed `min-height`: the irreducible part, the thing the
    // stylesheet says it may never go below (the dock's is `var(--mtk-bottom-bar-height)`, which
    // resolves here to the same 42px the layout rule means by it). Everything above that minimum is
    // space the sibling is supposed to yield, so the floor is measured against a column where it
    // already has.
    //
    // In-flow is the general structural test and not a reading of the dock's mechanism — a floating
    // sheet takes no layout space, so the stage's room is the whole column, and that is true of any
    // future overlay for the same reason rather than because it is the dock.
    if (e.stage_floor != null) {
      const col = frame.querySelector(".mtk-stage-column");
      const vp = frame.querySelector("[data-testid='viewport']");
      if (!col || !vp) out.push("stage_floor needs .mtk-stage-column and the viewport, and one is not there");
      else {
        // …AND A SIBLING CANNOT RAISE ITS OWN ENTITLEMENT. Reading the declared `min-height` alone
        // still lets the stylesheet argue with the gate: set the dock to `min-height: 230px`, let it
        // yield to exactly that, and the cap becomes 169px — the stage is "owed" 169, gets 169, the
        // workspace measures its full 188px content box, and every claim here is satisfied by a
        // repair that quietly halved the stage on a window with room for the floor. So the
        // entitlement is capped by `--mtk-bottom-bar-height`: the shell's ONE statement of the
        // irreducible part of the thing below the stage — the strip you click to open it, the number
        // `dockForm` reads for the same purpose, and a constant a reviewer sees change. A sibling
        // that genuinely needs more than a bar has to make that argument on THIS line, in review,
        // instead of silently in its own rule.
        const barH = Number.parseFloat(getComputedStyle(col).getPropertyValue("--mtk-bottom-bar-height")) || 0;
        let taken = 0;
        for (const sib of col.children) {
          if (sib === vp || sib.contains(vp)) continue;
          const cs = getComputedStyle(sib);
          if (cs.position === "absolute" || cs.position === "fixed") continue; // floats over the stage, costs it nothing
          taken += Math.min(Number.parseFloat(cs.minHeight) || 0, barH); // `auto`/`0px` → 0: entitled to nothing
        }
        const room = Math.max(0, Math.round(col.clientHeight - taken));
        const want = Math.min(e.stage_floor, room);
        const seen = Math.round(window.__mtkClipRect(vp, frame).height);
        if (seen < want - 1) {
          out.push(
            `the stage measures ${seen}px tall against the ${want}px it is owed here — its ${e.stage_floor}px ` +
              `floor capped by the ${room}px this ${Math.round(col.clientHeight)}px column has left after ` +
              `everything in flow beside it. ` +
              (want === e.stage_floor
                ? "There was room for the floor and something else took it"
                : "The window cannot give the full floor, but it can give this, and it did not"),
          );
        }
      }
    }
    // A CONTROL THAT IS NOT ON SCREEN IS NOT A CONTROL. R1 and R2 both let an element off when a
    // scrollable ancestor clips it — "the overflow is deliberate and bounded", "the user can pan to
    // it" — and both exemptions are bought with an affordance: a scrollbar. `.mtk-dock-tabs` sets
    // `scrollbar-width: none` and hides the WebKit one, so the strip pans and NOTHING says it panned.
    // The two invariants waved it through and the defect it was hiding is the largest of this
    // session: with the bottom dock open, **`Runtime` measured 0 of its 92px at 1280, 1000 and 700px
    // windows** — seven authored workspaces, and on any ordinary laptop you could reach five.
    //
    // Declared per scene rather than made universal, deliberately. The universal form would fire on
    // every collapsed disclosure and every row below the fold of a long list, which are hidden BY
    // DESIGN; separating those from this needs a rule nobody has had to write yet, and an invariant
    // that cries wolf is one that gets waived at its first real defect (ADR-124's own argument
    // against building a waiver mechanism early). A scene that claims "these controls are reachable
    // here" is a claim with an owner, and it is the claim that was missing.
    for (const sel of e.unclipped ?? []) {
      const els = [...frame.querySelectorAll(sel)];
      if (!els.length) {
        out.push(`unclipped needs \`${sel}\`, which matches nothing`);
        continue;
      }
      for (const el of els) {
        const r = el.getBoundingClientRect();
        const c = window.__mtkClipRect(el, frame);
        const lost = Math.round(r.width - c.width);
        const lostY = Math.round(r.height - c.height);
        if (lost > 1 || lostY > 1) {
          const label = (el.textContent ?? "").replace(/\s+/g, " ").trim().slice(0, 24) || sel;
          out.push(
            `\`${sel}\` "${label}" is ${Math.round(r.width)}×${Math.round(r.height)}px but only ` +
              `${Math.round(c.width)}×${Math.round(c.height)}px of it is on screen — an ancestor ` +
              "clips it away, and a scroll container that shows no scrollbar says nothing about it",
          );
        }
      }
    }
    // CONSERVATION, WHERE A FLOOR IS ARITHMETICALLY UNREACHABLE. See `Expect.fills` for why this
    // exists rather than a waiver on `min_height`. The children must tile the container's content
    // box: measured on screen (so a child clipped away is a child that did not tile), summed, and
    // compared to the container. A gap means something is missing; a surplus means two of them are
    // on the same pixels; either way the column is not accounted for.
    for (const [csel, kids] of e.fills ?? []) {
      const container = frame.querySelector(csel);
      if (!container) {
        out.push(`fills needs \`${csel}\`, which is not there`);
        continue;
      }
      const have = kids.map((k) => [k, frame.querySelector(k)]);
      const gone = have.filter(([, el]) => !el).map(([k]) => k);
      if (gone.length) {
        out.push(`fills \`${csel}\` names ${gone.join(", ")}, which ${gone.length > 1 ? "are" : "is"} not there`);
        continue;
      }
      const box = container.clientHeight;
      const parts = have.map(([k, el]) => [k, Math.round(window.__mtkClipRect(el, frame).height)]);
      const sum = parts.reduce((a, [, h]) => a + h, 0);
      if (Math.abs(sum - box) > 1) {
        out.push(
          `\`${csel}\` is ${box}px tall and the ${parts.length} things inside it measure ` +
            `${parts.map(([k, h]) => `${k} ${h}px`).join(" + ")} = ${sum}px — ` +
            (sum < box
              ? `${box - sum}px of it is accounted for by nothing, so something was cut or never laid out`
              : `${sum - box}px more than there is, so two of them are on the same pixels or one is clipped away`),
        );
      }
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
  // point is that the next panel added here inherits them for nothing. The one argument is R5's
  // precondition: a scene that set the WINDOW is a scene whose bottom edge is the window's, and a
  // scene that set a frame width is a `maxWidth` box on a page that is allowed to scroll. The
  // driver already rejects a scene that states both, so this reads one flag and not two.
  const layout = await page.evaluate(layoutInvariants, Boolean(scene.viewport));

  // A cheap, stable fingerprint of the layout that was just judged: how wide the frame is, and where
  // every visible control sits inside it. Compared against the same reading taken after the capture.
  const fingerprint = () => {
    const frame = document.querySelector('[data-testid="shot-frame"]');
    if (!frame) return "no frame";
    const box = frame.getBoundingClientRect();
    const sel = 'button, a[href], input, select, textarea, [role="button"]';
    const rows = [...frame.querySelectorAll(sel)].map((el) => {
      const r = el.getBoundingClientRect();
      return `${el.tagName}:${Math.round(r.left)},${Math.round(r.top)},${Math.round(r.width)}`;
    });
    return `${Math.round(box.width)}×${Math.round(box.height)}|${rows.length}|${rows.join(" ")}`;
  };
  const before = await page.evaluate(fingerprint);

  // `fullPage` exists so a panel taller than the window is captured whole. A scene that set its own
  // `viewport` has declared the window to BE the subject — there is nothing beyond it to reach for —
  // and asking for it anyway is what triggers the resize that moved the shell. So it is not asked
  // for; the two cases are now distinguished by what the scene actually needs.
  const path = resolve(outDir, `${id}.png`);
  await page.screenshot({ path, fullPage: !viewport });
  const colours = distinctColours(readFileSync(path));

  // A GATE ON THE GATE. The reordering above stops the assertions reading a disturbed layout, but on
  // its own it would leave the PNG showing something the assertions never saw — evidence of a state
  // nothing checked, which is the failure mode this harness was built against. So the layout is read
  // again afterwards and must be unchanged: whatever the capture does, the picture and the verdict
  // are about the same thing, and any future capture path that starts moving the page fails here
  // instead of quietly making every screenshot a lie.
  const after = await page.evaluate(fingerprint);
  const disturbed =
    before === after
      ? []
      : [
          "the capture DISTURBED the layout it captured — the assertions above were checked against " +
            "one layout and the PNG shows another, so the picture is not evidence of the verdict",
        ];

  const bad = [...problems, ...claim, ...layout, ...disturbed];
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
