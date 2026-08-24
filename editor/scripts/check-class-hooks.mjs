#!/usr/bin/env node
//! Does the class the markup ASKS FOR exist in the stylesheet, and does the rule the stylesheet
//! WRITES reach any markup?
//!
//! WHY THIS EXISTS, IN ONE COMMIT. `ff5b73a` landed an `App.tsx` that wraps every shell region in
//! `<div className="mtk-shell-track …"><div className="mtk-shell-card">`, and the `global.css` that
//! defines `.mtk-shell-track` and `.mtk-shell-card` stayed in the working tree. HEAD therefore shipped
//! **four class hooks with no rules behind them**, and the result is not cosmetic: that card is what
//! carries the opaque background over a deliberately transparent native root (ADR-008) and what gives
//! `WorkspacePanel` a height to fill. The captured shell has no panel surfaces, no gutters, no
//! elevation, and both docks stop mid-column with dead ground below them.
//!
//! NOT ONE GATE IN THIS REPOSITORY COULD SEE IT. `tsc` type-checks `className` as `string` and is
//! finished. The CSS parser accepts a stylesheet that defines rules nobody uses. vitest renders the
//! components and asserts on roles, labels and `data-testid` — none of which a missing rule disturbs.
//! `check-ui-constitution` reads inline styles. `shots` captured the broken shell and passed every
//! scene, because each scene's `expect` is a DOM claim and the DOM was correct: **an element with an
//! unstyled class is still an element**, exactly as a button with an undrawable glyph still clicks
//! (`check-glyph-coverage.mjs`, ADR-131).
//!
//! THE GENERAL RULE IT BELONGS TO is `<test_and_ci_discipline>` 6 — *compiling is not the same as
//! agreeing*. A class name is one contract stated **twice, in two languages**: once as a string in TSX
//! and once as a selector in CSS. Each statement is validated on its own and neither against the other,
//! so the browser is the first thing that ever compares them. That is the same shape as a WGSL
//! `@location` against a Rust `VertexBufferLayout`, and `tools/gpu-contract-audit` is the same answer:
//! run the comparison at rest, in milliseconds.
//!
//! IT IS ALSO THE PREDICTABLE COST OF THE PATHSPEC COMMIT (ADR-132). `git commit -- <path>` cannot
//! revert a concurrent writer, which is why it is the right rule with three agents in one tree — but it
//! builds its tree from HEAD plus that one path, so a change that is coherent across two files lands as
//! one. This gate is what makes that half-landing loud instead of silent.
//!
//! TWO DIRECTIONS, because the contract has two ends:
//!
//!   unstyled hook  — markup emits a class the CSS never defines. The shipped defect. Blocking.
//!   orphan rule    — the CSS defines a class no markup emits. Dead weight, and the residue a rename
//!                    leaves behind. Blocking, so it stays at zero rather than accumulating.
//!
//! WHAT COUNTS AS A CLASS NAME, and why each exclusion is provable from the source rather than guessed:
//!
//!   * a class is a **whitespace-delimited token inside a string literal** — that is precisely how the
//!     browser tokenises `class`, so it is the right unit, and it is what keeps `"text/mtk-id"` (a MIME
//!     type) from being read as a hook;
//!   * a token preceded by `-` is a **custom property** (`var(--mtk-bg-panel)`), never a class;
//!   * a token followed by `${` is **open-ended**: the interpolation may extend the name
//!     (`` `mtk-btn--${variant}` `` → `.mtk-btn--primary`) or may only append a space-separated state
//!     class (`` `mtk-disclosure__caret${expanded ? " is-open" : ""}` `` → `.mtk-disclosure__caret`).
//!     Both readings are legal, so either an exact rule or any rule extending it satisfies it;
//!   * a literal in an **id position** (`id=`, `htmlFor=`, `aria-labelledby=`, or assigned to a `…Id`
//!     binding) is an element id — `` const rootId = id ?? `mtk-disclosure-${generatedId}` ``;
//!   * a name declared by `@keyframes` is an **animation name**, not a class.
//!
//! Getting these wrong in the LENIENT direction is what a gate is judged on, so the exclusions were
//! developed against the reverse check: the orphan direction is what proved that an early version was
//! dropping real `className` literals whenever an `aria-labelledby` appeared earlier on the same line.
//!
//! THE RESIDUE IS DECLARED, NOT INFERRED — same shape as `first-paint.json` and `glyph-coverage.json`.
//! A handful of hooks legitimately carry no rule: a BEM block root whose surface comes from a shared
//! component, an automation anchor an E2E spec queries, one absent member of a modifier family. That is
//! a decision a human makes once and writes down, not a pattern a script can recognise. Anything not in
//! `scripts/class-hooks.json` fails.
//!
//! Node built-ins only, so it runs in CI before the install, beside the constitution, palette,
//! first-paint and glyph checks.
//! Run: `node scripts/check-class-hooks.mjs` · self-test: `--self-test`.

import { mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const editorRoot = fileURLToPath(new URL("../", import.meta.url));
const MANIFEST = join(editorRoot, "scripts", "class-hooks.json");

/** The project's own namespace. Third-party and state classes (`is-open`, `data-*`) are deliberately
 *  out of scope: this gate is about the contract Metrocalk owns both ends of. */
const NS = /mtk-[\w-]*/g;

function walk(dir, out = []) {
  for (const entry of readdirSync(dir)) {
    if (entry === "node_modules" || entry === "dist") continue;
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) walk(full, out);
    else out.push(full);
  }
  return out;
}

/** Replace a matched span with spaces so every later offset — and therefore every reported line
 *  number — stays exactly where it was in the original file. */
const blank = (m) => m.replace(/[^\n]/g, " ");

/** Block comments always; line comments only when the `//` is not part of a `://` URL and not inside
 *  an obvious string. Conservative on purpose: over-blanking would hide a real use. */
function stripComments(source) {
  return source
    .replace(/\/\*[\s\S]*?\*\//g, blank)
    .replace(/(^|[^:/"'`\\])\/\/[^\n]*/g, (m, keep) => keep + " ".repeat(m.length - keep.length));
}

/** The literal is an element id, not a class list. Anchored hard against the literal's opening quote —
 *  an earlier `aria-labelledby` on the same line must not swallow the `className` that follows it. */
const ID_POSITION =
  /(?:\bid|\bhtmlFor|\baria-labelledby|\baria-describedby|\baria-controls|\baria-owns)\s*=\s*\{?\s*$|\b(?:const|let|var)\s+[A-Za-z_$][\w$]*[Ii]d\s*=\s*(?:[A-Za-z_$][\w$]*\s*\?\?\s*)?$/;

/** Every `mtk-*` class name the TS/TSX claims, with where it is claimed and whether an interpolation
 *  immediately follows it. */
export function claimedHooks(root) {
  const claims = new Map();
  const files = walk(root).filter((f) => /\.tsx?$/.test(f) && !/\.test\.tsx?$/.test(f));
  for (const file of files) {
    const source = stripComments(readFileSync(file, "utf8"));
    let line = 1;
    for (let i = 0; i < source.length; i += 1) {
      const ch = source[i];
      if (ch === "\n") { line += 1; continue; }
      if (ch !== '"' && ch !== "'" && ch !== "`") continue;
      // Walk to the closing quote of the same kind, honouring backslash escapes.
      let j = i + 1;
      while (j < source.length) {
        if (source[j] === "\\") { j += 2; continue; }
        if (source[j] === ch) break;
        j += 1;
      }
      const body = source.slice(i + 1, j);
      const isId = ID_POSITION.test(source.slice(Math.max(0, i - 96), i));
      NS.lastIndex = 0;
      let m;
      while ((m = NS.exec(body))) {
        const before = m.index === 0 ? undefined : body[m.index - 1];
        const after = body[m.index + m[0].length];
        // A class name is a whitespace-delimited token. `}` opens the run after an interpolation
        // closes; `$` closes it before one opens.
        const delimitedLeft = before === undefined || /\s/.test(before) || before === "}";
        const delimitedRight = after === undefined || /\s/.test(after) || after === "$";
        if (!delimitedLeft || !delimitedRight) continue;
        if (!claims.has(m[0])) claims.set(m[0], { sites: [], openEnded: false, idOnly: true });
        const claim = claims.get(m[0]);
        const site = `${relative(root, file).replace(/\\/g, "/")}:${line + body.slice(0, m.index).split("\n").length - 1}`;
        if (!claim.sites.includes(site) && claim.sites.length < 4) claim.sites.push(site);
        if (after === "$") claim.openEnded = true;
        if (!isId) claim.idOnly = false;
      }
      line += body.split("\n").length - 1;
      i = j;
    }
  }
  return claims;
}

/** Every `.mtk-*` selector the stylesheets define, and every `@keyframes` name they declare. */
export function definedRules(root) {
  const rules = new Map();
  const keyframes = new Set();
  for (const file of walk(root).filter((f) => f.endsWith(".css"))) {
    const source = readFileSync(file, "utf8").replace(/\/\*[\s\S]*?\*\//g, blank);
    source.split(/\r?\n/).forEach((text, index) => {
      for (const m of text.matchAll(/\.(mtk-[\w-]*)/g)) {
        if (!rules.has(m[1])) rules.set(m[1], `${relative(root, file).replace(/\\/g, "/")}:${index + 1}`);
      }
      for (const m of text.matchAll(/@keyframes\s+(mtk-[\w-]*)/g)) keyframes.add(m[1]);
    });
  }
  return { rules, keyframes };
}

export function check(root, manifest) {
  const findings = [];
  const claims = claimedHooks(root);
  const { rules, keyframes } = definedRules(root);
  const ruleNames = [...rules.keys()];
  const declaredUnstyled = manifest.unstyled ?? {};
  const declaredUnused = manifest.unused ?? {};

  /** Rules any claim reaches — the exact hit plus, for an open-ended claim, everything extending it. */
  const reached = new Set();
  let counted = 0;

  for (const [hook, claim] of [...claims].sort()) {
    if (claim.idOnly || keyframes.has(hook)) continue;
    counted += 1;
    const exact = rules.has(hook);
    const extensions = claim.openEnded ? ruleNames.filter((r) => r.startsWith(hook)) : [];
    if (exact) reached.add(hook);
    for (const r of extensions) reached.add(r);
    if (exact || extensions.length) continue;
    if (hook in declaredUnstyled) continue;
    findings.push({
      level: "blocking",
      kind: "unstyled-hook",
      key: hook,
      message:
        `markup emits '${hook}' and no stylesheet defines it. Either add the rule, or record in ` +
        `class-hooks.json why this hook deliberately carries none.`,
      sites: claim.sites,
    });
  }

  for (const rule of ruleNames) {
    if (reached.has(rule) || rule in declaredUnused) continue;
    findings.push({
      level: "blocking",
      kind: "orphan-rule",
      key: rule,
      message:
        `'.${rule}' is defined and no markup emits it. Delete the rule, or record in class-hooks.json ` +
        `what reaches it from outside the React source.`,
      sites: [rules.get(rule)],
    });
  }

  // A manifest that outlives its reason is the failure mode of every declared gate, so both maps are
  // swept for entries that have stopped doing anything.
  const stale = [
    ...Object.keys(declaredUnstyled).filter((h) => !claims.has(h) || rules.has(h)),
    ...Object.keys(declaredUnused).filter((r) => !rules.has(r) || reached.has(r)),
  ];
  return { findings, counted, ruleCount: ruleNames.length, stale };
}

function report(root, manifest, { quiet = false } = {}) {
  const { findings, counted, ruleCount, stale } = check(root, manifest);
  if (!quiet) {
    for (const finding of findings) {
      console.log(`  ERROR  ${finding.message}`);
      for (const site of finding.sites) console.log(`           ${site}`);
    }
    if (stale.length) {
      console.log(`  note: ${stale.length} declared entr(y/ies) no longer needed — prune: ${stale.join(", ")}`);
    }
    if (findings.length === 0) {
      const declared =
        Object.keys(manifest.unstyled ?? {}).length + Object.keys(manifest.unused ?? {}).length;
      console.log(
        `class hooks: ${counted} class name(s) in the markup against ${ruleCount} rule(s) in the ` +
          `stylesheets — every hook has a rule and every rule has a hook, ${declared} declared ` +
          `exception(s). 0 findings.`,
      );
    }
  }
  return findings;
}

// ── self-test ────────────────────────────────────────────────────────────────────────────────────────
// Mutation pairs: each defect must FIRE when introduced and be SILENT when repaired, so a gate that
// stops discriminating fails here instead of going quiet. Case 2 is the defect exactly as `ff5b73a`
// shipped it.
function selfTest() {
  const root = mkdtempSync(join(tmpdir(), "mtk-hooks-"));
  mkdirSync(join(root, "src"), { recursive: true });
  const cases = [];

  const run = (label, expected, files, manifest = {}) => {
    rmSync(join(root, "src"), { recursive: true, force: true });
    mkdirSync(join(root, "src"), { recursive: true });
    for (const [name, body] of Object.entries(files)) writeFileSync(join(root, "src", name), body, "utf8");
    const findings = report(join(root, "src"), manifest, { quiet: true });
    const ok = findings.length === expected;
    cases.push({ ok, label });
    console.log(`${ok ? "ok  " : "FAIL"}  ${label} — expected ${expected} finding(s), got ${findings.length}`);
    if (!ok) for (const f of findings) console.log(`        → [${f.kind}] ${f.message}`);
  };

  run("a hook with a rule is silent", 0, {
    "a.tsx": 'export const A = () => <div className="mtk-card" />;\n',
    "a.css": ".mtk-card { color: red; }\n",
  });

  // THE SHIPPED DEFECT. `App.tsx` emits the track and the card; the stylesheet defines neither. The
  // third class in the same attribute DOES have a rule, so the pair proves the gate discriminates
  // inside one file rather than simply distrusting the file.
  run("the shell hooks with no stylesheet fire", 2, {
    "a.tsx": 'export const A = () => <div className="mtk-shell-track mtk-ok"><div className="mtk-shell-card" /></div>;\n',
    "a.css": ".mtk-ok { color: red; }\n",
  });
  run("...and are silent once the stylesheet lands", 0, {
    "a.tsx": 'export const A = () => <div className="mtk-shell-track mtk-ok"><div className="mtk-shell-card" /></div>;\n',
    "a.css": ".mtk-ok { color: red; }\n.mtk-shell-track { padding: 8px; }\n.mtk-shell-card { border-radius: 16px; }\n",
  });

  run("a declared unstyled hook does not block", 0,
    { "a.tsx": 'export const A = () => <div className="mtk-editor-root" />;\n', "a.css": ".mtk-x { color: red; }\n.mtk-x-used { color: red; }\n" },
    { unstyled: { "mtk-editor-root": { reason: "an E2E anchor" } }, unused: { "mtk-x": { reason: "t" }, "mtk-x-used": { reason: "t" } } });

  run("an orphan rule fires", 1, {
    "a.tsx": "export const A = () => <div />;\n",
    "a.css": ".mtk-group-body { padding: 8px; }\n",
  });
  run("a declared unused rule does not block", 0,
    { "a.tsx": "export const A = () => <div />;\n", "a.css": ".mtk-group-body { padding: 8px; }\n" },
    { unused: { "mtk-group-body": { reason: "set from the host page" } } });

  // The four exclusions, each with a control proving the gate still sees a real hook in the same file.
  run("a custom property is not a class", 0, {
    "a.tsx": 'export const A = () => <div className="mtk-card" style={{ color: "var(--mtk-text-muted)" }} />;\n',
    "a.css": ".mtk-card { color: red; }\n",
  });
  run("a MIME type that ends in the namespace is not a class", 0, {
    "a.tsx": 'const DRAG_MIME = "text/mtk-id";\nexport const A = () => <div className="mtk-card" data-m={DRAG_MIME} />;\n',
    "a.css": ".mtk-card { color: red; }\n",
  });
  run("an interpolated modifier is satisfied by any rule extending it", 0, {
    "a.tsx": "export const A = ({ v }) => <div className={`mtk-btn mtk-btn--${v}`} />;\n",
    "a.css": ".mtk-btn { color: red; }\n.mtk-btn--primary { color: blue; }\n",
  });
  run("an interpolated STATE suffix is satisfied by the exact rule", 0, {
    "a.tsx": 'export const A = ({ open }) => <div className={`mtk-caret${open ? " is-open" : ""}`} />;\n',
    "a.css": ".mtk-caret { color: red; }\n",
  });
  run("an element id is not a class", 0, {
    "a.tsx": "export const A = ({ n }) => { const rowId = `mtk-row-${n}`; return <div id={rowId} className=\"mtk-card\" />; };\n",
    "a.css": ".mtk-card { color: red; }\n",
  });
  // The regression the orphan direction found in this gate's own first draft.
  run("an aria-labelledby earlier on the line does not hide the className after it", 1, {
    "a.tsx": 'export const A = () => <div aria-labelledby="h" className="mtk-missing mtk-ok" />;\n',
    "a.css": ".mtk-ok { color: red; }\n",
  });
  run("a keyframes name is not a class", 0, {
    "a.tsx": 'export const A = () => <div className="mtk-card" style={{ animation: "mtk-fade-in 1s" }} />;\n',
    "a.css": ".mtk-card { color: red; }\n@keyframes mtk-fade-in { from { opacity: 0; } }\n",
  });
  run("a class named only in a comment is not a use", 0, {
    "a.tsx": '// the shell wraps everything in mtk-shell-track\nexport const A = () => <div className="mtk-card" />;\n',
    "a.css": ".mtk-card { color: red; }\n",
  });
  run("a stale declaration does not block but is reported", 0,
    { "a.tsx": 'export const A = () => <div className="mtk-card" />;\n', "a.css": ".mtk-card { color: red; }\n" },
    { unstyled: { "mtk-gone": { reason: "deleted last milestone" } } });

  rmSync(root, { recursive: true, force: true });
  const failed = cases.filter((c) => !c.ok).length;
  console.log(
    `\nclass-hooks self-test: ${cases.length - failed}/${cases.length} passed` +
      (failed ? " — a case stopped discriminating" : ""),
  );
  return failed === 0;
}

if (process.argv.includes("--self-test")) {
  process.exit(selfTest() ? 0 : 1);
} else {
  const manifest = JSON.parse(readFileSync(MANIFEST, "utf8"));
  const findings = report(join(editorRoot, "src"), manifest);
  process.exit(findings.length === 0 ? 0 : 1);
}
