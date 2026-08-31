import path from "node:path";

export const NATIVE_IMPORT_LIFECYCLE_EVENT = "mtk://import-lifecycle";

export const PERSISTED_APP_STATE_FILES = Object.freeze([
  "metrocalk-scene.jsonl",
  "metrocalk-wallet.json",
  "metrocalk-recents.json",
  "metrocalk-window.json",
  "metrocalk-animation-asset-identities.json",
]);

export const PERSISTED_APP_CACHE_DIRECTORIES = Object.freeze([
  "metrocalk-assets",
  "metrocalk-cad-meshes",
]);

const TERMINAL_PHASES = new Set(["succeeded", "refused", "failed", "cancelled"]);

/** Resolve one literal filename directly below a validated parent; traversal and nested paths are refused. */
export function exactNamedChild(parent, name) {
  if (typeof name !== "string" || name.length === 0 || path.basename(name) !== name || name === "." || name === "..") {
    throw new Error(`Unsafe child filename: ${JSON.stringify(name)}`);
  }
  const resolvedParent = path.resolve(parent);
  const candidate = path.resolve(resolvedParent, name);
  const sameDirectory = path.dirname(candidate).toLocaleLowerCase() === resolvedParent.toLocaleLowerCase();
  if (!sameDirectory || path.basename(candidate) !== name) {
    throw new Error(`Resolved path leaves its parent: ${candidate}`);
  }
  return candidate;
}

export function createNativeCadDropRunId(now = new Date(), pid = process.pid) {
  const stamp = now.toISOString().replace(/[-:]/g, "").replace(/\.\d{3}Z$/, "Z");
  return `native-cad-drop-${stamp}-${pid}`;
}

const payloadOf = (record) => record?.payload ?? record;

/**
 * Reduce the recorded event stream into a gateable proof. Completion means every dropped batch and every
 * file index observed queued → importing → succeeded exactly once. Refusal/failure is always fatal.
 */
export function analyzeNativeImportLifecycle(records) {
  const events = Array.isArray(records) ? records.map(payloadOf) : [];
  const errors = [];
  const failures = [];
  const pending = [];
  const batches = new Map();
  const hovered = events
    .map((event, at) => ({ event, at }))
    .filter(({ event }) => event?.phase === "hovered");

  for (const [at, event] of events.entries()) {
    if (event?.phase !== "dropped") continue;
    if (!Number.isInteger(event.batchId) || event.batchId < 1) {
      errors.push(`dropped event has invalid batchId ${JSON.stringify(event.batchId)}`);
      continue;
    }
    if (!Array.isArray(event.files) || event.files.length === 0) {
      errors.push(`batch ${event.batchId} has no dropped files`);
      continue;
    }
    if (batches.has(event.batchId)) {
      errors.push(`batch ${event.batchId} emitted dropped more than once`);
      continue;
    }
    batches.set(event.batchId, {
      batchId: event.batchId,
      files: event.files,
      droppedAt: at,
      progress: new Map(),
      terminals: new Map(),
    });
  }

  for (const [at, event] of events.entries()) {
    if (!event || (event.phase !== "progress" && !TERMINAL_PHASES.has(event.phase))) continue;
    if (event.phase === "refused" || event.phase === "failed" || event.phase === "cancelled") {
      failures.push(`${event.phase} batch=${event.batchId ?? "none"} index=${event.index}: ${event.message ?? event.fileName ?? "no explanation"}`);
    }
    const batch = batches.get(event.batchId);
    if (!batch) {
      errors.push(`${event.phase} references unknown batch ${JSON.stringify(event.batchId)}`);
      continue;
    }
    if (!Number.isInteger(event.index) || event.index < 1 || event.index > batch.files.length) {
      errors.push(`${event.phase} for batch ${event.batchId} has invalid index ${JSON.stringify(event.index)}`);
      continue;
    }
    if (event.total !== batch.files.length) {
      errors.push(`${event.phase} for batch ${event.batchId} reports total=${event.total}, expected ${batch.files.length}`);
    }
    const droppedFile = batch.files[event.index - 1];
    if (event.fileName !== droppedFile.name) {
      errors.push(`${event.phase} for batch ${event.batchId} file ${event.index} names ${JSON.stringify(event.fileName)}, expected ${JSON.stringify(droppedFile.name)}`);
    }
    if (event.phase === "progress") {
      const stages = batch.progress.get(event.index) ?? [];
      stages.push({ stage: event.stage, at });
      batch.progress.set(event.index, stages);
      continue;
    }
    if (batch.terminals.has(event.index)) {
      errors.push(`batch ${event.batchId} file ${event.index} emitted more than one terminal event`);
      continue;
    }
    batch.terminals.set(event.index, { event, at });
  }

  const batchSummary = [];
  for (const batch of batches.values()) {
    const files = [];
    for (let index = 1; index <= batch.files.length; index += 1) {
      const droppedFile = batch.files[index - 1];
      const stages = batch.progress.get(index) ?? [];
      const queued = stages.filter(({ stage }) => stage === "queued");
      const importing = stages.filter(({ stage }) => stage === "importing");
      const delayed = stages.filter(({ stage }) => stage === "delayed");
      const terminalRecord = batch.terminals.get(index);
      const terminal = terminalRecord?.event;
      if (!terminal) {
        pending.push(`batch ${batch.batchId} file ${index}/${batch.files.length} (${droppedFile.name}) has no terminal event`);
      } else {
        if (queued.length !== 1) {
          errors.push(`batch ${batch.batchId} file ${index} emitted queued progress ${queued.length} times; expected exactly once`);
        }
        if (importing.length !== 1) {
          errors.push(`batch ${batch.batchId} file ${index} emitted importing progress ${importing.length} times; expected exactly once`);
        }
        if (queued.length === 1 && queued[0].at <= batch.droppedAt) {
          errors.push(`batch ${batch.batchId} file ${index} queued before its dropped event`);
        }
        if (queued.length === 1 && importing.length === 1 && importing[0].at <= queued[0].at) {
          errors.push(`batch ${batch.batchId} file ${index} imported before it was queued`);
        }
        if (importing.length === 1 && terminalRecord.at <= importing[0].at) {
          errors.push(`batch ${batch.batchId} file ${index} reached ${terminal.phase} before importing progress`);
        }
        if (delayed.some(({ at }) => (importing.length === 1 && at <= importing[0].at) || at >= terminalRecord.at)) {
          errors.push(`batch ${batch.batchId} file ${index} emitted delayed progress outside its importing interval`);
        }
      }
      files.push({
        index,
        name: droppedFile.name,
        supported: droppedFile.supported,
        progressStages: stages.map(({ stage }) => stage),
        eventIndices: {
          dropped: batch.droppedAt + 1,
          progress: stages.map(({ at }) => at + 1),
          terminal: terminalRecord ? terminalRecord.at + 1 : null,
        },
        terminalPhase: terminal?.phase ?? null,
        subject: terminal?.subject ?? null,
      });
    }
    const matchingHover = hovered.find(({ event, at }) => {
      if (at >= batch.droppedAt || !Array.isArray(event.files) || event.files.length !== batch.files.length) return false;
      return event.files.every((file, index) =>
        file?.name === batch.files[index]?.name && file?.supported === batch.files[index]?.supported);
    });
    if (!matchingHover) errors.push(`no matching hovered event preceded dropped batch ${batch.batchId}`);
    batchSummary.push({ batchId: batch.batchId, total: batch.files.length, files });
  }

  if (batches.size === 0) pending.push("no dropped batch has been recorded");

  return {
    complete: batches.size > 0 && errors.length === 0 && failures.length === 0 && pending.length === 0,
    hovered: hovered.map(({ event }) => event.files ?? []),
    batches: batchSummary,
    errors,
    failures,
    pending,
  };
}

/**
 * Did the drag helper refuse the gesture because a window that is NOT the editor covered the drop point?
 *
 * The distinction is the whole point. A foreign window taking the foreground mid-drag (a console another
 * tool spawned, a notification toast, a driver overlay) is an accident of a shared desktop and is worth
 * retrying. The editor covering its OWN drop target is a product defect — a modal that swallows drops —
 * and retrying that would turn a real bug into a flaky test that eventually passes.
 *
 * WHY THIS LIVES HERE AND HAS A TEST
 * ----------------------------------
 * The previous version of this predicate was a private function in the cinematic spec that matched
 *
 *   /DROP_TARGET: pid=\d+ hwnd=(\d+)/
 *
 * against helper output whose only DROP_TARGET line is, and always has been,
 *
 *   DROP_TARGET: hwnd=2295898 title='metrocalk editor'
 *
 * There is no `pid=` in it. The match therefore returned null on every log ever produced, the predicate
 * returned false unconditionally, and `startOleDropSurvivingOcclusion` — a loop written to take up to
 * three attempts — could never take its second. The one production run that a foreign window stole
 * ended, after a single attempt, in an error explaining that a retry was not warranted.
 *
 * Two of the six previous production runs died in this step. The retry meant to survive that was, in
 * effect, not installed. So the predicate is now exported, and the tests beside it are fed the VERBATIM
 * output of a real helper run: a format drift breaks the test rather than silently disarming the retry.
 *
 * @param {string} log combined stdout+stderr of one `scripts/ole-drop-file.ps1` attempt
 * @returns {boolean} true when the gesture was refused because something else owned the drop point
 */
export function occludedByForeignWindow(log) {
  const gesture = /DROP_GESTURE: target-hit-test-failed: under=\d+ root=(\d+)/.exec(log ?? "");
  // No hit-test failure at all: the attempt died of something else, and this predicate has no opinion.
  if (!gesture) return false;
  const target = /DROP_TARGET: hwnd=(\d+)/.exec(log ?? "");
  if (!target) {
    // "I could not read the log" must never be delivered as "the editor blocked itself". That
    // substitution is the whole reason this function needed rewriting.
    throw new Error(
      "The drag helper reported a hit-test failure but printed no parsable DROP_TARGET line, so whether "
        + `a foreign window owned the drop point cannot be answered from this log:\n${log}`,
    );
  }
  return gesture[1] !== target[1];
}

/**
 * Did the synthetic gesture strand before the application was ever involved?
 *
 * `ole-drop-file.ps1` prints its first four lines while setting up — the resolved target, the geometry,
 * the file, and what is under the cursor at the source point — and everything after that only once the
 * drag has produced an outcome. A log that stops after `DROP_UNDER_CURSOR`, with no `DROP_GESTURE` and
 * no `DROP_RESULT`, is a drag that never finished: the helper is still inside ole32's modal loop.
 *
 * That is worth retrying and a hit-test refusal naming the editor's own root is not, for the same
 * reason in both directions: this one never asked the application for anything, so it cannot be evidence
 * of a defect in the application.
 *
 * HONEST LIMIT, because this predicate decides whether a failure is allowed a second chance: a hang
 * inside the application's OWN `DragEnter` would leave exactly this log, since the helper prints nothing
 * between starting the drag and its outcome. The helper's output cannot separate the two. What can is
 * the run's own lifecycle evidence — the app emits a `hovered` phase when it accepts the drag, and
 * `native-import-lifecycle.json` preserves it for every run. A stranded gesture that DID reach the
 * application will have that record; one that never reached it will not. If this retry ever starts
 * firing repeatedly, that file is the thing to read before widening it further.
 *
 * @param {string} log combined stdout+stderr of one `scripts/ole-drop-file.ps1` attempt
 * @returns {boolean} true when the gesture produced no outcome at all
 */
export function gestureStrandedBeforeTheApplication(log) {
  const text = String(log ?? "");
  if (!/DROP_UNDER_CURSOR:/.test(text)) return false;
  return !/DROP_GESTURE:/.test(text) && !/DROP_RESULT:/.test(text);
}
