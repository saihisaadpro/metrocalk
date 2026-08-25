// Regression gate for the window the harness aims at.
//
// The packaged app publishes TWO visible, unowned, top-level windows:
//   class 'Tauri Window'             ~1296x839  <- the composited host
//   class 'Tao Thread Event Target'      16x16  <- tao's message-loop event target
//
// `Process.MainWindowHandle` returns the first visible unowned top-level window in Z-order, so it
// matches either one non-deterministically. When it picked the event target, the screenshot gate wrote
// a 16x16 PNG and threw "unexpectedly small (158 bytes)", the OLE drop aimed at a 16x16 rect, and the
// film crop would have framed 256 pixels. Those read as three unrelated flakes; they were one wrong
// window. Every harness script must therefore resolve the host through lib/app-window.ps1, which
// selects on the window class and size-checks the result.

import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const scriptsDir = path.dirname(fileURLToPath(import.meta.url));
const helper = path.join(scriptsDir, "lib", "app-window.ps1");

/** Every harness PowerShell script except the shared helper itself. */
const harnessScripts = fs
  .readdirSync(scriptsDir)
  .filter((name) => name.endsWith(".ps1"))
  .map((name) => path.join(scriptsDir, name));

test("the shared host-window resolver exists and selects on window class", () => {
  assert.ok(fs.existsSync(helper), `Missing shared resolver: ${helper}`);
  const source = fs.readFileSync(helper, "utf8");
  assert.match(source, /\$script:MetrocalkHostWindowClass\s*=\s*"Tauri Window"/,
    "The resolver must select the host by its 'Tauri Window' class.");
  assert.match(source, /function Get-MetrocalkAppWindow/,
    "The resolver must expose Get-MetrocalkAppWindow.");
  assert.match(source, /MetrocalkMinimumHostEdge/,
    "The resolver must size-check the window it returns, so a class rename fails loudly.");
});

/**
 * PowerShell source with comments removed, so this gate grades code rather than the prose explaining
 * why the code looks the way it does. (A comment-blind grep is exactly how a sibling gate in this repo
 * started reporting its own doc comment as a violation.)
 *
 * BOTH comment forms, and the second one is here because leaving it out would have left the same door
 * open one shape over: `ole-drop-file.ps1` opens with a 45-line `<# … #>` block, and had
 * `keep-display-awake.ps1` written its rationale as comment-based help rather than `#` lines, the
 * repair below would not have worked at all. Line-oriented on purpose — a `<#` that is not the first
 * thing on its line is left alone, because in PowerShell it may be inside a string, and a filter that
 * corrupts the lines it is supposed to grade is worse than one that reads a comment.
 *
 * A `#` mid-line is likewise left alone for the same reason, which `trailing.ps1` in the fixtures pins:
 * a line that calls and then comments is a call.
 */
function executableSource(file) {
  const out = [];
  let inBlock = false;
  for (const line of fs.readFileSync(file, "utf8").split(/\r?\n/)) {
    if (inBlock) {
      if (line.includes("#>")) inBlock = false;
      continue;
    }
    if (/^\s*<#/.test(line)) {
      // A one-line `<# … #>` opens and closes on the same line; only a block that stays open sets the flag.
      if (!line.includes("#>")) inBlock = true;
      continue;
    }
    if (/^\s*#/.test(line)) continue;
    out.push(line);
  }
  return out.join("\n");
}

test("no harness script selects its target window via MainWindowHandle", () => {
  const offenders = harnessScripts.filter((file) =>
    executableSource(file).includes("MainWindowHandle"));
  assert.deepEqual(
    offenders.map((file) => path.basename(file)),
    [],
    "MainWindowHandle also matches tao's 16x16 event target; use Get-MetrocalkAppWindow instead.",
  );
});

/**
 * The window-targeting Win32 surface. A script that calls any of these has a HWND in its hands and
 * must have got it from the shared resolver.
 *
 * THE LIST IS THE GATE'S RESOLUTION, so it is written wider than the calls that happen to appear
 * today. The first version named five, and the four scripts it caught were caught only because each
 * of them *also* used one of those five — `window-client-rect.ps1` reaches for `ClientToScreen`,
 * `WindowFromPoint`, `GetAncestor` and `GetWindowText`; `ole-drop-file.ps1` adds `IsWindowVisible`,
 * `ShowWindow`, `SetForegroundWindow`, `PostMessage` and `EnumWindows`. A future script whose only
 * HWND operation was `MoveWindow` would have walked straight past. Widening changes no verdict on the
 * tree as it stands (the same four are selected, the other four score zero, and the three `.ps1` in
 * `e2e/` itself score zero), which is the point: a reach extended while the answers stay put.
 */
const WINDOW_TARGETING_CALLS =
  /GetWindowRect|SetWindowPos|PrintWindow|GetClientRect|BringWindowToTop|MoveWindow|ShowWindow|SetForegroundWindow|GetForegroundWindow|IsIconic|IsWindowVisible|ClientToScreen|WindowFromPoint|GetAncestor|GetWindowText|SetWindowText|PostMessage|EnumWindows/;

/**
 * Whether `file` targets a window — judged on the code, never on the prose.
 *
 * This reads `executableSource` and not the raw file, and that distinction is the whole test:
 * `keep-display-awake.ps1` explains, in a comment, why the film needs the display awake when
 * `captureComposited` **uses PrintWindow** — and on the raw text that sentence is indistinguishable
 * from a call. The script owns no HWND, has nothing to resolve, and was condemned for its own
 * documentation. That is exactly the failure mode `executableSource`'s doc comment already names two
 * functions above; this gate made it anyway, one test lower down, which is why the predicate is now a
 * named function with fixtures of its own rather than a regex inlined in the assertion that uses it.
 */
function targetsAWindow(file) {
  return WINDOW_TARGETING_CALLS.test(executableSource(file));
}

/**
 * The rule itself, as data: one complaint per script that holds a HWND without having resolved it.
 * An empty array means the rule holds. It is a function rather than an inlined loop so the fixtures
 * below can grade the *same* code the real tree is graded by — one source of truth for the rule and
 * for the cases that prove the rule can still tell right from wrong.
 */
function unresolvedTargeting(files) {
  const complaints = [];
  for (const file of files.filter(targetsAWindow)) {
    const source = executableSource(file);
    const name = path.basename(file);
    if (!/lib\/app-window\.ps1/.test(source)) {
      complaints.push(`${name} targets a window without dot-sourcing lib/app-window.ps1`);
    } else if (!/Get-MetrocalkAppWindow/.test(source)) {
      complaints.push(`${name} dot-sources the resolver but never calls Get-MetrocalkAppWindow`);
    }
  }
  return complaints;
}

test("the targeting rule reads code, not the prose about the code", (t) => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "mtk-app-window-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));

  const write = (name, body) => {
    const file = path.join(dir, name);
    fs.writeFileSync(file, body);
    return file;
  };

  const RESOLVED = '. "$PSScriptRoot/lib/app-window.ps1"\n$hwnd = Get-MetrocalkAppWindow\n';

  // The real shape of the false positive: the call named only inside a whole-line comment.
  assert.equal(
    targetsAWindow(write("mentions.ps1", "# captureComposited uses PrintWindow, which reads the\n# window's own presentation.\nStart-Sleep -Seconds 20\n")),
    false,
    "A call named only in a comment is documentation, not a window being targeted.",
  );
  // Indented comments are comments too — the leading-whitespace case the filter allows for.
  assert.equal(
    targetsAWindow(write("indented.ps1", "if ($true) {\n    # SetWindowPos is what resize-window.ps1 uses.\n    Write-Output 'hi'\n}\n")),
    false,
    "An indented comment is still a comment.",
  );
  // The OTHER comment form. `ole-drop-file.ps1` opens with a 45-line one, so a filter that only knew
  // about `#` would have left this exact door open one shape over.
  assert.equal(
    targetsAWindow(write("block.ps1", "<#\n  .DESCRIPTION\n  Unlike PrintWindow, this reads the desktop.\n#>\nStart-Sleep -Seconds 20\n")),
    false,
    "A `<# … #>` block is documentation too.",
  );
  assert.equal(
    targetsAWindow(write("block-indent.ps1", "function F {\n  <#\n    GetClientRect is what window-client-rect.ps1 uses.\n  #>\n  Write-Output 'hi'\n}\n")),
    false,
    "An indented block comment is still a block comment.",
  );
  // …and the block must CLOSE, or everything after the first one would be invisible to the gate.
  assert.equal(
    targetsAWindow(write("after-block.ps1", "<#\n  prose\n#>\n$r = [Mtk.Win]::GetWindowRect($hwnd, [ref]$rect)\n")),
    true,
    "Code after a closed block comment is still code.",
  );
  // And the predicate must still SEE a real call, or making it comment-blind would blind it entirely.
  assert.equal(
    targetsAWindow(write("targets.ps1", "# Resolve the host first.\n$rect = [Mtk.Win]::GetWindowRect($hwnd, [ref]$r)\n")),
    true,
    "A call in executable code must still be found, or the gate stops gating.",
  );
  assert.equal(
    targetsAWindow(write("trailing.ps1", "[void][Mtk.Win]::PrintWindow($hwnd, $dc, 0)  # grade the pixels after\n")),
    true,
    "A line that calls and then comments is a call.",
  );
  // A HWND operation none of today's scripts happens to use ALONE. This is the case the first, narrower
  // list would have missed entirely, and it is here so the widened reach is asserted rather than assumed.
  assert.equal(
    targetsAWindow(write("moveonly.ps1", "[void][Mtk.Win]::MoveWindow($hwnd, 0, 0, 1296, 839, $true)\n")),
    true,
    "A script whose only window call is MoveWindow is still targeting a window.",
  );

  // The rule, not just the predicate: a script that really does target a window is still caught…
  assert.deepEqual(
    unresolvedTargeting([write("offender.ps1", "$r = [Mtk.Win]::GetWindowRect($hwnd, [ref]$rect)\n")]),
    ["offender.ps1 targets a window without dot-sourcing lib/app-window.ps1"],
  );
  // …and is NOT let off by naming the resolver in a comment, which is the same blindness one level up.
  assert.deepEqual(
    unresolvedTargeting([write("commented.ps1", "# resolve via lib/app-window.ps1 -> Get-MetrocalkAppWindow\n$r = [Mtk.Win]::GetWindowRect($hwnd, [ref]$rect)\n")]),
    ["commented.ps1 targets a window without dot-sourcing lib/app-window.ps1"],
  );
  // Dot-sourced but never called is its own complaint, so the two halves cannot cover for each other.
  assert.deepEqual(
    unresolvedTargeting([write("halfway.ps1", '. "$PSScriptRoot/lib/app-window.ps1"\n$r = [Mtk.Win]::GetWindowRect($hwnd, [ref]$rect)\n')]),
    ["halfway.ps1 dot-sources the resolver but never calls Get-MetrocalkAppWindow"],
  );
  // A properly resolved script, and a script that only talks about windows, are both clean.
  assert.deepEqual(
    unresolvedTargeting([
      write("clean.ps1", `${RESOLVED}$r = [Mtk.Win]::GetWindowRect($hwnd, [ref]$rect)\n`),
      write("prose.ps1", "# PrintWindow reads the window's own presentation.\nStart-Sleep -Seconds 20\n"),
    ]),
    [],
  );
});

test("every script that targets a window dot-sources the shared resolver", () => {
  const targeting = harnessScripts.filter(targetsAWindow);
  // The reach is asserted and printed: a run that graded four scripts and a run that graded none
  // otherwise produce the same silence.
  assert.ok(targeting.length >= 4,
    `Expected the window-targeting scripts to be found, saw ${targeting.length}: ` +
    `${targeting.map((f) => path.basename(f)).join(", ") || "none"}.`);
  assert.deepEqual(unresolvedTargeting(harnessScripts), []);
});

test("the resolver can name a foreign full-screen window that minimises the editor", () => {
  const source = fs.readFileSync(helper, "utf8");
  assert.match(source, /function Get-MetrocalkForegroundConflict/,
    "The harness must be able to name the application holding the foreground.");
  assert.match(source, /IsIconic/,
    "A minimised window reports the icon rect; the resolver must say so rather than just 'too small'.");
});

test("the drop preflights the foreground before entering ole32's modal loop", () => {
  const drop = path.join(scriptsDir, "ole-drop-file.ps1");
  const source = fs.readFileSync(drop, "utf8");
  assert.match(source, /Get-MetrocalkForegroundConflict/,
    "A drag onto a re-minimised window spins until SIGTERM; preflight it instead.");
  assert.match(source, /DROP_RESULT: FAIL foreground-conflict/,
    "The preflight must report a distinct, greppable failure token.");
});

test("the drag has an upper bound on the path that actually executes", () => {
  const drop = path.join(scriptsDir, "ole-drop-file.ps1");
  const source = executableSource(path.join(scriptsDir, "ole-drop-file.ps1"));
  // The hand-written IDropSource that used to carry the deadline was never instantiated.
  assert.ok(!/class DropSource/.test(source),
    "The dead IDropSource must stay deleted; it bounded nothing while reading as if it did.");
  assert.match(fs.readFileSync(drop, "utf8"), /add_QueryContinueDrag/,
    "Control.DoDragDrop needs a QueryContinueDrag deadline, or the gesture is unbounded.");
  assert.match(fs.readFileSync(drop, "utf8"), /PostReleaseNudgeMs/,
    "OLE's loop must keep receiving input after the button-up or it may never observe the release.");
});

test("the screenshot gate grades PrintWindow's pixels before trusting its TRUE", () => {
  const capture = path.join(scriptsDir, "capture-composited-window.ps1");
  const source = fs.readFileSync(capture, "utf8");
  assert.match(source, /function Test-CaptureHasContrast/,
    "PrintWindow reports success for unpainted bitmaps; the capture must grade contrast itself.");
  // The BitBlt fallback is only reachable if a blank PrintWindow result leaves $printed unset.
  const printedGuard = /if \(Test-CaptureHasContrast -Image \$image\) \{\s*\$image\.Save\([^)]*\)\s*\$printed = \$true/;
  assert.match(source, printedGuard,
    "$printed must be set only for a frame with real contrast, or the BitBlt fallback is dead code.");
});
