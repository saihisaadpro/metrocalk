// Genuine end-user CAD drop regression: one Windows OLE/CF_HDROP gesture, the complete native lifecycle,
// the stage overlay in the real composited window, and the resulting wrapper/tree/workspace/report state.

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
  analyzeNativeImportLifecycle,
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
const fixture = realpathSync.native(path.resolve(e2eDir, "samples/analytic_trio.stp"));
if (fixture.toLocaleLowerCase() !== context.fixture.path.toLocaleLowerCase()) {
  throw new Error(`Fixture differs from the preflighted run context: ${fixture}`);
}
const oleDropScript = realpathSync.native(path.resolve(e2eDir, "scripts/ole-drop-file.ps1"));
const captureScript = realpathSync.native(path.resolve(e2eDir, "scripts/capture-composited-window.ps1"));
const processName = path.basename(context.application.path, path.extname(context.application.path));
const fixtureName = path.basename(fixture);
const hoverReady = exactNamedChild(runDir, "hover-ready.txt");
const release = exactNamedChild(runDir, "release-hover.txt");
const cadLog = path.resolve(os.tmpdir(), "mtk-cad-import.log");
const importTimeoutMs = 120_000;
const artifacts = [];

const delay = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));
const sha256 = (file) => createHash("sha256").update(readFileSync(file)).digest("hex");
const artifact = (file) => ({ file: path.basename(file), bytes: statSync(file).size, sha256: sha256(file) });
const writeJson = (name, value) => writeFileSync(
  exactNamedChild(runDir, name),
  `${JSON.stringify(value, null, 2)}\n`,
  { encoding: "utf8", flag: "wx" },
);

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
      timeout: 30_000,
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
  artifacts.push(artifact(output));
  return output;
}

async function installLifecycleRecorder() {
  const result = await browser.executeAsync((eventName, done) => {
    const key = "__MTK_NATIVE_CAD_DROP_E2E__";
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
  if (!result?.ok) throw new Error(`Could not install native import lifecycle recorder: ${result?.error ?? "unknown error"}`);
}

const readLifecycle = () => browser.execute(() => {
  const state = window.__MTK_NATIVE_CAD_DROP_E2E__;
  return { events: state?.events ?? [], listenerError: state?.listenerError ?? null };
});

async function waitForCompleteLifecycle(timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  let last = analyzeNativeImportLifecycle([]);
  while (Date.now() < deadline) {
    const recording = await readLifecycle();
    if (recording.listenerError) throw new Error(`Native lifecycle recorder failed: ${recording.listenerError}`);
    last = analyzeNativeImportLifecycle(recording.events);
    if (last.failures.length > 0) throw new Error(`Native import failed: ${last.failures.join("; ")}`);
    if (last.errors.length > 0) throw new Error(`Native lifecycle is invalid: ${last.errors.join("; ")}`);
    if (last.complete) return { recording, analysis: last };
    await browser.pause(200);
  }
  throw new Error(`Native import lifecycle timed out after ${timeoutMs}ms: ${JSON.stringify(last)}`);
}

function startOleDrop() {
  const args = [
    "-NoLogo",
    "-NoProfile",
    "-NonInteractive",
    "-Sta",
    "-WindowStyle",
    "Hidden",
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
    "60",
    "-HoverReadyPath",
    hoverReady,
    "-ReleasePath",
    release,
    "-HoverTimeoutSeconds",
    "45",
  ];
  const child = spawn("powershell.exe", args, {
    stdio: ["ignore", "pipe", "pipe"],
    // PowerShell hides only its console via -WindowStyle above. CREATE_NO_WINDOW also hides the first
    // WinForms HWND, preventing the real source mouse-down/capture that Control.DoDragDrop requires.
    windowsHide: false,
  });
  const handle = { child, stdout: "", stderr: "", result: null, completion: null };
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

async function waitForHoverBarrier(handle, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (existsSync(hoverReady)) return readFileSync(hoverReady, "utf8");
    if (handle.result) {
      throw new Error(`OLE helper exited before held hover: ${JSON.stringify(handle.result)}\n${handle.stdout}\n${handle.stderr}`);
    }
    await delay(100);
  }
  throw new Error(`OLE helper did not reach its held-hover barrier within ${timeoutMs}ms.`);
}

async function waitForDropExit(handle, timeoutMs) {
  let timer;
  const timeout = new Promise((resolve) => {
    timer = setTimeout(() => resolve({ timeout: true }), timeoutMs);
  });
  const result = await Promise.race([handle.completion, timeout]);
  clearTimeout(timer);
  if (result?.timeout) {
    handle.child.kill();
    throw new Error(`OLE helper timed out after ${timeoutMs}ms and was terminated.`);
  }
  return result;
}

function persistOleLog(handle) {
  const output = exactNamedChild(runDir, "ole-drop.log");
  if (existsSync(output)) return;
  writeFileSync(
    output,
    `result=${JSON.stringify(handle?.result ?? null)}\n--- stdout ---\n${handle?.stdout ?? ""}\n--- stderr ---\n${handle?.stderr ?? ""}\n`,
    { encoding: "utf8", flag: "wx" },
  );
  artifacts.push(artifact(output));
}

function preserveCadLog(fromOffset) {
  const output = exactNamedChild(runDir, "cad-import.log");
  if (existsSync(output)) return readFileSync(output, "utf8");
  const bytes = existsSync(cadLog) ? readFileSync(cadLog) : Buffer.alloc(0);
  const start = bytes.length >= fromOffset ? fromOffset : 0;
  const delta = bytes.subarray(start);
  writeFileSync(output, delta, { flag: "wx" });
  artifacts.push(artifact(output));
  return delta.toString("utf8");
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

const hierarchySnapshot = () => browser.execute(() => [...document.querySelectorAll('[data-testid="hrow"]')].map((row) => ({
  id: row.getAttribute("data-id"),
  kind: row.getAttribute("data-kind"),
  name: (row.querySelector('[data-testid="thumb"]')?.getAttribute("title") || "").trim(),
  level: Number(row.getAttribute("aria-level")),
  selected: row.getAttribute("aria-selected") === "true",
})));

describe("genuine native CAD drag/drop", () => {
  it("holds hover for composited evidence, completes every lifecycle file, and lands one selected wrapper tree", async () => {
    const manifest = {
      runId: context.runId,
      startedAt: new Date().toISOString(),
      status: "running",
      fixture: context.fixture,
      cleanState: null,
      hoverBarrier: null,
      lifecycle: null,
      hierarchy: null,
      workspace: null,
      report: null,
      artifacts,
      cleanupErrors: [],
    };
    let failure = null;
    let dropHandle = null;
    let cadLogOffset = existsSync(cadLog) ? statSync(cadLog).size : 0;

    try {
      await browser.waitUntil(
        async () => browser.execute(() => !!window.__TAURI__?.core && !!window.__TAURI__?.event),
        { timeout: 30_000, interval: 200, timeoutMsg: "Tauri core/event globals never appeared" },
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
      let lastCleanProbe = null;
      await browser.waitUntil(
        async () => {
          lastCleanProbe = await browser.execute(async () => {
            if (!window.__TAURI__?.core) return { ready: false, report: null, hierarchyRows: null, error: null };
            try {
              const report = await window.__TAURI__.core.invoke("cad_report");
              const hierarchy = [...document.querySelectorAll('[data-testid="hrow"]')].map((row) => ({
                id: row.getAttribute("data-id"),
                kind: row.getAttribute("data-kind"),
                name: (row.querySelector('[data-testid="thumb"]')?.getAttribute("title") || "").trim(),
                level: Number(row.getAttribute("aria-level")),
              }));
              return {
                ready: true,
                report,
                hierarchy,
                hierarchyRows: hierarchy.length,
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
          manifest.cleanStateProbe = lastCleanProbe;
          return lastCleanProbe.report?.total === 0 && lastCleanProbe.hierarchyRows === 0;
        },
        {
          timeout: 30_000,
          interval: 250,
          timeoutMsg: "Clean app did not reconnect with an empty authoritative CAD report and hierarchy; inspect cleanStateProbe in evidence.json",
        },
      );
      const beforeReport = await invoke("cad_report");
      const beforeRows = await hierarchySnapshot();
      if (beforeReport.total !== 0 || beforeRows.length !== 0) {
        throw new Error(`Persisted app state was not clean: report=${JSON.stringify(beforeReport)}, rows=${JSON.stringify(beforeRows)}`);
      }
      manifest.cleanState = { entityCount: 0, hierarchyRows: 0, cadReportTotal: 0 };
      captureComposited("00-clean");

      await installLifecycleRecorder();
      cadLogOffset = existsSync(cadLog) ? statSync(cadLog).size : 0;
      dropHandle = startOleDrop();
      manifest.hoverBarrier = await waitForHoverBarrier(dropHandle, 20_000);
      // No WebDriver calls while OLE owns the modal drag loop. The external barrier proves the real cursor
      // is held over the target; a short presentation interval lets the React projection paint before the
      // preserve-window capture reads the existing DWM composition.
      await delay(650);
      captureComposited("01-held-hover-overlay", true);
      writeFileSync(release, `release=${new Date().toISOString()}\n`, { encoding: "utf8", flag: "wx" });

      const dropResult = await waitForDropExit(dropHandle, 30_000);
      persistOleLog(dropHandle);
      if (dropResult.error || dropResult.code !== 0) {
        throw new Error(`OLE helper failed: ${JSON.stringify(dropResult)}\n${dropHandle.stdout}\n${dropHandle.stderr}`);
      }
      if (!/DROP_RESULT:\s+OK\s+os-accepted/i.test(dropHandle.stdout)) {
        throw new Error(`OLE helper did not prove OS acceptance:\n${dropHandle.stdout}`);
      }
      if (!/DROP_GESTURE:\s+released/i.test(dropHandle.stdout)) {
        throw new Error(`OLE helper did not prove an authorized held-hover release:\n${dropHandle.stdout}`);
      }

      const lifecycle = await waitForCompleteLifecycle(importTimeoutMs);
      manifest.lifecycle = lifecycle.analysis;
      const events = lifecycle.recording.events.map((record) => record.payload);
      const hover = events.find((event) => event.phase === "hovered");
      if (!hover?.files?.some((file) => file.name === fixtureName && file.supported === true)) {
        throw new Error(`Hovered lifecycle did not classify ${fixtureName} as supported: ${JSON.stringify(hover)}`);
      }
      if (lifecycle.analysis.batches.length !== 1 || lifecycle.analysis.batches[0].total !== 1) {
        throw new Error(`Expected exactly one one-file drop batch: ${JSON.stringify(lifecycle.analysis)}`);
      }
      const succeeded = events.find((event) => event.phase === "succeeded");
      const rootId = succeeded?.subject?.kind === "entity" ? succeeded.subject.rootId : null;
      if (!rootId) throw new Error(`CAD success did not return an entity wrapper: ${JSON.stringify(succeeded)}`);

      await browser.waitUntil(
        async () => browser.execute((expectedRoot) => {
          const rows = [...document.querySelectorAll('[data-testid="hrow"]')];
          const root = rows.find((row) => row.getAttribute("data-id") === expectedRoot);
          return rows.length === 4 && root?.getAttribute("aria-selected") === "true";
        }, rootId),
        { timeout: 30_000, interval: 250, timeoutMsg: "Imported wrapper/tree did not settle to exactly four selected hierarchy rows" },
      );

      const rows = await hierarchySnapshot();
      const wrappers = rows.filter((row) => row.kind === "group" && row.level === 1);
      const parts = rows.filter((row) => row.kind !== "group");
      if (wrappers.length !== 1 || wrappers[0].id !== rootId || wrappers[0].selected !== true) {
        throw new Error(`Expected exactly one selected top-level wrapper ${rootId}: ${JSON.stringify(rows)}`);
      }
      if (parts.length !== 3 || parts.some((part) => part.level !== 2)) {
        throw new Error(`Expected exactly three leaf parts nested one level under the wrapper: ${JSON.stringify(rows)}`);
      }
      const expectedNames = ["cone frustum", "cylinder", "sphere band"];
      const actualNames = parts.map((part) => part.name).sort();
      if (JSON.stringify(actualNames) !== JSON.stringify(expectedNames)) {
        throw new Error(`Analytic trio names differ: expected ${JSON.stringify(expectedNames)}, got ${JSON.stringify(actualNames)}`);
      }
      for (const part of parts) {
        const parent = await invoke("part_parent", { id: part.id });
        if (parent !== rootId) throw new Error(`Part ${part.id} belongs to ${parent}, expected wrapper ${rootId}`);
      }
      manifest.hierarchy = { rootId, wrappers, parts };

      const modelWorkspace = await browser.execute((expectedRoot) => ({
        overlayPhase: document.querySelector('[data-testid="native-import-overlay"]')?.getAttribute("data-phase") ?? null,
        modelSelected: document.querySelector('[data-testid="engine-model"]')?.getAttribute("aria-selected") === "true",
        dockOpen: document.querySelector('[data-testid="bottom-dock"]')?.classList.contains("is-open") ?? false,
        assetSelected: document.querySelector("#bottom-workspaces-asset-tab")?.getAttribute("aria-selected") === "true",
        wrapperSelected: document.querySelector(`[data-testid="hrow"][data-id="${CSS.escape(expectedRoot)}"]`)?.getAttribute("aria-selected") === "true",
      }), rootId);
      if (modelWorkspace.overlayPhase !== "succeeded" || !modelWorkspace.modelSelected || !modelWorkspace.dockOpen || !modelWorkspace.assetSelected || !modelWorkspace.wrapperSelected) {
        throw new Error(`Success selection/model routing is incoherent: ${JSON.stringify(modelWorkspace)}`);
      }
      manifest.workspace = { afterSuccess: modelWorkspace };
      captureComposited("02-imported-selected-model");

      const reportClicked = await browser.execute(() => {
        const overlay = document.querySelector('[data-testid="native-import-overlay"]');
        const button = [...(overlay?.querySelectorAll("button") ?? [])].find((candidate) => candidate.textContent?.trim() === "Import report");
        button?.click();
        return !!button;
      });
      if (!reportClicked) throw new Error("Succeeded overlay did not expose the Import report action.");
      await browser.waitUntil(
        async () => browser.execute(() =>
          document.querySelector("#bottom-workspaces-import-tab")?.getAttribute("aria-selected") === "true" &&
          document.querySelector('[data-testid="import-report"]')?.getAttribute("data-total") === "3" &&
          document.querySelectorAll('[data-testid="import-row"]').length === 3),
        { timeout: 20_000, interval: 250, timeoutMsg: "Import workspace/report did not render all three analytic parts" },
      );
      const report = await invoke("cad_report");
      const accounted = report.exactBrep + report.tessellationOnly + report.aiReconstructed + report.proxy + report.accessDenied + report.failed;
      if (report.total !== 3 || report.parts?.length !== 3 || accounted !== 3 || report.failed !== 0) {
        throw new Error(`Import report did not account for exactly three successful parts: ${JSON.stringify(report)}`);
      }
      const selectedFromReport = await browser.execute(() => {
        const row = document.querySelector('[data-testid="import-row"]');
        const id = row?.getAttribute("data-id") ?? null;
        row?.click();
        return id;
      });
      if (!selectedFromReport) throw new Error("Import report did not expose a selectable part row.");
      await browser.waitUntil(
        async () => browser.execute((id) => document.querySelector(`[data-testid="hrow"][data-id="${CSS.escape(id)}"]`)?.getAttribute("aria-selected") === "true", selectedFromReport),
        { timeout: 10_000, interval: 200, timeoutMsg: "Selecting an import report row did not update hierarchy selection" },
      );
      manifest.workspace.afterReportAction = { importTabSelected: true, selectedPartId: selectedFromReport };
      manifest.report = { total: report.total, accounted, failed: report.failed, rows: report.parts.length };
      captureComposited("03-import-report-selected-part");

      const cadLogText = preserveCadLog(cadLogOffset);
      if (!cadLogText.includes(fixture) || !cadLogText.includes("CAD_IMPORT_METRICS") || !cadLogText.includes("CAD import commit OK")) {
        throw new Error(`Run-scoped CAD log lacks source, metrics, or commit proof:\n${cadLogText}`);
      }
      manifest.status = "passed";
    } catch (error) {
      failure = error;
      manifest.status = "failed";
      manifest.error = error instanceof Error ? { name: error.name, message: error.message, stack: error.stack } : { message: String(error) };
    } finally {
      if (dropHandle && !dropHandle.result) {
        try {
          if (!existsSync(release)) writeFileSync(release, `cleanup-release=${new Date().toISOString()}\n`, { encoding: "utf8", flag: "wx" });
          await waitForDropExit(dropHandle, 10_000);
        } catch (error) {
          manifest.cleanupErrors.push(`OLE cleanup: ${String(error)}`);
        }
      }
      if (dropHandle) persistOleLog(dropHandle);
      try {
        const recording = await readLifecycle();
        if (!manifest.lifecycle) manifest.lifecycle = analyzeNativeImportLifecycle(recording.events);
        writeJson("native-import-lifecycle.json", recording);
        artifacts.push(artifact(exactNamedChild(runDir, "native-import-lifecycle.json")));
        await browser.execute(async () => {
          const state = window.__MTK_NATIVE_CAD_DROP_E2E__;
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
      manifest.finishedAt = new Date().toISOString();
      manifest.artifacts = artifacts;
      writeJson("evidence.json", manifest);
    }

    if (failure) throw failure;
    if (manifest.cleanupErrors.length > 0) throw new Error(`Evidence cleanup failed: ${manifest.cleanupErrors.join("; ")}`);
  });
});
