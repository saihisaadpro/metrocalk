import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import {
  analyzeNativeImportLifecycle,
  createNativeCadDropRunId,
  exactNamedChild,
  gestureStrandedBeforeTheApplication,
  occludedByForeignWindow,
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

/**
 * The fixture is the VERBATIM stdout of the drag helper from the production run of 2026-08-25 that was
 * lost to this defect. Any change to the helper's output format now breaks this test instead of
 * silently disarming the retry that depends on parsing it.
 */
const refusedByForeignWindow = `DROP_TARGET: hwnd=2295898 title='metrocalk editor'
DROP_GEOM: client=1280x800 start=(1316,299) drop=(868,627)
DROP_FILE: X:\Work\Metrocalk\Games Projects\Unreal\Skid Weld Line A.1\Skid Weld Line A.1_(1).stp
DROP_UNDER_CURSOR: hwnd=1378478 root=2295898 expectedRoot=2295898
DROP_GESTURE: target-hit-test-failed: under=854098 root=67116
DROP_PRESS: pressed visible=True rectOk=True rect=1206,264..1442,373 point=1324,330
DROP_WATCHDOG: disarmed
DROP_SOURCE_GEOM: measured=(1324,330) requested=(1316,299)
DROP_SOURCE: started=True status=ole-returned
DROP_EFFECT: None elapsedMs=899 ending=button-released
DROP_GESTURE: target-hit-test-failed: under=854098 root=67116
DROP_RESULT: CANCELLED
`;

test("a drop point stolen by a foreign window is retryable, and the editor blocking itself is not", () => {
  // The regression: this exact log used to answer `false`, because the pattern being matched
  // (`DROP_TARGET: pid=<n> hwnd=<n>`) is not what the helper prints. The retry loop guarded by it
  // could never take a second attempt, and two production runs were lost in this step.
  assert.equal(occludedByForeignWindow(refusedByForeignWindow), true);

  // The editor covering its own drop target is a PRODUCT defect. Retrying it would convert a real bug
  // into a test that eventually passes, so the same shape with the editor's own root must not retry.
  const blockedByItself = refusedByForeignWindow.replaceAll("root=67116", "root=2295898");
  assert.equal(occludedByForeignWindow(blockedByItself), false);

  // A failure that is not a hit-test refusal at all is not this function's business.
  assert.equal(occludedByForeignWindow("DROP_TARGET: hwnd=2295898 title='metrocalk editor'\nDROP_RESULT: CANCELLED\n"), false);
  assert.equal(occludedByForeignWindow(""), false);

  // And an unreadable log must say so rather than answer "not occluded", which is the substitution
  // that made the original defect invisible.
  assert.throws(
    () => occludedByForeignWindow("DROP_GESTURE: target-hit-test-failed: under=854098 root=67116\n"),
    /no parsable DROP_TARGET line/,
  );
});

/**
 * Verbatim stdout of the third gesture of the 2026-08-25 production run, which stranded: the helper
 * reported its target, its geometry, its file and what was under the cursor, and then said nothing for
 * thirty seconds because it was still inside ole32's modal drag loop.
 */
const strandedBeforeTheApplication = `DROP_TARGET: hwnd=5374260 title='metrocalk editor'
DROP_GEOM: client=1680x979 start=(1512,160) drop=(924,561)
DROP_FILE: X:\Work\Metrocalk\Games Projects\Unreal\Skid Weld Line A.1\Skid Weld Line A.1_(1).stp
DROP_UNDER_CURSOR: hwnd=1706486 root=5374260 expectedRoot=5374260
`;

test("a gesture that produced no outcome never involved the application, and may be retried", () => {
  assert.equal(gestureStrandedBeforeTheApplication(strandedBeforeTheApplication), true);

  // A gesture that reached an outcome — any outcome — is not stranded, whatever that outcome was. The
  // refusal below is the application declining the drop, which is a result and must never be retried
  // as though the drag had not happened.
  assert.equal(gestureStrandedBeforeTheApplication(refusedByForeignWindow), false);
  assert.equal(
    gestureStrandedBeforeTheApplication(`${strandedBeforeTheApplication}DROP_RESULT: CANCELLED\n`),
    false,
  );

  // And a helper that died before it even resolved its target is a different failure with a different
  // cause; this predicate has no opinion on it.
  assert.equal(gestureStrandedBeforeTheApplication("DROP_TARGET: hwnd=5374260 title='metrocalk editor'\n"), false);
  assert.equal(gestureStrandedBeforeTheApplication(""), false);
});
