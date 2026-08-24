//! Typed subscription and pure view-state reduction for the native OS drop lifecycle.
//!
//! The desktop bundle intentionally uses Tauri's injected global instead of taking a second dependency on
//! `@tauri-apps/api`. Keeping the boundary here gives React one stable contract and makes late events from an
//! older queued batch unable to overwrite the newer drop the user is looking at.

export const NATIVE_IMPORT_LIFECYCLE_EVENT = "mtk://import-lifecycle";

export interface DropFile {
  name: string;
  supported: boolean;
  reason?: string;
}

export type ImportedSubject =
  | { kind: "entity"; rootId: string }
  | { kind: "environment"; label: string };

export type NativeImportLifecycleEvent =
  | { phase: "hovered"; files: DropFile[] }
  | { phase: "left" }
  | { phase: "dropped"; batchId: number; files: DropFile[] }
  | {
      phase: "progress";
      batchId: number;
      index: number;
      total: number;
      completed: number;
      fileName: string;
      stage: "queued" | "importing" | "delayed";
      elapsedMs: number;
    }
  | {
      phase: "succeeded";
      batchId: number;
      index: number;
      total: number;
      fileName: string;
      subject: ImportedSubject;
      message: string;
    }
  | {
      phase: "refused";
      batchId: number | null;
      index: number;
      total: number;
      fileName: string;
      message: string;
      recoverable: boolean;
    }
  | {
      phase: "failed";
      batchId: number;
      index: number;
      total: number;
      fileName: string;
      message: string;
      recoverable: boolean;
    }
  | {
      phase: "cancelled";
      batchId: number;
      index: number;
      total: number;
      fileName: string;
      message: string;
    };

export interface NativeImportViewState {
  currentBatchId: number;
  event: NativeImportLifecycleEvent | null;
  stoppingBatchId: number | null;
}

export const EMPTY_NATIVE_IMPORT_STATE: NativeImportViewState = {
  currentBatchId: 0,
  event: null,
  stoppingBatchId: null,
};

const batchIdOf = (event: NativeImportLifecycleEvent): number | null => {
  switch (event.phase) {
    case "dropped":
    case "progress":
    case "succeeded":
    case "failed":
    case "cancelled":
      return event.batchId;
    case "refused":
      return event.batchId;
    default:
      return null;
  }
};

/** Reduce the currently visible lifecycle event; referential equality tells callers to ignore stale effects too. */
export function reduceNativeImportState(
  state: NativeImportViewState,
  event: NativeImportLifecycleEvent,
): NativeImportViewState {
  if (event.phase === "left") {
    // A Leave after a completed Drop must not hide the import now running in the worker.
    return state.event?.phase === "hovered" ? { ...state, event: null } : state;
  }
  if (event.phase === "hovered") return { ...state, event };

  const batchId = batchIdOf(event);
  if (batchId != null && batchId < state.currentBatchId) return state;
  const terminal = event.phase === "succeeded"
    || event.phase === "refused"
    || event.phase === "failed"
    || event.phase === "cancelled";
  return {
    currentBatchId: batchId == null ? state.currentBatchId : Math.max(state.currentBatchId, batchId),
    event,
    stoppingBatchId: terminal || (batchId != null && batchId !== state.stoppingBatchId)
      ? null
      : state.stoppingBatchId,
  };
}

/** Project the user's accepted intent while Rust works toward the next safe checkpoint. */
export function markNativeImportStopping(
  state: NativeImportViewState,
  batchId: number,
): NativeImportViewState {
  const event = state.event;
  if (
    !event
    || (event.phase !== "dropped" && event.phase !== "progress")
    || event.batchId !== batchId
    || state.stoppingBatchId === batchId
  ) {
    return state;
  }
  return { ...state, stoppingBatchId: batchId };
}

/** Re-open the action only when the direct command could not accept the request. */
export function clearNativeImportStopping(
  state: NativeImportViewState,
  batchId: number,
): NativeImportViewState {
  return state.stoppingBatchId === batchId ? { ...state, stoppingBatchId: null } : state;
}

type Unlisten = () => void;
interface TauriEventEnvelope<T> {
  event: string;
  id: number;
  payload: T;
}
interface TauriEventApi {
  listen<T>(event: string, handler: (event: TauriEventEnvelope<T>) => void): Promise<Unlisten>;
}
interface TauriCoreApi {
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
}

const eventApi = (): TauriEventApi | null => {
  const global = globalThis as unknown as { __TAURI__?: { event?: TauriEventApi } };
  return global.__TAURI__?.event ?? null;
};

const coreApi = (): TauriCoreApi | null => {
  const global = globalThis as unknown as { __TAURI__?: { core?: TauriCoreApi } };
  return global.__TAURI__?.core ?? null;
};

/** Subscribe once to the Rust lifecycle stream. Browser-only previews intentionally become a no-op. */
export async function subscribeNativeImportLifecycle(
  handler: (event: NativeImportLifecycleEvent) => void,
): Promise<Unlisten> {
  const api = eventApi();
  if (!api) return () => {};
  return api.listen<NativeImportLifecycleEvent>(NATIVE_IMPORT_LIFECYCLE_EVENT, ({ payload }) => {
    handler(payload);
  });
}

/** Ask the Rust-owned direct control plane to stop one native-drop batch. */
export async function cancelNativeImport(batchId: number): Promise<boolean> {
  const api = coreApi();
  if (!api) return false;
  return api.invoke<boolean>("cancel_native_import", { batchId });
}
