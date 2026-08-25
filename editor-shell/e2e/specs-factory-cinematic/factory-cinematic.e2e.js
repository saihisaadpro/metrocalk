// Production Skid Weld Line direction pass on the packaged Windows application.
//
// This is intentionally not a synthetic invoke-only import. Both attempts enter through the real Win32
// CF_HDROP/OLE gesture. Attempt one is cancelled after Rust reports the file as importing and must leave
// zero committed CAD. Attempt two imports the same source, then the test searches the 15k-part report,
// authors 24 conservative mechanism tracks, directs 30 Calm shots across 15 named subjects, and saves a
// reusable .mtk package plus a machine-auditable evidence manifest.

import { browser } from "@wdio/globals";
import { execFileSync, spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  appendFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  realpathSync,
  statSync,
  writeFileSync,
} from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  buildCalmShotAssignments,
  buildKeysForProfile,
  chooseFilmedSubjects,
  closedLoopEndValue,
  FACTORY_ACCEPTANCE,
  isNeutralPose,
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
const windowGeometryScript = realpathSync.native(path.resolve(e2eDir, "scripts/window-client-rect.ps1"));
const videoQualityGateScript = realpathSync.native(path.resolve(e2eDir, "scripts/video-quality-gate.mjs"));
const ffmpegDirectory = path.resolve(
  os.homedir(),
  "AppData/Local/Microsoft/WinGet/Packages/Gyan.FFmpeg_Microsoft.Winget.Source_8wekyb3d8bbwe/ffmpeg-8.0.1-full_build/bin",
);
const ffmpeg = realpathSync.native(process.env.MTK_FFMPEG ? path.resolve(process.env.MTK_FFMPEG) : path.join(ffmpegDirectory, "ffmpeg.exe"));
const ffprobe = realpathSync.native(process.env.MTK_FFPROBE ? path.resolve(process.env.MTK_FFPROBE) : path.join(ffmpegDirectory, "ffprobe.exe"));
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

function recordDirectoryArtifacts(directory) {
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const child = path.join(directory, entry.name);
    if (entry.isDirectory()) recordDirectoryArtifacts(child);
    else if (entry.isFile()) recordArtifact(child);
  }
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

/**
 * Measure how much two captured frames actually differ, in luma.
 *
 * Screenshot evidence is only evidence if somebody looks at it. A "before" and an "after" that are
 * byte-identical prove the opposite of what they are filed under, and an engine number reported by the
 * same process that failed to redraw cannot notice. This decodes both PNGs, blends them in difference
 * mode and reads the real per-pixel statistics back out of FFmpeg.
 */
function capturedFrameDelta(beforePath, afterPath) {
  // `metadata=print` writes through FFmpeg's LOGGER, i.e. stderr - reading only stdout returned an
  // empty string, both statistics parsed as null, and the gate failed claiming it "could not measure"
  // rather than reporting a difference. Read BOTH streams so it does not matter which one it uses.
  const completed = spawnSync(ffmpeg, [
    "-hide_banner",
    "-nostdin",
    "-i", beforePath,
    "-i", afterPath,
    "-lavfi", "[0][1]blend=all_mode=difference,signalstats,metadata=print",
    "-f", "null",
    "-",
  ], { encoding: "utf8", timeout: 120_000, windowsHide: true });
  if (completed.error) throw completed.error;
  const result = `${completed.stdout ?? ""}
${completed.stderr ?? ""}`;
  const read = (key) => {
    const match = new RegExp(`lavfi\\.signalstats\\.${key}=([-0-9.]+)`).exec(result);
    return match ? Number(match[1]) : null;
  };
  return { meanLuma: read("YAVG"), peakLuma: read("YMAX") };
}

/**
 * Fail unless the viewport visibly changed between two captures.
 *
 * `peakLuma` rather than the mean on purpose: one mechanism moving in a wide frame changes a small
 * fraction of the pixels, so a mean-difference threshold would have to be set so low it stopped
 * discriminating. A real move always produces strongly differing pixels SOMEWHERE.
 */
function assertViewportChanged(beforePath, afterPath, what, minimumPeakLuma = 24) {
  const delta = capturedFrameDelta(beforePath, afterPath);
  invariant(delta.peakLuma !== null && delta.meanLuma !== null,
    `Could not measure the frame difference for ${what}: ${JSON.stringify(delta)}`);
  invariant(delta.peakLuma >= minimumPeakLuma,
    `${what} did not visibly change the viewport (peak luma difference ${delta.peakLuma}, mean ${delta.meanLuma}). `
    + `The engine reported the change but the rendered pixels are ${delta.peakLuma === 0 ? "identical" : "effectively identical"}.`);
  return delta;
}

function readWindowClientRect() {
  const stdout = execFileSync("powershell.exe", [
    "-NoLogo",
    "-NoProfile",
    "-NonInteractive",
    "-ExecutionPolicy",
    "Bypass",
    "-File",
    windowGeometryScript,
    "-ProcName",
    processName,
  ], { encoding: "utf8", timeout: 30_000, windowsHide: true });
  return JSON.parse(stdout.trim());
}

async function measureViewportCapture() {
  const client = readWindowClientRect();
  const dom = await browser.execute(() => {
    const viewport = document.querySelector("#viewport");
    if (!viewport) return null;
    const rect = viewport.getBoundingClientRect();
    return {
      rect: { left: rect.left, top: rect.top, width: rect.width, height: rect.height },
      innerWidth: window.innerWidth,
      innerHeight: window.innerHeight,
      devicePixelRatio: window.devicePixelRatio,
    };
  });
  invariant(dom && dom.innerWidth > 0 && dom.innerHeight > 0, `The live viewport could not be measured: ${JSON.stringify(dom)}`);
  const scaleX = client.width / dom.innerWidth;
  const scaleY = client.height / dom.innerHeight;
  invariant(Number.isFinite(scaleX) && Number.isFinite(scaleY) && Math.abs(scaleX - scaleY) < 0.03,
    `WebView/client scaling is incoherent: ${JSON.stringify({ client, dom, scaleX, scaleY })}`);
  const x = Math.round(client.x + dom.rect.left * scaleX);
  const y = Math.round(client.y + dom.rect.top * scaleY);
  const width = Math.floor((dom.rect.width * scaleX) / 2) * 2;
  const height = Math.floor((dom.rect.height * scaleY) / 2) * 2;
  invariant(width >= 1280 && height >= 720,
    `The maximised clean stage is below the 1280x720 delivery floor: ${JSON.stringify({ x, y, width, height, client, dom })}`);
  invariant(x >= client.x && y >= client.y && x + width <= client.x + client.width && y + height <= client.y + client.height,
    `The stage capture rectangle escaped the measured client: ${JSON.stringify({ x, y, width, height, client, dom })}`);
  // gdigrab records the DESKTOP at this rectangle, not the window. If anything covers the editor, the
  // film is of THAT application - and a luminance/motion quality gate cannot tell the difference, so the
  // run would go green over a recording of the wrong program. Refuse, and name what is in the way.
  invariant(client.occluded !== true,
    `The editor is occluded by "${client.occludedBy}", so a desktop recording would film that window `
      + `instead of the cinematic. Close or windowed-mode it before recording: ${JSON.stringify(client)}`);
  return { x, y, width, height, client, dom, scaleX, scaleY };
}

function encoderArguments(encoder) {
  return encoder === "h264_nvenc"
    ? ["-c:v", "h264_nvenc", "-preset", "p7", "-tune", "hq", "-rc", "vbr", "-cq", "16", "-b:v", "0"]
    : ["-c:v", "libx264", "-preset", "veryfast", "-crf", "14"];
}

function startStageRecording(output, geometry, seconds, encoder) {
  const args = [
    "-nostdin",
    "-hide_banner",
    "-loglevel",
    "warning",
    "-n",
    "-f",
    "gdigrab",
    "-framerate",
    "30",
    "-draw_mouse",
    "0",
    "-offset_x",
    String(geometry.x),
    "-offset_y",
    String(geometry.y),
    "-video_size",
    `${geometry.width}x${geometry.height}`,
    "-i",
    "desktop",
    "-t",
    String(seconds),
    "-an",
    ...encoderArguments(encoder),
    "-pix_fmt",
    "yuv420p",
    "-color_primaries",
    "bt709",
    "-color_trc",
    "bt709",
    "-colorspace",
    "bt709",
    "-color_range",
    "tv",
    "-movflags",
    "+faststart",
    output,
  ];
  const child = spawn(ffmpeg, args, { stdio: ["ignore", "pipe", "pipe"], windowsHide: true });
  const handle = { child, args, encoder, output, stdout: "", stderr: "", result: null, completion: null };
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

async function waitForRecording(handle, timeoutMs) {
  let timer;
  const timeout = new Promise((resolve) => { timer = setTimeout(() => resolve({ timeout: true }), timeoutMs); });
  const result = await Promise.race([handle.completion, timeout]);
  clearTimeout(timer);
  if (result?.timeout) {
    const terminated = handle.child.kill();
    const afterKill = await Promise.race([
      handle.completion,
      delay(5_000).then(() => null),
    ]);
    return afterKill
      ? { ...afterKill, timeout: true, terminated }
      : {
          code: null,
          signal: null,
          timeout: true,
          terminated,
          error: `ffmpeg ${handle.encoder} capture exceeded ${timeoutMs}ms and did not exit within 5s of termination.`,
        };
  }
  return result;
}

function persistRecordingLog(name, handle) {
  const output = exactNamedChild(runDir, name);
  writeFileSync(output, [
    `encoder=${handle.encoder}`,
    `result=${JSON.stringify(handle.result)}`,
    `command=${JSON.stringify([ffmpeg, ...handle.args])}`,
    "--- stdout ---",
    handle.stdout,
    "--- stderr ---",
    handle.stderr,
    "",
  ].join("\n"), { encoding: "utf8", flag: "wx" });
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
  // The helper moves PowerShell's console off-screen itself. Process-wide hiding also suppresses its first
  // WinForms source HWND, which prevents genuine mouse capture and leaves Control.DoDragDrop modal forever.
  const child = spawn("powershell.exe", args, { stdio: ["ignore", "pipe", "pipe"], windowsHide: false });
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
  // A synthetic OLE drag depends on Windows foreground/input rules holding still for its duration: any
  // window that takes the foreground mid-drag (a console spawned by another tool, an installer toast)
  // can strand the modal drag loop. Report what the helper had actually said so that a stranded drag is
  // distinguishable from a target that refused the drop.
  throw new Error(
    `OLE ${handle.attempt} did not reach held hover in ${timeoutMs}ms.`
    + `\nhelper stdout: ${handle.stdout.trim() || "(silent)"}`
    + `\nhelper stderr: ${handle.stderr.trim() || "(silent)"}`,
  );
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

/**
 * Did the helper refuse the gesture because a window that is NOT the editor covered the drop point?
 *
 * The distinction is the whole point. A foreign window taking the foreground mid-drag (a console another
 * tool spawned, a notification toast) is an accident of a shared desktop and is worth retrying. The
 * editor covering its OWN drop target is a product defect — a modal that swallows drops — and retrying
 * that would turn a real bug into a flaky test that eventually passes.
 */
function occludedByForeignWindow(log) {
  const gesture = /DROP_GESTURE: target-hit-test-failed: under=\d+ root=(\d+)/.exec(log);
  const target = /DROP_TARGET: pid=\d+ hwnd=(\d+)/.exec(log);
  return Boolean(gesture && target && gesture[1] !== target[1]);
}

/**
 * Perform the real OS drag, retrying only when a foreign window stole the drop point.
 *
 * Each attempt is a complete, genuine Win32 CF_HDROP gesture and keeps its own evidence file, so a run
 * that needed two tries says so rather than hiding it.
 */
async function startOleDropSurvivingOcclusion(attempt, dropHandles, maxAttempts = 3) {
  for (let index = 0; ; index += 1) {
    const handle = startOleDrop(index === 0 ? attempt : `${attempt}-again${index}`);
    dropHandles.push(handle);
    try {
      return { handle, hoverBarrier: await waitForHoverBarrier(handle), gestures: index + 1 };
    } catch (error) {
      if (!handle.result) {
        releaseOleDrop(handle, "occlusion-abandon");
        await waitForDropExit(handle, 20_000).catch(() => {});
      }
      persistOleLog(handle);
      const log = `${handle.stdout}\n${handle.stderr}`;
      if (index + 1 >= maxAttempts || !occludedByForeignWindow(log)) throw error;
      // Let whatever took the foreground finish, then put the editor back in front and try again.
      await browser.pause(2_000);
      await browser.maximizeWindow();
      await browser.pause(800);
    }
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

/** Read the part's live transform and derive the mechanism it should perform. No document mutation yet. */
async function planMechanismTrack(part, index) {
  const transform = await invoke("read_transform", { id: part.id });
  invariant(Array.isArray(transform) && transform.length === 8 && transform.every(Number.isFinite),
    `Part ${part.name} returned a non-finite transform: ${JSON.stringify(transform)}`);
  const profile = motionProfileFor(part);
  const pivot = transform.slice(0, 3);
  const limit = Math.abs(profile.amplitude) * 1.15;
  return { part, index, transform, profile, pivot, limit, keys: buildKeysForProfile(profile, index) };
}

/**
 * Author every mechanism as ONE transaction through the shell's `joint_author_batch` command.
 *
 * This is the same durable engine capability the editor's own mechanism authoring uses, and it is the
 * whole point of the command: repeating `set_joint`/`joint_value`/`joint_key` per key made the shell
 * recompile the animation plan and republish all 17k render instances 336 times for a single authoring
 * gesture. The batch pays those two costs exactly once, leaves one Ctrl-Z step behind, and is verified
 * below by ordinary per-joint readback — the assertions did not get weaker, only the round trips fewer.
 */
async function authorMechanismTracks(selected) {
  const plans = [];
  for (const [index, part] of selected.entries()) plans.push(await planMechanismTrack(part, index));
  const expectedKeys = plans.reduce((count, plan) => count + plan.keys.length, 0);

  const started = Date.now();
  const batch = await invoke("joint_author_batch", {
    requests: plans.map(({ part, profile, pivot, limit, keys }) => ({
      id: part.id,
      revolute: profile.revolute,
      axis: profile.axis,
      pivot,
      min: -limit,
      max: limit,
      source: "manual",
      keys: keys.map(({ t, value }) => ({ t, value })),
    })),
  });
  const elapsedMs = Date.now() - started;
  invariant(batch?.ok === true, `The mechanism transaction was refused: ${JSON.stringify(batch)}`);
  invariant(batch.authoredJoints === plans.length && batch.authoredKeys === expectedKeys,
    `The mechanism transaction reported ${batch.authoredJoints}/${batch.authoredKeys}, expected ${plans.length}/${expectedKeys}.`);
  // A production-size scene must stay workable. One authoring gesture that takes minutes is a defect in
  // the engine, not an acceptable cost of a large assembly — this gate is what keeps it fixed.
  invariant(elapsedMs < 60_000,
    `Authoring ${plans.length} joints and ${expectedKeys} keys took ${elapsedMs}ms in a ${FACTORY_ACCEPTANCE.report.total}-part scene.`);

  const tracks = [];
  for (const plan of plans) {
    const { part, profile, pivot, limit, keys, transform } = plan;
    const jointInfo = await invoke("joint_info", { id: part.id });
    invariant(jointInfo?.keys === keys.length && closeEnough(jointInfo.trackEnd, 12),
      `Joint readback for ${part.name} does not prove the seven-key 12s loop: ${JSON.stringify(jointInfo)}`);
    invariant(jointInfo.source === "manual" && jointInfo.jointType === profile.motion,
      `Joint readback for ${part.name} changed type/source: ${JSON.stringify(jointInfo)}`);
    invariant(closeEnough(jointInfo.value, closedLoopEndValue(profile), 1e-6),
      `The track did not end ${part.name} on its authored final value: ${JSON.stringify(jointInfo)}`);
    invariant(isNeutralPose(profile, jointInfo.value, 1e-6),
      `The closed loop did not return ${part.name} to its neutral pose: ${JSON.stringify(jointInfo)}`);
    const partDebug = await invoke("part_debug", { id: part.id });
    invariant(Array.isArray(partDebug) && partDebug[3] === true, `Authored mechanism is inactive: ${part.name} ${JSON.stringify(partDebug)}`);
    tracks.push({
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
    });
  }
  return { tracks, batch, elapsedMs };
}

async function authorCinematics(subjects, assemblyId) {
  const catalog = await invoke("cinema_catalog");
  const knownKinds = new Set(catalog.map(({ kind }) => kind));
  // Cutscenes are played in entity-key order by the runtime, so the direction is built against exactly
  // that order: the first subject's sequence opens the film and the last one's closes it.
  const orderedSubjects = [...subjects].sort((left, right) => left.id.localeCompare(right.id));
  const assignments = buildCalmShotAssignments(orderedSubjects, { assemblyId });
  const directed = [];
  for (const assignment of assignments) {
    invariant(assignment.kinds.every(({ kind }) => knownKinds.has(kind)),
      `The direction references a shot kind missing from the live catalogue: ${JSON.stringify(assignment)}`);
    const mood = await invoke("cinema_set_mood", { id: assignment.id, mood: "calm" });
    invariant(mood.reason == null && mood.mood === "calm", `Could not set Calm pacing on ${assignment.name}: ${JSON.stringify(mood)}`);
    const replies = [];
    for (const { kind, subject } of assignment.kinds) {
      const reply = await invoke("cinema_add_shot", { id: assignment.id, kind, subject });
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
  await browser.maximizeWindow();
  await browser.pause(800);
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

async function installCleanCaptureStage() {
  await browser.execute(() => {
    const existing = document.querySelector("#mtk-cinematic-capture-style");
    existing?.remove();
    const style = document.createElement("style");
    style.id = "mtk-cinematic-capture-style";
    style.textContent = [
      "#viewport > * { display: none !important; }",
      "#viewport { outline: none !important; box-shadow: none !important; transition: none !important; }",
    ].join("\n");
    document.head.append(style);
  });
}

async function removeCleanCaptureStage() {
  await browser.execute(() => document.querySelector("#mtk-cinematic-capture-style")?.remove());
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
      video: null,
      artifacts,
      cleanupErrors: [],
    };
    const dropHandles = [];
    let failure = null;
    let lifecycleRecording = { events: [], listenerError: null };
    let playing = false;
    let cleanCaptureStage = false;
    let recordingHandle = null;
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

      // The real OLE drag is the default, and remains the acceptance path for the drop gesture itself.
      // It can only land on a target that is visible when the button comes up: if another application
      // owns a full-screen foreground, Windows re-minimises the editor mid-gesture and ole32's modal
      // loop spins over the desktop until the harness kills it. MTK_FACTORY_IMPORT_MODE=command imports
      // the same fixture through the `import_asset` IPC the e2e already owns, so the scene, rendering,
      // animation and cinematic acceptance below still runs on such a machine. It deliberately does NOT
      // claim the drag/cancel evidence - that stays the drag path's to prove.
      const commandImport = process.env.MTK_FACTORY_IMPORT_MODE === "command";
      let succeeded;
      let retryBatchId = null;
      if (commandImport) {
        // FIRE AND FORGET, then poll. This import takes ~55s on the production fixture, and WebDriver
        // caps a synchronous `execute` at 30s - awaiting the invoke fails the script, not the import,
        // which then completes anyway and leaves the session desynchronised. Start it, stash the promise
        // on window for the result, and watch the engine's own state for the answer.
        await browser.execute((path) => {
          const slot = (window.__MTK_COMMAND_IMPORT__ = { done: false, id: null, error: null });
          window.__TAURI__.core.invoke("import_asset", { path })
            .then((id) => { slot.id = id; slot.done = true; })
            .catch((error) => { slot.error = String(error); slot.done = true; });
        }, fixture);
        await browser.waitUntil(
          async () => (await browser.execute(() => window.__MTK_COMMAND_IMPORT__?.done === true)) === true,
          { timeout: completedImportTimeoutMs, interval: 1_000, timeoutMsg: "import_asset never settled." },
        );
        const outcome = await browser.execute(() => ({
          id: window.__MTK_COMMAND_IMPORT__.id,
          error: window.__MTK_COMMAND_IMPORT__.error,
        }));
        invariant(!outcome.error, `import_asset failed: ${outcome.error}`);
        const rootId = outcome.id;
        invariant(typeof rootId === "string" && rootId.length > 0,
          `import_asset did not return the imported root entity: ${JSON.stringify(outcome)}`);
        succeeded = { subject: { kind: "entity", rootId }, stage: "succeeded", via: "import_asset" };
        manifest.retryImport = { via: "import_asset", rootId };
        manifest.cancellation = {
          skipped: "command-import mode does not exercise the drag/cancel choreography",
        };
        captureComposited("05-retry-import-in-progress");
      } else {
      // Attempt 1: real OS gesture, then the same visible Cancel control an operator uses while the
      // engine owns the long parse. The terminal lifecycle below remains the authoritative acceptance.
      const cancelledGesture = await startOleDropSurvivingOcclusion("01-cancel-attempt", dropHandles);
      const cancelledDrop = cancelledGesture.handle;
      manifest.cancellation = {
        hoverBarrier: cancelledGesture.hoverBarrier,
        gestures: cancelledGesture.gestures,
      };
      await delay(700);
      captureComposited("01-cancel-held-hover", true);
      releaseOleDrop(cancelledDrop);
      cancelledDrop.result = await waitForDropExit(cancelledDrop);
      persistOleLog(cancelledDrop);
      assertOleAccepted(cancelledDrop);
      const cancelledBatchId = await waitForNewDroppedBatch(0);
      inFlightBatchId = cancelledBatchId;
      const importing = await waitForBatchProgress(cancelledBatchId, "importing");
      let cancellationControl = null;
      await browser.waitUntil(async () => {
        cancellationControl = await browser.execute(() => {
          const overlay = document.querySelector('[data-testid="native-import-overlay"]');
          const button = [...(overlay?.querySelectorAll("button") ?? [])]
            .find((candidate) => candidate.getAttribute("aria-label") === "Cancel import");
          return button ? { found: true, disabled: button.disabled } : { found: false, disabled: null };
        });
        return cancellationControl.found && cancellationControl.disabled === false;
      }, { timeout: 15_000, interval: 100, timeoutMsg: `Batch ${cancelledBatchId} never exposed an enabled Cancel import control.` });
      const clicked = await browser.execute(() => {
        const overlay = document.querySelector('[data-testid="native-import-overlay"]');
        const button = [...(overlay?.querySelectorAll("button") ?? [])]
          .find((candidate) => candidate.getAttribute("aria-label") === "Cancel import");
        if (!button || button.disabled) return false;
        button.click();
        return true;
      });
      invariant(clicked === true, `The visible Cancel import control did not accept batch ${cancelledBatchId}.`);
      const accepted = clicked;
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
        control: { ...cancellationControl, clicked, ariaLabel: "Cancel import" },
        terminal: cancelled,
        zeroCommit: { report: postCancelReport, hierarchyRows: postCancelRows },
      };

      // Attempt 2: the exact same real OLE path, now allowed to complete.
      const retryGesture = await startOleDropSurvivingOcclusion("02-retry-attempt", dropHandles);
      const retryDrop = retryGesture.handle;
      manifest.retryImport = {
        hoverBarrier: retryGesture.hoverBarrier,
        gestures: retryGesture.gestures,
      };
      await delay(700);
      captureComposited("04-retry-held-hover", true);
      releaseOleDrop(retryDrop);
      retryDrop.result = await waitForDropExit(retryDrop);
      persistOleLog(retryDrop);
      assertOleAccepted(retryDrop);
      retryBatchId = await waitForNewDroppedBatch(cancelledBatchId);
      inFlightBatchId = retryBatchId;
      await waitForBatchProgress(retryBatchId, "importing");
      captureComposited("05-retry-import-in-progress");
      succeeded = await waitForBatchTerminal(retryBatchId, "succeeded", completedImportTimeoutMs);
      inFlightBatchId = null;
      invariant(succeeded.subject?.kind === "entity" && typeof succeeded.subject.rootId === "string",
        `Successful factory import did not return its wrapper entity: ${JSON.stringify(succeeded)}`);
      }

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
      const exposure = await invoke("set_exposure", { exposure: 0.45 });
      const environment = await invoke("reset_environment");
      invariant(profile === "cinematic" && workingSpace === "acesCg" && closeEnough(exposure, 0.45),
        `The cinematic presentation did not apply: ${JSON.stringify({ profile, workingSpace, exposure })}`);
      await invoke("view_preset", { preset: "persp" });
      await invoke("frame_all");
      await browser.pause(1_000);
      manifest.presentation = { profile, workingSpace, exposure, environment };
      captureComposited("06-imported-factory-overview");

      // The drop overlay's "Import report" button is the operator's route to this panel, and stays the
      // drag path's evidence. A command import raises no overlay, so open the same bottom workspace the
      // button opens - the panel under test is identical either way.
      const reportClicked = commandImport
        ? await (async () => {
          // Collapsed, the dock shows a summary and keeps its workspace list inside an unrendered
          // popup; expanded, it renders a real tablist. Expand first, then pick the tab.
          await browser.execute(() => {
            const dock = document.querySelector('[data-testid="bottom-dock"]');
            if (dock && !dock.classList.contains("is-open")) {
              const toggle = document.querySelector('[data-testid="bottom-dock-toggle"]');
              if (toggle instanceof HTMLElement) toggle.click();
            }
          });
          await browser.waitUntil(
            async () => (await browser.execute(() =>
              !!document.querySelector("#bottom-workspaces-import-tab"))) === true,
            { timeout: 15_000, interval: 200, timeoutMsg: "The expanded bottom dock never rendered its workspace tabs." },
          );
          return browser.execute(() => {
            const tab = document.querySelector("#bottom-workspaces-import-tab");
            if (!(tab instanceof HTMLElement)) return false;
            tab.click();
            return true;
          });
        })()
        : await browser.execute(() => {
          const overlay = document.querySelector('[data-testid="native-import-overlay"]');
          const button = [...(overlay?.querySelectorAll("button") ?? [])]
            .find((candidate) => candidate.textContent?.trim() === "Import report");
          button?.click();
          return !!button;
        });
      invariant(reportClicked, commandImport
        ? "The Import bottom workspace option was not present to open the report."
        : "Succeeded production overlay did not expose Import report.");
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

      const authored = await authorMechanismTracks(selected);
      const tracks = authored.tracks;
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
        revolvingMechanisms: tracks.filter(({ profile }) => profile.cycle === "revolve").length,
        transaction: {
          command: "joint_author_batch",
          undoSteps: 1,
          authoredJoints: authored.batch.authoredJoints,
          authoredKeys: authored.batch.authoredKeys,
          elapsedMs: authored.elapsedMs,
          message: authored.batch.message,
        },
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
      // CAPTURE-INTEGRITY CONTROL. Every "did it move?" gate here compares two OS captures, and the
      // capture reads the window's own presentation. A change that touches only the native viewport and
      // no DOM might therefore not force a recomposite, in which case a stale frame would read as "the
      // engine did not move" no matter what the engine did. Exposure is a large, 3D-only, DOM-free
      // change with a known-correct implementation: if THIS pair is identical, the capture is stale and
      // no viewport-delta measurement in this environment means anything.
      const controlBefore = captureComposited("07a-capture-control-exposure-base");
      await invoke("set_exposure", { exposure: 1.6 });
      await browser.pause(900);
      const controlAfter = captureComposited("07b-capture-control-exposure-raised");
      const controlDelta = capturedFrameDelta(controlBefore, controlAfter);
      await invoke("set_exposure", { exposure: 0.45 });
      await browser.pause(600);
      manifest.captureIntegrity = { change: "exposure 0.45 -> 1.6", viewportDelta: controlDelta };
      invariant(controlDelta.peakLuma !== null && controlDelta.peakLuma > 0,
        `The OS capture did not observe a large 3D-only change (exposure 0.45 -> 1.6): `
        + `${JSON.stringify(controlDelta)}. Every viewport-delta gate below would be measuring a stale `
        + `frame rather than the renderer, so they are reported as unproven rather than passed.`);

      // DIAGNOSTIC PAIR, whole-factory framing. The focused pair below proves one mechanism moved in
      // close-up, but a close-up cannot distinguish "the mechanisms did not move" from "the one part in
      // frame happens not to move much at this instant". Scrub the same timeline with every animated
      // subject on screen and record the delta without gating on it, so a zero here is attributable.
      await invoke("view_preset", { preset: "persp" });
      await invoke("frame_all");
      await browser.pause(900);
      const wideNeutralFrame = captureComposited("08a-mechanisms-wide-neutral");
      // ENGINE TRUTH, not a screenshot. `read_transform` reports the DOCUMENT, and a scrub deliberately
      // does not touch it, so the only previous way to ask "did the geometry move?" was to diff two
      // captures - a measurement that cannot tell "the engine did not move it" apart from "it moved
      // where the camera could not see it", from a part rotationally symmetric about the axis it turns
      // on, or from one occluded by the machine it is bolted inside. `rendered_transform` reads the
      // instance the render thread uploads, so this is the engine answering for itself.
      const neutralRender = {};
      for (const track of selected) neutralRender[track.id] = await invoke("rendered_transform", { id: track.id });
      const widePosed = await invoke("joint_scrub", { t: 5.8 });
      await browser.pause(900);
      const posedRender = {};
      for (const track of selected) posedRender[track.id] = await invoke("rendered_transform", { id: track.id });
      const widePosedFrame = captureComposited("08b-mechanisms-wide-posed");

      const renderDeltas = selected.map((track) => {
        const before = neutralRender[track.id];
        const after = posedRender[track.id];
        const published = Array.isArray(before) && Array.isArray(after) && before.length === 8 && after.length === 8;
        const translation = published
          ? Math.hypot(...[0, 1, 2].map((axis) => after[axis] - before[axis]))
          : null;
        const rotation = published
          ? Math.max(...[3, 4, 5, 6].map((axis) => Math.abs(after[axis] - before[axis])))
          : null;
        return { id: track.id, name: track.name, family: track.mechanismFamily, published, translation, rotation };
      });
      const movedInRender = renderDeltas.filter(({ translation, rotation }) =>
        (translation ?? 0) > 1e-5 || (rotation ?? 0) > 1e-5);
      manifest.animation.wideScrubProof = {
        timeSeconds: 5.8,
        posedEntities: widePosed,
        viewportDelta: capturedFrameDelta(wideNeutralFrame, widePosedFrame),
        publishedInstances: renderDeltas.filter(({ published }) => published).length,
        movedInRender: movedInRender.length,
        renderDeltas,
      };
      invariant(renderDeltas.every(({ published }) => published),
        `Some authored mechanisms publish no render instance at all: `
        + `${JSON.stringify(renderDeltas.filter(({ published }) => !published))}`);
      invariant(movedInRender.length === selected.length,
        `Scrubbing to t=5.8s moved only ${movedInRender.length} of ${selected.length} mechanisms in the `
        + `render projection: ${JSON.stringify(renderDeltas.filter((row) => !movedInRender.includes(row)))}`);
      await invoke("joint_scrub", { t: -1 });
      await browser.pause(400);

      // Film the mechanism that actually travels furthest, measured above, rather than whichever part
      // happens to sort first. The previous choice was a weld-boom BASE: a revolute joint pivoted on its
      // own axis, on a part that is very nearly a solid of revolution about that axis, framed so tight
      // that its neighbours fill the shot. It turns 20 degrees and looks identical, which is a perfectly
      // good reason for two captures to match and a very bad shot to prove anything with.
      const showcase = [...renderDeltas]
        .filter(({ published }) => published)
        .sort((left, right) => (right.translation ?? 0) - (left.translation ?? 0))[0];
      invariant(showcase && (showcase.translation ?? 0) > 1e-4,
        `No authored mechanism translates enough to be filmed in close-up: ${JSON.stringify(renderDeltas)}`);
      manifest.animation.closeUpSubject = showcase;
      await invoke("focus_entity", { id: showcase.id });
      await browser.pause(600);
      const neutralFrame = captureComposited("08-featured-mechanism-neutral");
      const posed = await invoke("joint_scrub", { t: 5.8 });
      invariant(posed >= FACTORY_ACCEPTANCE.animationTracks,
        `Scrubbing the shared timeline posed ${posed} mechanisms, expected at least ${FACTORY_ACCEPTANCE.animationTracks}.`);
      // Give the viewport a moment to present the posed frame before the OS capture reads the window.
      await browser.pause(900);
      const posedFrame = captureComposited("09-featured-mechanism-posed");
      const scrubDelta = assertViewportChanged(neutralFrame, posedFrame,
        `Scrubbing ${posed} mechanisms to t=5.8s`);
      await invoke("joint_scrub", { t: -1 });
      await invoke("unfocus");
      manifest.animation.scrubProof = { timeSeconds: 5.8, posedEntities: posed, viewportDelta: scrubDelta };

      const assemblyId = manifest.retryImport.rootId;
      invariant(typeof assemblyId === "string" && assemblyId.length > 0,
        `The import did not report the assembly wrapper to establish on: ${JSON.stringify(manifest.retryImport)}`);
      const cinematics = await authorCinematics(subjects, assemblyId);
      // The opening and closing shots must genuinely frame the whole factory rather than a part of it,
      // otherwise the film never shows the scale of what was imported.
      const assemblyFramedShots = cinematics.cutscenes
        .flatMap(({ kinds }) => kinds)
        .filter(({ subject }) => subject === assemblyId);
      invariant(assemblyFramedShots.length === 2,
        `Expected an establishing and a closing shot framed on the assembly, got ${JSON.stringify(assemblyFramedShots)}`);
      manifest.cinematics = {
        subjectCount: cinematics.cutscenes.length,
        distinctAnimatedSubjectIds: new Set(cinematics.cutscenes.map(({ id }) => id)).size,
        totalShots: cinematics.totalShots,
        totalSeconds: cinematics.totalSeconds,
        minimumClipSeconds: FACTORY_ACCEPTANCE.minimumCinematicSeconds,
        recommendedCaptureSeconds: Math.ceil(cinematics.totalSeconds) + 1,
        assemblyId,
        assemblyFramedShots,
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
      const openingFrame = captureComposited("11-live-cinematic-opening");
      const openingClockAt = Date.now();
      await browser.pause(2_200);
      const movingCamera = await invoke("camera_probe");
      const movingAnimation = await invoke("animation_state", { id: null });
      const movingClockAt = Date.now();
      const cameraTravel = Math.hypot(...openingCamera.eye.map((value, index) => value - movingCamera.eye[index]));
      invariant(openingCamera.cinematic && movingCamera.cinematic && cameraTravel > 0.01,
        `The opening Hero camera did not visibly move: ${JSON.stringify({ openingCamera, movingCamera, cameraTravel })}`);
      invariant(movingAnimation.currentTick !== openingAnimation.currentTick,
        `The 24-track mechanism timeline did not advance: ${JSON.stringify({ openingAnimation, movingAnimation })}`);
      const movingFrame = captureComposited("12-live-cinematic-mechanisms-moving");
      // The camera reports that it moved and the transport reports that time advanced. Neither is worth
      // anything if the window is presenting a stale frame, which is exactly what a starved renderer does.
      const previewDelta = assertViewportChanged(openingFrame, movingFrame,
        "2.2 seconds of live cinematic playback");
      manifest.preview = { play, openingCamera, movingCamera, cameraTravel, openingAnimation, movingAnimation, viewportDelta: previewDelta };
      await invoke("stop");
      playing = false;
      const stoppedCamera = await invoke("camera_probe");
      const projectState = await invoke("project_state");
      invariant(stoppedCamera.cinematic === false && projectState.dirty === false,
        `Stop did not restore the authored edit state: ${JSON.stringify({ stoppedCamera, projectState })}`);
      manifest.preview.stoppedCamera = stoppedCamera;
      manifest.preview.projectState = projectState;

      // Deliver the actual film, not merely its direction metadata. Crop the maximised native viewport,
      // remove all DOM chrome inside that transparent stage, prove an encoder on a short capture, then run
      // the complete 208.75-second direction with enough head/tail margin to exceed three minutes.
      await installCleanCaptureStage();
      cleanCaptureStage = true;
      const geometry = await measureViewportCapture();
      const encoderPreflights = [];
      let encoder = null;
      for (const candidate of ["h264_nvenc", "libx264"]) {
        const slug = candidate.replace(/[^a-z0-9]+/gi, "-").toLocaleLowerCase();
        const output = exactNamedChild(runDir, `capture-preflight-${slug}.mp4`);
        const handle = startStageRecording(output, geometry, 2, candidate);
        handle.result = await waitForRecording(handle, 30_000);
        const log = persistRecordingLog(`capture-preflight-${slug}.log`, handle);
        const bytes = existsSync(output) ? statSync(output).size : 0;
        if (bytes > 0) recordArtifact(output);
        const passed = !handle.result.error && !handle.result.timeout && handle.result.code === 0 && bytes > 1024;
        encoderPreflights.push({ encoder: candidate, result: handle.result, bytes, output: path.relative(runDir, output).replaceAll("\\", "/"), log: path.relative(runDir, log).replaceAll("\\", "/"), passed });
        if (passed) {
          encoder = candidate;
          break;
        }
      }
      invariant(encoder, `Neither hardware nor software H.264 capture passed preflight: ${JSON.stringify(encoderPreflights)}`);

      // SIZE THE RECORDING BY THE CLOCK THE FILM ACTUALLY RUNS ON.
      //
      // A cutscene advances on the engine's play clock, which is a TICK COUNT, not the wall clock. On a
      // 17,793-entity scene the engine does not hold 60 Hz, so the film's authored 194 s takes longer
      // than 194 s to play, and a recorder sized to the authored length stops part way through - which
      // is exactly what happened: 202 s of capture covered 7 of the 15 directed subjects.
      //
      // Measured rather than padded with a guessed factor: the live preview above already advanced the
      // play clock over a known wall-clock interval, so the ratio is observable. `+ 25%` and `+ 12 s`
      // are ordinary head/tail margin on top of the measurement, and the whole thing is capped so a
      // pathological reading cannot produce an hour-long recording.
      const measuredTicksPerSecond = (movingAnimation.currentTick - openingAnimation.currentTick)
        / Math.max(0.001, (movingClockAt - openingClockAt) / 1_000);
      const nominalTicksPerSecond = 60;
      const slowdown = measuredTicksPerSecond > 0
        ? Math.max(1, nominalTicksPerSecond / measuredTicksPerSecond)
        : 1;
      const captureSeconds = Math.min(
        900,
        Math.ceil(cinematics.totalSeconds * slowdown * 1.25) + 12,
      );
      manifest.preview.playClock = {
        ticksPerSecond: measuredTicksPerSecond,
        nominalTicksPerSecond,
        slowdown,
        authoredSeconds: cinematics.totalSeconds,
        captureSeconds,
      };
      const videoFile = exactNamedChild(runDir, `skid-weld-line-cinematic-${captureSeconds}s.mp4`);
      // ROLL WHEN THE CINEMATIC OWNS THE CAMERA, not before it.
      //
      // Entering Play on this scene takes ~15 seconds (measured; see the engine's diagnostics log), and
      // for all of it the viewport is still an EDITOR: the transform gizmo, the selection outline and
      // the grid are drawn, and the camera is wherever authoring left it. Starting the recorder first
      // put that straight into the delivered film -- fifteen seconds of chrome before the first shot.
      // Waiting costs the opening fraction of a second of the first shot, which is a hold anyway.
      const filmPlay = await invoke("play");
      invariant(filmPlay.playing === true, `The recorded direction did not enter Play: ${JSON.stringify(filmPlay)}`);
      playing = true;
      await browser.waitUntil(async () => (await invoke("camera_probe")).cinematic === true,
        { timeout: 60_000, interval: 150, timeoutMsg: "The recorded direction never gave the cinematic camera authority." });
      recordingHandle = startStageRecording(videoFile, geometry, captureSeconds, encoder);
      await delay(900);
      invariant(!recordingHandle.result,
        `The ${encoder} recorder exited as soon as it was started: ${JSON.stringify(recordingHandle.result)}
${recordingHandle.stderr}`);
      recordingHandle.result = await waitForRecording(recordingHandle, (captureSeconds + 75) * 1_000);
      const recordingLog = persistRecordingLog("factory-cinematic-recording.log", recordingHandle);
      invariant(!recordingHandle.result.error && !recordingHandle.result.timeout && recordingHandle.result.code === 0,
        `The ${captureSeconds}s ${encoder} recording failed: ${JSON.stringify(recordingHandle.result)}\n${recordingHandle.stderr}`);
      invariant(existsSync(videoFile) && statSync(videoFile).size > 1_000_000,
        `The recorded cinematic is missing or implausibly small: ${videoFile}`);
      recordArtifact(videoFile);
      const completedCamera = await invoke("camera_probe");
      const expectedSubjectIds = subjects.map(({ id }) => id).sort();
      const visitedSubjectIds = [...new Set(completedCamera.visitedSubjects ?? [])].sort();
      invariant(completedCamera.cinematic === false,
        `The capture hard limit truncated the directed film before camera handoff: ${JSON.stringify(completedCamera)}`);
      invariant(JSON.stringify(visitedSubjectIds) === JSON.stringify(expectedSubjectIds),
        `Runtime camera coverage missed directed subjects: ${JSON.stringify({ expectedSubjectIds, visitedSubjectIds, completedCamera })}`);
      await invoke("stop");
      playing = false;
      recordingHandle = null;
      await removeCleanCaptureStage();
      cleanCaptureStage = false;

      const qualityDirectory = exactNamedChild(runDir, "video-quality");
      const gateArguments = [
        videoQualityGateScript,
        "--input",
        videoFile,
        "--output-dir",
        qualityDirectory,
        "--ffmpeg",
        ffmpeg,
        "--ffprobe",
        ffprobe,
        "--sample-count",
        "15",
      ];
      let gateStdout = "";
      let gateStderr = "";
      try {
        gateStdout = execFileSync(process.execPath, gateArguments, {
          encoding: "utf8",
          timeout: 10 * 60_000,
          maxBuffer: 64 * 1024 * 1024,
          windowsHide: true,
        });
      } catch (error) {
        gateStdout = error?.stdout ?? "";
        gateStderr = error?.stderr ?? String(error);
        const gateFailureLog = exactNamedChild(runDir, "video-quality-gate.log");
        writeFileSync(gateFailureLog, `${gateStdout}\n--- stderr ---\n${gateStderr}\n`, { encoding: "utf8", flag: "wx" });
        recordArtifact(gateFailureLog);
        if (existsSync(qualityDirectory)) recordDirectoryArtifacts(qualityDirectory);
        throw new Error(`The recorded film failed its media quality gate: ${gateStderr || gateStdout}`);
      }
      const gateLog = exactNamedChild(runDir, "video-quality-gate.log");
      writeFileSync(gateLog, `${gateStdout}\n--- stderr ---\n${gateStderr}\n`, { encoding: "utf8", flag: "wx" });
      recordArtifact(gateLog);
      recordDirectoryArtifacts(qualityDirectory);
      const qualityManifestPath = path.join(qualityDirectory, "video-quality-validation.json");
      const quality = JSON.parse(readFileSync(qualityManifestPath, "utf8"));
      invariant(quality.passed === true, `The recorded film quality manifest did not pass: ${JSON.stringify(quality)}`);
      manifest.video = {
        file: path.relative(runDir, videoFile).replaceAll("\\", "/"),
        bytes: statSync(videoFile).size,
        sha256: sha256(videoFile),
        captureSeconds,
        encoder,
        encoderPreflights,
        geometry,
        recordingLog: path.relative(runDir, recordingLog).replaceAll("\\", "/"),
        qualityManifest: path.relative(runDir, qualityManifestPath).replaceAll("\\", "/"),
        contactSheet: "video-quality/video-quality-contact-sheet.png",
        sampledFrames: quality.representativeFrames.frames.length,
        media: quality.probe.media,
        strictDecode: quality.strictDecode.passed,
        visualContent: {
          passed: quality.visualContent.passed,
          luminance: quality.visualContent.luminance.summary,
          motion: quality.visualContent.motion.summary,
        },
        completedCamera,
        visitedSubjectIds,
      };

      lifecycleRecording = await readLifecycle();
      // The batch lifecycle and the cancellation proof belong to the drag path. Command-import mode
      // never issues a dropped batch or a cancellation, so asserting either would be asserting a
      // fiction; the CAD log's source/metrics/commit proof still applies to both paths.
      if (!commandImport) {
        manifest.cancellation.lifecycle = validateBatchLifecycle(lifecycleRecording, manifest.cancellation.batchId, "cancelled");
        manifest.retryImport.lifecycle = validateBatchLifecycle(lifecycleRecording, manifest.retryImport.batchId, "succeeded");
      }
      const cadLogText = preserveCadLog(cadLogOffset);
      invariant(cadLogText.includes(fixtureName) && /CAD_IMPORT_METRICS/.test(cadLogText) && /CAD import commit OK/.test(cadLogText),
        `Run-scoped CAD log lacks retry source, metrics, or commit proof:\n${cadLogText}`);
      if (!commandImport) {
        invariant(/CAD import cancelled/i.test(cadLogText) && /no scene changes were committed/i.test(cadLogText),
          `Run-scoped CAD log lacks atomic cancellation proof:\n${cadLogText}`);
      }
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
      if (recordingHandle && !recordingHandle.result) {
        try {
          recordingHandle.child.kill();
          recordingHandle.result = await waitForRecording(recordingHandle, 10_000);
        } catch (error) {
          manifest.cleanupErrors.push(`Video recorder cleanup: ${String(error)}`);
        }
      }
      if (recordingHandle) {
        try {
          persistRecordingLog("factory-cinematic-recording-partial.log", recordingHandle);
          if (existsSync(recordingHandle.output) && statSync(recordingHandle.output).size > 0) {
            recordArtifact(recordingHandle.output);
          }
        } catch (error) {
          manifest.cleanupErrors.push(`Partial video evidence preservation: ${String(error)}`);
        }
      }
      if (cleanCaptureStage) {
        try {
          await removeCleanCaptureStage();
        } catch (error) {
          manifest.cleanupErrors.push(`Clean capture stage cleanup: ${String(error)}`);
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
      if (manifest.cleanupErrors.length > 0) {
        manifest.status = "failed";
        const cleanupFailure = new Error(`Evidence cleanup failed: ${manifest.cleanupErrors.join("; ")}`);
        if (!failure) {
          failure = cleanupFailure;
          manifest.error = { name: cleanupFailure.name, message: cleanupFailure.message, stack: cleanupFailure.stack };
        }
      }
      manifest.finishedAt = new Date().toISOString();
      manifest.artifacts = artifacts;
      const evidencePath = exactNamedChild(runDir, "evidence.json");
      writeFileSync(evidencePath, `${JSON.stringify(manifest, null, 2)}\n`, { encoding: "utf8", flag: "wx" });
    }

    if (failure) throw failure;
  });
});
