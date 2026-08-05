import fs from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const e2eDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const evidenceDir = process.env.MTK_A11Y_EVIDENCE_DIR
  ? path.resolve(process.env.MTK_A11Y_EVIDENCE_DIR)
  : path.resolve(e2eDir, "../evidence/accessibility");
const matrixPath = process.env.MTK_A11Y_MATRIX_PATH
  ? path.resolve(process.env.MTK_A11Y_MATRIX_PATH)
  : path.join(e2eDir, "accessibility/manual-at-matrix.json");
const matrixOnly = process.argv.includes("--matrix-only");
const requireCertified = process.argv.includes("--require-certified");
const expectedAudits = [
  "wide-editor",
  "command-palette",
  "command-palette-empty",
  "reflow-scene-drawer",
  "reflow-inspector-drawer",
];
const requiredManualRuns = [
  "keyboard-only-webview2",
  "nvda-webview2",
  "jaws-webview2",
  "narrator-webview2",
  "voiceover-webkit",
  "windows-high-contrast",
  "magnifier-reflow",
  "speech-input",
  "reduced-motion",
];

const matrix = JSON.parse(await fs.readFile(matrixPath, "utf8"));
const matrixErrors = [];
const sha256BuildPattern = /^sha256:[a-f0-9]{64}$/;
const sha256DigestPattern = /^[a-f0-9]{64}$/;
const isIsoDate = (value) => typeof value === "string" && !Number.isNaN(Date.parse(value));
const hasText = (value) => typeof value === "string" && value.trim().length > 0;

if (matrix.schemaVersion !== 1) matrixErrors.push("manual matrix schemaVersion must be 1");
if (matrix.target !== "WCAG 2.2 Level AA") matrixErrors.push("manual matrix target must be WCAG 2.2 Level AA");
if (!Array.isArray(matrix.runs)) matrixErrors.push("manual matrix runs must be an array");
if (!Array.isArray(matrix.allowedStatuses) || matrix.allowedStatuses.length === 0) {
  matrixErrors.push("manual matrix allowedStatuses must be a non-empty array");
}
if (!hasText(matrix.scope)) matrixErrors.push("manual matrix scope is required");
if (!hasText(matrix.certification?.policy)) matrixErrors.push("manual matrix certification policy is required");

const runs = Array.isArray(matrix.runs) ? matrix.runs : [];
const allowedStatuses = Array.isArray(matrix.allowedStatuses) ? matrix.allowedStatuses : [];
const ids = new Set();
for (const run of runs) {
  if (!run.id || ids.has(run.id)) matrixErrors.push(`manual run id is missing or duplicated: ${run.id ?? "<missing>"}`);
  ids.add(run.id);
  if (!allowedStatuses.includes(run.status)) matrixErrors.push(`${run.id}: unsupported status '${run.status}'`);
  for (const field of ["platform", "assistiveTechnology", "host"]) {
    if (!hasText(run[field])) matrixErrors.push(`${run.id}: '${field}' is required`);
  }
  if (!Array.isArray(run.criteria) || run.criteria.length === 0) matrixErrors.push(`${run.id}: criteria are required`);
  if (!Array.isArray(run.journeys) || run.journeys.length === 0) matrixErrors.push(`${run.id}: journeys are required`);
  if (run.status === "passed" || run.status === "failed") {
    for (const field of ["tester", "testedAt", "build", "assistiveTechnologyVersion", "hostVersion"]) {
      if (!hasText(run[field])) matrixErrors.push(`${run.id}: '${field}' is required before an executed run can be marked ${run.status}`);
    }
    if (!Array.isArray(run.evidence) || run.evidence.length === 0) {
      matrixErrors.push(`${run.id}: at least one evidence reference is required before an executed run can be marked ${run.status}`);
    } else if (run.evidence.some((reference) => !hasText(reference))) {
      matrixErrors.push(`${run.id}: every evidence reference must be a non-empty path relative to the evidence directory`);
    }
    if (!isIsoDate(run.testedAt)) matrixErrors.push(`${run.id}: testedAt must be an ISO-8601 date`);
    if (!sha256BuildPattern.test(run.build ?? "")) {
      matrixErrors.push(`${run.id}: build must use the exact 'sha256:<64 lowercase hex characters>' candidate identifier`);
    }
  }
  if (run.status === "blocked" && !hasText(run.notes)) matrixErrors.push(`${run.id}: blocked runs require a concrete blocker in notes`);
  if (run.required && run.status === "not-applicable") matrixErrors.push(`${run.id}: a required run cannot be marked not-applicable`);
}
for (const id of requiredManualRuns) {
  const run = runs.find((candidate) => candidate.id === id);
  if (!run) matrixErrors.push(`required manual run '${id}' is missing`);
  else if (!run.required) matrixErrors.push(`manual run '${id}' must remain required`);
}

const requiredRuns = runs.filter((run) => run.required);
const manualStatus = requiredRuns.some((run) => run.status === "failed")
  ? "failed"
  : requiredRuns.some((run) => run.status === "blocked")
    ? "blocked"
  : requiredRuns.length > 0 && requiredRuns.every((run) => run.status === "passed")
    ? "passed"
    : "pending";

if (matrixOnly) {
  if (matrixErrors.length > 0) {
    console.error(`manual accessibility matrix is invalid:\n${matrixErrors.map((error) => `  - ${error}`).join("\n")}`);
    process.exit(1);
  }
  const counts = Object.fromEntries(allowedStatuses.map((status) => [status, runs.filter((run) => run.status === status).length]));
  console.log(`manual accessibility matrix is valid; certification ${matrix.certification.status}; runs ${JSON.stringify(counts)}`);
  process.exit(0);
}

if (matrixErrors.length > 0) {
  console.error(`manual accessibility matrix is invalid:\n${matrixErrors.map((error) => `  - ${error}`).join("\n")}`);
  process.exit(1);
}

let runContext = null;
const contextErrors = [];
try {
  runContext = JSON.parse(await fs.readFile(path.join(evidenceDir, "run-context.json"), "utf8"));
  if (runContext.schemaVersion !== 1) contextErrors.push("run context schemaVersion must be 1");
  if (!hasText(runContext.runId)) contextErrors.push("run context is missing runId");
  if (!isIsoDate(runContext.startedAt) || !isIsoDate(runContext.completedAt)) {
    contextErrors.push("run context requires valid startedAt and completedAt timestamps");
  } else if (Date.parse(runContext.completedAt) < Date.parse(runContext.startedAt)) {
    contextErrors.push("run context completedAt cannot precede startedAt");
  }
  if (!sha256BuildPattern.test(runContext.buildId ?? "")) {
    contextErrors.push("run context buildId must be an exact sha256 candidate identifier");
  }
  if (!hasText(runContext.application?.path) || !Number.isSafeInteger(runContext.application?.bytes) || runContext.application.bytes <= 0) {
    contextErrors.push("run context requires the packaged application path and positive byte size");
  }
  if (!sha256DigestPattern.test(runContext.application?.sha256 ?? "")) {
    contextErrors.push("run context application.sha256 must be 64 lowercase hexadecimal characters");
  } else if (runContext.buildId !== `sha256:${runContext.application.sha256}`) {
    contextErrors.push("run context buildId does not match application.sha256");
  }
  if (!Array.isArray(runContext.audits)) contextErrors.push("run context audits must be an array");
  else {
    const completed = new Set(runContext.audits);
    for (const label of expectedAudits) {
      if (!completed.has(label)) contextErrors.push(`run context did not complete '${label}'`);
    }
  }
} catch (error) {
  contextErrors.push(`run-context.json: ${error instanceof Error ? error.message : String(error)}`);
}

const audits = [];
const missingAudits = [];
for (const label of expectedAudits) {
  const auditPath = path.join(evidenceDir, `${label}.axe.json`);
  try {
    const envelope = JSON.parse(await fs.readFile(auditPath, "utf8"));
    const result = envelope.results ?? envelope;
    if (envelope.schemaVersion !== 1) throw new Error("schemaVersion must be 1");
    if (envelope.label !== label) throw new Error(`envelope label '${envelope.label}' does not match filename`);
    if (!hasText(envelope.runId) || envelope.runId !== runContext?.runId) throw new Error("runId does not match run-context.json");
    if (envelope.buildId !== runContext?.buildId) throw new Error("buildId does not match run-context.json");
    if (!isIsoDate(envelope.completedAt)) throw new Error("completedAt is missing or invalid");
    if (envelope.tool?.name !== "axe-core" || !hasText(envelope.tool?.version)) {
      throw new Error("axe-core tool name/version is missing");
    }
    if (!Array.isArray(result.violations)) throw new Error("violations array is missing");
    if (!Array.isArray(result.incomplete)) throw new Error("incomplete array is missing");
    const incompleteReview = envelope.incompleteReview ?? {
      reviewed: [],
      // Backward-compatible fail-closed handling for older/raw fixtures: every unreviewed axe
      // incomplete remains unresolved until the packaged audit records a measurement.
      unresolved: result.incomplete.flatMap((check) =>
        Array.isArray(check.nodes) && check.nodes.length > 0 ? check.nodes : [check],
      ),
    };
    if (!Array.isArray(incompleteReview.reviewed) || !Array.isArray(incompleteReview.unresolved)) {
      throw new Error("incomplete contrast review is malformed");
    }
    await fs.access(path.join(evidenceDir, `${label}.png`));
    audits.push({
      label,
      file: path.relative(e2eDir, auditPath).replaceAll("\\", "/"),
      completedAt: envelope.completedAt ?? result.timestamp ?? null,
      engine: envelope.tool ?? result.testEngine ?? null,
      violations: result.violations.length,
      incomplete: result.incomplete.length,
      reviewedIncomplete: incompleteReview.reviewed.length,
      unresolvedIncomplete: incompleteReview.unresolved.length,
      passes: Array.isArray(result.passes) ? result.passes.length : null,
    });
  } catch (error) {
    missingAudits.push(`${label}: ${error instanceof Error ? error.message : String(error)}`);
  }
}

const violationCount = audits.reduce((total, audit) => total + audit.violations, 0);
const incompleteCount = audits.reduce((total, audit) => total + audit.incomplete, 0);
const reviewedIncompleteCount = audits.reduce((total, audit) => total + audit.reviewedIncomplete, 0);
const unresolvedIncompleteCount = audits.reduce((total, audit) => total + audit.unresolvedIncomplete, 0);
const automatedStatus = missingAudits.length > 0 || contextErrors.length > 0
  ? "incomplete"
  : violationCount > 0 || unresolvedIncompleteCount > 0
    ? "failed"
    : "passed";
const attestation = matrix.certification.attestation ?? {};
const attestationApproved = attestation.status === "approved"
  && hasText(attestation.reviewer)
  && hasText(attestation.organisation)
  && isIsoDate(attestation.approvedAt)
  && attestation.build === runContext?.buildId
  && hasText(attestation.report);
const attestationErrors = [];
if (attestation.status === "approved") {
  if (!hasText(attestation.reviewer)) attestationErrors.push("approved attestation requires a named reviewer");
  if (!hasText(attestation.organisation)) attestationErrors.push("approved attestation requires the review organisation");
  if (!isIsoDate(attestation.approvedAt)) attestationErrors.push("approved attestation requires an ISO-8601 approvedAt date");
  if (attestation.build !== runContext?.buildId) attestationErrors.push("attestation build does not match the automated candidate SHA-256");
  if (!hasText(attestation.report)) attestationErrors.push("approved attestation requires a report evidence path");
}
const buildMismatches = requiredRuns
  .filter((run) => run.status === "passed" && run.build !== runContext?.buildId)
  .map((run) => `${run.id}: ${run.build ?? "<missing>"}`);
const manualEvidenceErrors = [];
if (buildMismatches.length > 0) manualEvidenceErrors.push(`manual results are not for the automated candidate: ${buildMismatches.join(", ")}`);
if (attestationApproved) {
  try {
    const reportPath = path.resolve(evidenceDir, attestation.report);
    if (!reportPath.startsWith(`${evidenceDir}${path.sep}`)) throw new Error("path leaves the evidence directory");
    await fs.access(reportPath);
  } catch (error) {
    attestationErrors.push(`attestation report is unavailable: ${error instanceof Error ? error.message : String(error)}`);
  }
}
for (const run of requiredRuns.filter((candidate) => candidate.status === "passed")) {
  for (const reference of run.evidence) {
    try {
      const evidencePath = path.resolve(evidenceDir, reference);
      if (!evidencePath.startsWith(`${evidenceDir}${path.sep}`)) throw new Error("path leaves the evidence directory");
      await fs.access(evidencePath);
    } catch (error) {
      manualEvidenceErrors.push(`${run.id} evidence '${reference}' is unavailable: ${error instanceof Error ? error.message : String(error)}`);
    }
  }
}
const certificationStatus = automatedStatus !== "passed" || manualStatus !== "passed" || manualEvidenceErrors.length > 0
  ? "not-certified"
  : attestation.status === "approved" && attestationApproved && attestationErrors.length === 0
    ? "certified"
    : attestation.status === "pending"
      ? "ready-for-independent-review"
      : "not-certified";

const summary = {
  schemaVersion: 1,
  generatedAt: new Date().toISOString(),
  target: matrix.target,
  scope: matrix.scope,
  claim: certificationStatus === "certified"
    ? "Certified for the exact reviewed build identified in the independent attestation."
    : "Not certified. Automated engineering evidence does not replace manual assistive-technology testing and independent review.",
  automated: {
    status: automatedStatus,
    expectedAuditCount: expectedAudits.length,
    completedAuditCount: audits.length,
    violationCount,
    incompleteCount,
    reviewedIncompleteCount,
    unresolvedIncompleteCount,
    missing: missingAudits,
    contextErrors,
    audits,
    runContext,
  },
  manual: {
    status: manualStatus,
    requiredRunCount: requiredRuns.length,
    counts: Object.fromEntries(allowedStatuses.map((status) => [status, runs.filter((run) => run.status === status).length])),
    matrix: path.relative(e2eDir, matrixPath).replaceAll("\\", "/"),
    evidenceErrors: manualEvidenceErrors,
  },
  independentAttestation: attestation,
  attestationErrors,
  releaseBuild: runContext?.buildId ?? null,
  certificationStatus,
};

const markdown = [
  "# Metrocalk accessibility evidence summary",
  "",
  `Generated: ${summary.generatedAt}`,
  "",
  `Target: **${summary.target}**`,
  "",
  `Certification status: **${summary.certificationStatus}**`,
  "",
  summary.claim,
  "",
  "## Automated packaged-app checks",
  "",
  `Status: **${automatedStatus}**; ${audits.length}/${expectedAudits.length} audited states; ${violationCount} axe violations; ${reviewedIncompleteCount} axe ambiguities resolved by recorded contrast measurement; ${unresolvedIncompleteCount} checks still needing review.`,
  "",
  "| State | Violations | Measured axe ambiguities | Unresolved checks | Passed rules | Evidence |",
  "| --- | ---: | ---: | ---: | ---: | --- |",
  ...audits.map((audit) => `| ${audit.label} | ${audit.violations} | ${audit.reviewedIncomplete ?? "unknown"} | ${audit.unresolvedIncomplete ?? "unknown"} | ${audit.passes ?? "unknown"} | ${audit.file} |`),
  ...(missingAudits.length > 0 ? ["", `Missing evidence: ${missingAudits.join("; ")}`] : []),
  ...(contextErrors.length > 0 ? ["", `Run-context errors: ${contextErrors.join("; ")}`] : []),
  "",
  "## Manual assistive-technology matrix",
  "",
  `Status: **${manualStatus}**. Required runs: ${requiredRuns.length}. Results: ${JSON.stringify(summary.manual.counts)}.`,
  "",
  "A named tester, exact AT/WebView2 versions, exact build, date, and evidence references are mandatory for every passing row.",
  "An independent reviewer must then approve the exact build before the status can become certified.",
  ...(manualEvidenceErrors.length > 0 ? ["", `Manual evidence blockers: ${manualEvidenceErrors.join("; ")}`] : []),
  ...(attestationErrors.length > 0 ? ["", `Attestation blockers: ${attestationErrors.join("; ")}`] : []),
  "",
].join("\n");

await fs.mkdir(evidenceDir, { recursive: true });
await fs.writeFile(path.join(evidenceDir, "accessibility-summary.json"), `${JSON.stringify(summary, null, 2)}\n`, "utf8");
await fs.writeFile(path.join(evidenceDir, "accessibility-summary.md"), markdown, "utf8");

if (automatedStatus !== "passed") {
  console.error(`automated accessibility evidence is ${automatedStatus}; see ${path.join(evidenceDir, "accessibility-summary.json")}`);
  process.exit(1);
}
if (requireCertified && certificationStatus !== "certified") {
  console.error(`accessibility certification gate is ${certificationStatus}; all manual rows and independent attestation must be complete`);
  process.exit(2);
}
console.log(`accessibility evidence passed automated gate; certification ${certificationStatus}; report ${path.join(evidenceDir, "accessibility-summary.json")}`);
