#!/usr/bin/env node
//! Which characters the editor is allowed to DRAW A CONTROL WITH.
//!
//! WHY THIS EXISTS, IN ONE PICTURE. `animation-timeline-tracks.png` captured the animation transport:
//! four buttons, three of them empty boxes, and the one that rendered was `▶`. That is the entire
//! diagnosis. `--mtk-font-ui` is `Inter · Segoe UI · system-ui · -apple-system · Helvetica Neue · Arial ·
//! sans-serif`. `▶` (U+25B6) is Geometric Shapes, Unicode 1.1, 1993 — every one of those fonts has it.
//! `⏮ ⏭ ⏸ ⏹` (U+23EE/23ED/23F8/23F9) are Miscellaneous Technical media controls added in Unicode 6.0,
//! 2010, and NOT ONE font in that stack carries them. So the browser leaves the declared stack: on a
//! Linux host it finds nothing and draws .notdef; on Windows it lands on Segoe UI Emoji and draws a
//! COLOUR pictograph into a monochrome toolbar. Both are wrong; the second is worse, because it looks
//! deliberate.
//!
//! EVERY FUNCTIONAL TEST WAS GREEN THE WHOLE TIME. Each button had an `aria-label`, a `data-testid` and
//! a click handler, and a control with no visible glyph still clicks. A state-only gate cannot see an
//! empty button — which is what `<visual_acceptance>` is for, and the capture is how this was found.
//!
//! THE RULE CANNOT BE COMPUTED, SO IT IS DECLARED — same shape as `check-first-paint.mjs`. Font coverage
//! is a property of fonts on the user's machine, not of the repository, so there is nothing here to
//! measure. `scripts/glyph-coverage.json` therefore lists every non-ASCII character the source uses to
//! draw something, each with a status and a reason a human wrote:
//!
//!   allowed    — a block the declared stack genuinely covers (arrows, math, box drawing, geometric
//!                shapes), or a symbol SEEN RENDERING in a named capture in `progress/`.
//!   emoji      — renders, but as a COLOUR pictograph from a fallback emoji font on Windows. Tracked,
//!                not forbidden: it is a design-consistency debt, not a blank control.
//!   unverified — present in the source, no capture has been checked for it yet. Visible backlog.
//!   forbidden  — proven absent from the stack. Using one fails this gate.
//!
//! A character that is in the source and NOT in the manifest also fails: the point is that adding a new
//! symbol is a DECISION someone records, not something that lands silently and is discovered in a
//! screenshot two milestones later.
//!
//! Comment lines are skipped — this file and the ADR both have to be able to name `⏸` in prose in order
//! to explain it, and a gate that cannot survive its own documentation is a gate people delete.
//!
//! Node built-ins only, so it runs in CI before the install, beside the constitution and palette checks.
//! Run: `node scripts/check-glyph-coverage.mjs` · self-test: `--self-test`.

import { mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const editorRoot = fileURLToPath(new URL("../", import.meta.url));
const MANIFEST = join(editorRoot, "scripts", "glyph-coverage.json");

/** A line that is entirely a comment. Deliberately conservative: it only skips lines that OPEN with a
 *  comment marker, so a trailing `// ...` after real JSX still gets scanned. */
const COMMENT_LINE = /^\s*(\/\/|\/\*|\*\/|\*)/;

/** Below this, everything is ASCII, Latin-1 or general punctuation — universally covered, never a finding. */
const FLOOR = 0x2000;
/** General Punctuation (dashes, quotes, ellipsis, NBSP) is covered by every font in the stack. */
const PUNCTUATION = [0x2010, 0x205f];

function inPunctuation(cp) {
  return cp >= PUNCTUATION[0] && cp <= PUNCTUATION[1];
}

function walk(dir, out = []) {
  for (const entry of readdirSync(dir)) {
    if (entry === "node_modules" || entry === "dist") continue;
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) walk(full, out);
    else if (/\.(tsx?|jsx?)$/.test(entry)) out.push(full);
  }
  return out;
}

/** Every non-ASCII drawing character used outside a comment, with where it is used. */
export function inventory(root) {
  const found = new Map();
  for (const file of walk(root)) {
    const lines = readFileSync(file, "utf8").split(/\r?\n/);
    lines.forEach((line, index) => {
      if (COMMENT_LINE.test(line)) return;
      for (const ch of line) {
        const cp = ch.codePointAt(0);
        if (cp < FLOOR || inPunctuation(cp)) continue;
        const key = `U+${cp.toString(16).toUpperCase().padStart(4, "0")}`;
        if (!found.has(key)) found.set(key, { char: ch, sites: [] });
        const site = `${relative(root, file).replace(/\\/g, "/")}:${index + 1}`;
        if (found.get(key).sites.length < 4) found.get(key).sites.push(site);
      }
    });
  }
  return found;
}

export function check(root, manifest) {
  const findings = [];
  const declared = manifest.glyphs;
  const used = inventory(root);

  for (const [key, { char, sites }] of used) {
    const entry = declared[key];
    if (!entry) {
      findings.push({
        level: "blocking",
        key,
        message:
          `${key} '${char}' is drawn by the UI and is not in glyph-coverage.json. Adding a symbol is a ` +
          `decision: record it with a status and a reason, or use an inline SVG icon instead.`,
        sites,
      });
      continue;
    }
    if (entry.status === "forbidden") {
      findings.push({
        level: "blocking",
        key,
        message: `${key} '${char}' is forbidden — ${entry.reason}`,
        sites,
      });
    }
  }

  const stale = Object.keys(declared).filter(
    (key) => declared[key].status !== "forbidden" && !used.has(key),
  );
  return { findings, used, stale };
}

function report(root, manifest, { quiet = false } = {}) {
  const { findings, used, stale } = check(root, manifest);
  const counts = { allowed: 0, emoji: 0, unverified: 0 };
  for (const key of used.keys()) {
    const status = manifest.glyphs[key]?.status;
    if (status in counts) counts[status] += 1;
  }
  if (!quiet) {
    for (const finding of findings) {
      console.log(`  ERROR  ${finding.message}`);
      for (const site of finding.sites) console.log(`           ${site}`);
    }
    if (stale.length) {
      console.log(
        `  note: ${stale.length} declared glyph(s) no longer used — prune them: ${stale.join(", ")}`,
      );
    }
    if (findings.length === 0) {
      console.log(
        `glyph coverage: ${used.size} drawing character(s) — ${counts.allowed} covered by the declared ` +
          `font stack, ${counts.emoji} tracked as colour-emoji fallback, ${counts.unverified} not yet ` +
          `seen in a capture. 0 findings.`,
      );
    }
  }
  return findings;
}

// ── self-test ────────────────────────────────────────────────────────────────────────────────────────
// The pair is the mutation test: each case must FIRE when drifted and be ABSENT when repaired, so a gate
// that stops discriminating fails here rather than going quiet.
function selfTest() {
  const root = mkdtempSync(join(tmpdir(), "mtk-glyph-"));
  mkdirSync(join(root, "src"), { recursive: true });
  const write = (name, body) => writeFileSync(join(root, "src", name), body, "utf8");
  const manifest = {
    glyphs: {
      "U+25B6": { status: "allowed", reason: "Geometric Shapes, 1993 — in every font in the stack." },
      "U+23F8": { status: "forbidden", reason: "Unicode 6.0 media control; no font in --mtk-font-ui has it." },
      "U+2705": { status: "emoji", reason: "renders, but as a colour pictograph on Windows." },
    },
  };
  const cases = [];
  const run = (label, expected, files) => {
    for (const name of readdirSync(join(root, "src"))) rmSync(join(root, "src", name));
    for (const [name, body] of Object.entries(files)) write(name, body);
    const findings = report(root, manifest, { quiet: true });
    const ok = findings.length === expected;
    cases.push({ ok, label, expected, actual: findings.length });
    console.log(
      `${ok ? "ok  " : "FAIL"}  ${label} — expected ${expected} finding(s), got ${findings.length}`,
    );
    if (!ok) for (const f of findings) console.log(`        → ${f.message}`);
  };

  run("a covered glyph is silent", 0, { "a.tsx": "export const a = <b>▶</b>;\n" });
  run("a forbidden glyph fires", 1, { "a.tsx": "export const a = <b>⏸</b>;\n" });
  run("an undeclared glyph fires", 1, { "a.tsx": "export const a = <b>⛓</b>;\n" });
  run("a tracked emoji does not block", 0, { "a.tsx": "export const a = <b>✅</b>;\n" });
  // The control that matters most: this gate's own documentation, and the ADR's, must be able to NAME
  // the forbidden character in prose. A gate that fails on the file explaining it is a gate people delete.
  run("a forbidden glyph inside a comment is not a use", 0, {
    "a.tsx": "// the transport used ⏸ and it did not render\nexport const a = 1;\n",
  });
  run("a doc-comment banner is not a use", 0, {
    "a.tsx": "//! ⏸ ⏹ ⏮ ⏭ — the four that were empty boxes\nexport const a = 1;\n",
  });
  // A trailing comment must NOT hide a real use on the same line.
  run("a real use is still caught when the line also has a comment", 1, {
    "a.tsx": "export const a = <b>⏸</b>; // still a use\n",
  });

  rmSync(root, { recursive: true, force: true });
  const failed = cases.filter((c) => !c.ok).length;
  console.log(
    `\nglyph-coverage self-test: ${cases.length - failed}/${cases.length} passed` +
      (failed ? " — a case stopped discriminating" : ""),
  );
  return failed === 0;
}

const isSelfTest = process.argv.includes("--self-test");
if (isSelfTest) {
  process.exit(selfTest() ? 0 : 1);
} else {
  const manifest = JSON.parse(readFileSync(MANIFEST, "utf8"));
  const findings = report(join(editorRoot, "src"), manifest);
  process.exit(findings.length === 0 ? 0 : 1);
}
