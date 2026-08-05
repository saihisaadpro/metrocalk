import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const scriptsDir = path.dirname(fileURLToPath(import.meta.url));
const reportScript = path.join(scriptsDir, "accessibility-report.mjs");
const canonicalMatrixPath = path.resolve(scriptsDir, "../accessibility/manual-at-matrix.json");
const labels = [
  "wide-editor",
  "command-palette",
  "command-palette-empty",
  "reflow-scene-drawer",
  "reflow-inspector-drawer",
];

async function evidenceSet(violationLabel = null) {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), "metrocalk-a11y-report-"));
  const runId = "test-run-2026-07-16";
  const buildId = `sha256:${"a".repeat(64)}`;
  for (const label of labels) {
    const violations = label === violationLabel
      ? [{ id: "button-name", impact: "critical", help: "Buttons must have discernible text", nodes: [] }]
      : [];
    await fs.writeFile(path.join(directory, `${label}.axe.json`), JSON.stringify({
      schemaVersion: 1,
      label,
      runId,
      buildId,
      completedAt: "2026-07-16T00:00:00.000Z",
      tool: { name: "axe-core", version: "test-fixture" },
      results: { violations, incomplete: [], passes: [] },
    }));
    await fs.writeFile(path.join(directory, `${label}.png`), "test screenshot fixture");
  }
  await fs.writeFile(path.join(directory, "run-context.json"), JSON.stringify({
    schemaVersion: 1,
    runId,
    startedAt: "2026-07-16T00:00:00.000Z",
    completedAt: "2026-07-16T00:01:00.000Z",
    buildId,
    audits: labels,
    application: { path: "fixture/metrocalk-editor-shell.exe", bytes: 1234, sha256: "a".repeat(64) },
  }));
  return directory;
}

function runReportWithEnvironment(directory, environment, ...arguments_) {
  return spawnSync(process.execPath, [reportScript, ...arguments_], {
    cwd: path.resolve(scriptsDir, ".."),
    env: { ...process.env, MTK_A11Y_EVIDENCE_DIR: directory, ...environment },
    encoding: "utf8",
    windowsHide: true,
  });
}

function runReport(directory, ...arguments_) {
  return runReportWithEnvironment(directory, {}, ...arguments_);
}

async function writeMatrix(directory, mutate) {
  const matrix = JSON.parse(await fs.readFile(canonicalMatrixPath, "utf8"));
  await mutate(matrix);
  const matrixPath = path.join(directory, "manual-at-matrix.json");
  await fs.writeFile(matrixPath, JSON.stringify(matrix, null, 2));
  return matrixPath;
}

test("clean automated evidence passes while unfinished manual rows remain explicitly not certified", async (context) => {
  const directory = await evidenceSet();
  context.after(() => fs.rm(directory, { recursive: true, force: true }));

  const result = runReport(directory);
  assert.equal(result.status, 0, result.stderr || result.stdout);
  const summary = JSON.parse(await fs.readFile(path.join(directory, "accessibility-summary.json"), "utf8"));
  assert.equal(summary.automated.status, "passed");
  assert.equal(summary.manual.status, "blocked");
  assert.equal(summary.certificationStatus, "not-certified");
  assert.match(summary.claim, /Not certified/);
});

test("an axe violation fails the engineering gate and is counted in the report", async (context) => {
  const directory = await evidenceSet("command-palette");
  context.after(() => fs.rm(directory, { recursive: true, force: true }));

  const result = runReport(directory);
  assert.equal(result.status, 1, result.stderr || result.stdout);
  const summary = JSON.parse(await fs.readFile(path.join(directory, "accessibility-summary.json"), "utf8"));
  assert.equal(summary.automated.status, "failed");
  assert.equal(summary.automated.violationCount, 1);
  assert.equal(summary.certificationStatus, "not-certified");
});

test("an axe check that needs review fails instead of being reported as an automated pass", async (context) => {
  const directory = await evidenceSet();
  context.after(() => fs.rm(directory, { recursive: true, force: true }));
  const auditPath = path.join(directory, "wide-editor.axe.json");
  const audit = JSON.parse(await fs.readFile(auditPath, "utf8"));
  audit.results.incomplete = [{ id: "color-contrast", impact: "serious", nodes: [] }];
  await fs.writeFile(auditPath, JSON.stringify(audit));

  const result = runReport(directory);
  assert.equal(result.status, 1, result.stderr || result.stdout);
  const summary = JSON.parse(await fs.readFile(path.join(directory, "accessibility-summary.json"), "utf8"));
  assert.equal(summary.automated.status, "failed");
  assert.equal(summary.automated.incompleteCount, 1);
});

test("mixed-run evidence is rejected even when every individual axe result is clean", async (context) => {
  const directory = await evidenceSet();
  context.after(() => fs.rm(directory, { recursive: true, force: true }));
  const auditPath = path.join(directory, "command-palette.axe.json");
  const audit = JSON.parse(await fs.readFile(auditPath, "utf8"));
  audit.runId = "stale-other-run";
  await fs.writeFile(auditPath, JSON.stringify(audit));

  const result = runReport(directory);
  assert.equal(result.status, 1, result.stderr || result.stdout);
  const summary = JSON.parse(await fs.readFile(path.join(directory, "accessibility-summary.json"), "utf8"));
  assert.equal(summary.automated.status, "incomplete");
  assert.match(summary.automated.missing.join(" "), /runId does not match/);
});

test("run context build identity must match the recorded executable SHA-256", async (context) => {
  const directory = await evidenceSet();
  context.after(() => fs.rm(directory, { recursive: true, force: true }));
  const contextPath = path.join(directory, "run-context.json");
  const runContext = JSON.parse(await fs.readFile(contextPath, "utf8"));
  runContext.application.sha256 = "b".repeat(64);
  await fs.writeFile(contextPath, JSON.stringify(runContext));

  const result = runReport(directory);
  assert.equal(result.status, 1, result.stderr || result.stdout);
  const summary = JSON.parse(await fs.readFile(path.join(directory, "accessibility-summary.json"), "utf8"));
  assert.equal(summary.automated.status, "incomplete");
  assert.match(summary.automated.contextErrors.join(" "), /buildId does not match application\.sha256/);
});

test("matrix validation rejects a claimed pass without tester, versions, build and evidence", async (context) => {
  const directory = await evidenceSet();
  context.after(() => fs.rm(directory, { recursive: true, force: true }));
  const matrixPath = await writeMatrix(directory, (matrix) => {
    matrix.runs.find((run) => run.id === "nvda-webview2").status = "passed";
  });

  const result = runReportWithEnvironment(directory, { MTK_A11Y_MATRIX_PATH: matrixPath }, "--matrix-only");
  assert.equal(result.status, 1, result.stderr || result.stdout);
  assert.match(result.stderr, /nvda-webview2: 'tester' is required/);
  assert.match(result.stderr, /nvda-webview2: 'assistiveTechnologyVersion' is required/);
  assert.match(result.stderr, /nvda-webview2: at least one evidence reference is required/);
  assert.match(result.stderr, /nvda-webview2: build must use the exact 'sha256:/);
});

test("the strict certification command rejects clean automation while human evidence is pending", async (context) => {
  const directory = await evidenceSet();
  context.after(() => fs.rm(directory, { recursive: true, force: true }));

  const result = runReport(directory, "--require-certified");
  assert.equal(result.status, 2, result.stderr || result.stdout);
  assert.match(result.stderr, /not-certified/);
});

test("the strict gate can certify only complete same-build human evidence and independent attestation", async (context) => {
  const directory = await evidenceSet();
  context.after(() => fs.rm(directory, { recursive: true, force: true }));
  const build = `sha256:${"a".repeat(64)}`;
  const matrixPath = await writeMatrix(directory, async (matrix) => {
    for (const run of matrix.runs.filter((candidate) => candidate.required)) {
      const evidence = `${run.id}.txt`;
      await fs.writeFile(path.join(directory, evidence), `${run.id} fixture`);
      Object.assign(run, {
        status: "passed",
        tester: "Independent fixture tester",
        testedAt: "2026-07-16T00:02:00.000Z",
        build,
        assistiveTechnologyVersion: "fixture-version",
        hostVersion: "fixture-host-version",
        evidence: [evidence],
        notes: "Fixture proving the strict gate's positive path.",
      });
    }
    await fs.writeFile(path.join(directory, "independent-attestation.txt"), "signed fixture report");
    matrix.certification.status = "approved";
    matrix.certification.attestation = {
      status: "approved",
      reviewer: "Fixture reviewer",
      organisation: "Independent fixture organisation",
      approvedAt: "2026-07-16T00:03:00.000Z",
      build,
      report: "independent-attestation.txt",
    };
  });

  const result = runReportWithEnvironment(directory, { MTK_A11Y_MATRIX_PATH: matrixPath }, "--require-certified");
  assert.equal(result.status, 0, result.stderr || result.stdout);
  const summary = JSON.parse(await fs.readFile(path.join(directory, "accessibility-summary.json"), "utf8"));
  assert.equal(summary.automated.status, "passed");
  assert.equal(summary.manual.status, "passed");
  assert.equal(summary.certificationStatus, "certified");
});
