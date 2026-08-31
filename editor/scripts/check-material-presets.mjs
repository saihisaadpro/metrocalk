#!/usr/bin/env node
//! Does the editor's material picker offer the finishes the RENDERER actually knows, with the numbers
//! the renderer actually uses?
//!
//! WHY THIS EXISTS. `material_preset()` in `editor-shell/src-tauri/src/main.rs` is the whole material
//! vocabulary: six match arms mapping fourteen accepted spellings to `(base_color, metallic,
//! roughness)`, applied as a per-entity override of the asset's baked material (M11.2 / ADR-041). Any
//! name it does not know returns `None` — the renderer silently draws the object unchanged, and
//! `ai_edit` refuses outright with "unknown material".
//!
//! The editor now states that table a second time (`editor/src/theme/materials.tsx`), because a swatch
//! has to SHADE ITSELF and the shading has to be the surface the viewport will show. That is the exact
//! hazard `<test_and_ci_discipline>` 6 names: one contract, two languages, each validated alone. `tsc`
//! type-checks the TypeScript against an interface, which is a shape and not a vocabulary; `cargo`
//! checks the Rust against nothing at all, because a match arm is self-consistent by construction. The
//! first thing that would ever compare them is a user clicking a swatch in the packaged `.exe` and
//! getting a picture that does not change — the silent half of the failure — or an offered finish the
//! renderer has never heard of.
//!
//! WHAT IT CHECKS, and each one is a way the two can drift apart:
//!
//!   missing-preset  — the renderer knows a finish the picker does not offer (a capability goes dark)
//!   unknown-preset  — the picker offers a finish the renderer does not know (a click does nothing)
//!   alias-drift     — the two disagree about which spellings mean the same finish, so a stored or
//!                     imported value lights up the wrong swatch or none
//!   value-drift     — same name, different base colour / metalness / roughness: the swatch previews a
//!                     material the viewport will not draw, which is the failure a picture is FOR
//!   canonical-drift — the picker writes a spelling that is not the arm's FIRST literal; harmless to
//!                     the renderer, but it makes two stored scenes disagree about the same finish
//!
//! WHAT IT DELIBERATELY DOES NOT CHECK. The preview's shading maths (`materialSwatch`). That is a
//! rendering choice, not a contract with the core, and it is covered by unit assertions on ordering in
//! `theme/materials.test.tsx`. The gate's subject is the three numbers and the names, which is exactly
//! the part the renderer also reads.
//!
//! REACH, IN NUMBERS. The run prints how many arms it parsed and how many presets it compared, because
//! a run that parsed nothing and a run that agreed about everything otherwise print the same verdict
//! (the `<test_and_ci_discipline>` 6b rule). A parse that finds fewer than `MIN_ARMS` arms is a failure,
//! not a pass — that is the mode where the Rust file is refactored and this gate quietly stops looking.

import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const editorRoot = fileURLToPath(new URL("../", import.meta.url));
const repoRoot = fileURLToPath(new URL("../../", import.meta.url));
const RENDER_TABLE = join(repoRoot, "editor-shell", "src-tauri", "src", "main.rs");
const EDITOR_TABLE = join(editorRoot, "src", "theme", "materials.tsx");

/** The renderer ships six finishes. Fewer parsed means the parse broke, not that a finish was removed —
 *  removing one is a deliberate act that also edits this number. */
const MIN_ARMS = 6;
/** Floats printed in two languages are compared with a tolerance, not with `===`. `0.6` and `0.60` are
 *  the same number; `0.6` and `0.65` are a different material. */
const EPSILON = 1e-6;

// ── the renderer's table ──────────────────────────────────────────────────────────────────────────

/** Parse `fn material_preset` — the match arms only, so nothing else in a 28k-line file can reach in.
 *  Each arm is `"a" | "b" => ([r, g, b], m, rg),`. */
export function parseRenderTable(source) {
  const problems = [];
  const start = source.search(/fn\s+material_preset\s*\(/);
  if (start < 0) {
    return { presets: [], problems: ["material_preset() not found in the render table"] };
  }
  // The body is the balanced brace block after the signature; the arms live inside `Some(match name {…})`.
  const open = source.indexOf("{", start);
  let depth = 0;
  let end = -1;
  for (let i = open; i < source.length; i += 1) {
    if (source[i] === "{") depth += 1;
    else if (source[i] === "}") {
      depth -= 1;
      if (depth === 0) {
        end = i;
        break;
      }
    }
  }
  if (end < 0) return { presets: [], problems: ["material_preset() body is unterminated"] };
  const body = source.slice(open, end);

  const presets = [];
  const ARM =
    /((?:"[\w-]+"\s*\|\s*)*"[\w-]+")\s*=>\s*\(\s*\[\s*([-\d.]+)\s*,\s*([-\d.]+)\s*,\s*([-\d.]+)\s*\]\s*,\s*([-\d.]+)\s*,\s*([-\d.]+)\s*\)/g;
  for (const m of body.matchAll(ARM)) {
    const names = [...m[1].matchAll(/"([\w-]+)"/g)].map((n) => n[1]);
    const numbers = [m[2], m[3], m[4], m[5], m[6]].map(Number);
    if (numbers.some((n) => Number.isNaN(n))) {
      problems.push(`arm "${names[0]}" has an unreadable number`);
      continue;
    }
    presets.push({
      id: names[0],
      aliases: names.slice(1),
      baseColor: numbers.slice(0, 3),
      metallic: numbers[3],
      roughness: numbers[4],
    });
  }
  return { presets, problems };
}

// ── the editor's table ────────────────────────────────────────────────────────────────────────────

/** Parse the `MATERIAL_PRESETS` literal. Deliberately a narrow structural read of the object entries
 *  rather than an import: the gate has to run with no editor dependency tree installed, exactly like
 *  every other `check-*.mjs` here, and an import would also execute React. */
export function parseEditorTable(source) {
  const problems = [];
  const declaration = /export\s+const\s+MATERIAL_PRESETS[^=]*=\s*\[/.exec(source);
  if (!declaration) return { presets: [], problems: ["MATERIAL_PRESETS not found in the editor table"] };
  // The array's own bracket, NOT the first one after the declaration: the type annotation
  // `readonly MaterialPreset[]` carries a pair of its own, and indexing to the first `[` lands inside it
  // and then balances to depth zero immediately — which parses cleanly as an EMPTY table and reports
  // every finish as missing. Six false findings, all confident.
  const open = declaration.index + declaration[0].length - 1;
  let depth = 0;
  let end = -1;
  for (let i = open; i < source.length; i += 1) {
    if (source[i] === "[") depth += 1;
    else if (source[i] === "]") {
      depth -= 1;
      if (depth === 0) {
        end = i;
        break;
      }
    }
  }
  if (end < 0) return { presets: [], problems: ["MATERIAL_PRESETS is unterminated"] };
  const body = source.slice(open, end);

  const presets = [];
  // Entries are `{ … }` at the array's top level; split on brace depth so a nested array cannot confuse it.
  let entry = null;
  let braces = 0;
  for (let i = 0; i < body.length; i += 1) {
    const c = body[i];
    if (c === "{") {
      if (braces === 0) entry = i;
      braces += 1;
    } else if (c === "}") {
      braces -= 1;
      if (braces === 0 && entry != null) {
        presets.push(body.slice(entry, i + 1));
        entry = null;
      }
    }
  }

  const parsed = [];
  for (const text of presets) {
    const id = /\bid:\s*"([\w-]+)"/.exec(text)?.[1];
    if (!id) {
      problems.push("a MATERIAL_PRESETS entry has no readable id");
      continue;
    }
    const aliasBlock = /\baliases:\s*\[([^\]]*)\]/.exec(text);
    if (!aliasBlock) {
      problems.push(`preset "${id}" has no readable aliases list`);
      continue;
    }
    const colour = /\bbaseColor:\s*\[\s*([-\d.]+)\s*,\s*([-\d.]+)\s*,\s*([-\d.]+)\s*\]/.exec(text);
    const metallic = /\bmetallic:\s*([-\d.]+)/.exec(text);
    const roughness = /\broughness:\s*([-\d.]+)/.exec(text);
    if (!colour || !metallic || !roughness) {
      problems.push(`preset "${id}" is missing baseColor/metallic/roughness`);
      continue;
    }
    parsed.push({
      id,
      aliases: [...aliasBlock[1].matchAll(/"([\w-]+)"/g)].map((m) => m[1]),
      baseColor: [Number(colour[1]), Number(colour[2]), Number(colour[3])],
      metallic: Number(metallic[1]),
      roughness: Number(roughness[1]),
    });
  }
  // A declaration that yields NO entries is a parse that stopped working, not a picker with nothing in
  // it — and it is the mode this gate shipped with for one run: `indexOf("[")` landed inside the type
  // annotation, balanced immediately, and reported an empty table with total confidence. Reported as
  // unreadable so it can never be mistaken for the (also invalid) state of offering no finishes.
  if (parsed.length === 0 && problems.length === 0) {
    problems.push("MATERIAL_PRESETS parsed as empty — the reader is stale, or the picker offers nothing");
  }
  return { presets: parsed, problems };
}

// ── the comparison ────────────────────────────────────────────────────────────────────────────────

const near = (a, b) => Math.abs(a - b) <= EPSILON;
const sameSet = (a, b) => {
  const x = [...a].sort();
  const y = [...b].sort();
  return x.length === y.length && x.every((v, i) => v === y[i]);
};

export function compare(renderSource, editorSource) {
  const findings = [];
  const { presets: render, problems: rp } = parseRenderTable(renderSource);
  const { presets: editor, problems: ep } = parseEditorTable(editorSource);
  for (const p of [...rp, ...ep]) findings.push({ kind: "unreadable", key: p, message: p });

  if (render.length < MIN_ARMS && rp.length === 0) {
    const message = `parsed only ${render.length} render arms (expected at least ${MIN_ARMS}) — the parse is stale, not the table`;
    findings.push({ kind: "unreadable", key: message, message });
  }

  // Every spelling the renderer accepts, mapped to the arm that owns it — that is what "does this value
  // light up the right swatch" actually depends on.
  const renderById = new Map(render.map((p) => [p.id, p]));
  const editorById = new Map(editor.map((p) => [p.id, p]));

  for (const p of render) {
    if (!editorById.has(p.id)) {
      findings.push({
        kind: "missing-preset",
        key: p.id,
        message: `the renderer knows "${p.id}" and the picker does not offer it`,
      });
    }
  }
  for (const p of editor) {
    const r = renderById.get(p.id);
    if (!r) {
      // Is it an alias of some arm? Then the picker is writing a non-canonical spelling.
      const owner = render.find((arm) => arm.aliases.includes(p.id));
      findings.push(
        owner
          ? {
              kind: "canonical-drift",
              key: p.id,
              message: `the picker writes "${p.id}", which the renderer accepts only as an alias of "${owner.id}"`,
            }
          : {
              kind: "unknown-preset",
              key: p.id,
              message: `the picker offers "${p.id}" and the renderer has no preset for it — the click would do nothing`,
            },
      );
      continue;
    }
    if (!sameSet(r.aliases, p.aliases)) {
      findings.push({
        kind: "alias-drift",
        key: p.id,
        message: `"${p.id}" aliases disagree — renderer [${r.aliases.join(", ")}] vs picker [${p.aliases.join(", ")}]`,
      });
    }
    const drift = [];
    for (let i = 0; i < 3; i += 1) {
      if (!near(r.baseColor[i], p.baseColor[i])) {
        drift.push(`baseColor[${i}] ${r.baseColor[i]} vs ${p.baseColor[i]}`);
      }
    }
    if (!near(r.metallic, p.metallic)) drift.push(`metallic ${r.metallic} vs ${p.metallic}`);
    if (!near(r.roughness, p.roughness)) drift.push(`roughness ${r.roughness} vs ${p.roughness}`);
    if (drift.length > 0) {
      findings.push({
        kind: "value-drift",
        key: p.id,
        message: `"${p.id}" would preview a different surface than it renders — ${drift.join("; ")}`,
      });
    }
  }

  return { findings, renderCount: render.length, editorCount: editor.length };
}

// ── self-test ─────────────────────────────────────────────────────────────────────────────────────

/** Each mutant is a way the two tables have actually drifted apart in similar contracts here, and each
 *  is pinned to text only its own mechanism produces — a self-test that any mutation satisfies is a
 *  self-test that stopped testing (the ADR-137 lesson). */
function selfTest() {
  const RENDER = `
fn material_preset(name: &str) -> Option<([f32; 3], f32, f32)> {
    Some(match name {
        "rusty" | "rust" => ([0.42, 0.22, 0.12], 0.55, 0.65),
        "metal" | "steel" => ([0.56, 0.57, 0.58], 1.0, 0.35),
        "gold" => ([1.0, 0.78, 0.34], 1.0, 0.22),
        _ => return None,
    })
}
`;
  const entry = (id, aliases, colour, metallic, roughness) =>
    `  { id: "${id}", label: "X", hint: "y", aliases: [${aliases.map((a) => `"${a}"`).join(", ")}], baseColor: [${colour.join(", ")}], metallic: ${metallic}, roughness: ${roughness} },`;
  const editorWith = (entries) => `export const MATERIAL_PRESETS: readonly MaterialPreset[] = [\n${entries.join("\n")}\n];\n`;

  const agreed = [
    entry("rusty", ["rust"], [0.42, 0.22, 0.12], 0.55, 0.65),
    entry("metal", ["steel"], [0.56, 0.57, 0.58], 1.0, 0.35),
    entry("gold", [], [1.0, 0.78, 0.34], 1.0, 0.22),
  ];

  const clean = compare(RENDER, editorWith(agreed));
  // MIN_ARMS is 6 in production and the fixture has 3, so the reach guard fires here by construction —
  // filter it out and assert the COMPARISON is otherwise silent.
  const realFindings = (r) => r.findings.filter((f) => !/the parse is stale/.test(f.message));
  assert.deepEqual(realFindings(clean), [], "the agreeing tables must produce no finding");
  assert.equal(clean.renderCount, 3, "the reach number must count the arms it actually parsed");
  assert.equal(clean.editorCount, 3, "the reach number must count the presets it actually parsed");

  const mutants = [
    {
      name: "a finish the picker never offers",
      editor: editorWith(agreed.slice(0, 2)),
      expect: /renderer knows "gold" and the picker does not offer it/,
    },
    {
      name: "a finish the renderer has never heard of",
      editor: editorWith([...agreed, entry("velvet", [], [0.2, 0.1, 0.3], 0.0, 0.9)]),
      expect: /picker offers "velvet".*click would do nothing/,
    },
    {
      name: "the picker writing an alias as if it were the name",
      editor: editorWith([...agreed.slice(1), entry("rust", ["rusty"], [0.42, 0.22, 0.12], 0.55, 0.65)]),
      expect: /picker writes "rust".*alias of "rusty"/,
    },
    {
      name: "a spelling the picker would not recognise on a stored scene",
      editor: editorWith([agreed[0], entry("metal", ["iron"], [0.56, 0.57, 0.58], 1.0, 0.35), agreed[2]]),
      expect: /"metal" aliases disagree/,
    },
    {
      name: "a swatch previewing a colour the viewport will not draw",
      editor: editorWith([agreed[0], agreed[1], entry("gold", [], [1.0, 0.5, 0.34], 1.0, 0.22)]),
      expect: /"gold" would preview a different surface.*baseColor\[1\] 0\.78 vs 0\.5/,
    },
    {
      name: "a swatch previewing a roughness the viewport will not draw",
      editor: editorWith([agreed[0], agreed[1], entry("gold", [], [1.0, 0.78, 0.34], 1.0, 0.5)]),
      expect: /"gold" would preview a different surface.*roughness 0\.22 vs 0\.5/,
    },
    {
      name: "a renderer refactor this parser can no longer read",
      render: "fn something_else() {}\n",
      editor: editorWith(agreed),
      expect: /material_preset\(\) not found/,
    },
    {
      name: "a picker table this parser can no longer read",
      editor: "export const SOMETHING_ELSE = [];\n",
      expect: /MATERIAL_PRESETS not found/,
    },
    {
      name: "a picker table that parses as empty (the reader's own first bug)",
      editor: "export const MATERIAL_PRESETS: readonly MaterialPreset[] = [];\n",
      expect: /parsed as empty/,
    },
  ];

  for (const mutant of mutants) {
    const result = compare(mutant.render ?? RENDER, mutant.editor);
    const hit = result.findings.some((f) => mutant.expect.test(f.message));
    assert.ok(hit, `self-test mutant not caught (${mutant.name}): ${JSON.stringify(result.findings)}`);
  }

  // The reach guard itself: a render table with too few arms must FAIL, or a refactor that leaves one
  // arm behind reads as agreement.
  const thin = compare(
    'fn material_preset(name: &str) -> Option<([f32; 3], f32, f32)> {\n    Some(match name {\n        "gold" => ([1.0, 0.78, 0.34], 1.0, 0.22),\n        _ => return None,\n    })\n}\n',
    editorWith([agreed[2]]),
  );
  assert.ok(
    thin.findings.some((f) => /parsed only 1 render arms/.test(f.message)),
    "the reach guard must fire when the parse finds fewer arms than the renderer ships",
  );

  // And the real files must round-trip through a temp copy unchanged, so the self-test exercises the
  // same file reads the run does.
  const dir = mkdtempSync(join(tmpdir(), "mtk-material-presets-"));
  try {
    const r = join(dir, "main.rs");
    const e = join(dir, "materials.tsx");
    writeFileSync(r, readFileSync(RENDER_TABLE, "utf8"));
    writeFileSync(e, readFileSync(EDITOR_TABLE, "utf8"));
    const live = compare(readFileSync(r, "utf8"), readFileSync(e, "utf8"));
    assert.deepEqual(live.findings, [], "the checked-in tables must agree");
    assert.ok(live.renderCount >= MIN_ARMS, "the checked-in render table must parse");
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }

  console.log(`check-material-presets self-test: ${mutants.length + 3} cases passed`);
}

// ── run ───────────────────────────────────────────────────────────────────────────────────────────

function main() {
  if (process.argv.includes("--self-test")) {
    selfTest();
    return;
  }
  const result = compare(readFileSync(RENDER_TABLE, "utf8"), readFileSync(EDITOR_TABLE, "utf8"));
  console.log(
    `check-material-presets: compared ${result.editorCount} picker preset(s) against ${result.renderCount} renderer arm(s)`,
  );
  if (result.findings.length === 0) {
    console.log("the material vocabulary agrees across Rust and TypeScript");
    return;
  }
  for (const f of result.findings) console.error(`  [${f.kind}] ${f.message}`);
  console.error(`\n${result.findings.length} material-vocabulary disagreement(s).`);
  process.exitCode = 1;
}

main();
