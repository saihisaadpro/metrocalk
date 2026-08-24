import { describe, expect, it } from "vitest";
import {
  clearNativeImportStopping,
  EMPTY_NATIVE_IMPORT_STATE,
  markNativeImportStopping,
  reduceNativeImportState,
  type NativeImportLifecycleEvent,
} from "./nativeImportLifecycle";

const progress = (
  batchId: number,
  fileName = `batch-${batchId}.stp`,
): Extract<NativeImportLifecycleEvent, { phase: "progress" }> => ({
  phase: "progress",
  batchId,
  index: 1,
  total: 1,
  completed: 0,
  fileName,
  stage: "importing",
  elapsedMs: 250,
});

describe("native import lifecycle reduction", () => {
  it("clears a hover on leave but keeps a dropped batch visible", () => {
    const hovered = reduceNativeImportState(EMPTY_NATIVE_IMPORT_STATE, {
      phase: "hovered",
      files: [{ name: "assembly.step", supported: true }],
    });
    const leftHover = reduceNativeImportState(hovered, { phase: "left" });
    expect(leftHover.event).toBeNull();

    const dropped = reduceNativeImportState(leftHover, {
      phase: "dropped",
      batchId: 4,
      files: [{ name: "assembly.step", supported: true }],
    });
    expect(reduceNativeImportState(dropped, { phase: "left" })).toBe(dropped);
  });

  it("accepts the newest batch and ignores late events from older batches", () => {
    const batch12 = reduceNativeImportState(EMPTY_NATIVE_IMPORT_STATE, progress(12));
    const lateBatch11: NativeImportLifecycleEvent = {
      phase: "succeeded",
      batchId: 11,
      index: 1,
      total: 1,
      fileName: "old.step",
      subject: { kind: "entity", rootId: "old-root" },
      message: "Imported old.step",
    };

    expect(reduceNativeImportState(batch12, lateBatch11)).toBe(batch12);

    const batch13 = reduceNativeImportState(batch12, progress(13));
    expect(batch13.currentBatchId).toBe(13);
    expect(batch13.event).toMatchObject({ phase: "progress", batchId: 13 });
    expect(reduceNativeImportState(batch13, progress(12))).toBe(batch13);
  });

  it("marks only a visible busy batch as stopping and clears it on terminal cancellation", () => {
    const importing = reduceNativeImportState(EMPTY_NATIVE_IMPORT_STATE, progress(31));
    expect(markNativeImportStopping(importing, 30)).toBe(importing);

    const stopping = markNativeImportStopping(importing, 31);
    expect(stopping.stoppingBatchId).toBe(31);
    const delayed = reduceNativeImportState(stopping, {
      ...progress(31),
      stage: "delayed",
      elapsedMs: 9_000,
    });
    expect(delayed.stoppingBatchId).toBe(31);

    const cancelled = reduceNativeImportState(delayed, {
      phase: "cancelled",
      batchId: 31,
      index: 1,
      total: 1,
      fileName: "batch-31.stp",
      message: "Stopped at a safe checkpoint. No scene changes were committed.",
    });
    expect(cancelled.stoppingBatchId).toBeNull();
    expect(cancelled.event).toMatchObject({ phase: "cancelled", batchId: 31 });

    const retry = reduceNativeImportState(cancelled, progress(32, "retry.step"));
    expect(retry.currentBatchId).toBe(32);
    expect(retry.stoppingBatchId).toBeNull();
    expect(retry.event).toMatchObject({ phase: "progress", fileName: "retry.step" });
  });

  it("re-opens cancellation only for the same still-busy batch", () => {
    const importing = reduceNativeImportState(EMPTY_NATIVE_IMPORT_STATE, progress(40));
    const stopping = markNativeImportStopping(importing, 40);
    expect(clearNativeImportStopping(stopping, 39)).toBe(stopping);
    expect(clearNativeImportStopping(stopping, 40).stoppingBatchId).toBeNull();
  });
});
