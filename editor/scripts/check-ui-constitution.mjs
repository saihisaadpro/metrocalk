#!/usr/bin/env node

/**
 * Engine UI/UX Constitution regression ratchet.
 *
 * This intentionally uses only Node built-ins so the architectural gate is available before the
 * editor dependency tree is installed. It scans production UI sources, compares each file with the
 * checked-in debt baseline, and treats every unlisted/new file as having a zero-debt baseline.
 *
 * A genuinely exceptional authored-data or dynamic-preview literal may carry one narrowly scoped
 * waiver immediately before (or on) the affected line:
 *
 *   // ui-constitution-allow literal-ui-color: authored graph swatch from imported material data
 *
 * One annotation suppresses one finding, is always reported, and fails when it is stale or unused.
 */

import assert from "node:assert/strict";
import {
  existsSync,
  readFileSync,
  readdirSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { dirname, extname, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url));
const EDITOR_ROOT = resolve(SCRIPT_DIR, "..");
const BASELINE_PATH = resolve(SCRIPT_DIR, "ui-constitution-baseline.json");

const SCAN_ROOTS = [
  "src/app",
  "src/graph",
  "src/inspector",
  "src/panels",
  "src/theme",
];

const SOURCE_EXTENSIONS = new Set([".css", ".js", ".jsx", ".scss", ".ts", ".tsx"]);
const EXCLUDED_SOURCE = /\.(?:test|spec|stories)\.[^.]+$/i;

const RULES = Object.freeze({
  "raw-native-control": "raw native controls outside the shared theme authorities",
  "literal-ui-color": "literal UI colours outside the colour token authorities",
  "first-party-css": "first-party CSS outside the shared theme",
  "literal-motion": "literal motion timing outside the motion authority",
  "unitless-length": "a spacing token interpolated into a length with no unit, which the browser drops",
});
const RULE_IDS = Object.freeze(Object.keys(RULES));

const COLOR_AUTHORITIES = new Set([
  "src/theme/global.css",
  "src/theme/tokens.ts",
]);
const MOTION_AUTHORITIES = new Set([
  "src/theme/global.css",
  "src/theme/tokens.ts",
]);

const HEX_COLOR = /(?<![\w-])#(?:[0-9a-f]{3,4}|[0-9a-f]{6}|[0-9a-f]{8})(?![0-9a-f\w-])/gi;
const FUNCTION_COLOR = /\b(?:color|hsl|hsla|hwb|lab|lch|oklab|oklch|rgb|rgba)\s*\(/gi;
const COLOR_PROPERTY =
  /\b(?:accentColor|background|backgroundColor|border|borderBottom|borderBottomColor|borderColor|borderLeft|borderLeftColor|borderRight|borderRightColor|borderTop|borderTopColor|boxShadow|caretColor|color|fill|outline|outlineColor|stroke|textDecorationColor|textShadow)\s*:\s*([^\n;}]+)/gi;
const NAMED_COLOR =
  /\b(?:aqua|black|blue|brown|crimson|cyan|fuchsia|gold|gray|green|grey|indigo|lime|magenta|maroon|navy|olive|orange|pink|purple|red|silver|teal|violet|white|yellow)\b/gi;
const CSS_TIME = /(?<![\w.])(?:\d+(?:\.\d+)?|\.\d+)(?:ms|s)\b/gi;
const MOTION_PROPERTY =
  /\b(?:animation(?:-(?:delay|duration)|Delay|Duration)?|transition(?:-(?:delay|duration)|Delay|Duration)?)\s*:/gi;
// JSX intrinsic controls are lowercase. Keep this case-sensitive so shared `Button`, `SelectField`,
// `TextField`, and future design-system components are not misclassified as raw browser controls.
const NATIVE_CONTROL = /<\s*(?:button|input|select|textarea)\b/g;
const WAIVER =
  /ui-constitution-allow\s+([a-z][a-z0-9-]*)\s*:\s*(.*?)(?=\*\/|-->|}\s*$|$)/gi;

// A CSS length written as a TEMPLATE LITERAL, and an interpolation inside it that carries no unit.
// React appends `px` to a NUMBER; it does not look inside a string, so `` `${space.sm} ${space.xs}` ``
// reaches the DOM as the length list "6 4", which is invalid, and the browser drops the whole
// declaration in silence. `tsc` sees a string and is satisfied. The colour and motion rules above
// read values, not units. The `shots` invariants are all geometric and a missing padding overflows
// nothing, clips nothing, overlaps nothing and reads perfectly — so R1–R8 are silent too.
//
// Eleven live instances were found the day this rule was written (ADR-128), across the engine rail
// (whose heading and every group title had therefore never had any padding, visible in `shell-wide`
// as labels flush against the window edge beside their own indented items), MatchPanel and
// TerrainPanel. Every other spacing interpolation in the tree already writes `px`, which is what
// makes this a slip rather than a convention.
const LENGTH_TEMPLATE =
  /\b(?:padding|margin|gap|rowGap|columnGap|inset|top|right|bottom|left|width|height|minWidth|minHeight|maxWidth|maxHeight|blockSize|inlineSize|borderRadius|flexBasis|outlineOffset|(?:padding|margin|inset)(?:Top|Right|Bottom|Left|Inline|Block)(?:Start|End)?)\s*:\s*`([^`]*)`/g;
// A unit, a percentage, or a CSS-wide keyword may follow; anything else means the value is dropped.
const UNITLESS_INTERPOLATION = /\$\{[^}]*\}(?!\s*(?:px|%|em|rem|ex|ch|vh|vw|vmin|vmax|cm|mm|in|pt|pc|fr|deg|s\b|ms\b))/g;

function normalisePath(path) {
  return path.split(sep).join("/");
}

function relativeToEditor(path) {
  return normalisePath(relative(EDITOR_ROOT, path));
}

function isThemeAuthority(path) {
  return path === "src/theme" || path.startsWith("src/theme/");
}

function emptyCounts() {
  return Object.fromEntries(RULE_IDS.map((rule) => [rule, 0]));
}

function listProductionSources() {
  const files = [];
  const visit = (absolute) => {
    for (const entry of readdirSync(absolute, { withFileTypes: true })) {
      const child = resolve(absolute, entry.name);
      if (entry.isDirectory()) {
        visit(child);
      } else if (
        entry.isFile() &&
        SOURCE_EXTENSIONS.has(extname(entry.name).toLowerCase()) &&
        !EXCLUDED_SOURCE.test(entry.name) &&
        !entry.name.endsWith(".d.ts")
      ) {
        files.push(child);
      }
    }
  };

  for (const root of SCAN_ROOTS) {
    const absolute = resolve(EDITOR_ROOT, root);
    if (existsSync(absolute) && statSync(absolute).isDirectory()) visit(absolute);
  }
  return files.sort((a, b) => relativeToEditor(a).localeCompare(relativeToEditor(b)));
}

/**
 * Blank comments while preserving strings, offsets, and newlines. Findings therefore keep exact
 * source locations and literals inside style strings remain visible to the colour/motion rules.
 */
function maskComments(source) {
  const output = source.split("");
  let state = "code";
  let quote = "";

  for (let index = 0; index < source.length; index += 1) {
    const current = source[index];
    const next = source[index + 1];

    if (state === "line-comment") {
      if (current === "\n" || current === "\r") {
        state = "code";
      } else {
        output[index] = " ";
      }
      continue;
    }

    if (state === "block-comment") {
      if (current === "*" && next === "/") {
        output[index] = " ";
        output[index + 1] = " ";
        index += 1;
        state = "code";
      } else if (current !== "\n" && current !== "\r") {
        output[index] = " ";
      }
      continue;
    }

    if (state === "string") {
      if (current === "\\") {
        index += 1;
      } else if (current === quote) {
        state = "code";
        quote = "";
      }
      continue;
    }

    if (current === "'" || current === '"' || current === "`") {
      state = "string";
      quote = current;
    } else if (current === "/" && next === "/") {
      output[index] = " ";
      output[index + 1] = " ";
      index += 1;
      state = "line-comment";
    } else if (current === "/" && next === "*") {
      output[index] = " ";
      output[index + 1] = " ";
      index += 1;
      state = "block-comment";
    }
  }

  return output.join("");
}

function lineStartsFor(source) {
  const starts = [0];
  for (let index = 0; index < source.length; index += 1) {
    if (source[index] === "\n") starts.push(index + 1);
  }
  return starts;
}

function locateOffset(starts, offset) {
  let low = 0;
  let high = starts.length - 1;
  while (low <= high) {
    const middle = Math.floor((low + high) / 2);
    if (starts[middle] <= offset) low = middle + 1;
    else high = middle - 1;
  }
  const lineIndex = Math.max(0, high);
  return { line: lineIndex + 1, column: offset - starts[lineIndex] + 1 };
}

function excerptAt(lines, line) {
  return (lines[line - 1] ?? "").trim().slice(0, 180);
}

function findAll(regex, text, callback) {
  regex.lastIndex = 0;
  let match;
  while ((match = regex.exec(text)) !== null) {
    callback(match);
    if (match[0].length === 0) regex.lastIndex += 1;
  }
}

function motionValueEnd(source, valueStart, cssSource) {
  let quote = "";
  let parentheses = 0;
  let brackets = 0;

  for (let index = valueStart; index < source.length; index += 1) {
    const current = source[index];
    if (quote) {
      if (current === "\\") index += 1;
      else if (current === quote) quote = "";
      continue;
    }
    if (current === "'" || current === '"' || current === "`") {
      quote = current;
      continue;
    }
    if (current === "(") parentheses += 1;
    else if (current === ")") parentheses = Math.max(0, parentheses - 1);
    else if (current === "[") brackets += 1;
    else if (current === "]") brackets = Math.max(0, brackets - 1);
    else if (parentheses === 0 && brackets === 0) {
      if (current === ";" || current === "}") return index;
      if (!cssSource && current === ",") return index;
    }
  }
  return source.length;
}

function nextCodeLine(lines, annotationLine) {
  for (let index = annotationLine; index < lines.length; index += 1) {
    const trimmed = lines[index].trim();
    if (
      trimmed === "" ||
      /^(?:\/\/|\/\*|\*|\*\/|{\s*\/\*|<!--|-->)/.test(trimmed)
    ) {
      continue;
    }
    return index + 1;
  }
  return null;
}

function parseWaivers(source, path) {
  const lines = source.split(/\r?\n/);
  const waivers = [];
  const errors = [];

  lines.forEach((line, index) => {
    findAll(WAIVER, line, (match) => {
      const rule = match[1].toLowerCase();
      const reason = match[2].trim().replace(/\s*(?:\*\/|-->|})\s*$/, "").trim();
      const annotationLine = index + 1;
      const markerIndex = match.index;
      const commentIndexCandidates = [
        line.lastIndexOf("//", markerIndex),
        line.lastIndexOf("/*", markerIndex),
        line.lastIndexOf("<!--", markerIndex),
      ].filter((candidate) => candidate >= 0);
      const commentIndex =
        commentIndexCandidates.length > 0 ? Math.max(...commentIndexCandidates) : markerIndex;
      // A standalone JSX comment is wrapped in `{/* ... */}`; that opening brace is comment
      // punctuation, not source code that should retarget the waiver to its own line.
      const hasCodeBefore = line
        .slice(0, commentIndex)
        .replace(/[{\s]/g, "")
        .trim().length > 0;
      const targetLine = hasCodeBefore ? annotationLine : nextCodeLine(lines, annotationLine);

      if (!Object.hasOwn(RULES, rule)) {
        errors.push(
          `${path}:${annotationLine}: unknown waiver rule "${rule}" (expected ${RULE_IDS.join(", ")})`,
        );
      } else if (reason.length === 0) {
        errors.push(`${path}:${annotationLine}: waiver requires a specific reason`);
      } else if (targetLine === null) {
        errors.push(`${path}:${annotationLine}: waiver has no following source line`);
      } else {
        waivers.push({
          rule,
          reason,
          annotationLine,
          targetLine,
          used: false,
        });
      }
    });
  });

  return { waivers, errors };
}

function analyseFile(path, source) {
  const masked = maskComments(source);
  const lines = source.split(/\r?\n/);
  const starts = lineStartsFor(source);
  const findings = [];
  const findingKeys = new Set();
  const { waivers, errors } = parseWaivers(source, path);

  const addFinding = (rule, offset, detail) => {
    const key = `${rule}:${offset}`;
    if (findingKeys.has(key)) return;
    findingKeys.add(key);
    const location = locateOffset(starts, offset);
    findings.push({
      rule,
      ...location,
      detail,
      excerpt: excerptAt(lines, location.line),
      waived: false,
    });
  };

  if (!isThemeAuthority(path)) {
    findAll(NATIVE_CONTROL, masked, (match) => {
      addFinding("raw-native-control", match.index, match[0].trim());
    });
  }

  if (!COLOR_AUTHORITIES.has(path)) {
    findAll(HEX_COLOR, masked, (match) => {
      addFinding("literal-ui-color", match.index, match[0]);
    });
    findAll(FUNCTION_COLOR, masked, (match) => {
      addFinding("literal-ui-color", match.index, match[0]);
    });
    findAll(COLOR_PROPERTY, masked, (propertyMatch) => {
      const value = propertyMatch[1];
      findAll(NAMED_COLOR, value, (colorMatch) => {
        const valueOffset = propertyMatch.index + propertyMatch[0].indexOf(value);
        addFinding("literal-ui-color", valueOffset + colorMatch.index, colorMatch[0]);
      });
    });
  }

  if (/\.(?:css|scss)$/i.test(path) && !isThemeAuthority(path)) {
    findAll(/\{/g, masked, (match) => {
      addFinding("first-party-css", match.index, "CSS rule block");
    });
    findAll(/(?:\{|;)\s*([-\w]+)\s*:/g, masked, (match) => {
      const propertyOffset = match.index + match[0].lastIndexOf(match[1]);
      addFinding("first-party-css", propertyOffset, `CSS declaration "${match[1]}"`);
    });
    findAll(/@import\b[^;]*;/gi, masked, (match) => {
      addFinding("first-party-css", match.index, "CSS import");
    });
  }

  // Applies to EVERY source, theme authorities included: `tokens.ts` composing a length badly is the
  // same dropped declaration, and there is no authority that gets to write invalid CSS.
  if (!/\.(?:css|scss)$/i.test(path)) {
    findAll(LENGTH_TEMPLATE, masked, (propertyMatch) => {
      const value = propertyMatch[1];
      // `calc()` and `var()` legitimately carry a unitless operand (`calc(${n} * 1px)`), and the
      // custom property's own declaration is where a `var()`'s unit lives. Judging inside either
      // would be guessing at arithmetic this rule has no business doing.
      if (value.includes("calc(") || value.includes("var(")) return;
      const valueOffset = propertyMatch.index + propertyMatch[0].indexOf("`") + 1;
      findAll(UNITLESS_INTERPOLATION, value, (match) => {
        addFinding("unitless-length", valueOffset + match.index, `${match[0]} in \`${value}\``);
      });
    });
  }

  if (!MOTION_AUTHORITIES.has(path)) {
    const cssSource = /\.(?:css|scss)$/i.test(path);
    findAll(MOTION_PROPERTY, masked, (propertyMatch) => {
      const valueStart = propertyMatch.index + propertyMatch[0].length;
      const valueEnd = motionValueEnd(masked, valueStart, cssSource);
      const value = masked.slice(valueStart, valueEnd);
      findAll(CSS_TIME, value, (timeMatch) => {
        addFinding("literal-motion", valueStart + timeMatch.index, timeMatch[0]);
      });
    });
  }

  findings.sort((left, right) =>
    left.line - right.line || left.column - right.column || left.rule.localeCompare(right.rule),
  );

  for (const finding of findings) {
    const waiver = waivers.find(
      (candidate) =>
        !candidate.used &&
        candidate.rule === finding.rule &&
        candidate.targetLine === finding.line,
    );
    if (waiver) {
      waiver.used = true;
      finding.waived = true;
    }
  }

  for (const waiver of waivers) {
    if (!waiver.used) {
      errors.push(
        `${path}:${waiver.annotationLine}: unused ${waiver.rule} waiver targeting line ${waiver.targetLine}`,
      );
    }
  }

  const counts = emptyCounts();
  for (const finding of findings) {
    if (!finding.waived) counts[finding.rule] += 1;
  }

  return {
    path,
    counts,
    findings: findings.filter((finding) => !finding.waived),
    waivers: waivers.filter((waiver) => waiver.used),
    errors,
  };
}

function scanRepository() {
  return listProductionSources().map((absolute) =>
    analyseFile(relativeToEditor(absolute), readFileSync(absolute, "utf8")),
  );
}

function makeBaseline(results) {
  return {
    version: 1,
    description:
      "Per-file UI constitution debt ceiling. Missing and newly added files have an implicit zero baseline.",
    scanRoots: SCAN_ROOTS,
    rules: RULES,
    files: Object.fromEntries(results.map((result) => [result.path, result.counts])),
  };
}

function loadBaseline() {
  if (!existsSync(BASELINE_PATH)) {
    throw new Error(
      `Missing ${relativeToEditor(BASELINE_PATH)}. Generate reviewed output with --print-baseline.`,
    );
  }
  const baseline = JSON.parse(readFileSync(BASELINE_PATH, "utf8"));
  if (baseline.version !== 1 || typeof baseline.files !== "object" || baseline.files === null) {
    throw new Error("UI constitution baseline has an unsupported or malformed schema");
  }
  return baseline;
}

function compareWithBaseline(results, baseline) {
  const current = new Map(results.map((result) => [result.path, result]));
  const paths = new Set([...Object.keys(baseline.files), ...current.keys()]);
  const regressions = [];
  const reductions = [];

  for (const path of [...paths].sort()) {
    const result = current.get(path);
    const currentCounts = result?.counts ?? emptyCounts();
    const baselineCounts = baseline.files[path] ?? emptyCounts();
    for (const rule of RULE_IDS) {
      const actual = Number(currentCounts[rule] ?? 0);
      const ceiling = Number(baselineCounts[rule] ?? 0);
      const comparison = { path, rule, actual, baseline: ceiling, result };
      if (actual > ceiling) regressions.push(comparison);
      else if (actual < ceiling) reductions.push(comparison);
    }
  }

  return { regressions, reductions };
}

function totalsFor(results) {
  const totals = emptyCounts();
  for (const result of results) {
    for (const rule of RULE_IDS) totals[rule] += result.counts[rule];
  }
  return totals;
}

function printWaivers(results) {
  const waivers = results.flatMap((result) =>
    result.waivers.map((waiver) => ({ path: result.path, ...waiver })),
  );
  if (waivers.length === 0) {
    console.log("Waivers: none.");
    return;
  }
  console.log(`Waivers (${waivers.length}, each suppresses exactly one finding):`);
  for (const waiver of waivers) {
    console.log(
      `  - ${waiver.path}:${waiver.annotationLine} ${waiver.rule} -> line ${waiver.targetLine}: ${waiver.reason}`,
    );
  }
}

function printTotals(results) {
  const totals = totalsFor(results);
  console.log(`Scanned ${results.length} production UI source files.`);
  for (const rule of RULE_IDS) {
    console.log(`  ${rule.padEnd(22)} ${String(totals[rule]).padStart(4)}  ${RULES[rule]}`);
  }
}

function runSelfTests() {
  // How many fixtures this run actually exercised. `analyseFile` is the only door into the scanner,
  // so counting the calls counts the contracts — and a fixture deleted in a refactor lowers the
  // number rather than leaving a confident constant behind.
  let selfTestFixtures = 0;
  const analyse = (path, source) => {
    selfTestFixtures += 1;
    return analyseFile(path, source);
  };
  const component = analyse(
    "src/panels/Synthetic.tsx",
    'export const Synthetic = () => <button style={{ color: "#fff", transition: "opacity 120ms ease" }}>Go</button>;\n',
  );
  assert.equal(component.counts["raw-native-control"], 1);
  assert.equal(component.counts["literal-ui-color"], 1);
  assert.equal(component.counts["literal-motion"], 1);

  const primitive = analyse(
    "src/theme/SyntheticPrimitive.tsx",
    "export const SyntheticPrimitive = () => <button>Go</button>;\n",
  );
  assert.equal(primitive.counts["raw-native-control"], 0);

  const sharedConsumer = analyse(
    "src/panels/SyntheticSharedConsumer.tsx",
    "export const SyntheticSharedConsumer = () => <><Button>Go</Button><SelectField /></>;\n",
  );
  assert.equal(sharedConsumer.counts["raw-native-control"], 0);

  const authority = analyse(
    "src/theme/tokens.ts",
    'export const sample = { color: "#fff", transition: "opacity 120ms ease" };\n',
  );
  assert.equal(authority.counts["literal-ui-color"], 0);
  assert.equal(authority.counts["literal-motion"], 0);

  const css = analyse(
    "src/panels/Synthetic.css",
    ".synthetic { color: #fff; transition: opacity 120ms ease; }\n",
  );
  assert.equal(css.counts["first-party-css"], 3);
  assert.equal(css.counts["literal-ui-color"], 1);
  assert.equal(css.counts["literal-motion"], 1);

  const multilineMotion = analyse(
    "src/panels/MultilineMotion.tsx",
    "export const style = {\n  transition:\n    \"opacity 180ms ease, transform 240ms ease\",\n};\n",
  );
  assert.equal(multilineMotion.counts["literal-motion"], 2);

  // `unitless-length`. The first fixture is the shape that actually shipped eleven times: a spacing
  // token interpolated straight into a length list, which the browser discards whole.
  const droppedLength = analyse(
    "src/app/SyntheticRail.tsx",
    "export const rail = { padding: `${space.sm} ${space.xs}`, margin: `0 0 ${space.xs}` };\n",
  );
  assert.equal(droppedLength.counts["unitless-length"], 3);

  // The control that has to hold, or the rule would rewrite the whole codebase: the same tokens,
  // written correctly, plus every legitimate way a unit can arrive.
  const correctLengths = analyse(
    "src/app/SyntheticCorrect.tsx",
    "export const ok = {\n" +
      "  padding: `${space.sm}px ${space.xs}px`,\n" +
      "  width: `${pct}%`,\n" +
      "  height: `${rows}rem`,\n" +
      "  transform: `translateY(${dy})`,\n" +
      "  gridTemplateColumns: `repeat(${n}, 1fr)`,\n" +
      "  minHeight: `calc(100% - ${gap})`,\n" +
      "  maxWidth: `var(--mtk-dock-w)`,\n" +
      "  flexBasis: `0 0 ${w}px`,\n" +
      "};\n",
  );
  assert.equal(correctLengths.counts["unitless-length"], 0);

  // A number is React's own job — it appends the unit itself — so a bare `padding: space.sm` is
  // correct and must stay silent. Only a TEMPLATE LITERAL loses the unit.
  const numericLength = analyse(
    "src/app/SyntheticNumeric.tsx",
    "export const ok = { padding: space.sm, gap: 4, top: someNumber };\n",
  );
  assert.equal(numericLength.counts["unitless-length"], 0);

  // No file is exempt, including the theme authorities — invalid CSS is invalid there too.
  const authorityLength = analyse(
    "src/theme/tokens.ts",
    "export const pad = { padding: `${space.sm} 0` };\n",
  );
  assert.equal(authorityLength.counts["unitless-length"], 1);

  const waived = analyse(
    "src/panels/AuthoredPreview.tsx",
    "// ui-constitution-allow literal-ui-color: authored preview swatch supplied by imported material data\n" +
      'export const swatch = { color: "#c0ffee" };\n',
  );
  assert.equal(waived.counts["literal-ui-color"], 0);
  assert.equal(waived.waivers.length, 1);
  assert.deepEqual(waived.errors, []);

  const jsxWaived = analyse(
    "src/panels/JsxAuthoredPreview.tsx",
    "export const Preview = () => <>\n" +
      "  {/* ui-constitution-allow literal-ui-color: imported author swatch rendered without reinterpretation */}\n" +
      '  <span style={{ color: "#c0ffee" }}>swatch</span>\n' +
      "</>;\n",
  );
  assert.equal(jsxWaived.counts["literal-ui-color"], 0);
  assert.equal(jsxWaived.waivers.length, 1);
  assert.deepEqual(jsxWaived.errors, []);

  const stale = analyse(
    "src/panels/Stale.tsx",
    "// ui-constitution-allow raw-native-control: temporary compatibility seam\n" +
      "export const value = 1;\n",
  );
  assert.equal(stale.errors.length, 1);

  const newFileComparison = compareWithBaseline(
    [component],
    { version: 1, files: {} },
  );
  assert.equal(newFileComparison.regressions.length, 3);

  const reductionComparison = compareWithBaseline(
    [primitive],
    {
      version: 1,
      files: {
        "src/theme/SyntheticPrimitive.tsx": {
          ...emptyCounts(),
          "literal-ui-color": 1,
        },
      },
    },
  );
  assert.equal(reductionComparison.regressions.length, 0);
  assert.equal(reductionComparison.reductions.length, 1);

  // Counted from the fixtures above, not restated: the previous hard-coded "11" had already drifted
  // by four before anyone noticed, which is the smallest possible instance of the thing this whole
  // file exists to catch — a second statement of a fact, compared to the first by nothing.
  console.log(`UI constitution scanner self-tests passed (${selfTestFixtures} fixtures/contracts).`);
}

function main() {
  const args = new Set(process.argv.slice(2));
  if (args.has("--self-test")) {
    runSelfTests();
    return;
  }

  const results = scanRepository();
  const sourceErrors = results.flatMap((result) => result.errors);
  if (sourceErrors.length > 0) {
    console.error("UI constitution waiver errors:");
    for (const error of sourceErrors) console.error(`  - ${error}`);
    process.exitCode = 1;
    return;
  }

  if (args.has("--print-baseline")) {
    console.log(`${JSON.stringify(makeBaseline(results), null, 2)}\n`);
    return;
  }

  if (args.has("--write-baseline")) {
    if (existsSync(BASELINE_PATH)) {
      const prior = loadBaseline();
      const { regressions } = compareWithBaseline(results, prior);
      if (regressions.length > 0) {
        console.error("Refusing to raise UI constitution debt ceilings:");
        for (const regression of regressions) {
          console.error(
            `  - ${regression.path} ${regression.rule}: ${regression.actual} > ${regression.baseline}`,
          );
        }
        process.exitCode = 1;
        return;
      }
    }
    writeFileSync(BASELINE_PATH, `${JSON.stringify(makeBaseline(results), null, 2)}\n`, "utf8");
    console.log(`Locked current non-increasing counts in ${relativeToEditor(BASELINE_PATH)}.`);
    return;
  }

  const baseline = loadBaseline();
  const { regressions, reductions } = compareWithBaseline(results, baseline);
  printTotals(results);
  printWaivers(results);

  if (regressions.length > 0) {
    console.error("\nUI constitution regression detected:");
    for (const regression of regressions) {
      console.error(
        `  - ${regression.path} ${regression.rule}: ${regression.actual} > baseline ${regression.baseline}`,
      );
      const examples = (regression.result?.findings ?? [])
        .filter((finding) => finding.rule === regression.rule)
        .slice(0, 3);
      for (const finding of examples) {
        console.error(
          `      ${regression.path}:${finding.line}:${finding.column} ${finding.detail} | ${finding.excerpt}`,
        );
      }
    }
    console.error(
      "\nUse a shared authority, reduce existing debt, or add a one-finding ui-constitution-allow annotation with a specific reason.",
    );
    process.exitCode = 1;
    return;
  }

  if (reductions.length > 0) {
    console.log("\nDebt reductions detected (the gate passes; lock them with --write-baseline):");
    for (const reduction of reductions) {
      console.log(
        `  - ${reduction.path} ${reduction.rule}: ${reduction.baseline} -> ${reduction.actual}`,
      );
    }
  }

  console.log("\nEngine UI/UX Constitution ratchet passed.");
}

main();
