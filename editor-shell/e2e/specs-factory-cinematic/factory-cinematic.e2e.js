// Production Skid Weld Line direction pass on the packaged Windows application.
//
// This is intentionally not a synthetic invoke-only import. Both attempts enter through the real Win32
// CF_HDROP/OLE gesture. Attempt one is cancelled after Rust reports the file as importing and must leave
// zero committed CAD. Attempt two imports the same source, then the test searches the 15k-part report,
// authors 24 conservative mechanism tracks, directs 30 Calm shots across 15 named subjects, and saves a
// reusable .mtk package plus a machine-auditable evidence manifest.

import { browser } from "@wdio/globals";
import { execFileSync, spawn } from "node:child_process";
import { createHash } from "node:crypto";
import {
  appendFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  realpathSync,
  statSync,
  writeFileSync,
} from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  buildCalmShotAssignments,
  buildMechanismKeys,
  chooseFilmedSubjects,
  FACTORY_ACCEPTANCE,
  motionProfileFor,
  normalizedPartName,
  selectMechanismParts,
  SEMANTIC_SEARCHES,
} from "../lib/factory-cinematic.js";
import {
  exactNamedChild,
  NATIVE_IMPORT_LIFECYCLE_EVENT,
} from "../lib/native-cad-drop.js";

const e2eDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const evidenceRoot = realpathSync.native(path.resolve(e2eDir, "../evidence/native-cad-drop"));
const configuredRunDir = process.env.MTK_NATIVE_DROP_RUN_DIR;
if (!configuredRunDir) throw new Error("MTK_NATIVE_DROP_RUN_DIR was not supplied by the run-scoped config.");
const runDir = realpathSync.native(configuredRunDir);
if (path.dirname(runDir).toLocaleLowerCase() !== evidenceRoot.toLocaleLowerCase()) {
  throw new Error(`Run evidence directory escaped its fixed root: ${runDir}`);
}

const context = JSON.parse(readFileSync(exactNamedChild(runDir, "run-context.json"), "utf8"));
const fixture = realpathSync.native(context.fixture.path);
const configuredFixture = process.env.MTK_NATIVE_DROP_FIXTURE
  ? realpathSync.native(path.resolve(process.env.MTK_NATIVE_DROP_FIXTURE))
  : null;
if (!configuredFixture || configuredFixture.toLocaleLowerCase() !== fixture.toLocaleLowerCase()) {
  throw new Error(`The preflighted fixture differs from MTK_NATIVE_DROP_FIXTURE: ${fixture}`);
}
if (path.basename(fixture) !== FACTORY_ACCEPTANCE.fixtureBasename) {
  throw new Error(`This acceptance harness is calibrated for ${FACTORY_ACCEPTANCE.fixtureBasename}, got ${path.basename(fixture)}.`);
}
if (statSync(fixture).size < FACTORY_ACCEPTANCE.minimumFixtureBytes) {
  throw new Error(`The production STEP source is unexpectedly small (${statSync(fixture).size} bytes): ${fixture}`);
}

const oleDropScript = realpathSync.native(path.resolve(e2eDir, "scripts/ole-drop-file.ps1"));
const captureScript = realpathSync.native(path.resolve(e2eDir, "scripts/capture-composited-window.ps1"));
const processName = path.basename(context.application.path, path.extname(context.application.path));
const fixtureName = path.basename(fixture);
const cadLog = path.resolve(os.tmpdir(), "mtk-cad-import.log");
const cancelledImportTimeoutMs = 12 * 60_000;
const completedImportTimeoutMs = 20 * 60_000;
const artifacts = [];
const artifactPaths = new Set();

const delay = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));
const sha256 = (file) => createHash("sha256").update(readFileSync(file)).digest("hex");
const invariant = (condition, message) => {
  if (!condition) throw new Error(message);
};
const closeEnough = (left, right, tolerance = 1e-4) => Math.abs(left - right) <= tolerance;

function recordArtifact(file) {
  const absolute = path.resolve(file);
  const relative = path.relative(runDir, absolute);
  invariant(relative && !relative.startsWith("..") && !path.isAbsolute(relative), `Artifact escaped the run directory: ${absolute}`);
  if (artifactPaths.has(relative)) return;
  artifactPaths.add(relative);
  artifacts.push({ file: relative.replaceAll("\\", "/"), bytes: statSync(absolute).size, sha256: sha256(absolute) });
}

function writeJson(name, value) {
  const output = exactNamedChild(runDir, name);
  writeFileSync(output, `${JSON.stringify(value, null, 2)}\n`, { encoding: "utf8", flag: "wx" });
  recordArtifact(output);
  return output;
}

function captureComposited(label, preserveWindow = false) {
  const output = exactNamedChild(runDir, `${label}.png`);
  const args = [
    "-NoLogo",
    "-NoProfile",
    "-NonInteractive",
    "-ExecutionPolicy",
    "Bypass",
    "-File",
    captureScript,
    "-Out",
    output,
    "-ProcName",
    processName,
  ];
  if (preserveWindow) args.push("-PreserveWindow");
  try {
    const stdout = execFileSync("powershell.exe", args, {
      encoding: "utf8",
      timeout: 45_000,
      windowsHide: true,
    });
    appendFileSync(exactNamedChild(runDir, "capture.log"), `[${new Date().toISOString()}] ${label}\n${stdout}\n`, "utf8");
  } catch (error) {
    appendFileSync(
      exactNamedChild(runDir, "capture.log"),
      `[${new Date().toISOString()}] ${label} FAILED\n${error?.stdout ?? ""}\n${error?.stderr ?? ""}\n${String(error)}\n`,
      "utf8",
    );
    throw error;
  }
  recordArtifact(output);
  return output;
}

async function invoke(command, args = {}) {
  const response = await browser.execute(async (name, invokeArgs) => {
    try {
      return { ok: true, value: await window.__TAURI__.core.invoke(name, invokeArgs) };
    } catch (error) {
      return { ok: false, error: String(error) };
    }
  }, command, args);
  if (!response.ok) throw new Error(`${command} failed: ${response.error}`);
  return response.value;
}

async function installLifecycleRecorder() {
  const result = await browser.executeAsync((eventName, done) => {
    const key = "__MTK_FACTORY_CINEMATIC_E2E__";
    const state = { events: [], listenerError: null, unlisten: null };
    window[key] = state;
    window.__TAURI__.event.listen(eventName, (envelope) => {
      state.events.push({
        sequence: state.events.length + 1,
        receivedAt: new Date().toISOString(),
        payload: JSON.parse(JSON.stringify(envelope.payload)),
      });
    }).then((unlisten) => {
      state.unlisten = unlisten;
      done({ ok: true });
    }).catch((error) => {
      state.listenerError = String(error);
      done({ ok: false, error: state.listenerError });
    });
  }, NATIVE_IMPORT_LIFECYCLE_EVENT);
  invariant(result?.ok, `Could not install native import lifecycle recorder: ${result?.error ?? "unknown error"}`);
}

const readLifecycle = () => browser.execute(() => {
  const state = window.__MTK_FACTORY_CINEMATIC_E2E__;
  return { events: state?.events ?? [], listenerError: state?.listenerError ?? null };
});

async function waitForNewDroppedBatch(afterBatchId, timeoutMs = 60_000) {
  let last = null;
  await browser.waitUntil(async () => {
    const recording = await readLifecycle();
    if (recording.listenerError) throw new Error(`Native lifecycle recorder failed: ${recording.listenerError}`);
    const dropped = recording.events
      .filter(({ payload }) => payload?.phase === "dropped" && payload.batchId > afterBatchId)
      .sort((left, right) => left.payload.batchId - right.payload.batchId);
    last = dropped[0] ?? null;
    return !!last;
  }, { timeout: timeoutMs, interval: 150, timeoutMsg: `No dropped batch appeared after batch ${afterBatchId}.` });
  return last.payload.batchId;
}

async function waitForBatchProgress(batchId, stage, timeoutMs = 60_000) {
  let match = null;
  await browser.waitUntil(async () => {
    const recording = await readLifecycle();
    if (recording.listenerError) throw new Error(`Native lifecycle recorder failed: ${recording.listenerError}`);
    match = recording.events.find(({ payload }) =>
      payload?.phase === "progress" && payload.batchId === batchId && payload.stage === stage) ?? null;
    if (match) return true;
    const terminal = recording.events.find(({ payload }) =>
      payload?.batchId === batchId && ["succeeded", "failed", "refused", "cancelled"].includes(payload.phase));
    if (terminal && terminal.payload.phase !== "cancelled") {
      throw new Error(`Batch ${batchId} became ${terminal.payload.phase} before ${stage}: ${JSON.stringify(terminal.payload)}`);
    }
    return false;
  }, { timeout: timeoutMs, interval: 150, timeoutMsg: `Batch ${batchId} never reached ${stage} progress.` });
  return match.payload;
}

async function waitForBatchTerminal(batchId, expectedPhase, timeoutMs) {
  let terminal = null;
  await browser.waitUntil(async () => {
    const recording = await readLifecycle();
    if (recording.listenerError) throw new Error(`Native lifecycle recorder failed: ${recording.listenerError}`);
    const terminals = recording.events.filter(({ payload }) =>
      payload?.batchId === batchId && ["succeeded", "failed", "refused", "cancelled"].includes(payload.phase));
    if (terminals.some(({ payload }) => payload.phase !== expectedPhase)) {
      throw new Error(`Batch ${batchId} reached an unexpected terminal: ${JSON.stringify(terminals.map(({ payload }) => payload))}`);
    }
    terminal = terminals.find(({ payload }) => payload.phase === expectedPhase) ?? null;
    return !!terminal;
  }, { timeout: timeoutMs, interval: 250, timeoutMsg: `Batch ${batchId} did not reach terminal ${expectedPhase}.` });
  return terminal.payload;
}

function validateBatchLifecycle(recording, batchId, expectedPhase) {
  const dropped = recording.events.filter(({ payload }) => payload?.phase === "dropped" && payload.batchId === batchId);
  invariant(dropped.length === 1, `Batch ${batchId} emitted dropped ${dropped.length} times.`);
  const drop = dropped[0];
  invariant(drop.payload.files?.length === 1, `Batch ${batchId} was not a one-file drop: ${JSON.stringify(drop.payload)}`);
  invariant(drop.payload.files[0].name === fixtureName && drop.payload.files[0].supported === true,
    `Batch ${batchId} did not classify ${fixtureName} as supported: ${JSON.stringify(drop.payload)}`);
  const hovered = recording.events
    .filter(({ sequence, payload }) => sequence < drop.sequence && payload?.phase === "hovered")
    .at(-1);
  invariant(hovered?.payload?.files?.some((file) => file.name === fixtureName && file.supported === true),
    `No matching supported hover preceded batch ${batchId}.`);

  const progress = recording.events.filter(({ payload }) => payload?.phase === "progress" && payload.batchId === batchId);
  for (const stage of ["queued", "importing"]) {
    invariant(progress.filter(({ payload }) => payload.stage === stage).length === 1,
      `Batch ${batchId} emitted ${stage} progress an unexpected number of times: ${JSON.stringify(progress)}`);
  }
  const terminals = recording.events.filter(({ payload }) =>
    payload?.batchId === batchId && ["succeeded", "failed", "refused", "cancelled"].includes(payload.phase));
  invariant(terminals.length === 1 && terminals[0].payload.phase === expectedPhase,
    `Batch ${batchId} terminal stream is invalid: ${JSON.stringify(terminals)}`);
  invariant(progress.every(({ payload }) => payload.index === 1 && payload.total === 1 && payload.fileName === fixtureName),
    `Batch ${batchId} progress identity drifted: ${JSON.stringify(progress)}`);
  const terminal = terminals[0];
  invariant(terminal.sequence > progress.find(({ payload }) => payload.stage === "importing").sequence,
    `Batch ${batchId} terminal preceded importing progress.`);
  return {
    batchId,
    terminalPhase: expectedPhase,
    eventRange: [drop.sequence, terminal.sequence],
    progressStages: progress.map(({ payload }) => payload.stage),
    delayedUpdates: progress.filter(({ payload }) => payload.stage === "delayed").length,
    terminal: terminal.payload,
  };
}

function startOleDrop(attempt) {
  const hoverReady = exactNamedChild(runDir, `${attempt}-hover-ready.txt`);
  const release = exactNamedChild(runDir, `${attempt}-release-hover.txt`);
  const args = [
    "-NoLogo",
    "-NoProfile",
    "-NonInteractive",
    "-Sta",
    "-ExecutionPolicy",
    "Bypass",
    "-File",
    oleDropScript,
    "-Files",
    fixture,
    "-WindowTitleLike",
    "metrocalk",
    "-DropX",
    "0.55",
    "-DropY",
    "0.55",
    "-StartX",
    "0.90",
    "-StartY",
    "0.14",
    "-TimeoutSeconds",
    "90",
    "-HoverReadyPath",
    hoverReady,
    "-ReleasePath",
    release,
    "-HoverTimeoutSeconds",
    "60",
  ];
  const child = spawn("powershell.exe", args, { stdio: ["ignore", "pipe", "pipe"], windowsHide: true });
  const handle = { attempt, child, hoverReady, release, stdout: "", stderr: "", result: null, completion: null };
  child.stdout.on("data", (chunk) => { handle.stdout += chunk.toString(); });
  child.stderr.on("data", (chunk) => { handle.stderr += chunk.toString(); });
  handle.completion = new Promise((resolve) => {
    let settled = false;
    const finish = (result) => {
      if (settled) return;
      settled = true;
      handle.result = result;
      resolve(result);
    };
    child.once("error", (error) => finish({ code: null, signal: null, error: String(error) }));
    child.once("close", (code, signal) => finish({ code, signal, error: null }));
  });
  return handle;
}

async function waitForHoverBarrier(handle, timeoutMs = 30_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (existsSync(handle.hoverReady)) return readFileSync(handle.hoverReady, "utf8");
    if (handle.result) throw new Error(`OLE ${handle.attempt} exited before held hover: ${JSON.stringify(handle.result)}\n${handle.stdout}\n${handle.stderr}`);
    await delay(100);
  }
  throw new Error(`OLE ${handle.attempt} did not reach held hover in ${timeoutMs}ms.`);
}

function releaseOleDrop(handle, reason = "authorized-release") {
  if (!existsSync(handle.release)) {
    writeFileSync(handle.release, `${reason}=${new Date().toISOString()}\n`, { encoding: "utf8", flag: "wx" });
  }
}

async function waitForDropExit(handle, timeoutMs = 45_000) {
  let timer;
  const timeout = new Promise((resolve) => { timer = setTimeout(() => resolve({ timeout: true }), timeoutMs); });
  const result = await Promise.race([handle.completion, timeout]);
  clearTimeout(timer);
  if (result?.timeout) {
    handle.child.kill();
    throw new Error(`OLE ${handle.attempt} timed out after ${timeoutMs}ms and was terminated.`);
  }
  return result;
}

function persistOleLog(handle) {
  const output = exactNamedChild(runDir, `${handle.attempt}-ole-drop.log`);
  if (!existsSync(output)) {
    writeFileSync(
      output,
      `result=${JSON.stringify(handle.result)}\n--- stdout ---\n${handle.stdout}\n--- stderr ---\n${handle.stderr}\n`,
      { encoding: "utf8", flag: "wx" },
    );
    recordArtifact(output);
  }
}

function assertOleAccepted(handle) {
  invariant(!handle.result?.error && handle.result?.code === 0,
    `OLE ${handle.attempt} failed: ${JSON.stringify(handle.result)}\n${handle.stdout}\n${handle.stderr}`);
  invariant(/DROP_RESULT:\s+OK\s+os-accepted/i.test(handle.stdout),
    `OLE ${handle.attempt} did not prove OS acceptance:\n${handle.stdout}`);
  invariant(/DROP_GESTURE:\s+released/i.test(handle.stdout),
    `OLE ${handle.attempt} did not prove held-hover release:\n${handle.stdout}`);
}

function preserveCadLog(fromOffset) {
  const output = exactNamedChild(runDir, "cad-import.log");
  if (!existsSync(output)) {
    const bytes = existsSync(cadLog) ? readFileSync(cadLog) : Buffer.alloc(0);
    const start = bytes.length >= fromOffset ? fromOffset : 0;
    writeFileSync(output, bytes.subarray(start), { flag: "wx" });
    recordArtifact(output);
  }
  return readFileSync(output, "utf8");
}

async function waitForAuthoritativeEmpty() {
  let last = null;
  await browser.waitUntil(async () => {
    last = await browser.execute(async () => {
      if (!window.__TAURI__?.core) return { ready: false, report: null, hierarchyRows: null, error: null };
      try {
        const report = await window.__TAURI__.core.invoke("cad_report");
        return {
          ready: true,
          report,
          hierarchyRows: document.querySelectorAll('[data-testid="hrow"]').length,
          error: null,
        };
      } catch (error) {
        return {
          ready: true,
          report: null,
          hierarchyRows: document.querySelectorAll('[data-testid="hrow"]').length,
          error: String(error),
        };
      }
    });
    return last.report?.total === 0 && last.hierarchyRows === 0;
  }, {
    timeout: 45_000,
    interval: 300,
    timeoutMsg: `The clean app did not expose an empty authoritative report and hierarchy: ${JSON.stringify(last)}`,
  });
  return last;
}

function reportAccounting(report) {
  return report.exactBrep
    + report.tessellationOnly
    + report.aiReconstructed
    + report.proxy
    + report.accessDenied
    + report.failed;
}

function assertFactoryReport(report) {
  const expected = FACTORY_ACCEPTANCE.report;
  for (const field of Object.keys(expected)) {
    invariant(report[field] === expected[field],
      `Factory CAD report ${field} expected ${expected[field]}, got ${report[field]}: ${JSON.stringify(report)}`);
  }
  invariant(reportAccounting(report) === report.total,
    `Factory CAD fidelity accounting does not equal total: ${JSON.stringify(report)}`);
}

async function fetchSemanticCandidates() {
  const limit = 50;
  const maxPagesPerQuery = 6;
  const candidateMap = new Map();
  const searches = [];
  let exercisedSecondPage = false;
  for (const query of SEMANTIC_SEARCHES) {
    const pages = [];
    const pageIds = new Set();
    let offset = 0;
    let total = null;
    for (let pageIndex = 0; pageIndex < maxPagesPerQuery; pageIndex += 1) {
      const page = await invoke("cad_report_page", { query, offset, limit });
      invariant(reportAccounting(page) === page.total,
        `Search ${query} has incoherent fidelity totals: ${JSON.stringify(page)}`);
      if (total == null) total = page.total;
      invariant(page.total === total, `Search ${query} total changed during deterministic paging (${total} -> ${page.total}).`);
      invariant(Array.isArray(page.parts) && page.parts.length <= limit, `Search ${query} exceeded page limit ${limit}.`);
      for (const part of page.parts) {
        invariant(String(part.name).toLocaleLowerCase().includes(query.toLocaleLowerCase()),
          `cad_report_page returned a non-matching ${query} row: ${JSON.stringify(part)}`);
        invariant(!pageIds.has(part.id), `Search ${query} repeated ${part.id} across pages.`);
        pageIds.add(part.id);
        const existing = candidateMap.get(part.id);
        candidateMap.set(part.id, existing
          ? { ...existing, matchedQueries: [...new Set([...existing.matchedQueries, query])] }
          : { ...part, matchedQueries: [query] });
      }
      pages.push({ offset, rows: page.parts.length, total: page.total });
      if (offset > 0) exercisedSecondPage = true;
      offset += page.parts.length;
      if (page.parts.length === 0 || offset >= page.total) break;
    }
    searches.push({ query, total: total ?? 0, pages, collected: pageIds.size, truncated: pageIds.size < (total ?? 0) });
  }
  if (!exercisedSecondPage) {
    // Keep pagination an explicit contract even if a future source happens to have fewer than 51 rows for
    // every individual search term. The offset-one probe must return the second deterministic result.
    const probeTarget = searches.find(({ total }) => total > 1);
    invariant(probeTarget, `No semantic factory query returned enough rows to exercise paging: ${JSON.stringify(searches)}`);
    const probe = await invoke("cad_report_page", { query: probeTarget.query, offset: 1, limit: 1 });
    invariant(probe.total === probeTarget.total && probe.parts.length === 1,
      `Offset paging probe failed for ${probeTarget.query}: ${JSON.stringify(probe)}`);
    probeTarget.paginationProbe = { offset: 1, rows: probe.parts.length, id: probe.parts[0].id };
    exercisedSecondPage = true;
  }
  invariant(exercisedSecondPage, `The production report did not exercise a non-zero paging offset: ${JSON.stringify(searches)}`);
  const candidates = [...candidateMap.values()];
  invariant(candidates.length >= FACTORY_ACCEPTANCE.animationTracks,
    `Semantic CAD searches yielded only ${candidates.length} distinct parts: ${JSON.stringify(searches)}`);
  return { searches, candidates };
}

async function saveThumbnails(parts) {
  const directory = exactNamedChild(runDir, "semantic-thumbnails");
  mkdirSync(directory);
  const rows = [];
  for (const [index, part] of parts.entries()) {
    const dataUrl = await invoke("thumbnail", { id: part.id, size: 384 });
    if (typeof dataUrl !== "string" || !dataUrl.startsWith("data:image/png;base64,")) {
      rows.push({ id: part.id, name: part.name, file: null, reason: "native thumbnail unavailable" });
      continue;
    }
    const slug = normalizedPartName(part).replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "").slice(0, 48) || "part";
    const name = `${String(index + 1).padStart(2, "0")}-${slug}.png`;
    const output = exactNamedChild(directory, name);
    writeFileSync(output, Buffer.from(dataUrl.slice("data:image/png;base64,".length), "base64"), { flag: "wx" });
    recordArtifact(output);
    rows.push({ id: part.id, name: part.name, file: path.relative(runDir, output).replaceAll("\\", "/") });
  }
  invariant(rows.filter(({ file }) => file).length >= FACTORY_ACCEPTANCE.filmedSubjects,
    `Fewer than ${FACTORY_ACCEPTANCE.filmedSubjects} selected mechanisms produced live thumbnails: ${JSON.stringify(rows)}`);
  return rows;
}

async function authorMechanismTrack(part, index) {
  const transform = await invoke("read_transform", { id: part.id });
  invariant(Array.isArray(transform) && transform.length === 8 && transform.every(Number.isFinite),
    `Part ${part.name} returned a non-finite transform: ${JSON.stringify(transform)}`);
  const profile = motionProfileFor(part);
  const pivot = transform.slice(0, 3);
  const limit = profile.amplitude * 1.15;
  const set = await invoke("set_joint", {
    id: part.id,
    revolute: profile.revolute,
    axis: profile.axis,
    pivot,
    min: -limit,
    max: limit,
    source: "manual",
  });
  invariant(set === true, `set_joint refused ${part.name} (${part.id}) with ${JSON.stringify(profile)}.`);

  const keys = buildMechanismKeys(profile.amplitude, index);
  for (const key of keys) {
    invariant(await invoke("joint_value", { id: part.id, value: key.value, commit: true }) === true,
      `joint_value refused ${part.name} at t=${key.t}, value=${key.value}.`);
    invariant(await invoke("joint_key", { id: part.id, t: key.t }) === true,
      `joint_key refused ${part.name} at t=${key.t}.`);
  }
  const jointInfo = await invoke("joint_info", { id: part.id });
  invariant(jointInfo?.keys === keys.length && closeEnough(jointInfo.trackEnd, 12),
    `Joint readback for ${part.name} does not prove the seven-key 12s loop: ${JSON.stringify(jointInfo)}`);
  invariant(jointInfo.source === "manual" && jointInfo.jointType === profile.motion,
    `Joint readback for ${part.name} changed type/source: ${JSON.stringify(jointInfo)}`);
  invariant(closeEnough(jointInfo.value, 0), `The closed-loop track did not return ${part.name} to zero: ${JSON.stringify(jointInfo)}`);
  const partDebug = await invoke("part_debug", { id: part.id });
  invariant(Array.isArray(partDebug) && partDebug[3] === true, `Authored mechanism is inactive: ${part.name} ${JSON.stringify(partDebug)}`);
  return {
    id: part.id,
    name: part.name,
    fidelity: part.fidelity,
    mechanismFamily: part.mechanismFamily,
    matchedQueries: part.matchedQueries,
    transform,
    profile: { ...profile, pivot, limits: [-limit, limit] },
    keys,
    jointInfo,
    partDebug,
  };
}

async function authorCinematics(subjects) {
  const catalog = await invoke("cinema_catalog");
  const knownKinds = new Set(catalog.map(({ kind }) => kind));
  const orderedSubjects = [...subjects].sort((left, right) => left.id.localeCompare(right.id));
  const assignments = buildCalmShotAssignments(orderedSubjects);
  const directed = [];
  for (const assignment of assignments) {
    invariant(assignment.kinds.every((kind) => knownKinds.has(kind)),
      `The direction references a shot kind missing from the live catalogue: ${JSON.stringify(assignment)}`);
    const mood = await invoke("cinema_set_mood", { id: assignment.id, mood: "calm" });
    invariant(mood.reason == null && mood.mood === "calm", `Could not set Calm pacing on ${assignment.name}: ${JSON.stringify(mood)}`);
    const replies = [];
    for (const kind of assignment.kinds) {
      const reply = await invoke("cinema_add_shot", { id: assignment.id, kind });
      invariant(reply.reason == null, `Shot ${kind} was refused for ${assignment.name}: ${JSON.stringify(reply)}`);
      replies.push(reply);
    }
    const readback = await invoke("cinema_list", { id: assignment.id });
    invariant(readback.shots === assignment.kinds.length && readback.mood === "calm",
      `Cinema readback drifted for ${assignment.name}: ${JSON.stringify(readback)}`);
    invariant(closeEnough(readback.seconds, assignment.plannedSeconds, 0.02),
      `Calm duration drifted for ${assignment.name}: planned ${assignment.plannedSeconds}, live ${readback.seconds}.`);
    invariant(readback.problems.length === 0,
      `Continuity/pacing diagnostics rejected the direction for ${assignment.name}: ${JSON.stringify(readback.problems)}`);
    directed.push({ ...assignment, replies, readback });
  }
  const totalShots = directed.reduce((count, cutscene) => count + cutscene.readback.shots, 0);
  const totalSeconds = directed.reduce((seconds, cutscene) => seconds + cutscene.readback.seconds, 0);
  invariant(directed.length >= FACTORY_ACCEPTANCE.filmedSubjects,
    `Only ${directed.length} subjects received direction.`);
  invariant(totalShots >= FACTORY_ACCEPTANCE.cinematicShots,
    `Only ${totalShots} Calm shots were authored.`);
  invariant(totalSeconds >= FACTORY_ACCEPTANCE.minimumCinematicSeconds,
    `The authored film is only ${totalSeconds}s; three minutes are required.`);
  return { catalog, cutscenes: directed, totalShots, totalSeconds };
}

async function dismissImportOverlay() {
  await browser.execute(() => {
    const overlay = document.querySelector('[data-testid="native-import-overlay"]');
    const dismiss = [...(overlay?.querySelectorAll("button") ?? [])]
      .find((button) => button.textContent?.trim() === "Dismiss");
    dismiss?.click();
  });
}

async function prepareCinematicStage() {
  await dismissImportOverlay();
  await browser.execute(() => {
    const clickNamed = (label) => {
      const control = [...document.querySelectorAll("button")].find((button) => button.getAttribute("aria-label") === label);
      control?.click();
    };
    clickNamed("Collapse left dock");
    clickNamed("Collapse Inspector dock");
    clickNamed("Collapse task workspace");
    clickNamed("Collapse viewport tool names");
  });
  await browser.pause(600);
}

describe("production factory cinematic direction", () => {
  it("cancels cleanly, retries the real STEP, authors 24 mechanisms and a 3+ minute 15-subject film", async () => {
    const manifest = {
      schema: "metrocalk.factory-cinematic.evidence.v1",
      runId: context.runId,
      startedAt: new Date().toISOString(),
      status: "running",
      constitution: "Engine UI/UX Architecture Constitution",
      fixture: context.fixture,
      acceptance: FACTORY_ACCEPTANCE,
      cleanState: null,
      cancellation: null,
      retryImport: null,
      report: null,
      semanticSearch: null,
      selectedParts: null,
      thumbnails: null,
      animation: null,
      cinematics: null,
      presentation: null,
      savedProject: null,
      preview: null,
      artifacts,
      cleanupErrors: [],
    };
    const dropHandles = [];
    let failure = null;
    let lifecycleRecording = { events: [], listenerError: null };
    let playing = false;
    let inFlightBatchId = null;
    const cadLogOffset = existsSync(cadLog) ? statSync(cadLog).size : 0;

    try {
      await browser.waitUntil(
        async () => browser.execute(() => !!window.__TAURI__?.core && !!window.__TAURI__?.event),
        { timeout: 45_000, interval: 200, timeoutMsg: "Tauri core/event globals never appeared." },
      );
      await browser.execute(() => {
        localStorage.setItem("mtk.onboarded.v1", "1");
        for (const key of [
          "metrocalk:shell-layout:v1:left-collapsed",
          "metrocalk:shell-layout:v1:right-collapsed",
          "metrocalk:shell-layout:v1:tool-rail-minimized",
        ]) localStorage.removeItem(key);
      });
      await browser.refresh();
      manifest.cleanState = await waitForAuthoritativeEmpty();
      captureComposited("00-clean-production-shell");
      await installLifecycleRecorder();

      // Attempt 1: real OS gesture, then direct cancellation while the engine owns the long parse.
      const cancelledDrop = startOleDrop("01-cancel-attempt");
      dropHandles.push(cancelledDrop);
      manifest.cancellation = { hoverBarrier: await waitForHoverBarrier(cancelledDrop) };
      await delay(700);
      captureComposited("01-cancel-held-hover", true);
      releaseOleDrop(cancelledDrop);
      cancelledDrop.result = await waitForDropExit(cancelledDrop);
      persistOleLog(cancelledDrop);
      assertOleAccepted(cancelledDrop);
      const cancelledBatchId = await waitForNewDroppedBatch(0);
      inFlightBatchId = cancelledBatchId;
      const importing = await waitForBatchProgress(cancelledBatchId, "importing");
      const accepted = await invoke("cancel_native_import", { batchId: cancelledBatchId });
      invariant(accepted === true, `The direct cancellation plane did not accept batch ${cancelledBatchId}.`);
      captureComposited("02-first-cancellation-requested");
      const cancelled = await waitForBatchTerminal(cancelledBatchId, "cancelled", cancelledImportTimeoutMs);
      inFlightBatchId = null;
      invariant(/safe checkpoint/i.test(cancelled.message) && /no scene changes were committed/i.test(cancelled.message),
        `Cancellation terminal was not truthful: ${JSON.stringify(cancelled)}`);
      await browser.waitUntil(async () => {
        const report = await invoke("cad_report");
        return report.total === 0;
      }, { timeout: 60_000, interval: 300, timeoutMsg: "Cancelled production import left committed CAD." });
      const postCancelReport = await invoke("cad_report");
      const postCancelRows = await browser.execute(() => document.querySelectorAll('[data-testid="hrow"]').length);
      invariant(postCancelReport.total === 0 && postCancelRows === 0,
        `Cancellation was not atomic: report=${JSON.stringify(postCancelReport)}, hierarchyRows=${postCancelRows}.`);
      captureComposited("03-cancelled-zero-commit");
      manifest.cancellation = {
        ...manifest.cancellation,
        batchId: cancelledBatchId,
        importing,
        accepted,
        terminal: cancelled,
        zeroCommit: { report: postCancelReport, hierarchyRows: postCancelRows },
      };

      // Attempt 2: the exact same real OLE path, now allowed to complete.
      const retryDrop = startOleDrop("02-retry-attempt");
      dropHandles.push(retryDrop);
      manifest.retryImport = { hoverBarrier: await waitForHoverBarrier(retryDrop) };
      await delay(700);
      captureComposited("04-retry-held-hover", true);
      releaseOleDrop(retryDrop);
      retryDrop.result = await waitForDropExit(retryDrop);
      persistOleLog(retryDrop);
      assertOleAccepted(retryDrop);
      const retryBatchId = await waitForNewDroppedBatch(cancelledBatchId);
      inFlightBatchId = retryBatchId;
      await waitForBatchProgress(retryBatchId, "importing");
      captureComposited("05-retry-import-in-progress");
      const succeeded = await waitForBatchTerminal(retryBatchId, "succeeded", completedImportTimeoutMs);
      inFlightBatchId = null;
      invariant(succeeded.subject?.kind === "entity" && typeof succeeded.subject.rootId === "string",
        `Successful factory import did not return its wrapper entity: ${JSON.stringify(succeeded)}`);

      const report = await invoke("cad_report");
      assertFactoryReport(report);
      manifest.report = {
        ...Object.fromEntries(Object.keys(FACTORY_ACCEPTANCE.report).map((field) => [field, report[field]])),
        accounted: reportAccounting(report),
        defaultPageRows: report.parts.length,
      };
      manifest.retryImport = { ...manifest.retryImport, batchId: retryBatchId, terminal: succeeded, rootId: succeeded.subject.rootId };

      const profile = await invoke("set_render_profile", { profile: "cinematic" });
      const workingSpace = await invoke("set_working_space", { space: "acescg" });
      const exposure = await invoke("set_exposure", { exposure: 1.0 });
      const environment = await invoke("reset_environment");
      invariant(profile === "cinematic" && workingSpace === "acescg" && closeEnough(exposure, 1),
        `The cinematic presentation did not apply: ${JSON.stringify({ profile, workingSpace, exposure })}`);
      await invoke("view_preset", { preset: "persp" });
      await invoke("frame_all");
      await browser.pause(1_000);
      manifest.presentation = { profile, workingSpace, exposure, environment };
      captureComposited("06-imported-factory-overview");

      const reportClicked = await browser.execute(() => {
        const overlay = document.querySelector('[data-testid="native-import-overlay"]');
        const button = [...(overlay?.querySelectorAll("button") ?? [])]
          .find((candidate) => candidate.textContent?.trim() === "Import report");
        button?.click();
        return !!button;
      });
      invariant(reportClicked, "Succeeded production overlay did not expose Import report.");
      await browser.waitUntil(
        async () => browser.execute((total) =>
          document.querySelector('[data-testid="import-report"]')?.getAttribute("data-total") === String(total), report.total),
        { timeout: 30_000, interval: 300, timeoutMsg: "Import workspace did not expose the 15,711-part report." },
      );
      captureComposited("07-production-fidelity-report");

      const semantic = await fetchSemanticCandidates();
      manifest.semanticSearch = semantic.searches;
      writeJson("semantic-search.json", { searches: semantic.searches, candidates: semantic.candidates });
      const selected = selectMechanismParts(semantic.candidates);
      invariant(selected.length === FACTORY_ACCEPTANCE.animationTracks,
        `Director selection found ${selected.length}, expected ${FACTORY_ACCEPTANCE.animationTracks}: ${JSON.stringify(selected)}`);
      invariant(new Set(selected.map(({ id }) => id)).size === FACTORY_ACCEPTANCE.animationTracks,
        "Mechanism selection contains duplicate entity ids.");
      invariant(new Set(selected.map(normalizedPartName)).size >= FACTORY_ACCEPTANCE.filmedSubjects,
        `Mechanism selection contains fewer than ${FACTORY_ACCEPTANCE.filmedSubjects} distinct readable names.`);
      manifest.selectedParts = selected;
      manifest.thumbnails = await saveThumbnails(selected);

      const tracks = [];
      for (const [index, part] of selected.entries()) tracks.push(await authorMechanismTrack(part, index));
      const animationState = await invoke("animation_state", { id: null });
      const selectedIds = new Set(selected.map(({ id }) => id));
      const authoredTracks = animationState.tracks.filter(({ targetId }) => selectedIds.has(targetId));
      invariant(new Set(authoredTracks.map(({ targetId }) => targetId)).size >= FACTORY_ACCEPTANCE.animationTracks,
        `Animation readback proves only ${new Set(authoredTracks.map(({ targetId }) => targetId)).size} selected targets: ${JSON.stringify(authoredTracks)}`);
      invariant(authoredTracks.every((track) => track.keys.length === 7 && track.runtimeSink === "kinematic-joint"),
        `A mechanism track is not a seven-key native kinematic binding: ${JSON.stringify(authoredTracks)}`);
      manifest.animation = {
        trackCount: tracks.length,
        distinctTargets: new Set(tracks.map(({ id }) => id)).size,
        keyframeCount: tracks.reduce((count, track) => count + track.keys.length, 0),
        loopSeconds: 12,
        tracks,
        workspace: {
          revision: animationState.revision,
          durationTick: animationState.durationTick,
          ticksPerSecond: animationState.ticksPerSecond,
          totalTracks: animationState.tracks.length,
          selectedTrackReadbacks: authoredTracks,
          issues: animationState.issues,
        },
      };

      const subjects = chooseFilmedSubjects(selected);
      invariant(subjects.length === FACTORY_ACCEPTANCE.filmedSubjects, `Only ${subjects.length} named animated subjects were filmable.`);
      await invoke("focus_entity", { id: subjects[0].id });
      await browser.pause(600);
      captureComposited("08-featured-mechanism-neutral");
      const posed = await invoke("joint_scrub", { t: 5.8 });
      invariant(posed >= FACTORY_ACCEPTANCE.animationTracks,
        `Scrubbing the shared timeline posed ${posed} mechanisms, expected at least ${FACTORY_ACCEPTANCE.animationTracks}.`);
      captureComposited("09-featured-mechanism-posed");
      await invoke("joint_scrub", { t: -1 });
      await invoke("unfocus");
      manifest.animation.scrubProof = { timeSeconds: 5.8, posedEntities: posed };

      const cinematics = await authorCinematics(subjects);
      manifest.cinematics = {
        subjectCount: cinematics.cutscenes.length,
        distinctAnimatedSubjectIds: new Set(cinematics.cutscenes.map(({ id }) => id)).size,
        totalShots: cinematics.totalShots,
        totalSeconds: cinematics.totalSeconds,
        minimumClipSeconds: FACTORY_ACCEPTANCE.minimumCinematicSeconds,
        recommendedCaptureSeconds: Math.ceil(cinematics.totalSeconds) + 1,
        transitionPolicy: "Calm 0.9s blends within each two-shot subject sequence; deterministic hard cuts between named subjects",
        runtimeOrder: cinematics.cutscenes.map(({ id, name, mood, kinds, plannedSeconds, readback }) => ({
          id,
          name,
          mood,
          kinds,
          plannedSeconds,
          liveSeconds: readback.seconds,
          reads: readback.reads,
          problems: readback.problems,
        })),
        catalogKinds: cinematics.catalog.map(({ kind }) => kind),
      };

      const projectFile = exactNamedChild(runDir, "skid-weld-line-24-track-30-shot-cinematic.mtk");
      const planFile = writeJson("factory-cinematic-plan.json", {
        source: context.fixture,
        projectFile,
        presentation: manifest.presentation,
        importReport: manifest.report,
        animation: manifest.animation,
        cinematics: manifest.cinematics,
      });
      const saved = await invoke("save_project", { path: projectFile });
      invariant(saved.error == null && saved.dirty === false && typeof saved.path === "string",
        `Saving the directed factory project failed: ${JSON.stringify(saved)}`);
      invariant(path.resolve(saved.path).toLocaleLowerCase() === projectFile.toLocaleLowerCase(),
        `Project saved to an unexpected path: ${JSON.stringify(saved)}`);
      invariant(existsSync(projectFile) && statSync(projectFile).size > 0, `Saved project is missing or empty: ${projectFile}`);
      recordArtifact(projectFile);
      const presentationSidecar = `${projectFile}.view.json`;
      invariant(existsSync(presentationSidecar), `The ACEScg cinematic presentation sidecar was not saved: ${presentationSidecar}`);
      recordArtifact(presentationSidecar);
      manifest.savedProject = {
        ...saved,
        file: path.relative(runDir, projectFile).replaceAll("\\", "/"),
        bytes: statSync(projectFile).size,
        sha256: sha256(projectFile),
        presentationSidecar: path.relative(runDir, presentationSidecar).replaceAll("\\", "/"),
        directorPlan: path.relative(runDir, planFile).replaceAll("\\", "/"),
      };
      captureComposited("10-directed-project-saved");

      await prepareCinematicStage();
      const play = await invoke("play");
      invariant(play.playing === true, `The directed factory did not enter Play: ${JSON.stringify(play)}`);
      playing = true;
      await browser.waitUntil(async () => (await invoke("camera_probe")).cinematic === true,
        { timeout: 15_000, interval: 150, timeoutMsg: "No authored cutscene took camera authority." });
      const openingCamera = await invoke("camera_probe");
      const openingAnimation = await invoke("animation_state", { id: null });
      invariant(openingAnimation.playing === true && openingAnimation.loopPolicy === "loop",
        `The mechanism timeline is not looping during Play: ${JSON.stringify(openingAnimation)}`);
      captureComposited("11-live-cinematic-opening");
      await browser.pause(2_200);
      const movingCamera = await invoke("camera_probe");
      const movingAnimation = await invoke("animation_state", { id: null });
      const cameraTravel = Math.hypot(...openingCamera.eye.map((value, index) => value - movingCamera.eye[index]));
      invariant(openingCamera.cinematic && movingCamera.cinematic && cameraTravel > 0.01,
        `The opening Hero camera did not visibly move: ${JSON.stringify({ openingCamera, movingCamera, cameraTravel })}`);
      invariant(movingAnimation.currentTick !== openingAnimation.currentTick,
        `The 24-track mechanism timeline did not advance: ${JSON.stringify({ openingAnimation, movingAnimation })}`);
      captureComposited("12-live-cinematic-mechanisms-moving");
      manifest.preview = { play, openingCamera, movingCamera, cameraTravel, openingAnimation, movingAnimation };
      await invoke("stop");
      playing = false;
      const stoppedCamera = await invoke("camera_probe");
      const projectState = await invoke("project_state");
      invariant(stoppedCamera.cinematic === false && projectState.dirty === false,
        `Stop did not restore the authored edit state: ${JSON.stringify({ stoppedCamera, projectState })}`);
      manifest.preview.stoppedCamera = stoppedCamera;
      manifest.preview.projectState = projectState;

      lifecycleRecording = await readLifecycle();
      manifest.cancellation.lifecycle = validateBatchLifecycle(lifecycleRecording, manifest.cancellation.batchId, "cancelled");
      manifest.retryImport.lifecycle = validateBatchLifecycle(lifecycleRecording, manifest.retryImport.batchId, "succeeded");
      const cadLogText = preserveCadLog(cadLogOffset);
      invariant(cadLogText.includes(fixtureName) && /CAD_IMPORT_METRICS/.test(cadLogText) && /CAD import commit OK/.test(cadLogText),
        `Run-scoped CAD log lacks retry source, metrics, or commit proof:\n${cadLogText}`);
      invariant(/CAD import cancelled/i.test(cadLogText) && /no scene changes were committed/i.test(cadLogText),
        `Run-scoped CAD log lacks atomic cancellation proof:\n${cadLogText}`);
      manifest.status = "passed";
    } catch (error) {
      failure = error;
      manifest.status = "failed";
      manifest.error = error instanceof Error
        ? { name: error.name, message: error.message, stack: error.stack }
        : { message: String(error) };
      try {
        captureComposited("99-failure-state", true);
      } catch (captureError) {
        manifest.cleanupErrors.push(`Failure capture: ${String(captureError)}`);
      }
    } finally {
      if (inFlightBatchId != null) {
        try {
          await invoke("cancel_native_import", { batchId: inFlightBatchId });
        } catch (error) {
          manifest.cleanupErrors.push(`In-flight import cancellation cleanup: ${String(error)}`);
        }
      }
      if (playing) {
        try {
          await invoke("stop");
        } catch (error) {
          manifest.cleanupErrors.push(`Stop cleanup: ${String(error)}`);
        }
      }
      try {
        await invoke("joint_scrub", { t: -1 });
        await invoke("unfocus");
      } catch (error) {
        manifest.cleanupErrors.push(`Preview cleanup: ${String(error)}`);
      }
      for (const handle of dropHandles) {
        try {
          if (!handle.result) {
            releaseOleDrop(handle, "cleanup-release");
            handle.result = await waitForDropExit(handle, 15_000);
          }
          persistOleLog(handle);
        } catch (error) {
          manifest.cleanupErrors.push(`OLE ${handle.attempt} cleanup: ${String(error)}`);
        }
      }
      try {
        lifecycleRecording = (await readLifecycle()) ?? lifecycleRecording;
        writeJson("native-import-lifecycle.json", lifecycleRecording);
        await browser.execute(async () => {
          const state = window.__MTK_FACTORY_CINEMATIC_E2E__;
          if (state?.unlisten) await state.unlisten();
        });
      } catch (error) {
        manifest.cleanupErrors.push(`Lifecycle evidence cleanup: ${String(error)}`);
      }
      try {
        preserveCadLog(cadLogOffset);
      } catch (error) {
        manifest.cleanupErrors.push(`CAD log preservation: ${String(error)}`);
      }
      const captureLog = exactNamedChild(runDir, "capture.log");
      if (existsSync(captureLog)) recordArtifact(captureLog);
      manifest.finishedAt = new Date().toISOString();
      manifest.artifacts = artifacts;
      const evidencePath = exactNamedChild(runDir, "evidence.json");
      writeFileSync(evidencePath, `${JSON.stringify(manifest, null, 2)}\n`, { encoding: "utf8", flag: "wx" });
    }

    if (failure) throw failure;
    if (manifest.cleanupErrors.length > 0) throw new Error(`Evidence cleanup failed: ${manifest.cleanupErrors.join("; ")}`);
  });
});
