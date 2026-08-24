// Genuine Windows CAD drag/drop regression. This config launches one packaged app from an archived,
// validated clean state; the spec imports only through scripts/ole-drop-file.ps1 (CF_HDROP + OLE).
//
// Run:
//   node "node_modules/@wdio/cli/bin/wdio.js" run wdio.native-cad-drop.conf.js

import { spawn, spawnSync } from "node:child_process";
import {
  createHash,
} from "node:crypto";
import {
  createWriteStream,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  realpathSync,
  renameSync,
  statSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  createNativeCadDropRunId,
  exactNamedChild,
  PERSISTED_APP_CACHE_DIRECTORIES,
  PERSISTED_APP_STATE_FILES,
} from "./lib/native-cad-drop.js";

const e2eDir = path.dirname(fileURLToPath(import.meta.url));
const requestedApplication = process.env.MTK_EXE
  ? path.resolve(process.env.MTK_EXE)
  : path.resolve(e2eDir, "../src-tauri/target/release/metrocalk-editor-shell.exe");
const application = requestedApplication;
const exeDir = path.dirname(application);
const fixture = path.resolve(e2eDir, "samples/analytic_trio.stp");
const nativeDriver = path.resolve(e2eDir, ".driver/msedgedriver.exe");
const userProfile = process.env.USERPROFILE;
if (!userProfile) throw new Error("USERPROFILE is required to resolve tauri-driver exactly.");
const tauriDriverBin = path.resolve(userProfile, ".cargo/bin/tauri-driver.exe");

const suppliedRunId = process.env.MTK_NATIVE_DROP_RUN_ID;
const runId = suppliedRunId || createNativeCadDropRunId();
if (!/^[A-Za-z0-9][A-Za-z0-9._-]{0,95}$/.test(runId)) {
  throw new Error(`MTK_NATIVE_DROP_RUN_ID is not a safe single path segment: ${JSON.stringify(runId)}`);
}
const evidenceRoot = path.resolve(e2eDir, "../evidence/native-cad-drop");
const runDir = exactNamedChild(evidenceRoot, runId);
process.env.MTK_NATIVE_DROP_RUN_ID = runId;
process.env.MTK_NATIVE_DROP_RUN_DIR = runDir;

if (process.env.MTK_SCENE_N && process.env.MTK_SCENE_N !== "0") {
  throw new Error(`This clean-state regression requires MTK_SCENE_N=0, got ${process.env.MTK_SCENE_N}.`);
}
process.env.MTK_SCENE_N = "0";

let tauriDriver;
let driverStdout;
let driverStderr;

const sha256 = (file) => createHash("sha256").update(readFileSync(file)).digest("hex");
const delay = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));

function validateDirectFile(file, label) {
  if (!existsSync(file) || !statSync(file).isFile()) throw new Error(`${label} is missing: ${file}`);
  const actual = realpathSync.native(file);
  if (actual.toLocaleLowerCase() !== path.resolve(file).toLocaleLowerCase()) {
    throw new Error(`${label} must be addressed by its direct resolved path; requested ${file}, actual ${actual}`);
  }
  return actual;
}

function assertNoConflictingProcesses() {
  for (const imageName of [path.basename(application), "tauri-driver.exe", "msedgedriver.exe"]) {
    const result = spawnSync("tasklist.exe", ["/FI", `IMAGENAME eq ${imageName}`, "/FO", "CSV", "/NH"], {
      encoding: "utf8",
      windowsHide: true,
    });
    if (result.error) throw result.error;
    if (result.status !== 0) throw new Error(`tasklist failed while checking ${imageName}: ${result.stderr}`);
    if ((result.stdout || "").toLocaleLowerCase().includes(`"${imageName.toLocaleLowerCase()}"`)) {
      throw new Error(`Refusing to collide with an already-running ${imageName}; close that process and rerun.`);
    }
  }
}

function archiveNamedState(destination, phase) {
  mkdirSync(destination, { recursive: true });
  const archived = [];
  for (const name of PERSISTED_APP_STATE_FILES) {
    const source = exactNamedChild(exeDir, name);
    if (!existsSync(source)) continue;
    if (!statSync(source).isFile()) throw new Error(`${phase} state target is not a file: ${source}`);
    const target = exactNamedChild(destination, name);
    if (existsSync(target)) throw new Error(`${phase} archive target already exists: ${target}`);
    renameSync(source, target);
    archived.push({ name, source, archive: target, bytes: statSync(target).size });
  }
  return archived;
}

function archiveNamedCaches(destination, phase) {
  mkdirSync(destination, { recursive: true });
  const archived = [];
  for (const name of PERSISTED_APP_CACHE_DIRECTORIES) {
    const source = exactNamedChild(exeDir, name);
    if (!existsSync(source)) continue;
    if (!statSync(source).isDirectory()) throw new Error(`${phase} cache target is not a directory: ${source}`);
    const target = exactNamedChild(destination, name);
    if (existsSync(target)) throw new Error(`${phase} cache archive target already exists: ${target}`);
    // An exact-directory rename is recoverable and atomic on this volume; no recursive delete is used.
    renameSync(source, target);
    archived.push({ name, source, archive: target });
  }
  return archived;
}

function prepareRun() {
  validateDirectFile(application, "Tauri application");
  validateDirectFile(nativeDriver, "WebView2 driver");
  validateDirectFile(tauriDriverBin, "tauri-driver");
  validateDirectFile(fixture, "analytic STEP fixture");
  assertNoConflictingProcesses();

  mkdirSync(evidenceRoot, { recursive: true });
  if (existsSync(runDir)) {
    const contents = statSync(runDir).isDirectory() ? readdirSync(runDir) : ["not-a-directory"];
    throw new Error(`Run evidence path already exists; evidence is never overwritten: ${runDir} (${contents.length} entries)`);
  }
  mkdirSync(runDir);
  const archived = archiveNamedState(exactNamedChild(runDir, "pre-run-state"), "pre-run");
  const archivedCaches = archiveNamedCaches(exactNamedChild(runDir, "pre-run-caches"), "pre-run");
  const clean = PERSISTED_APP_STATE_FILES.map((name) => {
    const file = exactNamedChild(exeDir, name);
    return { name, file, absent: !existsSync(file) };
  });
  if (clean.some((entry) => !entry.absent)) throw new Error(`Named app state was not cleared: ${JSON.stringify(clean)}`);
  const cleanCaches = PERSISTED_APP_CACHE_DIRECTORIES.map((name) => {
    const directory = exactNamedChild(exeDir, name);
    return { name, directory, absent: !existsSync(directory) };
  });
  if (cleanCaches.some((entry) => !entry.absent)) throw new Error(`Named app caches were not archived: ${JSON.stringify(cleanCaches)}`);

  writeFileSync(
    exactNamedChild(runDir, "run-context.json"),
    `${JSON.stringify({
      runId,
      preparedAt: new Date().toISOString(),
      application: { path: application, bytes: statSync(application).size, sha256: sha256(application) },
      fixture: { path: fixture, bytes: statSync(fixture).size, sha256: sha256(fixture) },
      driver: nativeDriver,
      tauriDriver: tauriDriverBin,
      sceneSeed: 0,
      archived,
      archivedCaches,
      clean,
      cleanCaches,
    }, null, 2)}\n`,
    { encoding: "utf8", flag: "wx" },
  );
}

async function stopTauriDriver() {
  const driver = tauriDriver;
  tauriDriver = undefined;
  if (!driver || driver.exitCode !== null) return;
  await new Promise((resolve) => {
    let settled = false;
    const finish = () => {
      if (settled) return;
      settled = true;
      clearTimeout(timeout);
      resolve();
    };
    const timeout = setTimeout(finish, 3000);
    driver.once("exit", finish);
    if (!driver.kill()) finish();
  });
}

const closeStream = (stream) => new Promise((resolve) => {
  if (!stream || stream.closed) resolve();
  else stream.end(resolve);
});

async function archivePostRunState() {
  let lastError;
  for (let attempt = 0; attempt < 8; attempt += 1) {
    try {
      const archived = archiveNamedState(exactNamedChild(runDir, "post-run-state"), "post-run");
      const archivedCaches = archiveNamedCaches(exactNamedChild(runDir, "post-run-caches"), "post-run");
      writeFileSync(
        exactNamedChild(runDir, "post-run-state.json"),
        `${JSON.stringify({ archivedAt: new Date().toISOString(), archived, archivedCaches }, null, 2)}\n`,
        { encoding: "utf8", flag: "wx" },
      );
      return;
    } catch (error) {
      lastError = error;
      await delay(250);
    }
  }
  throw lastError;
}

export const config = {
  runner: "local",
  hostname: "127.0.0.1",
  port: 4444,
  path: "/",
  automationProtocol: "webdriver",
  specs: ["./specs-native-cad-drop/native-cad-drop.e2e.js"],
  maxInstances: 1,
  capabilities: [{ maxInstances: 1, "tauri:options": { application } }],
  logLevel: "warn",
  framework: "mocha",
  reporters: ["spec"],
  mochaOpts: { ui: "bdd", timeout: 240000 },
  connectionRetryTimeout: 120000,
  connectionRetryCount: 1,

  onPrepare: () => prepareRun(),

  beforeSession: () => new Promise((resolve, reject) => {
    driverStdout = createWriteStream(exactNamedChild(runDir, "tauri-driver.stdout.log"), { flags: "wx" });
    driverStderr = createWriteStream(exactNamedChild(runDir, "tauri-driver.stderr.log"), { flags: "wx" });
    tauriDriver = spawn(tauriDriverBin, ["--native-driver", nativeDriver], {
      stdio: ["ignore", "pipe", "pipe"],
      windowsHide: true,
    });
    tauriDriver.stdout.pipe(driverStdout);
    tauriDriver.stderr.pipe(driverStderr);
    let settled = false;
    let timer;
    const finish = (error) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      if (error) reject(error);
      else resolve();
    };
    tauriDriver.once("error", finish);
    timer = setTimeout(() => {
      if (tauriDriver.exitCode !== null) finish(new Error(`tauri-driver exited during startup with ${tauriDriver.exitCode}`));
      else finish();
    }, 2500);
  }),

  afterSession: async () => {
    await stopTauriDriver();
    await Promise.all([closeStream(driverStdout), closeStream(driverStderr)]);
    await archivePostRunState();
  },
};
