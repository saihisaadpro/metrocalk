#!/usr/bin/env node
//! What the editor must have ALREADY LOADED the first time a user looks at it.
//!
//! `check-bundle.mjs` measures size, and size is a one-directional rule: every surface is cheaper
//! absent, so a budget that only says "smaller" is satisfied most completely by an editor that ships
//! nothing and fetches everything. Followed exactly, it produced exactly that mistake — the first-run
//! card and the OS-drop listener were both moved behind `React.lazy(... )` with a `fallback={null}`
//! during a split pass that took the entry chunk from 287,644 to 230,618 bytes and was, by the only
//! rule anyone had written down, a success.
//!
//! It is not a thing a measurement can decide. No number distinguishes "this panel is fine to fetch
//! when the user clicks it" from "this is the first sentence the product ever says". So the rule is
//! DECLARED — `scripts/first-paint.json`, one entry per module with the reason in prose — and this
//! gate checks the declaration against the real import graph.
//!
//! WHY THE SOURCE GRAPH AND NOT THE BUILT CHUNKS. "Is this module on the startup path?" is exactly
//! "is it reachable from the entry across only STATIC import edges" — the same question Rollup asks,
//! answered from the same facts, before a build exists. That makes this gate Node-built-ins-only, so
//! it runs in CI before the install (beside the constitution ratchet and the palette check) and fails
//! in milliseconds at the moment the mistake is made, rather than after a production build.
//!
//! `import type` / `export type` are NOT edges. They erase completely, so a type-only reference to a
//! deferred panel is free and must not be reported — the exhaustive-union prop contracts this codebase
//! relies on would otherwise be unwritable.
//!
//! Run: `node scripts/check-first-paint.mjs` · self-test: `--self-test`.

import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const editorRoot = fileURLToPath(new URL("../", import.meta.url));
const EXTENSIONS = [".ts", ".tsx", ".js", ".jsx", ".mjs"];

/** Static edges only. Each pattern is anchored at a statement start so a `//` comment mentioning
 *  `import(` — of which this codebase has many, explaining these very decisions — cannot create one. */
const STATIC_IMPORT = /^[ \t]*import[ \t]+(?!type[ \t])(?:[^;'"]*?[ \t]from[ \t]*)?["']([^"']+)["']/gm;
const STATIC_EXPORT_FROM = /^[ \t]*export[ \t]+(?!type[ \t])[^;'"]*?[ \t]from[ \t]*["']([^"']+)["']/gm;

function resolveSpecifier(specifier, fromFile) {
  if (!specifier.startsWith(".")) return null; // bare = node_modules; not our graph
  const base = resolve(dirname(fromFile), specifier);
  const candidates = [
    base,
    ...EXTENSIONS.map((extension) => `${base}${extension}`),
    ...EXTENSIONS.map((extension) => join(base, `index${extension}`)),
  ];
  for (const candidate of candidates) {
    if (existsSync(candidate) && statSync(candidate).isFile()) return candidate;
  }
  return null;
}

/** Every module reachable from `entry` across static edges only, as editor-relative POSIX paths. */
export function staticallyReachable(entryRelative, root = editorRoot) {
  const entry = resolve(root, entryRelative);
  if (!existsSync(entry)) throw new Error(`first-paint entry '${entryRelative}' does not exist`);
  const seen = new Set();
  const queue = [entry];
  while (queue.length > 0) {
    const file = queue.pop();
    const key = relative(root, file).split("\\").join("/");
    if (seen.has(key)) continue;
    seen.add(key);
    if (!EXTENSIONS.some((extension) => file.endsWith(extension))) continue; // .css and friends: no edges to follow
    const source = readFileSync(file, "utf8");
    for (const pattern of [STATIC_IMPORT, STATIC_EXPORT_FROM]) {
      pattern.lastIndex = 0;
      for (const match of source.matchAll(pattern)) {
        const target = resolveSpecifier(match[1], file);
        if (target) queue.push(target);
      }
    }
  }
  return seen;
}

export function auditFirstPaint(manifest, root = editorRoot) {
  const reachable = staticallyReachable(manifest.entry, root);
  const findings = [];
  for (const { module, reason } of manifest.mustBeStaticallyReachable) {
    if (!existsSync(resolve(root, module))) {
      findings.push({ rule: "missing-module", module, detail: "declared on the first-paint path but the file does not exist" });
    } else if (!reachable.has(module)) {
      findings.push({ rule: "deferred-first-paint-surface", module, detail: reason });
    }
  }
  for (const { module, reason } of manifest.mustNotBeStaticallyReachable) {
    if (!existsSync(resolve(root, module))) {
      findings.push({ rule: "missing-module", module, detail: "declared as on-demand but the file does not exist" });
    } else if (reachable.has(module)) {
      findings.push({ rule: "on-demand-surface-pulled-into-startup", module, detail: reason });
    }
  }
  return { reachableCount: reachable.size, findings };
}

// ── self-test ───────────────────────────────────────────────────────────────────────────────────────
// A gate that has stopped discriminating must fail on ITSELF rather than going quiet (the convention
// `check-ui-constitution --self-test` and `check-palette-contrast --self-test` set). Both directions are
// mutated, and a control case asserts the type-only edge stays invisible — because the cheapest way to
// break this gate is to make `import type` count, at which point every deferred panel is "reachable"
// and the second half of the manifest silently passes forever.
function selfTest() {
  const root = mkdtempSync(join(tmpdir(), "first-paint-selftest-"));
  const write = (relativePath, body) => {
    const absolute = join(root, relativePath);
    mkdirSync(dirname(absolute), { recursive: true });
    writeFileSync(absolute, body, "utf8");
  };
  const cases = [];
  const check = (name, condition, detail) => cases.push({ name, passed: Boolean(condition), detail });

  write("src/deferred.tsx", "export const Deferred = 1;\n");
  write("src/needed.tsx", "export const Needed = 1;\n");
  write("src/types.ts", "export type Contract = { a: number };\n");

  const manifest = {
    entry: "src/main.tsx",
    mustBeStaticallyReachable: [{ module: "src/needed.tsx", reason: "r" }],
    mustNotBeStaticallyReachable: [{ module: "src/deferred.tsx", reason: "r" }],
  };

  // 1. The healthy shape: one static edge, one dynamic edge. Silent.
  write("src/main.tsx", [
    'import { Needed } from "./needed";',
    'const D = () => import("./deferred");',
    "export { Needed, D };",
  ].join("\n"));
  check("healthy graph is silent", auditFirstPaint(manifest, root).findings.length === 0);

  // 2. THE DEFECT THAT SHIPPED: a first-paint surface moved behind a dynamic edge.
  write("src/main.tsx", [
    'const N = () => import("./needed");',
    'const D = () => import("./deferred");',
    "export { N, D };",
  ].join("\n"));
  const deferred = auditFirstPaint(manifest, root).findings;
  check(
    "a deferred first-paint surface fires",
    deferred.length === 1 && deferred[0].rule === "deferred-first-paint-surface",
    JSON.stringify(deferred),
  );

  // 3. The opposite drift: an on-demand panel pulled back onto the startup path by a static import.
  write("src/main.tsx", [
    'import { Needed } from "./needed";',
    'import { Deferred } from "./deferred";',
    "export { Needed, Deferred };",
  ].join("\n"));
  const pulled = auditFirstPaint(manifest, root).findings;
  check(
    "an on-demand surface pulled into startup fires",
    pulled.length === 1 && pulled[0].rule === "on-demand-surface-pulled-into-startup",
    JSON.stringify(pulled),
  );

  // 4. CONTROL: `import type` is not an edge. If this ever fires, the gate has started counting erased
  //    imports and its second half is worthless.
  write("src/main.tsx", [
    'import { Needed } from "./needed";',
    'import type { Contract } from "./types";',
    'export type { Contract };',
    'const D = () => import("./deferred");',
    "export { Needed, D };",
  ].join("\n"));
  check("a type-only import is not a startup edge", auditFirstPaint(manifest, root).findings.length === 0);

  // 5. CONTROL: a comment that merely mentions a dynamic import cannot create an edge.
  write("src/main.tsx", [
    'import { Needed } from "./needed";',
    '// historical note: this used to be import("./deferred") and that was the bug',
    "export { Needed };",
  ].join("\n"));
  check("a commented-out import is not an edge", auditFirstPaint(manifest, root).findings.length === 0);

  // 6. A declared module that does not exist is a finding, not a pass. A manifest naming a deleted file
  //    would otherwise go quiet exactly when it stopped protecting anything.
  const stale = auditFirstPaint(
    { ...manifest, mustBeStaticallyReachable: [{ module: "src/gone.tsx", reason: "r" }] },
    root,
  ).findings;
  check("a manifest entry for a missing file fires", stale.length === 1 && stale[0].rule === "missing-module", JSON.stringify(stale));

  rmSync(root, { recursive: true, force: true });
  const failed = cases.filter((c) => !c.passed);
  for (const c of cases) console.log(`${c.passed ? "ok  " : "FAIL"} ${c.name}${c.passed ? "" : ` — ${c.detail ?? ""}`}`);
  console.log(`\nself-test: ${cases.length - failed.length}/${cases.length} passed`);
  if (failed.length > 0) process.exit(1);
}

if (process.argv.includes("--self-test")) {
  selfTest();
} else {
  const manifest = JSON.parse(readFileSync(join(editorRoot, "scripts/first-paint.json"), "utf8"));
  if (manifest.schemaVersion !== 1) throw new Error(`Unsupported first-paint manifest schemaVersion ${manifest.schemaVersion}`);
  const { reachableCount, findings } = auditFirstPaint(manifest, editorRoot);
  console.log(
    `first-paint: entry '${manifest.entry}' statically reaches ${reachableCount} modules; ` +
      `${manifest.mustBeStaticallyReachable.length} required, ${manifest.mustNotBeStaticallyReachable.length} forbidden.`,
  );
  if (findings.length === 0) {
    console.log("0 findings.");
  } else {
    for (const finding of findings) {
      console.error(`\n${finding.rule}: ${finding.module}\n  ${finding.detail}`);
    }
    console.error(`\n${findings.length} finding(s).`);
    process.exit(1);
  }
}
