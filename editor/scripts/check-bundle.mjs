import { gzipSync } from "node:zlib";
import { createHash } from "node:crypto";
import { mkdir, readFile, readdir, stat, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const assetsDir = fileURLToPath(new URL("../dist/assets/", import.meta.url));
const distDir = fileURLToPath(new URL("../dist/", import.meta.url));
const budgetPath = fileURLToPath(new URL("./bundle-budget.json", import.meta.url));
const durableEvidenceDir = fileURLToPath(new URL("../../editor-shell/evidence/accessibility/", import.meta.url));
const budget = JSON.parse(await readFile(budgetPath, "utf8"));
const files = await readdir(assetsDir);
const javascript = await Promise.all(
  files.filter((file) => file.endsWith(".js")).map(async (file) => {
    const absolutePath = join(assetsDir, file);
    const source = await readFile(absolutePath);
    return {
      file,
      bytes: (await stat(absolutePath)).size,
      gzipBytes: gzipSync(source, { level: 9 }).length,
      sha256: createHash("sha256").update(source).digest("hex"),
      source: source.toString("utf8"),
    };
  }),
);
javascript.sort((left, right) => right.bytes - left.bytes || left.file.localeCompare(right.file));

const entry = javascript.find(({ file }) => /^index-[\w-]+\.js$/.test(file));
if (!entry) throw new Error("Production entry chunk was not emitted");

const indexHtml = await readFile(join(distDir, "index.html"), "utf8");
const initialNames = new Set(
  Array.from(indexHtml.matchAll(/(?:src|href)="\/assets\/([^\"]+\.js)"/g), (match) => match[1]),
);
const chunkByName = new Map(javascript.map((chunk) => [chunk.file, chunk]));
const staticDependencies = (source) => {
  const dependencies = new Set();
  const patterns = [
    /\bimport(?!\s*\()[^;]*?["']\.\/([^"']+\.js)["']/g,
    /\bexport\s+[^;]*?\bfrom\s*["']\.\/([^"']+\.js)["']/g,
  ];
  for (const pattern of patterns) {
    for (const match of source.matchAll(pattern)) dependencies.add(match[1]);
  }
  return dependencies;
};

// Follow static ESM edges, not just index.html preloads. This prevents an on-demand panel from quietly
// becoming startup work if Vite's preload strategy changes or a source file gains a static re-export.
const pending = [...initialNames];
for (let index = 0; index < pending.length; index += 1) {
  const name = pending[index];
  const chunk = chunkByName.get(name);
  if (!chunk) throw new Error(`Initial module graph references missing JavaScript chunk '${name}'`);
  for (const dependency of staticDependencies(chunk.source)) {
    if (!chunkByName.has(dependency)) throw new Error(`${name} statically imports missing chunk '${dependency}'`);
    if (!initialNames.has(dependency)) {
      initialNames.add(dependency);
      pending.push(dependency);
    }
  }
}
const initialChunks = javascript.filter(({ file }) => initialNames.has(file));
const initialBytes = initialChunks.reduce((total, chunk) => total + chunk.bytes, 0);
const initialGzipBytes = initialChunks.reduce((total, chunk) => total + chunk.gzipBytes, 0);
const totalBytes = javascript.reduce((total, chunk) => total + chunk.bytes, 0);
const totalGzipBytes = javascript.reduce((total, chunk) => total + chunk.gzipBytes, 0);
const largest = javascript[0];

function requireBudget(condition, message) {
  if (!condition) throw new Error(message);
}

requireBudget(
  entry.bytes <= budget.maximumEntryBytes,
  `Entry chunk ${entry.file} is ${entry.bytes} bytes; budget is ${budget.maximumEntryBytes}`,
);
requireBudget(
  initialBytes <= budget.maximumInitialJavaScriptBytes,
  `Initial JavaScript is ${initialBytes} bytes; budget is ${budget.maximumInitialJavaScriptBytes}`,
);
requireBudget(
  largest.bytes <= budget.maximumChunkBytes,
  `Largest JavaScript chunk ${largest.file} is ${largest.bytes} bytes; budget is ${budget.maximumChunkBytes}`,
);
requireBudget(
  totalBytes <= budget.maximumTotalJavaScriptBytes,
  `Total JavaScript is ${totalBytes} bytes; budget is ${budget.maximumTotalJavaScriptBytes}`,
);
requireBudget(
  javascript.length >= budget.minimumJavaScriptChunks,
  `Only ${javascript.length} JavaScript chunks were emitted; at least ${budget.minimumJavaScriptChunks} are required`,
);

for (const required of budget.requiredChunkPrefixes) {
  if (!javascript.some(({ file }) => file.startsWith(required))) {
    throw new Error(`Required production split '${required}' was not emitted`);
  }
}

for (const forbidden of budget.forbiddenInitialChunkPrefixes) {
  if (initialChunks.some(({ file }) => file.startsWith(forbidden))) {
    throw new Error(`On-demand chunk '${forbidden}' was preloaded by dist/index.html`);
  }
}

const report = {
  schemaVersion: 1,
  generatedAt: new Date().toISOString(),
  measurement: "production files before gzip; gzip values use level 9 for comparison only",
  status: "passed",
  baseline: {
    entryBytes: budget.baselineEntryBytes,
    source: "pre-split production entry measurement",
  },
  budgets: budget,
  results: {
    entry,
    initialBytes,
    initialGzipBytes,
    totalBytes,
    totalGzipBytes,
    largestChunk: largest,
    chunkCount: javascript.length,
    entryReductionPercent: Number(
      ((1 - entry.bytes / budget.baselineEntryBytes) * 100).toFixed(1),
    ),
    initialChunks: initialChunks.map(({ file, bytes, gzipBytes }) => ({ file, bytes, gzipBytes })),
    chunks: javascript.map(({ source: _source, ...chunk }) => chunk),
  },
};
report.results.entry = (({ source: _source, ...chunk }) => chunk)(entry);
report.results.largestChunk = (({ source: _source, ...chunk }) => chunk)(largest);
report.results.initialChunks = initialChunks.map(({ file, bytes, gzipBytes, sha256 }) => ({ file, bytes, gzipBytes, sha256 }));
report.artifact = {
  indexHtmlSha256: createHash("sha256").update(indexHtml).digest("hex"),
  initialGraphDefinition: "index.html module scripts/preloads plus the transitive static ESM import and re-export graph; dynamic import() edges remain on demand",
};
const serializedReport = `${JSON.stringify(report, null, 2)}\n`;
await writeFile(join(distDir, "bundle-report.json"), serializedReport, "utf8");
await mkdir(durableEvidenceDir, { recursive: true });
await writeFile(join(durableEvidenceDir, "production-bundle-report.json"), serializedReport, "utf8");

console.log(
  [
    "bundle budget passed",
    `entry ${(entry.bytes / 1024).toFixed(1)} KiB (${report.results.entryReductionPercent}% below the ${budget.baselineEntryBytes.toLocaleString("en-US")}-byte baseline)`,
    `initial ${(initialBytes / 1024).toFixed(1)} KiB`,
    `largest ${(largest.bytes / 1024).toFixed(1)} KiB`,
    `total ${(totalBytes / 1024).toFixed(1)} KiB across ${javascript.length} chunks`,
    "evidence dist/bundle-report.json and editor-shell/evidence/accessibility/production-bundle-report.json",
  ].join("; "),
);
