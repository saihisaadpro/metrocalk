// `diff-region.ps1` measured against pictures whose answer is known by construction.
//
// The instrument this checks is the one the stage-hover spec's every claim rests on, and the failure
// it is written against is the one this repository keeps meeting: an instrument that returns a
// plausible number for the wrong thing. `mesh_frame_bench.rs` compiled through four renderer
// generations while every contract in it rotted; the film gate's legibility metric measured the docks
// instead of the viewport and could not fail. A diff that reported "changed" for every pixel, or zero
// for all of them, would make the hover spec pass or fail for reasons that have nothing to do with the
// hover — so each case here paints a rectangle of known size and asserts the count it must produce.
//
// Windows-only by nature (System.Drawing + PowerShell), like every other capture instrument here. On
// any other platform it reports that and exits clean rather than pretending to have measured.
//
// Run: node scripts/diff-region.test.mjs

import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const dir = path.dirname(fileURLToPath(import.meta.url));
const script = path.join(dir, "diff-region.ps1");

let failures = 0;
const check = (name, actual, expected) => {
  const ok = typeof expected === "function" ? expected(actual) : actual === expected;
  if (ok) console.log(`pass  ${name} (${JSON.stringify(actual)})`);
  else {
    failures += 1;
    console.log(`FAIL  ${name}: got ${JSON.stringify(actual)}`);
  }
};

if (process.platform !== "win32") {
  console.log("diff-region self-test: SKIPPED — System.Drawing and PowerShell are Windows-only.");
  process.exit(0);
}

const work = mkdtempSync(path.join(tmpdir(), "mtk-diff-"));
const ps = (args) =>
  execFileSync("powershell.exe", ["-NoProfile", "-ExecutionPolicy", "Bypass", ...args], { stdio: "pipe" })
    .toString()
    .trim();

/** Paint a `size`x`size` PNG: a flat background, with an optional filled rectangle in another colour. */
function paint(file, size, background, rect) {
  const [br, bg, bb] = background;
  const args = [
    "-Command",
    [
      "Add-Type -AssemblyName System.Drawing;",
      `$b = New-Object System.Drawing.Bitmap ${size}, ${size};`,
      "$g = [System.Drawing.Graphics]::FromImage($b);",
      `$g.Clear([System.Drawing.Color]::FromArgb(${br}, ${bg}, ${bb}));`,
      rect
        ? `$g.FillRectangle((New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(${rect.color.join(", ")}))), ${rect.x}, ${rect.y}, ${rect.w}, ${rect.h});`
        : "",
      "$g.Dispose();",
      `$b.Save('${file.replace(/\\/g, "\\\\")}', [System.Drawing.Imaging.ImageFormat]::Png);`,
      "$b.Dispose();",
    ].join(" "),
  ];
  ps(args);
}

const run = (before, after, x, y, w, h, extra = []) =>
  JSON.parse(
    ps([
      "-File",
      script,
      "-Before",
      before,
      "-After",
      after,
      "-X",
      String(x),
      "-Y",
      String(y),
      "-Width",
      String(w),
      "-Height",
      String(h),
      ...extra,
    ]),
  );

try {
  const flat = path.join(work, "flat.png");
  const flatAgain = path.join(work, "flat-again.png");
  const patched = path.join(work, "patched.png");
  const bigger = path.join(work, "bigger.png");
  const subtle = path.join(work, "subtle.png");
  const wrongSize = path.join(work, "wrong-size.png");

  const GREY = [90, 92, 96];
  // The hover cue's direction: bluer than it is red.
  const CYAN = [70, 150, 200];

  paint(flat, 100, GREY, null);
  paint(flatAgain, 100, GREY, null);
  paint(patched, 100, GREY, { x: 10, y: 10, w: 20, h: 20, color: CYAN });
  paint(bigger, 100, GREY, { x: 10, y: 10, w: 40, h: 40, color: CYAN });
  // Below the threshold: a change the eye cannot see must not count as the cue arriving.
  paint(subtle, 100, GREY, { x: 10, y: 10, w: 20, h: 20, color: [95, 97, 101] });
  paint(wrongSize, 120, GREY, null);

  // 1. THE NEGATIVE CONTROL, which is the whole reason the spec can trust a non-zero count.
  const control = run(flat, flatAgain, 0, 0, 100, 100);
  check("two identical pictures differ by nothing", control.changed, 0);
  check("and it says how much it looked at", control.sampled, (n) => n === 2500);

  // 2. A KNOWN RECTANGLE, counted exactly. 20x20 at step 2 is 10x10 sampled points.
  const one = run(flat, patched, 0, 0, 100, 100);
  check("a 20x20 patch at step 2 is 100 sampled pixels", one.changed, 100);
  check("and the direction is reported: bluer than red", one.meanDeltaB > one.meanDeltaR, true);

  // 3. A BIGGER RECTANGLE COUNTS BIGGER. This is the ladder assertion the spec makes: an assembly
  //    lights strictly more than one part inside it, and the instrument must be able to say so.
  const many = run(flat, bigger, 0, 0, 100, 100);
  check("a 40x40 patch is four times a 20x20", many.changed, 400);
  check("and more is more", many.changed > one.changed, true);

  // 4. THE REGION IS RESPECTED. A diff that ignored its rectangle would report the docks repainting as
  //    the stage lighting up — the exact mistake the film gate's luma metric made.
  const elsewhere = run(flat, patched, 50, 50, 50, 50);
  check("a change outside the measured region is not counted", elsewhere.changed, 0);
  const partly = run(flat, patched, 0, 0, 20, 20);
  check("a region that clips the change counts only the part inside it", partly.changed, 25);

  // 5. THE THRESHOLD DISCRIMINATES. A 5-per-channel shift is not a cue.
  const quiet = run(flat, subtle, 0, 0, 100, 100);
  check("a change below the threshold is not a change", quiet.changed, 0);
  const lowered = run(flat, subtle, 0, 0, 100, 100, ["-Threshold", "2"]);
  check("and lowering the threshold finds it, so the guard is the threshold and not blindness", lowered.changed, 100);

  // 6. TWO PICTURES OF DIFFERENT SIZES ARE REFUSED, LOUDLY. The window moved between the captures, so
  //    no pixel in one is about the same place in the other — and silently comparing them would
  //    produce a large, confident, meaningless number.
  let refused = "";
  try {
    run(flat, wrongSize, 0, 0, 100, 100);
  } catch (error) {
    refused = String(error.stderr ?? error.message ?? "");
  }
  check("captures of different sizes are refused", /different sizes/.test(refused), true);
} finally {
  rmSync(work, { recursive: true, force: true });
}

console.log(
  failures === 0
    ? "\ndiff-region self-test: every count is the one the picture was painted to produce."
    : `\ndiff-region self-test: FAILED (${failures})`,
);
process.exit(failures === 0 ? 0 : 1);
