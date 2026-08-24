import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import {
  analyzeNativeImportLifecycle,
  createNativeCadDropRunId,
  exactNamedChild,
} from "./native-cad-drop.js";

const here = path.dirname(fileURLToPath(import.meta.url));

const successStream = () => [
  { phase: "hovered", files: [{ name: "a.step", supported: true }, { name: "b.step", supported: true }] },
  { phase: "dropped", batchId: 7, files: [{ name: "a.step", supported: true }, { name: "b.step", supported: true }] },
  { phase: "progress", batchId: 7, index: 1, total: 2, completed: 0, fileName: "a.step", stage: "queued", elapsedMs: 0 },
  { phase: "progress", batchId: 7, index: 1, total: 2, completed: 0, fileName: "a.step", stage: "importing", elapsedMs: 0 },
  { phase: "succeeded", batchId: 7, index: 1, total: 2, fileName: "a.step", subject: { kind: "entity", rootId: "root-a" }, message: "Imported a.step" },
  { phase: "progress", batchId: 7, index: 2, total: 2, completed: 1, fileName: "b.step", stage: "queued", elapsedMs: 0 },
  { phase: "progress", batchId: 7, index: 2, total: 2, completed: 1, fileName: "b.step", stage: "importing", elapsedMs: 0 },
  { phase: "succeeded", batchId: 7, index: 2, total: 2, fileName: "b.step", subject: { kind: "entity", rootId: "root-b" }, message: "Imported b.step" },
];

test("lifecycle analysis requires every file in a batch to reach ordered success", () => {
  const complete = analyzeNativeImportLifecycle(successStream());
  assert.equal(complete.complete, true);
  assert.equal(complete.batches[0].files.length, 2);

  const missingSecondTerminal = analyzeNativeImportLifecycle(successStream().slice(0, -1));
  assert.equal(missingSecondTerminal.complete, false);
  assert.match(missingSecondTerminal.pending.join("\n"), /file 2\/2.*no terminal/i);

  const reversed = successStream();
  [reversed[2], reversed[3]] = [reversed[3], reversed[2]];
  const reversedResult = analyzeNativeImportLifecycle(reversed);
  assert.equal(reversedResult.complete, false);
  assert.match(reversedResult.errors.join("\n"), /imported before it was queued/i);

  const duplicateQueued = successStream();
  duplicateQueued.splice(3, 0, { ...duplicateQueued[2] });
  const duplicateResult = analyzeNativeImportLifecycle(duplicateQueued);
  assert.equal(duplicateResult.complete, false);
  assert.match(duplicateResult.errors.join("\n"), /queued progress 2 times/i);
});

test("refusal, failure, and skipped progress cannot be mistaken for completion", () => {
  const failed = successStream().slice(0, 4);
  failed.push({ phase: "failed", batchId: 7, index: 1, total: 2, fileName: "a.step", message: "decode failed", recoverable: true });
  const result = analyzeNativeImportLifecycle(failed);
  assert.equal(result.complete, false);
  assert.match(result.failures.join("\n"), /decode failed/);
  assert.match(result.pending.join("\n"), /file 2\/2/);

  const skipped = successStream().filter((event) => !(event.phase === "progress" && event.index === 1 && event.stage === "queued"));
  const skippedResult = analyzeNativeImportLifecycle(skipped);
  assert.equal(skippedResult.complete, false);
  assert.match(skippedResult.errors.join("\n"), /queued progress 0 times/);

  const cancelled = successStream();
  cancelled[4] = { phase: "cancelled", batchId: 7, index: 1, total: 2, fileName: "a.step", message: "Stopped at a safe checkpoint" };
  const cancelledResult = analyzeNativeImportLifecycle(cancelled);
  assert.equal(cancelledResult.complete, false);
  assert.match(cancelledResult.failures.join("\n"), /cancelled.*safe checkpoint/i);
});

test("hover evidence must match the batch and occur before the drop", () => {
  const hoverAfterDrop = successStream();
  hoverAfterDrop.splice(1, 0, hoverAfterDrop.shift());
  const result = analyzeNativeImportLifecycle(hoverAfterDrop);
  assert.equal(result.complete, false);
  assert.match(result.errors.join("\n"), /no matching hovered event preceded dropped batch 7/i);
});

test("exact named children reject traversal and run ids are filesystem-safe", () => {
  const parent = path.resolve("X:/evidence/run");
  assert.equal(exactNamedChild(parent, "metrocalk-scene.jsonl"), path.join(parent, "metrocalk-scene.jsonl"));
  assert.throws(() => exactNamedChild(parent, "../metrocalk-scene.jsonl"), /unsafe child/i);
  assert.throws(() => exactNamedChild(parent, "nested/metrocalk-scene.jsonl"), /unsafe child/i);
  assert.match(createNativeCadDropRunId(new Date("2026-08-23T10:20:30.000Z"), 42), /^native-cad-drop-[A-Za-z0-9]+-42$/);
});

test("the regression spec reaches import only through the OLE helper", () => {
  const spec = fs.readFileSync(path.resolve(here, "../specs-native-cad-drop/native-cad-drop.e2e.js"), "utf8");
  assert.match(spec, /ole-drop-file\.ps1/);
  assert.doesNotMatch(spec, /["']import_asset["']/);
});
