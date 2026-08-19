#!/usr/bin/env node

/**
 * The palette's own legibility claim, measured.
 *
 * WHY THIS EXISTS SEPARATELY FROM `shots` R8. R8 measures what is actually painted, which is the
 * stronger claim wherever a scene reaches — it sees `opacity`, composited backgrounds, and literal
 * colours, none of which a stylesheet audit can. But it only ever sees the pairs some scene happens
 * to render, and this session measured exactly how big that gap is: of the four palette repairs in
 * ADR-128, reverting `--mtk-accent` and `--mtk-warn-solid` left **all 23 scenes green**, because no
 * scene photographs accent text on the pressed fill or a filled warning button. Two repairs with no
 * gate on them is how they come back. This is the other half — every pair the palette can PRODUCE,
 * whether or not a capture exists for it — and it is ADR-118's rule again: a target that is only
 * ever compiled is not covered.
 *
 * Node built-ins only, like the constitution ratchet beside it, so the palette is gated before the
 * editor's dependency tree exists.
 *
 * ROLES ARE DECLARED, NOT INFERRED. A checker that discovers its own subjects by pattern goes quiet
 * the day someone renames one (`gpu-contract-audit` shipped with exactly that hole — ADR-118). Every
 * token below is named; a missing or unparseable one is a FAILURE, not a skip.
 */

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url));
const CSS_PATH = resolve(SCRIPT_DIR, "../src/theme/global.css");

// ── WCAG 2.2, normative definitions ───────────────────────────────────────────────────────────────
// Relative luminance and contrast ratio exactly as the criterion defines them. The 0.04045 knee is
// the post-2021 value; before then the glossary said 0.03928, which W3C's own errata note calls
// numerically irrelevant for 8-bit sRGB. The 0.05 term is Typical Viewing Flare from IEC 61966-2-1
// by way of ANSI/HFS 100-1988 — it is not a fudge factor, it is the ambient-light contribution the
// 1988 ergonomics standard requires be included.
const channel = (c) => {
  const s = c / 255;
  return s <= 0.04045 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
};
const luminance = ({ r, g, b }) => 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
export function contrast(a, b) {
  const l1 = luminance(a);
  const l2 = luminance(b);
  return (Math.max(l1, l2) + 0.05) / (Math.min(l1, l2) + 0.05);
}

// ── the roles this file gates ─────────────────────────────────────────────────────────────────────
// TEXT × SURFACE is the whole matrix: any panel may set any `tone`, so a text token that is only
// readable on some surfaces is a token that is readable by luck.
const SURFACES = [
  "bg-base", "bg-panel", "bg-raised", "bg-inset", "bg-input",
  "bg-canvas", "bg-hover", "bg-active",
  "accent-subtle", "success-bg", "warn-bg", "danger-bg", "info-bg",
];
// `text-faint` is deliberately absent: it is the INACTIVE-COMPONENT colour, which WCAG 2.2 SC 1.4.3
// exempts by name ("text ... that is part of an inactive user interface component"). It carries its
// own, lower, self-imposed floor below.
const BODY_TEXT = ["text", "text-secondary", "text-muted", "accent", "success", "warn", "danger", "info", "token"];
// White sits on every solid fill. `accent-solid-hover` included: a hover state is still read.
const SOLID_FILLS = ["accent-solid", "accent-solid-hover", "success-solid", "warn-solid", "danger-solid"];
const ON_SOLID = "on-accent";

const AA_TEXT = 4.5;
// The floor this project chose for the one thing the standard asks nothing of. WCAG exempts inactive
// components entirely; no major design system requires AA of disabled text (Material 3 composites at
// 38% alpha, which cannot reach even 3:1 by arithmetic); and APCA's own ladder puts "disabled element
// text" in its lowest tier rather than at parity. 3:1 is the lowest number that appears anywhere in
// SC 1.4.3/1.4.11 — a deliberate policy ABOVE the standard, recorded here so it reads as a choice.
const FAINT_FLOOR = 3;

function parseHex(value) {
  const m = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(value.trim());
  if (!m) return null;
  const h = m[1].length === 3 ? m[1].replace(/./g, (c) => c + c) : m[1];
  return { r: parseInt(h.slice(0, 2), 16), g: parseInt(h.slice(2, 4), 16), b: parseInt(h.slice(4, 6), 16) };
}

/** The `--mtk-*` declarations of one block, in source order (a later one wins, as in CSS). */
function declarations(block) {
  const out = new Map();
  for (const m of block.matchAll(/--mtk-([a-z0-9-]+)\s*:\s*([^;]+);/gi)) out.set(m[1], m[2].trim());
  return out;
}

/** Resolve a declared token to an sRGB colour, failing loudly rather than skipping. */
function colourOf(decls, name, findings, where) {
  const raw = decls.get(name);
  if (raw === undefined) {
    findings.push(`--mtk-${name} is not declared in ${where} — this gate names its subjects, so a renamed token is a failure, not a silence`);
    return null;
  }
  const c = parseHex(raw);
  if (!c) {
    findings.push(`--mtk-${name} is \`${raw}\` in ${where}, which this gate cannot measure — declare a hex literal, or the pair stops being checked without anyone noticing`);
    return null;
  }
  return c;
}

export function audit(css) {
  const findings = [];
  const rootAt = css.search(/(^|\n):root\s*\{/);
  if (rootAt < 0) return ["no top-level `:root` block in the stylesheet"];
  const balanced = (startIdx) => {
    const open = css.indexOf("{", startIdx);
    let depth = 0;
    for (let i = open; i < css.length; i++) {
      if (css[i] === "{") depth++;
      else if (css[i] === "}" && --depth === 0) return css.slice(open + 1, i);
    }
    return null;
  };
  const rootBody = balanced(rootAt);
  if (rootBody === null) return ["the `:root` block has unbalanced braces"];
  const base = declarations(rootBody);

  const moreAt = css.search(/@media\s*\(prefers-contrast:\s*more\)/);
  if (moreAt < 0) {
    findings.push("no `@media (prefers-contrast: more)` block — the Constitution's Accessibility section asks for a high-contrast mode, and a mode nothing declares is a mode nobody gets");
  }
  const moreBody = moreAt < 0 ? "" : (balanced(moreAt) ?? "");
  const more = declarations(moreBody);

  const surfaces = new Map();
  for (const s of SURFACES) {
    const c = colourOf(base, s, findings, "`:root`");
    if (c) surfaces.set(s, c);
  }
  if (surfaces.size === 0) return findings.length ? findings : ["no surface token resolved — this run checked nothing"];

  const check = (tokenName, colour, floor, label) => {
    for (const [sName, sColour] of surfaces) {
      const r = contrast(colour, sColour);
      // Understanding SC 1.4.3: the computed value is NOT rounded before comparison.
      if (r >= floor) continue;
      findings.push(
        `--mtk-${tokenName} on --mtk-${sName} is ${(Math.floor(r * 100) / 100).toFixed(2)}:1 and needs ${floor}:1 (${label})`,
      );
    }
  };

  for (const t of BODY_TEXT) {
    const c = colourOf(base, t, findings, "`:root`");
    if (c) check(t, c, AA_TEXT, "WCAG 2.2 SC 1.4.3, text on a surface this design system publishes");
  }
  const faint = colourOf(base, "text-faint", findings, "`:root`");
  if (faint) check("text-faint", faint, FAINT_FLOOR, "the inactive-component colour, held to this project's own floor");

  const onSolid = colourOf(base, ON_SOLID, findings, "`:root`");
  if (onSolid) {
    for (const f of SOLID_FILLS) {
      const c = colourOf(base, f, findings, "`:root`");
      if (!c) continue;
      const r = contrast(onSolid, c);
      if (r >= AA_TEXT) continue;
      findings.push(
        `--mtk-${ON_SOLID} on --mtk-${f} is ${(Math.floor(r * 100) / 100).toFixed(2)}:1 and needs ${AA_TEXT}:1 (a filled button's own label)`,
      );
    }
  }

  // A "more contrast" mode that is not more contrasting is worse than none: it answers the user's
  // request with the same picture. Judged against the WORST surface, which is the one that binds.
  const worstAgainstSurfaces = (c) => Math.min(...[...surfaces.values()].map((s) => contrast(c, s)));
  for (const t of [...BODY_TEXT, "text-faint"]) {
    if (!more.has(t)) continue; // a token the mode does not restate keeps its base value, which is checked above
    const baseColour = parseHex(base.get(t) ?? "");
    const moreColour = parseHex(more.get(t) ?? "");
    if (!baseColour || !moreColour) continue;
    if (worstAgainstSurfaces(moreColour) + 1e-9 < worstAgainstSurfaces(baseColour)) {
      findings.push(
        `--mtk-${t} is LESS readable under \`prefers-contrast: more\` (${worstAgainstSurfaces(moreColour).toFixed(2)}:1) than at rest (${worstAgainstSurfaces(baseColour).toFixed(2)}:1) — the high-contrast mode reduces contrast`,
      );
    }
  }
  return findings;
}

// ── self-test: the drifts that actually shipped, each replayed and each repaired ──────────────────
// A gate that stops discriminating must fail on itself. Every case below is a REAL value from this
// repository's history, plus the coverage cases that would otherwise switch the check off silently.
function runSelfTests() {
  const css = readFileSync(CSS_PATH, "utf8");
  const swap = (from, to) => {
    assert.ok(css.includes(from), `self-test fixture is stale: the stylesheet no longer contains \`${from}\``);
    return css.replace(from, to);
  };
  const cases = [
    ["the tree as it stands is clean", css, 0],
    ["--mtk-text-muted back to #6b7583 (the token whose comment said 'readable')",
      swap("--mtk-text-muted: #606976;", "--mtk-text-muted: #6b7583;"), 10],
    ["--mtk-text-faint back to #99a2af (below even the 3:1 floor, on every surface)",
      swap("--mtk-text-faint: #7a8596;", "--mtk-text-faint: #99a2af;"), 13],
    ["--mtk-accent back to #1a6ec2 (which no scene can catch — see ADR-128 M4)",
      swap("--mtk-accent: #196aba;", "--mtk-accent: #1a6ec2;"), 2],
    ["--mtk-warn-solid back to #a56f1c (white on amber, 4.29:1 — no scene can catch this either)",
      swap("--mtk-warn-solid: #9e6a1b;", "--mtk-warn-solid: #a56f1c;"), 1],
    ["a renamed surface token must FAIL, not be skipped",
      swap("--mtk-bg-inset: #f1f3f7;", "--mtk-bg-well: #f1f3f7;"), 1],
    ["a surface expressed as a var() this gate cannot measure must FAIL, not be skipped",
      swap("--mtk-bg-inset: #f1f3f7;", "--mtk-bg-inset: var(--mtk-bg-panel);"), 1],
    ["a high-contrast mode that LOWERS contrast must fail",
      swap("--mtk-text-muted: #4e5e70;", "--mtk-text-muted: #8b96a4;"), 1],
  ];
  let failed = 0;
  for (const [label, source, expected] of cases) {
    const found = audit(source);
    const ok = found.length === expected;
    if (!ok) failed++;
    console.log(`${ok ? "ok  " : "FAIL"}  ${label} — expected ${expected} finding(s), got ${found.length}`);
    if (!ok) for (const f of found.slice(0, 4)) console.log(`        ${f}`);
  }
  if (failed) {
    console.error(`\n${failed} self-test case(s) failed — this gate is not discriminating and must not be trusted.`);
    process.exit(1);
  }
  console.log(`\npalette-contrast self-test: ${cases.length} cases, all as expected.`);
}

function main() {
  if (process.argv.includes("--self-test")) {
    runSelfTests();
    return;
  }
  const findings = audit(readFileSync(CSS_PATH, "utf8"));
  if (findings.length === 0) {
    console.log(
      `palette contrast: every text token clears ${AA_TEXT}:1 on all ${SURFACES.length} published surfaces, ` +
        `--mtk-text-faint clears its ${FAINT_FLOOR}:1 inactive-component floor, and every solid fill carries its own label.`,
    );
    return;
  }
  console.error("palette contrast — a token cannot be read on a surface this design system publishes:\n");
  for (const f of findings) console.error(`  ${f}`);
  console.error(
    `\n${findings.length} pair(s). WCAG 2.2 SC 1.4.3 is not a house style here: EN 301 549 clause 11.1.4.3 ` +
      "binds non-web software to it, and Section 508 E207.2 binds it to the identical WCAG 2.0 criterion.",
  );
  process.exit(1);
}

main();
