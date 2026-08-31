#!/usr/bin/env node
//! Does the Inspector's typed-field table name components and fields the CORE has actually
//! registered — and does it read the semantics the core publishes about them?
//!
//! WHY THIS EXISTS, IN ONE GREP. `core/src/stdlib.rs` registers 25 standard components. Four of the
//! five entries in `editor/src/schema/registry.ts`'s `componentSchemas` — `Material`, `Provides`,
//! `Targeting`, `Socket` — are **not among them**, and never have been: they are the dev `MockCore`'s
//! own invention (`transport/session.ts`, `buildWorld`). `componentSchemas` exists for exactly one
//! purpose, to carry the `format` that makes a JSON-Forms tester fire, so the consequence is not
//! cosmetic:
//!
//!   * `ColorControl` is `rankWith(10, and(isStringControl, hasFormat("color")))` and the ONLY
//!     `format: "color"` in the repository is `Material.color`. Against the real core it can never
//!     fire — and the core has no colour field for it to fire on either: colour is modelled as
//!     separate `r`/`g`/`b` Numbers on `Light` and `UiStyle`, so a user editing a light's colour gets
//!     three unlabelled number boxes;
//!   * `EntityRefControl` is keyed on `Targeting.target`. The core's real entity references are
//!     `Joint.bodyA` and `Joint.bodyB` (`stdlib.rs`, `field_fmt(.., Some("entity-ref"))`), which the
//!     table does not mention — so the bind-target picker, an M3.1 north-star control, degrades to a
//!     plain text box;
//!   * `format: "asset"` is declared by the core on SIX fields (`Sprite.texture`, `MeshRenderer.mesh`,
//!     `MeshRenderer.material`, `AudioSource.clip`, `Animator.controller`, `Script.source`) and the
//!     editor has no renderer for it at all — an asset reference renders as a box you are invited to
//!     type a content hash into;
//!   * `ui_hint` carries the core's closed vocabularies (`"enum: directional|point|spot"`) and its
//!     units (`"companion move speed, metres per second"`). Nothing in the editor reads `ui_hint`.
//!
//! **AND THE ONE THIS GATE GOT BACKWARDS — read this before trusting a green run (ADR-172).** The
//! paragraph here used to say: *"Even `Transform` is wrong in the same way: the table says
//! `x`/`y`/`z`, the core registers `px`/`py`/`pz` plus rotation and scale. That one is survivable."*
//! It was survivable, and it was also **the only entry where the TABLE was right and the CORE was
//! wrong**. Every writer in the shell commits `Transform{x, y, z}` + a quaternion + a uniform
//! `scale`; `capscene::local_transform` reads the same eight names; the dev `MockCore` seeds them;
//! `.mtk` files on disk contain them. `px`/`py`/`pz`/`rx`..`sz` were declared in `stdlib.rs` in M1.3
//! and **written by nothing, ever**. Repairing the table to match the registry therefore made the
//! editor agree with the half that was fiction, and this gate then held it there: the curated
//! `Transform` entry could not fire on any entity in this product, ADR-155's vector rows were
//! declared over the same phantom fields and could not fire either, and both were photographed by a
//! `shots` scene that seeded `px/py/pz` **to prove it was capturing the real vocabulary**.
//!
//! The lesson is not about Transform. **This gate compares two statements of a contract; it cannot
//! tell you that BOTH are wrong.** What settles which one is right is the code that WRITES and READS
//! the document, and no static check here can see it — so the comparison to the writer lives where
//! the writer does: `stdlib_transform_matches_the_stored_transform` (`editor-shell/tests/
//! transform_commit.rs`) commits a pose through the only writer and compares the document's own key
//! set to `stdlib.rs`'s declaration. When you next find this gate disagreeing with the editor, ask
//! which of the two the ENGINE agrees with before repairing either.
//!
//! NOT ONE GATE COULD SEE IT. `tsc` type-checks the table against `JsonSchema7`, which is a shape, not
//! a vocabulary. vitest seeds `MockCore`'s entities, so every test agrees with the mock by
//! construction — the C6 failure this repository has a name for (green against the mock, wrong against
//! `/core`), and `src/schema/registry.ts`'s own header claims the opposite in prose: "so the inspector
//! renders the REAL /core vocabulary". `shots` has never photographed a populated Inspector at all.
//!
//! THE GENERAL RULE IT BELONGS TO is the one ADR-134 stated for class names, one level up: a component
//! vocabulary is ONE contract stated TWICE, once as a Rust builder table and once as a TypeScript
//! object literal, each validated alone and neither against the other — with the running `.exe` as the
//! first thing that ever compares them. `rule_registry` already ships the real table over IPC and two
//! panels already consume it; the Inspector, the panel the table was written for, does not.
//!
//! WHERE THE LINE IS DRAWN, and why it is principled rather than convenient. The gate does NOT demand
//! that the editor curate all 25 components. `buildEntitySchema` infers a schema from the projected
//! value and a Number, a Boolean and a String infer correctly. The gate covers exactly the fields
//! where **inference produces the wrong control**:
//!
//!   phantom-component — the table names a component the core has never registered
//!   phantom-field     — the table names a field that component does not have
//!   type-drift        — the table's JSON-Schema type disagrees with the core's FieldType
//!   unrouted-format   — a core field declares a `format` the table does not carry, so no tester fires
//!   unread-enum       — a core `ui_hint` declares `enum: a|b|c` and the table has no matching `enum`
//!
//! The last two are the direction that matters. The first three catch a table that says something
//! false; only these catch a table that stays SILENT about something true, which is the shape every
//! finding above actually has.
//!
//! THE PARSER REFUSES TO BE VACUOUS. A source reader that quietly returns nothing turns a gate into a
//! file: `mesh_frame_bench.rs` is this repository's standing lesson, and ADR-134's own self-test found
//! a case that had silently stopped testing what it named. So the Rust reader asserts its own catch —
//! fewer than `MIN_COMPONENTS` blocks, or any block with no fields, is a hard failure that says the
//! table moved, not a green run. That guard is itself a self-test case.
//!
//! THE RESIDUE IS DECLARED, NOT INFERRED — the same shape as `class-hooks.json`, `first-paint.json`
//! and `glyph-coverage.json`. The dev mock's capability vocabulary (`Provides`/`Socket`) is a real
//! divergence owned by the M3.1/M5 capability lane, not by this gate; it is written down in
//! `scripts/registry-vocab.json` with its reason and its closing gate. Anything not declared fails.
//!
//! Node built-ins only, so it runs in CI before the install, beside the constitution, palette,
//! first-paint, glyph and class-hook checks.
//! Run: `node scripts/check-registry-vocab.mjs` · self-test: `--self-test`. (ADR-136.)

import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const editorRoot = fileURLToPath(new URL("../", import.meta.url));
const repoRoot = fileURLToPath(new URL("../../", import.meta.url));
const MANIFEST = join(editorRoot, "scripts", "registry-vocab.json");
const CORE_TABLE = join(repoRoot, "core", "src", "stdlib.rs");
const EDITOR_TABLE = join(editorRoot, "src", "schema", "registry.ts");

/** The table has had 25 entries since M19; a reader that finds fewer than this has stopped reading it
 *  rather than found a smaller table, and must say so instead of passing. Deliberately well below the
 *  real count so that legitimately retiring a component is not a false alarm — the bar is "the parser
 *  still works", not "the table never changes". */
const MIN_COMPONENTS = 15;

/** `FieldType` (core/src/registry.rs) → the JSON-Schema `type` the editor must declare for it. */
const FIELD_TYPE = { Number: "number", Integer: "integer", Boolean: "boolean", Str: "string", String: "string" };

// ── the core's table, read from the Rust source ───────────────────────────────────────────────────

/**
 * Parse `standard_components()`'s builder table.
 *
 * It is parseable because it is deliberately flat: the function carries
 * `#[allow(clippy::too_many_lines)] // a flat data table of component definitions, not branching
 * logic`. Whitespace is collapsed per block first, because rustfmt wraps a long `.ui_hint(...)` call
 * across three lines and a line-oriented reader would drop exactly the longest vocabularies — which
 * are the ones worth reading.
 *
 * @returns {{ components: Map<string, {category: string|null, fields: Map<string, {ty: string, required: boolean, format: string|null}>, hints: Map<string,string>}>, problems: string[] }}
 */
export function parseCoreTable(src) {
  const problems = [];
  const components = new Map();

  // `let asset = Some("asset");` — a format named once and reused, so the reader must resolve the
  // binding rather than only understanding the literal spelling.
  const aliases = new Map();
  for (const m of src.matchAll(/\blet\s+(\w+)\s*(?::[^=]+)?=\s*Some\(\s*"([\w-]+)"\s*\)\s*;/g)) {
    aliases.set(m[1], m[2]);
  }

  const blocks = src.split(/ComponentMeta::builder\(/).slice(1);
  for (const raw of blocks) {
    const end = raw.indexOf(".build()");
    const block = (end < 0 ? raw : raw.slice(0, end)).replace(/\s+/g, " ");
    const name = block.match(/^\s*"([\w]+)"\s*\)/)?.[1];
    if (!name) {
      problems.push("a ComponentMeta::builder(..) call whose name is not a string literal");
      continue;
    }
    const fields = new Map();
    const hints = new Map();
    // `FieldType::String` and a trailing comma are both legal spellings rustfmt can produce, and both
    // were unmatched by the first draft. An adversarial review found the consequence: a `field_fmt`
    // spelled either way parses as NOTHING, so the format it declares is invisible and the gate goes
    // green about the one direction it exists for.
    const TY = "(?:FieldType::)?(\\w+)";
    for (const m of block.matchAll(new RegExp(`\\.field\\(\\s*"([\\w]+)"\\s*,\\s*${TY}\\s*,\\s*(true|false)\\s*,?\\s*\\)`, "g"))) {
      fields.set(m[1], { ty: FIELD_TYPE[m[2]] ?? m[2], required: m[3] === "true", format: null });
    }
    for (const m of block.matchAll(
      new RegExp(
        `\\.field_fmt\\(\\s*"([\\w]+)"\\s*,\\s*${TY}\\s*,\\s*(true|false)\\s*,\\s*(?:Some\\(\\s*"([\\w-]+)"\\s*\\)|(\\w+))\\s*,?\\s*\\)`,
        "g",
      ),
    )) {
      const format = m[4] ?? aliases.get(m[5]) ?? null;
      if (!format) problems.push(`${name}.${m[1]}: field_fmt's format is neither a literal nor a resolvable binding`);
      fields.set(m[1], { ty: FIELD_TYPE[m[2]] ?? m[2], required: m[3] === "true", format });
    }
    for (const m of block.matchAll(/\.ui_hint\(\s*"([\w]+)"\s*,\s*"((?:[^"\\]|\\.)*)"\s*,?\s*\)/g)) {
      hints.set(m[1], m[2].replace(/\\"/g, '"'));
    }
    // THE READER COUNTS WHAT IT SHOULD HAVE READ. "no fields at all" only catches a component that
    // parsed to nothing; a component that loses ONE field to an unrecognised spelling looks healthy,
    // and if the lost one carried the `format` the gate is silent about exactly what it is for. So the
    // number of `.field(`/`.field_fmt(` CALLS is compared against the number understood, and a
    // shortfall is a hard failure — the reader is required to know when it did not understand.
    const calls = (block.match(/\.field(?:_fmt)?\(/g) ?? []).length;
    if (fields.size < calls) {
      problems.push(
        `${name}: ${calls} field call(s) in the source, ${fields.size} understood — the reader met a ` +
          "spelling it does not know, and a format or type it cannot see is one it cannot check",
      );
    }
    if (fields.size === 0) problems.push(`${name}: parsed with NO fields — the reader lost the table's shape`);
    components.set(name, { category: block.match(/\.category\(\s*"([\w]+)"\s*\)/)?.[1] ?? null, fields, hints });
  }

  if (components.size < MIN_COMPONENTS) {
    problems.push(
      `read only ${components.size} component(s) from the core table (expected at least ${MIN_COMPONENTS}) — ` +
        "the reader has stopped reading it, and a gate that reads nothing agrees with everything",
    );
  }
  return { components, problems };
}

/** `"enum: directional|point|spot"` → `["directional","point","spot"]`; anything else → null. A hint
 *  is prose by default and only this one prefix is a machine-readable vocabulary.
 *
 *  ONE variant is still a closed vocabulary. The first draft required two, on the reasoning that a
 *  single value is not a choice — but the question this gate asks is "may the user type something
 *  else", and the answer for `enum: only` is no. Requiring two made a genuinely closed one-value
 *  vocabulary silent, which is the gate's own failure mode rather than a judgement call. */
export function hintEnum(hint) {
  const m = /^\s*enum:\s*(.+?)\s*$/.exec(hint ?? "");
  if (!m) return null;
  const variants = m[1].split("|").map((s) => s.trim()).filter(Boolean);
  return variants.length >= 1 ? variants : null;
}

/** Two vocabularies are the same set. Compared ELEMENT BY ELEMENT, never as a joined string: `["a,b"]`
 *  and `["a","b"]` join to the same text, so a curated entry with a comma where the core has a pipe
 *  read as covered — and incremented the `covered` count while doing it. */
const sameSet = (a, b) => {
  if (!Array.isArray(a) || !Array.isArray(b) || a.length !== b.length) return false;
  const x = [...a].map(String).sort();
  const y = [...b].map(String).sort();
  return x.every((v, i) => v === y[i]);
};

// ── the editor's table, read from the TypeScript source ───────────────────────────────────────────

/** Blank a span in place so every later offset stays exactly where it was — the same device the class
 *  -hooks extractor uses, and for the same reason: a reader that reports the wrong position sends
 *  someone to the wrong line. */
const blank = (s, a, b) => s.slice(a, b).replace(/[^\n]/g, " ");

/** Remove `//` and slash-star comments, preserving length.
 *
 *  IT IS NOT OPTIONAL, AND THE PROOF IS THIS FILE'S FIRST RUN. `registry.ts` documents the colour
 *  field with `` // `format: color` → the custom ColorControl renderer ``. The brace matcher below
 *  tracks string literals so a `{` inside one cannot unbalance it, and a BACKTICK is a string opener —
 *  so that comment opened a template literal that never closed, the matcher walked to EOF, and the
 *  gate reported "the object literal is unterminated". It still went red, which is why this was nearly
 *  missed: it failed for the wrong reason and reported the wrong findings, silently dropping every
 *  phantom-component result the table actually contains. Exactly ADR-134's apostrophe, in the gate
 *  written to learn from it, and exactly its lesson — a red run is not the same as the right finding.
 */
export function stripComments(src) {
  let out = src;
  for (const m of [...src.matchAll(/\/\*[\s\S]*?\*\/|\/\/[^\n]*/g)]) {
    out = out.slice(0, m.index) + blank(out, m.index, m.index + m[0].length) + out.slice(m.index + m[0].length);
  }
  return out;
}

/** Extract `componentSchemas`' object literal and evaluate it. Pure data — string/number/array/object
 *  literals, no identifiers — so it needs no TypeScript, which is what keeps this gate runnable before
 *  the install. A literal that does not evaluate to an object is a hard failure, not an empty table:
 *  an empty table would silently agree with the core about everything. */
export function parseEditorTable(rawSrc) {
  const problems = [];
  const src = stripComments(rawSrc);
  const start = src.search(/export\s+const\s+componentSchemas\s*(?::[^=]+)?=\s*\{/);
  if (start < 0) return { schemas: {}, problems: ["`componentSchemas` is not declared in registry.ts"] };
  let i = src.indexOf("{", start);
  let depth = 0;
  let end = -1;
  let quote = null;
  for (let j = i; j < src.length; j++) {
    const c = src[j];
    if (quote) {
      if (c === "\\") j++;
      else if (c === quote) quote = null;
      continue;
    }
    if (c === '"' || c === "'" || c === "`") quote = c;
    else if (c === "{") depth++;
    else if (c === "}" && --depth === 0) { end = j + 1; break; }
  }
  if (end < 0) return { schemas: {}, problems: ["`componentSchemas`' object literal is unterminated"] };
  let schemas;
  try {
    // eslint-disable-next-line no-new-func -- repo-local data literal; see the note above
    schemas = new Function(`return (${src.slice(i, end)});`)();
  } catch (e) {
    return { schemas: {}, problems: [`\`componentSchemas\` did not evaluate: ${String(e)}`] };
  }
  if (!schemas || typeof schemas !== "object") problems.push("`componentSchemas` did not evaluate to an object");
  return { schemas: schemas ?? {}, problems };
}

// ── the comparison ────────────────────────────────────────────────────────────────────────────────

/**
 * @returns {{ findings: {kind: string, key: string, message: string}[], stale: string[], covered: number }}
 */
export function check(coreSrc, editorSrc, manifest = {}) {
  const findings = [];
  const { components, problems } = parseCoreTable(coreSrc);
  const { schemas, problems: eProblems } = parseEditorTable(editorSrc);
  for (const p of [...problems, ...eProblems]) findings.push({ kind: "unreadable", key: p, message: p });

  // THE BUCKETS ARE NAMED, NOT DISCOVERED. Iterating `Object.entries(manifest)` reads the file's `//`
  // prose note as a bucket and its STRING value as a map, so `Object.keys` returns one index per
  // character: this gate's first run printed 444 "stale declaration" notes, one per letter of its own
  // documentation, burying the two real ones. A manifest key that is not a bucket is a mistake worth
  // saying so about, rather than one worth silently walking.
  const BUCKETS = ["phantom", "typeDrift", "unrouted", "unreadEnum"];
  for (const k of Object.keys(manifest)) {
    if (k.startsWith("//") || BUCKETS.includes(k)) continue;
    findings.push({ kind: "unreadable", key: k, message: `registry-vocab.json has an unknown bucket \`${k}\`` });
  }
  // A DECLARATION IS A SENTENCE, NOT A KEY. `hasOwnProperty` alone means `{ "Ghost": null }` suppresses
  // a finding while saying nothing about why, which is the suppression mechanism ADR-124 refused to
  // build wearing the manifest's clothes. An entry must be an object carrying a `reason`.
  const allow = (bucket, key) => {
    const entry = (manifest[bucket] ?? {})[key];
    if (entry === undefined) return false;
    if (!entry || typeof entry !== "object" || typeof entry.reason !== "string" || entry.reason.trim() === "") {
      findings.push({
        kind: "unreadable",
        key: `${bucket}:${key}`,
        message: `registry-vocab.json declares \`${bucket}.${key}\` with no \`reason\` — a suppression that does not say why is not a declaration`,
      });
      return false;
    }
    return true;
  };
  const used = new Set();
  const take = (bucket, key) => {
    if (!allow(bucket, key)) return false;
    used.add(`${bucket}:${key}`);
    return true;
  };

  // ── direction 1: everything the editor CLAIMS must exist ───────────────────────────────────────
  for (const [component, schema] of Object.entries(schemas)) {
    const core = components.get(component);
    if (!core) {
      if (!take("phantom", component)) {
        findings.push({
          kind: "phantom-component",
          key: component,
          message:
            `componentSchemas names \`${component}\`, which core/src/stdlib.rs has never registered — ` +
            "every typed control keyed on it is unreachable from the authoritative core",
        });
      }
      continue;
    }
    for (const [field, spec] of Object.entries(schema?.properties ?? {})) {
      const key = `${component}.${field}`;
      const cf = core.fields.get(field);
      if (!cf) {
        if (!take("phantom", key)) {
          findings.push({
            kind: "phantom-field",
            key,
            message: `componentSchemas names \`${key}\`; the core's \`${component}\` has [${[...core.fields.keys()].join(", ")}]`,
          });
        }
        continue;
      }
      const declared = spec?.type;
      // A CURATED FIELD WITHOUT A TYPE MATCHES NO TESTER. Every tester in the inspector's set is keyed
      // on a scalar type (`isStringControl`, `isNumberControl`, …), so a `{ title, enum }` with no
      // `type` resolves to nothing and JSON Forms paints "No applicable renderer found." into the
      // panel. `JsonSchema7.type` is optional, so TypeScript permits it; the `FieldSchema` alias now
      // narrows it too, and this is the belt to that brace — the shape is unrenderable, not merely
      // undeclared, and it is the only hole an adversarial pass found in the renderer set.
      if (!declared) {
        findings.push({
          kind: "untyped-field",
          key,
          message:
            `componentSchemas declares \`${key}\` with no \`type\`; every inspector tester is keyed on a ` +
            "scalar type, so this field would render as JSON Forms' \"No applicable renderer found.\"",
        });
        continue;
      }
      // integer is a number; the reverse is not true, and only the lossy direction is a finding.
      const compatible = declared === cf.ty || (declared === "number" && cf.ty === "integer");
      if (!compatible && !take("typeDrift", key)) {
        findings.push({
          kind: "type-drift",
          key,
          message: `componentSchemas types \`${key}\` as \`${declared}\`; the core registers it as \`${cf.ty}\``,
        });
      }
      // AND A FORMAT THE CORE DOES NOT DECLARE. Direction 2 catches a format the core publishes and the
      // table ignores; this is its dual, and it is the shipped defect stated directly rather than
      // reached through `phantom-component`: `Material.color` was the only `format: "color"` in the
      // repository and the core has no such field, so the control keyed on it could never fire. With
      // the phantom component deleted but the format left on a real field, nothing else here objects.
      const fmt = spec?.format;
      if (fmt && fmt !== cf.format && !take("unrouted", key)) {
        findings.push({
          kind: "invented-format",
          key,
          message:
            `componentSchemas gives \`${key}\` format \`${fmt}\`; the core declares ` +
            (cf.format ? `\`${cf.format}\`` : "no format for it") +
            " — a tester keyed on it fires only against data the core cannot send",
        });
      }
    }
  }

  // ── direction 2: everything the core PUBLISHES that inference cannot recover ────────────────────
  // This is the direction that matters. A table which merely stays silent is wrong in exactly the way
  // a one-directional gate cannot see, and every real finding here has that shape.
  let covered = 0;
  for (const [component, core] of components) {
    const props = schemas[component]?.properties ?? {};
    for (const [field, cf] of core.fields) {
      const key = `${component}.${field}`;
      if (cf.format) {
        if (props[field]?.format === cf.format) covered++;
        else if (!take("unrouted", key)) {
          findings.push({
            kind: "unrouted-format",
            key,
            message:
              `the core declares \`${key}\` with format \`${cf.format}\`; componentSchemas ` +
              (props[field] ? `types it as \`${props[field].type}\` with no format` : "does not mention it") +
              ` — no tester can fire, so it renders as a plain ${cf.ty} box`,
          });
        }
      }
      const variants = hintEnum(core.hints.get(field));
      if (variants) {
        const declared = props[field]?.enum;
        if (sameSet(declared, variants)) covered++;
        else if (!take("unreadEnum", key)) {
          findings.push({
            kind: "unread-enum",
            key,
            message:
              `the core's ui_hint declares \`${key}\` as a closed vocabulary [${variants.join(", ")}]; ` +
              (declared ? `componentSchemas declares [${declared.join(", ")}]` : "componentSchemas does not") +
              " — a free-text box invites a value the core will reject",
          });
        }
      }
    }
  }

  const declared = BUCKETS.flatMap((b) => Object.keys(manifest[b] ?? {}).map((k) => `${b}:${k}`));
  return { findings, stale: declared.filter((d) => !used.has(d)), covered };
}

export function report(coreSrc, editorSrc, manifest, { quiet = false } = {}) {
  const { findings, stale, covered } = check(coreSrc, editorSrc, manifest);
  if (!quiet) {
    for (const f of findings) console.error(`FAIL  [${f.kind}] ${f.message}`);
    for (const s of stale) console.warn(`note  the declared exception \`${s}\` no longer matches anything — delete it`);
    const { components } = parseCoreTable(coreSrc);
    const { schemas } = parseEditorTable(editorSrc);
    console.log(
      findings.length
        ? `\nregistry vocab: ${findings.length} finding(s) — the Inspector's typed-field table and the core's registry disagree.`
        : `registry vocab: ${Object.keys(schemas).length} curated component(s) against ${components.size} registered by the core — ` +
          `every name exists, every type agrees, and all ${covered} field(s) the core marks with a format or a closed ` +
          `vocabulary are routed. ${["phantom", "typeDrift", "unrouted", "unreadEnum"].reduce((n, b) => n + Object.keys(manifest[b] ?? {}).length, 0)} declared exception(s).`,
    );
  }
  return { findings, stale, covered };
}

// ── self-test ─────────────────────────────────────────────────────────────────────────────────────

function selfTest() {
  const root = mkdtempSync(join(tmpdir(), "mtk-regvocab-"));
  mkdirSync(root, { recursive: true });
  const cases = [];

  /** A miniature but SHAPE-FAITHFUL core table: the `let asset = Some("asset")` binding, a wrapped
   *  `.ui_hint(..)`, and enough entries to clear MIN_COMPONENTS so the vacuity guard is not what a
   *  case is accidentally measuring. */
  const coreOf = (extra = "") => `
pub fn standard_components() -> Vec<ComponentMeta> {
    use FieldType::{Boolean, Integer, Number, String as Str};
    let asset = Some("asset");
    vec![
        ComponentMeta::builder("Transform").category("Props")
            .field("px", Number, true).field("py", Number, true).build(),
        ComponentMeta::builder("Health").category("Gameplay")
            .field("hp", Integer, true).field("maxHp", Integer, true)
            .ui_hint("hp", "slider 0..maxHp").build(),
        ComponentMeta::builder("MeshRenderer").category("Props")
            .field_fmt("mesh", Str, true, asset).field("castShadows", Boolean, false).build(),
        ComponentMeta::builder("Joint").category("Gameplay")
            .field_fmt("bodyA", Str, true, Some("entity-ref"))
            .ui_hint(
                "kind",
                "enum: revolute|fixed|spherical",
            )
            .field("kind", Str, true).build(),
${Array.from({ length: 12 }, (_, i) => `        ComponentMeta::builder("Filler${i}").field("v", Number, false).build(),`).join("\n")}
${extra}
    ]
}
`;
  const editorOf = (obj) => `import type { JsonSchema7 } from "@jsonforms/core";
export const componentSchemas: Record<string, JsonSchema7> = ${obj};
export function buildEntitySchema() {}
`;

  /** Assert on KINDS and KEYS, never on a bare count. ADR-134's own self-test had three cases that
   *  compared `findings.length` only, so unanchoring the rule under test still printed `ok` while the
   *  finding had silently become a different one arriving in the same quantity. */
  const run = (label, want, core, editor, manifest = {}) => {
    const { findings, stale } = check(core, editor, manifest);
    const got = {
      count: findings.length,
      keys: findings.map((f) => f.key).sort(),
      kinds: [...new Set(findings.map((f) => f.kind))].sort(),
      stale: stale.length,
    };
    const bad = [];
    if (want.count !== undefined && got.count !== want.count) bad.push(`count ${got.count} != ${want.count}`);
    if (want.keys && String(got.keys) !== String([...want.keys].sort())) bad.push(`keys [${got.keys}] != [${[...want.keys].sort()}]`);
    if (want.kinds && String(got.kinds) !== String([...want.kinds].sort())) bad.push(`kinds [${got.kinds}] != [${[...want.kinds].sort()}]`);
    if (want.stale !== undefined && got.stale !== want.stale) bad.push(`stale ${got.stale} != ${want.stale}`);
    const ok = bad.length === 0;
    cases.push({ ok, label });
    console.log(`${ok ? "ok  " : "FAIL"}  ${label}${ok ? "" : " — " + bad.join("; ")}`);
    if (!ok) for (const f of findings) console.log(`        → [${f.kind}] ${f.message}`);
  };

  const TRUTHFUL = `{
    Transform: { type: "object", properties: { px: { type: "number" }, py: { type: "number" } } },
    Health: { type: "object", properties: { hp: { type: "integer", enum: undefined } } },
    MeshRenderer: { type: "object", properties: { mesh: { type: "string", format: "asset" } } },
    Joint: { type: "object", properties: {
      bodyA: { type: "string", format: "entity-ref" },
      kind: { type: "string", enum: ["revolute", "fixed", "spherical"] } } },
  }`;
  // `Health.hp`'s hint is "slider 0..maxHp", which is NOT an `enum:` and must not be demanded as one.
  run("a table that matches the core is silent", { count: 0 }, coreOf(), editorOf(TRUTHFUL));

  // THE SHIPPED DEFECT, both halves. `Material` is a component the core has never had; `Material.color`
  // is the only `format: "color"` in the repository, so the ColorControl's tester can never fire.
  run(
    "a component the core has never registered fires",
    { count: 1, kinds: ["phantom-component"], keys: ["Material"] },
    coreOf(),
    editorOf(`{
      Transform: { type: "object", properties: { px: { type: "number" }, py: { type: "number" } } },
      Health: { type: "object", properties: { hp: { type: "integer" } } },
      Material: { type: "object", properties: { color: { type: "string", format: "color" } } },
      MeshRenderer: { type: "object", properties: { mesh: { type: "string", format: "asset" } } },
      Joint: { type: "object", properties: {
        bodyA: { type: "string", format: "entity-ref" },
        kind: { type: "string", enum: ["revolute", "fixed", "spherical"] } } },
    }`),
  );

  // The Transform defect exactly as HEAD shipped it: the right component, the wrong field names.
  run(
    "a field the component does not have fires, per field",
    { count: 3, kinds: ["phantom-field"], keys: ["Transform.x", "Transform.y", "Transform.z"] },
    coreOf(),
    editorOf(`{
      Transform: { type: "object", properties: { x: { type: "number" }, y: { type: "number" }, z: { type: "number" } } },
      Health: { type: "object", properties: { hp: { type: "integer" } } },
      MeshRenderer: { type: "object", properties: { mesh: { type: "string", format: "asset" } } },
      Joint: { type: "object", properties: {
        bodyA: { type: "string", format: "entity-ref" },
        kind: { type: "string", enum: ["revolute", "fixed", "spherical"] } } },
    }`),
  );

  run(
    "a declared exception does not block",
    { count: 0, stale: 0 },
    coreOf(),
    editorOf(`{
      Transform: { type: "object", properties: { px: { type: "number" }, py: { type: "number" } } },
      Health: { type: "object", properties: { hp: { type: "integer" } } },
      Provides: { type: "object", properties: { capability: { type: "string" } } },
      MeshRenderer: { type: "object", properties: { mesh: { type: "string", format: "asset" } } },
      Joint: { type: "object", properties: {
        bodyA: { type: "string", format: "entity-ref" },
        kind: { type: "string", enum: ["revolute", "fixed", "spherical"] } } },
    }`),
    { phantom: { Provides: { reason: "the dev mock's capability vocabulary" } } },
  );

  run(
    "a declaration that matches nothing is counted, not blocking",
    { count: 0, stale: 1 },
    coreOf(),
    editorOf(TRUTHFUL),
    { phantom: { Gone: { reason: "retired last milestone" } } },
  );

  // ── the direction that matters: the table stays SILENT about something true ────────────────────
  run(
    "a format the core declares and the table does not carry fires",
    { count: 1, kinds: ["unrouted-format"], keys: ["MeshRenderer.mesh"] },
    coreOf(),
    editorOf(`{
      Transform: { type: "object", properties: { px: { type: "number" } } },
      MeshRenderer: { type: "object", properties: { mesh: { type: "string" } } },
      Joint: { type: "object", properties: {
        bodyA: { type: "string", format: "entity-ref" },
        kind: { type: "string", enum: ["revolute", "fixed", "spherical"] } } },
    }`),
  );
  // ...and the same field simply left out. Omission and mis-typing are the same defect to a user and
  // must be the same finding, or the gate teaches you to delete the row instead of fixing it.
  run(
    "a format on a field the table omits entirely fires the same way",
    { count: 1, kinds: ["unrouted-format"], keys: ["MeshRenderer.mesh"] },
    coreOf(),
    editorOf(`{
      Transform: { type: "object", properties: { px: { type: "number" } } },
      Joint: { type: "object", properties: {
        bodyA: { type: "string", format: "entity-ref" },
        kind: { type: "string", enum: ["revolute", "fixed", "spherical"] } } },
    }`),
  );
  // THE `let asset = Some("asset")` BINDING. Six of the core's eight format fields are spelled through
  // it; a reader that only understood the literal would report those six as having NO format and go
  // green on the very fields this gate exists for. Pinned by key: with the alias resolution deleted,
  // `MeshRenderer.mesh` stops being a finding here and the count drops to 0.
  // Both directions fire on the same field, which is the point: the table names a format the core does
  // not have AND stays silent about the one it does.
  run(
    "a format named through a `let` binding is still read",
    { count: 2, kinds: ["unrouted-format", "invented-format"], keys: ["MeshRenderer.mesh", "MeshRenderer.mesh"] },
    coreOf(),
    editorOf(`{
      MeshRenderer: { type: "object", properties: { mesh: { type: "string", format: "entity-ref" } } },
      Joint: { type: "object", properties: {
        bodyA: { type: "string", format: "entity-ref" },
        kind: { type: "string", enum: ["revolute", "fixed", "spherical"] } } },
    }`),
  );
  // A WRAPPED ui_hint. rustfmt breaks the longest vocabularies across three lines — ShapeRecipe's has
  // fourteen variants — so a line-oriented reader would drop exactly the ones worth reading. The
  // fixture's `Joint.kind` hint is wrapped; if the whitespace collapse is removed this goes silent.
  run(
    "a closed vocabulary the table does not declare fires, even when rustfmt wrapped it",
    { count: 1, kinds: ["unread-enum"], keys: ["Joint.kind"] },
    coreOf(),
    editorOf(`{
      MeshRenderer: { type: "object", properties: { mesh: { type: "string", format: "asset" } } },
      Joint: { type: "object", properties: {
        bodyA: { type: "string", format: "entity-ref" },
        kind: { type: "string" } } },
    }`),
  );
  run(
    "a vocabulary declared with the wrong variants fires",
    { count: 1, kinds: ["unread-enum"], keys: ["Joint.kind"] },
    coreOf(),
    editorOf(`{
      MeshRenderer: { type: "object", properties: { mesh: { type: "string", format: "asset" } } },
      Joint: { type: "object", properties: {
        bodyA: { type: "string", format: "entity-ref" },
        kind: { type: "string", enum: ["revolute", "fixed"] } } },
    }`),
  );

  run(
    "a type the core disagrees with fires",
    { count: 1, kinds: ["type-drift"], keys: ["MeshRenderer.castShadows"] },
    coreOf(),
    editorOf(`{
      MeshRenderer: { type: "object", properties: {
        mesh: { type: "string", format: "asset" },
        castShadows: { type: "string" } } },
      Joint: { type: "object", properties: {
        bodyA: { type: "string", format: "entity-ref" },
        kind: { type: "string", enum: ["revolute", "fixed", "spherical"] } } },
    }`),
  );
  // `integer` widening to `number` is lossless and is how `buildEntitySchema`'s inference reads any
  // number off the wire. Only the lossy direction is a finding, or the gate would fight the fallback.
  run(
    "an integer declared as a number is not drift",
    { count: 0 },
    coreOf(),
    editorOf(`{
      Health: { type: "object", properties: { hp: { type: "number" } } },
      MeshRenderer: { type: "object", properties: { mesh: { type: "string", format: "asset" } } },
      Joint: { type: "object", properties: {
        bodyA: { type: "string", format: "entity-ref" },
        kind: { type: "string", enum: ["revolute", "fixed", "spherical"] } } },
    }`),
  );

  // ── the vacuity guards, which are what keep this from becoming a file ──────────────────────────
  run(
    "a core table the reader cannot see is a FAILURE, not a green run",
    { count: 1, kinds: ["unreadable"] },
    "pub fn standard_components() -> Vec<ComponentMeta> { vec![] }",
    editorOf("{}"),
  );
  // AND IT FAILS TWICE OVER, WHICH IS THE POINT. A missing table is unreadable AND routes nothing, so
  // every format and vocabulary the core publishes is reported alongside it. The first draft of this
  // case expected the single `unreadable` finding and was wrong about the gate rather than about the
  // fixture — pinning the keys is what said so. An absent table is the most silent table there is, and
  // the direction that catches silence must not go quiet just because the file is gone.
  run(
    "a componentSchemas that is not there is a FAILURE, and still reports everything it routes nothing for",
    {
      count: 4,
      kinds: ["unreadable", "unrouted-format", "unread-enum"],
      keys: ["`componentSchemas` is not declared in registry.ts", "MeshRenderer.mesh", "Joint.bodyA", "Joint.kind"],
    },
    coreOf(),
    "export function buildEntitySchema() {}\n",
  );
  // ── the six holes an adversarial review opened after the first version shipped ─────────────────
  // Every one of them is a way for the two tables to genuinely disagree while `check` reports ZERO.
  // That is the only failure mode that matters for a gate, and none of them was reachable from the
  // cases above, which is why they are pinned individually and by KEY.

  // (1) A SPELLING THE READER DOES NOT KNOW. rustfmt may write `FieldType::String`, and a wrapped call
  // gets a trailing comma. Either one made `field_fmt` parse as NOTHING — so the format it declares was
  // invisible and the gate went green about the one direction it exists for. The "no fields at all"
  // guard could not see it: the component still has its other fields. Now the reader counts the calls
  // it should have understood.
  // A field whose NAME is a constant rather than a literal is the general shape: the call is there, the
  // reader cannot understand it, and before the call-count guard the component simply looked one field
  // smaller — with whatever `format` that field carried invisible.
  run(
    "a field the reader could not parse is a FAILURE, not a component with fewer fields",
    { count: 1, kinds: ["unreadable"] },
    coreOf(`        ComponentMeta::builder("Paint").category("Props")
            .field("opacity", Number, false)
            .field_fmt(COLOUR_FIELD, Str, true, Some("color"))
            .build(),`),
    editorOf(TRUTHFUL),
  );
  // ...and once it IS understood, the format it declares is reported like any other.
  run(
    "a `FieldType::` spelling with a trailing comma parses, and its format is checked",
    { count: 1, kinds: ["unrouted-format"], keys: ["Paint.colour"] },
    coreOf(`        ComponentMeta::builder("Paint").category("Props")
            .field("opacity", Number, false)
            .field_fmt("colour", FieldType::String, true, Some("color"),)
            .build(),`),
    editorOf(TRUTHFUL),
  );

  // (2) A FORMAT THE EDITOR INVENTED. Direction 2 catches a format the core publishes and the table
  // ignores; this is its dual. It is the SHIPPED defect stated directly: `Material.color` was the only
  // `format: "color"` in the repository and the core has no such field. Delete the phantom component
  // but leave the format on a REAL field and, before this, nothing objected.
  run(
    "a format the core does not declare fires",
    { count: 1, kinds: ["invented-format"], keys: ["Transform.px"] },
    coreOf(),
    editorOf(`{
      Transform: { type: "object", properties: { px: { type: "number", format: "color" } } },
      MeshRenderer: { type: "object", properties: { mesh: { type: "string", format: "asset" } } },
      Joint: { type: "object", properties: {
        bodyA: { type: "string", format: "entity-ref" },
        kind: { type: "string", enum: ["revolute", "fixed", "spherical"] } } },
    }`),
  );

  // (3) A ONE-VARIANT VOCABULARY IS STILL CLOSED. The question is "may the user type something else",
  // and for `enum: only` the answer is no. Requiring two variants made it silent.
  run(
    "a single-variant vocabulary is still a vocabulary",
    { count: 1, kinds: ["unread-enum"], keys: ["Solo.mode"] },
    coreOf(`        ComponentMeta::builder("Solo").field("mode", Str, true)
            .ui_hint("mode", "enum: only").build(),`),
    editorOf(TRUTHFUL),
  );

  // (4) A COMMA WHERE THE CORE HAS A PIPE. Compared as a joined string, `["a,b"]` covers `enum: a|b` —
  // and incremented `covered` while doing it. Element by element now.
  run(
    "a comma inside one variant does not cover two",
    { count: 1, kinds: ["unread-enum"], keys: ["Joint.kind"] },
    coreOf(),
    editorOf(`{
      MeshRenderer: { type: "object", properties: { mesh: { type: "string", format: "asset" } } },
      Joint: { type: "object", properties: {
        bodyA: { type: "string", format: "entity-ref" },
        kind: { type: "string", enum: ["revolute,fixed,spherical"] } } },
    }`),
  );

  // (5) A DECLARATION IS A SENTENCE, NOT A KEY. `{ Ghost: null }` suppressed by existing.
  run(
    "a declaration with no reason suppresses nothing and says so",
    { count: 2, kinds: ["phantom-component", "unreadable"], keys: ["Ghost", "phantom:Ghost"] },
    coreOf(),
    editorOf(`{
      Ghost: { type: "object", properties: {} },
      MeshRenderer: { type: "object", properties: { mesh: { type: "string", format: "asset" } } },
      Joint: { type: "object", properties: {
        bodyA: { type: "string", format: "entity-ref" },
        kind: { type: "string", enum: ["revolute", "fixed", "spherical"] } } },
    }`),
    { phantom: { Ghost: null } },
  );

  // (6) A CURATED FIELD WITH NO `type` MATCHES NO TESTER, and JSON Forms answers by painting "No
  // applicable renderer found." into the panel. `JsonSchema7.type` is optional, so this compiles.
  run(
    "a curated field with no type fires",
    { count: 1, kinds: ["untyped-field"], keys: ["Transform.px"] },
    coreOf(),
    editorOf(`{
      Transform: { type: "object", properties: { px: { title: "Position X" } } },
      MeshRenderer: { type: "object", properties: { mesh: { type: "string", format: "asset" } } },
      Joint: { type: "object", properties: {
        bodyA: { type: "string", format: "entity-ref" },
        kind: { type: "string", enum: ["revolute", "fixed", "spherical"] } } },
    }`),
  );

  // THE BACKTICK, WHICH IS THIS GATE'S OWN APOSTROPHE. `registry.ts` documents its colour field with
  // `` // `format: color` → … ``; read as a string opener it swallows the rest of the table and the
  // gate reports "unterminated" — red, but for the wrong reason and with none of the real findings.
  // Pinned by KEY: without `stripComments` the single finding becomes `unreadable` and `Material`
  // never appears, so a count-only assertion would have passed the broken reader.
  run(
    "a backtick in a comment does not truncate the table",
    { count: 1, kinds: ["phantom-component"], keys: ["Material"] },
    coreOf(),
    `import type { JsonSchema7 } from "@jsonforms/core";
export const componentSchemas: Record<string, JsonSchema7> = {
  Transform: { type: "object", properties: { px: { type: "number" } } },
  Material: {
    type: "object",
    properties: {
      // \`format: color\` → the custom ColorControl renderer (typed field, not a bare string box)
      color: { type: "string", format: "color" },
    },
  },
  MeshRenderer: { type: "object", properties: { mesh: { type: "string", format: "asset" } } },
  Joint: { type: "object", properties: {
    bodyA: { type: "string", format: "entity-ref" },
    kind: { type: "string", enum: ["revolute", "fixed", "spherical"] } } },
};
`,
  );

  // A manifest key that is not a bucket. Left undiscriminated, the `//` prose note's STRING value is
  // walked as a map and every character index becomes a stale declaration — 444 of them on this gate's
  // first real run, burying the two that were real.
  run(
    "a note in the manifest is not a bucket of 444 declarations",
    { count: 1, kinds: ["unreadable"], keys: ["notes"], stale: 0 },
    coreOf(),
    editorOf(TRUTHFUL),
    { "//": "prose that must be ignored", notes: "a bucket name nobody defined" },
  );

  {
    // The reader's own catch, measured against the REAL source rather than a fixture: this is the
    // number every finding above is stated against, and if the table ever stops parsing it is the
    // first thing that changes.
    const { components, problems } = parseCoreTable(readFileSync(CORE_TABLE, "utf8"));
    const formats = [...components].flatMap(([c, m]) => [...m.fields].filter(([, f]) => f.format).map(([f]) => `${c}.${f}`));
    const enums = [...components].flatMap(([c, m]) => [...m.hints].filter(([, h]) => hintEnum(h)).map(([f]) => `${c}.${f}`));
    const ok = problems.length === 0 && components.size >= MIN_COMPONENTS && formats.length > 0 && enums.length > 0;
    cases.push({ ok, label: "the real core table parses, with formats and vocabularies found" });
    console.log(
      `${ok ? "ok  " : "FAIL"}  the real core table parses, with formats and vocabularies found` +
        ` — ${components.size} components, ${formats.length} format field(s) [${formats.join(", ")}],` +
        ` ${enums.length} closed vocabular(y|ies) [${enums.join(", ")}]` +
        (problems.length ? ` — ${problems.join("; ")}` : ""),
    );
  }

  rmSync(root, { recursive: true, force: true });
  const failed = cases.filter((c) => !c.ok).length;
  console.log(
    `\nregistry-vocab self-test: ${cases.length - failed}/${cases.length} passed` +
      (failed ? " — a case stopped discriminating" : ""),
  );
  return failed === 0;
}

// Only when RUN, never when imported — an unguarded `process.exit` at module scope would kill any
// consumer of the exports above the moment it imported them, which makes the exports a lie.
const invokedDirectly =
  process.argv[1] !== undefined && import.meta.url === pathToFileURL(process.argv[1]).href;
if (invokedDirectly) {
  if (process.argv.includes("--self-test")) {
    process.exit(selfTest() ? 0 : 1);
  } else {
    const manifest = JSON.parse(readFileSync(MANIFEST, "utf8"));
    const { findings } = report(readFileSync(CORE_TABLE, "utf8"), readFileSync(EDITOR_TABLE, "utf8"), manifest);
    process.exit(findings.length === 0 ? 0 : 1);
  }
}
