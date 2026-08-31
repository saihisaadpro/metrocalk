// Presentation-environment look-dev lab — see wdio.hall-lab.conf.js for why this exists.
//
// Opens the .mtk a completed film run saved, applies the film's own presentation, and captures the
// viewport from a fixed set of vantages that between them exercise every scale the film shoots at:
// the whole plant, one cell, one mechanism, and a low eye-level view down the line. The same labels
// every run, so a before/after pair diffs frame for frame.

import { browser } from "@wdio/globals";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { appendFileSync, existsSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const e2eDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const captureScript = path.resolve(e2eDir, "scripts/capture-composited-window.ps1");
const shotsDir = process.env.MTK_HALL_LAB_SHOTS;
const project = process.env.MTK_HALL_LAB_PROJECT;
// Beside the EXECUTABLE, which is where `diag::path()` writes it - not in the temp directory. Pointed at
// the temp dir this scrape returned null on every run ever made, and `lab-notes.json` recorded
// "hall": null for runs whose renderer had in fact built and logged a hall. A reader who trusted the
// notes would have concluded the room was never built.
const exeForLog = process.env.MTK_EXE
  || path.resolve(e2eDir, "../src-tauri/target/release/metrocalk-editor-shell.exe");
const diagnosticsLog = path.resolve(path.dirname(exeForLog), "metrocalk-diagnostics.log");
if (!shotsDir || !project) throw new Error("The lab config did not supply its project or output directory.");

const notes = { project, captures: [], cameras: {}, hall: null, entities: null };

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

/**
 * Digests of the captures taken so far, so a vantage that did not move cannot be filed as one that did.
 *
 * The lab's first run captured `03-eye-level` and `04-low-vantage` as BYTE-IDENTICAL files (same md5,
 * same 194,343 bytes) and recorded identical camera probes for both, because `add_camera` places a
 * camera but carries no aim — the "looking up" vantage, the one that would show whether there is a roof,
 * was never expressible through that command and silently duplicated the shot before it. Two captures
 * with different labels and identical pixels are not two vantages, and a before/after diff built on them
 * would be comparing a picture with itself.
 */
const captureDigests = new Map();

/**
 * Height of the OS title bar and window border to cut before comparing two captures, in pixels.
 *
 * Generous rather than exact: nothing worth comparing lives in the top 44 rows of a maximised editor
 * window, and an over-tight crop that leaves one row of chrome in is the whole failure this constant
 * exists to prevent.
 */
const CHROME_ROWS = 44;

/** An md5 of the PIXELS below the window chrome — the render, rather than the window around it. */
function renderDigest(file) {
  const out = execFileSync(
    "ffmpeg",
    ["-hide_banner", "-loglevel", "error", "-i", file,
      "-vf", `crop=iw:ih-${CHROME_ROWS}:0:${CHROME_ROWS}`, "-f", "framemd5", "-"],
    { encoding: "utf8", timeout: 60_000, windowsHide: true },
  );
  const line = out.split(/\r?\n/).find((l) => l && !l.startsWith("#"));
  if (!line) throw new Error(`ffmpeg produced no frame digest for ${file}`);
  return line.trim().split(/[,\s]+/).pop();
}

/**
 * Capture one vantage.
 *
 * `mustMatch` names an earlier capture this one is DELIBERATELY a repeat of, which inverts the check:
 * the last shot returns to the first vantage on purpose, and for that pair being byte-identical is the
 * result, not the defect — it says the renderer put the same camera back and drew the same pixels, so
 * nothing in between left the scene in a different state. Without this the guard fails the run on its
 * own determinism check, which it did on first use.
 */
function capture(label, { mustMatch = null } = {}) {
  const output = path.join(shotsDir, `${label}.png`);
  const stdout = execFileSync(
    "powershell.exe",
    [
      "-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
      "-File", captureScript, "-Out", output, "-ProcName", "metrocalk-editor-shell",
    ],
    { encoding: "utf8", timeout: 60_000, windowsHide: true },
  );
  appendFileSync(path.join(shotsDir, "capture.log"), `[${new Date().toISOString()}] ${label}\n${stdout}\n`, "utf8");

  const digest = createHash("md5").update(readFileSync(output)).digest("hex");
  if (mustMatch) {
    // Compared over the RENDER, not over the window.
    //
    // The whole-file digest includes the OS title bar, and on its first use this check failed on a
    // studio run whose viewport was pixel-identical: the entire difference was 457 pixels in an 11-row
    // band at y=13..24 - the title bar redrawing in a different focus state. Failing a determinism
    // check on window chrome is the same defect this lane has now hit four times in four different
    // instruments: a measurement almost of the thing it names. Crop first, then compare.
    const twinFile = path.join(shotsDir, `${mustMatch}.png`);
    const [here, there] = [output, twinFile].map(renderDigest);
    if (here !== there) {
      throw new Error(
        `Capture '${label}' returns to the vantage of '${mustMatch}' but did not reproduce it ` +
        `(render digest ${here} vs ${there}). Something between the two changed the scene, the camera ` +
        `or the renderer's state and did not put it back.`,
      );
    }
  } else {
    const twin = captureDigests.get(digest);
    if (twin) {
      throw new Error(
        `Capture '${label}' is byte-identical to '${twin}'. The camera did not move between them, so this ` +
        `run has one vantage filed under two names rather than two vantages.`,
      );
    }
    captureDigests.set(digest, label);
  }

  notes.captures.push({ label, digest, repeatOf: mustMatch });
  return output;
}

/**
 * Hide every DOM panel so the capture is the wgpu viewport and nothing else.
 *
 * The film does the same thing before it rolls. Without it the left rail and the bottom dock are ~20%
 * of the window, they are white, and any per-frame image statistic computed over the whole window is
 * really a statistic about the editor's chrome — the exact defect that made an earlier pass's
 * legibility gate unable to report an illegible frame.
 */
async function hideChrome() {
  await browser.execute(() => {
    const style = document.createElement("style");
    style.id = "mtk-hall-lab-clean";
    style.textContent = `
      body > #root > *:not([data-testid="viewport"]) { visibility: hidden !important; }
      [data-testid="bottom-dock"], header, aside, nav, footer { display: none !important; }
    `;
    document.head.appendChild(style);
  });
}

/** Whatever the renderer said about the presentation set it last built, straight from its own log. */
function hallFromDiagnostics() {
  if (!existsSync(diagnosticsLog)) return null;
  const lines = readFileSync(diagnosticsLog, "utf8").split(/\r?\n/).filter((l) => l.includes("presentation hall:"));
  return lines.length ? lines[lines.length - 1] : null;
}

describe("presentation environment look-dev", () => {
  it("opens the filmed project and captures the plant at every scale the film shoots at", async () => {
    await browser.waitUntil(
      async () => (await browser.execute(() => !!window.__TAURI__?.core?.invoke)) === true,
      { timeout: 120_000, interval: 500, timeoutMsg: "The Tauri bridge never appeared." },
    );

    const opened = await invoke("open_project", { path: project });
    if (opened?.error) throw new Error(`open_project refused: ${JSON.stringify(opened)}`);

    // The project carries 17,793 entities; the publish into the render state is asynchronous.
    //
    // WHAT THIS WAIT MUST NOT BE: `cad_report`. That command answers a question about the DOCUMENT — it
    // is served from the import report the project carries — so it reads 15,711 parts for a project
    // whose geometry cache has been deleted and whose viewport is empty. This lab's first run waited on
    // exactly that, passed, and captured five stills of bare ground while its notes recorded
    // "entities: 15711". A wait that was already true before the open began is not a wait.
    await browser.waitUntil(
      async () => {
        const report = await invoke("cad_report").catch(() => null);
        return typeof report?.total === "number" && report.total > 15_000;
      },
      { timeout: 300_000, interval: 2_000, timeoutMsg: "The opened project never restored its import report." },
    );
    const report = await invoke("cad_report");
    notes.entities = { total: report.total, exactBrep: report.exactBrep, failed: report.failed };

    // Exactly the film's presentation, so the lab is judging the film's picture.
    await invoke("set_render_profile", { profile: "cinematic" });
    await invoke("set_working_space", { space: "acescg" });
    await invoke("set_exposure", { exposure: 0.45 });
    await invoke("reset_environment");

    // THE REAL PROBE, and it is set-independent on purpose. `sync_stage` sizes the hall from the
    // RENDERED instances, so a room can only be built if something is actually being drawn — which
    // makes "the hall reports dimensions" the one machine-readable proof this harness has that the
    // viewport is not empty. Ask for the hall first, read its extent, and only then apply whatever set
    // is really under test. A studio baseline that turns out to contain no machinery is a picture of a
    // failed reopen, and diffing a hall against it would credit the hall with the machinery too.
    await invoke("set_presentation_set", { set: "factoryHall" });
    await browser.waitUntil(
      async () => (await invoke("presentation_set_state").catch(() => null))?.built === true,
      {
        timeout: 300_000,
        interval: 2_000,
        timeoutMsg:
          "No drawn geometry: the opened project published no renderable instances, so no room could be sized around them. " +
          "Check the diagnostics log for an 'assets: restored 0/N' line - the persisted CAD mesh cache beside the .exe is the usual cause.",
      },
    );
    notes.geometryProbe = await invoke("presentation_set_state");

    const requestedSet = process.env.MTK_HALL_LAB_SET || "factoryHall";
    notes.presentationSet = await invoke("set_presentation_set", { set: requestedSet });
    if (notes.presentationSet !== requestedSet) {
      throw new Error(`The presentation set did not apply: asked for ${requestedSet}, got ${notes.presentationSet}`);
    }

    await hideChrome();
    await invoke("view_preset", { preset: "persp" });
    await invoke("frame_all");
    await browser.pause(2_500);
    notes.cameras.plantOverview = await invoke("camera_probe");
    capture("01-plant-overview");

    // ── The interior vantages ────────────────────────────────────────────────────────────────────
    //
    // Placed from the ROOM's own dimensions, not from `frame_all`'s stand-off, and identical whichever
    // set is under test — which is what makes studio and hall directly comparable rather than merely
    // adjacent. The first run of this lab derived its vantages from `frame_all().distance * 0.55`; on a
    // 320 m plant that is a camera 300 m from the centre, i.e. OUTSIDE the building, and the resulting
    // "down the line" still is a flat grey exterior wall. A room is judged from inside it.
    //
    // Every one of these is placed with `set_look_dev_camera`, which takes an aim. The `add_camera` these
    // replace does not: it carries a position and a fov and leaves the aim at the editor's orbit target,
    // so all three of this lab's close vantages reported the SAME eye and target in its own notes.
    const hall = notes.geometryProbe.hall;
    const [cx, cy, cz] = hall.centre;
    // `lengthMetres` is the X extent and `widthMetres` the Z extent, whatever their names suggest - for
    // this plant X is the SHORT side (82 m) and Z the long one (320 m). Named locally so the vantages
    // below read as geometry rather than as a guess about which field means which axis.
    const halfX = hall.lengthMetres / 2;
    const halfZ = hall.widthMetres / 2;
    const clear = hall.clearHeightMetres;
    const [longHalf, shortHalf, longIsZ] = halfZ >= halfX ? [halfZ, halfX, true] : [halfX, halfZ, false];
    /** A point in the room: `along` runs the long axis, `across` the short one, both in [-1, 1]. */
    const at = (along, across, height) => (longIsZ
      ? [cx + shortHalf * across, cy + height, cz + longHalf * along]
      : [cx + longHalf * along, cy + height, cz + shortHalf * across]);

    async function vantage(label, eye, lookAt, fov = 50) {
      const pose = await invoke("set_look_dev_camera", { eye, lookAt, fov });
      if (pose?.error) throw new Error(`${label}: the camera refused the pose - ${pose.error}`);
      await browser.pause(1_500);
      const probe = await invoke("camera_probe");
      notes.cameras[label] = { requested: pose, probe };
      capture(label);
    }

    // Down the long axis from just inside one end, at standing eye height: the establishing view a real
    // factory walkthrough opens on, and the one that shows whether there is a room here or a void.
    await vantage("02-down-the-line", at(-0.92, 0.35, 1.7), at(0.9, 0.0, 3.0));

    // Beside the line, close in: the scale most of the film's thirty shots actually work at.
    await vantage("03-eye-level", at(-0.25, 0.55, 1.7), at(-0.1, 0.0, 2.0));

    // LOOKING UP - the half of the frame that is void in every film so far, and the vantage this lab has
    // claimed to capture since it was written without ever once doing so.
    await vantage("04-low-vantage", at(-0.1, 0.6, 1.2), at(0.25, -0.2, clear * 0.95));

    // High under the roof, looking down the line: the `birdseye` card's geometry, from inside.
    await vantage("05-high-down-the-line", at(-0.7, 0.2, clear * 0.8), at(0.6, 0.0, 1.5));

    await invoke("look_through_camera", { on: false });
    await invoke("view_preset", { preset: "persp" });
    await invoke("frame_all");
    await browser.pause(1_500);
    // Same vantage as 01, on purpose: the run ends by proving the renderer puts the camera back and
    // draws the same picture, so nothing in the four vantages between them left state behind.
    capture("06-plant-overview-again", { mustMatch: "01-plant-overview" });

    notes.hall = hallFromDiagnostics();
    writeFileSync(path.join(shotsDir, "lab-notes.json"), `${JSON.stringify(notes, null, 2)}\n`, "utf8");
  });
});
