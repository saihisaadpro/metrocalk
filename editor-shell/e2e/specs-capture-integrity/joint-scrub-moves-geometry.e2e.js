// Does scrubbing an authored mechanism actually MOVE the geometry?
//
// In the factory acceptance run, `joint_scrub` reported 24 posed mechanisms while the two captures around
// it were byte-identical (YAVG=0, YMAX=0). The companion capture-integrity spec has already shown that an
// OS capture DOES observe a 3D-only change on this machine (a pure exposure change reads peakLuma=132),
// so a stale frame is not the explanation.
//
// This isolates the remaining question WITHOUT the 275 MB import, on an ordinary seeded entity:
//
//   * read_transform BEFORE            — the authored transform
//   * joint_author_batch               — one large prismatic joint (5 units of travel; unmissable)
//   * joint_scrub(t)                   — the engine's own posing entry point
//   * read_transform AFTER             — did the DOCUMENT-facing transform move?
//   * a capture pair                   — did the PIXELS move?
//
// Splitting "the transform moved" from "the pixels moved" is the point: they fail for different reasons
// and the factory run could not tell them apart.

import { browser } from "@wdio/globals";
import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, statSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const e2eDir = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const captureScript = path.join(e2eDir, "scripts", "capture-composited-window.ps1");
const ffmpeg = process.env.MTK_FFMPEG || "ffmpeg";
const outDir = path.resolve(e2eDir, "../evidence/joint-scrub-probe");

function capture(label) {
  mkdirSync(outDir, { recursive: true });
  const output = path.join(outDir, `${label}.png`);
  execFileSync("powershell.exe", [
    "-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
    "-File", captureScript, "-Out", output,
  ], { encoding: "utf8", timeout: 120_000, windowsHide: true });
  if (!existsSync(output) || statSync(output).size <= 1024) throw new Error(`Capture ${label} too small.`);
  return output;
}

function delta(before, after) {
  const done = spawnSync(ffmpeg, [
    "-hide_banner", "-nostdin", "-i", before, "-i", after,
    "-lavfi", "[0][1]blend=all_mode=difference,signalstats,metadata=print", "-f", "null", "-",
  ], { encoding: "utf8", timeout: 120_000, windowsHide: true });
  const text = `${done.stdout ?? ""}\n${done.stderr ?? ""}`;
  const read = (key) => {
    const m = new RegExp(`lavfi\\.signalstats\\.${key}=([-0-9.]+)`).exec(text);
    return m ? Number(m[1]) : null;
  };
  return { meanLuma: read("YAVG"), peakLuma: read("YMAX") };
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

describe("joint scrub", () => {
  it("moves the geometry it reports posing", async () => {
    await browser.waitUntil(
      async () => (await browser.execute(() =>
        document.querySelectorAll('[data-testid="hrow"]').length > 0)) === true,
      { timeout: 60_000, interval: 300, timeoutMsg: "The seeded scene never listed an entity." },
    );

    // A mesh-bearing seeded entity, so a move is actually visible.
    const target = await browser.execute(() => {
      const rows = [...document.querySelectorAll('[data-testid="hrow"]')];
      return rows.map((row) => row.getAttribute("data-id")).find(Boolean) ?? null;
    });
    if (!target) throw new Error("No entity id in the hierarchy.");

    // `gizmo_select` — NOT `select`, which this shell has never registered. The invented name rejected
    // at run time and the `.catch(() => {})` beside it swallowed the rejection, so this line selected
    // nothing and said nothing, in the one spec whose premise is that `target` is the entity the scrub
    // poses. It returns false for an id it does not know, so the boolean is the assertion: a refusal
    // here means the measurement below is being taken against an unselected entity.
    if ((await invoke("gizmo_select", { id: target })) !== true) {
      throw new Error(`gizmo_select refused ${target} — the scrub would be measured unselected.`);
    }
    await invoke("focus_entity", { id: target });
    await browser.pause(1_200);

    const before = await invoke("read_transform", { id: target });

    const AMPLITUDE = 5;
    const authored = await invoke("joint_author_batch", {
      requests: [{
        id: target,
        revolute: false,
        axis: [1, 0, 0],
        pivot: before.slice(0, 3),
        min: -AMPLITUDE * 1.15,
        max: AMPLITUDE * 1.15,
        source: "manual",
        // Peak displacement exactly at the scrub time, so there is no interpolation ambiguity.
        keys: [{ t: 0, value: 0 }, { t: 2, value: AMPLITUDE }, { t: 4, value: 0 }],
      }],
    });

    // Capture the neutral frame AFTER authoring and BEFORE scrubbing. Authoring repaints UI (selection,
    // the Animate badge), so a pre-authoring neutral would fold that repaint into the "did it move?"
    // measurement and could show motion where there is none. This pair differs by the scrub alone.
    await browser.pause(1_200);
    const beforeFrame = capture("01-neutral");

    const posed = await invoke("joint_scrub", { t: 2 });
    await browser.pause(1_500);
    const after = await invoke("read_transform", { id: target });
    const afterFrame = capture("02-posed");
    const pixels = delta(beforeFrame, afterFrame);

    const movedBy = Math.hypot(...[0, 1, 2].map((i) => after[i] - before[i]));
    const verdict = {
      target, authored, posedCount: posed,
      transformBefore: before.slice(0, 3),
      transformAfter: after.slice(0, 3),
      transformMovedBy: movedBy,
      expectedMove: AMPLITUDE,
      pixelDelta: pixels,
      documentMoved: movedBy > 0.01,
      pixelsMoved: (pixels.peakLuma ?? 0) > 0,
    };
    mkdirSync(outDir, { recursive: true });
    writeFileSync(path.join(outDir, "verdict.json"), `${JSON.stringify(verdict, null, 2)}\n`, "utf8");
    // eslint-disable-next-line no-console
    console.log(`JOINT_SCRUB_VERDICT ${JSON.stringify(verdict)}`);

    await invoke("joint_scrub", { t: -1 });

    if (posed < 1) throw new Error(`joint_scrub posed nothing: ${JSON.stringify(verdict)}`);
    if (!verdict.pixelsMoved) {
      throw new Error(`joint_scrub reported ${posed} posed but the viewport is identical: ${JSON.stringify(verdict)}`);
    }
  });
});
