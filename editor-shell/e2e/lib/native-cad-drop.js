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
