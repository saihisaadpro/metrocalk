// Screenshot-based viewport stress audit of the PACKAGED .exe.
//
// Every capture is an OS-composited BitBlt/PrintWindow of the real window, so the native wgpu surface is
// in the image. A WebDriver screenshot cannot do this: the viewport is a wgpu surface beneath a
// transparent WebView2, so a DOM screenshot shows the React panels and a hole where the 3D is.
//
// The point of this spec is NOT to assert that pixels are correct — no automated check can tell you a
// viewport looks professional. It is to produce a complete, labelled, reviewable set covering every
// viewport state the brief names, so a human can look at all of them at once and judge. Each capture is
// paired with the camera state the kernel reports, so a reader can tell "the camera moved" from "the
// image changed".

import { execFileSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const dir = path.dirname(fileURLToPath(import.meta.url));
const shots = path.resolve(dir, "../.shots-viewport-stress");
const capture = path.resolve(dir, "../scripts/capture-composited-window.ps1");

mkdirSync(shots, { recursive: true });

const frames = [];
let counter = 0;

function snap(name) {
  const out = path.join(shots, `${name}.png`);
  let last;
  for (let attempt = 0; attempt < 4; attempt += 1) {
    try {
      execFileSync(
        "powershell.exe",
        ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", capture, "-Out", out],
        { stdio: "pipe" },
      );
      return path.basename(out);
    } catch (error) {
      last = error;
      execFileSync("powershell.exe", ["-NoProfile", "-Command", "Start-Sleep -Milliseconds 700"], {
        stdio: "pipe",
      });
    }
  }
  throw last;
}

/** The camera the renderer is actually using: [orbit, elevation, distance, tx, ty, tz]. */
const camera = () => invoke("camera_debug");

async function shot(label, what, act) {
  if (act) await act();
  // Let the render loop present at least one frame in the new state before grabbing pixels.
  await browser.pause(450);
  counter += 1;
  const id = String(counter).padStart(2, "0");
  const cam = await camera().catch(() => null);
  const file = snap(`${id}-${label}`);
  frames.push({
    n: counter,
    label,
    what,
    camera: Array.isArray(cam)
      ? {
          orbit: Number(cam[0].toFixed(3)),
          elevation: Number(cam[1].toFixed(3)),
          distance: Number(cam[2].toFixed(3)),
          target: [cam[3], cam[4], cam[5]].map((v) => Number(v.toFixed(3))),
        }
      : null,
    shot: file,
  });
  return file;
}

async function pressLabelled(text) {
  const panel = await $("#inspector-workspaces-match-panel");
  const button = await panel.$(`button*=${text}`);
  await button.scrollIntoView({ block: "center" });
  await button.waitForClickable({ timeout: 15000 });
  await button.click();
}

describe("viewport stress audit", () => {
  it("captures the default scene as it first opens", async () => {
    // The state the brief calls the baseline defect: whatever the editor shows with no help at all.
    await shot("default-first-open", "The editor exactly as it opens, before any camera command.");
  });

  it("captures every canonical view of the default scene", async () => {
    for (const preset of ["persp", "top", "front", "side"]) {
      await shot(`default-${preset}`, `Default scene, ${preset} preset, auto-framed.`, async () => {
        await invoke("view_preset", { preset });
        await invoke("frame_all");
      });
    }
  });

  it("captures framing behaviour at both extremes of scale", async () => {
    // Same scene, deliberately over- and under-zoomed, then re-framed. This is the acceptance criterion
    // that a tiny and a huge subject both end up readable WITHOUT the user repairing the camera.
    await invoke("view_preset", { preset: "persp" });
    await shot("scale-zoomed-way-out", "Camera pushed far out, subject tiny.", async () => {
      // No absolute camera setter exists, so this uses the SAME zoom command the mouse wheel does.
      // POSITIVE delta increases the orbit distance — an earlier version had this inverted, which
      // swapped these two captions and left the grounding frame below empty.
      for (let i = 0; i < 40; i += 1) await invoke("zoom", { delta: 120.0 });
    });
    await shot("scale-recovered-by-frame-all", "One frame-all from that state.", async () => {
      await invoke("frame_all");
    });
    await shot("scale-zoomed-way-in", "Camera pushed inside the subject.", async () => {
      for (let i = 0; i < 60; i += 1) await invoke("zoom", { delta: -120.0 });
    });
    await shot("scale-recovered-again", "One frame-all from that state.", async () => {
      await invoke("frame_all");
    });
  });

  it("captures a running match: grounding, shadows and skinned assets", async () => {
    const tab = await $("#inspector-workspaces-match-tab");
    await tab.waitForExist({ timeout: 20000 });
    await tab.click();
    await (await $("#inspector-workspaces-match-panel")).waitForDisplayed({ timeout: 20000 });

    await shot("match-authored", "Starter match authored — three real imported meshes.", async () => {
      await pressLabelled("Create a starter match");
      await browser.waitUntil(async () => (await invoke("moba_validate")).ok, { timeout: 20000 });
      await invoke("view_preset", { preset: "persp" });
      await invoke("frame_all");
    });

    await shot("match-running-persp", "Running, perspective. Grounding and contact.", async () => {
      await pressLabelled("Start match");
      await browser.waitUntil(async () => (await invoke("moba_status")).running, { timeout: 20000 });
      await invoke("frame_all");
    });

    await shot("match-populated", "After a wave spawns — many casters at once.", async () => {
      await pressLabelled("Step 30");
      await browser.waitUntil(async () => (await invoke("moba_status")).tick >= 30, {
        timeout: 20000,
      });
      await invoke("frame_all");
    });

    for (const preset of ["top", "front", "side"]) {
      await shot(`match-${preset}`, `Running match, ${preset} preset.`, async () => {
        await invoke("view_preset", { preset });
        await invoke("frame_all");
      });
    }

    // A close inspection pass: this is where detached shadows and floating feet are visible or not.
    await shot("match-close-grounding", "Close on the actors — feet, contact and shadow anchor.", async () => {
      await invoke("view_preset", { preset: "persp" });
      await invoke("frame_all");
      for (let i = 0; i < 12; i += 1) await invoke("zoom", { delta: -120.0 });
    });
  });

  it("captures selection feedback", async () => {
    // NOT guarded by a length check. The previous version was, matched zero rows against a selector
    // that does not exist, and passed green having captured nothing at all — a silent hole in the audit
    // that only showed up when someone counted the frames.
    const rows = await $$("#hierarchy-panel button, [data-testid='hierarchy-row']");
    expect(rows.length).toBeGreaterThan(0);
    await shot("selection-none", "Nothing selected.");
    await shot("selection-one", "One object selected from the outliner.", async () => {
      await rows[0].click();
      await browser.pause(250);
    });
  });

  // KNOWN LIMITATION, measured not assumed: `browser.setWindowSize` does not resize a Tauri window
  // through tauri-driver. Every capture in this block came back 1696x1018 regardless of the size asked
  // for, so these three frames do NOT exercise resize — they are three identical views kept only so the
  // gap is visible in the audit index rather than silently absent. Resize must be checked by hand until
  // the harness can drive the native window.
  it("captures viewport resize behaviour (SEE COMMENT: the harness cannot resize this window)", async () => {
    const original = await browser.getWindowSize();
    await shot("resize-narrow", "Window narrowed — framing must adapt to the new aspect.", async () => {
      await browser.setWindowSize(900, 900);
      await browser.pause(700);
      await invoke("frame_all");
    });
    await shot("resize-wide", "Window widened.", async () => {
      await browser.setWindowSize(1600, 850);
      await browser.pause(700);
      await invoke("frame_all");
    });
    await shot("resize-restored", "Back to the original size.", async () => {
      await browser.setWindowSize(original.width, original.height);
      await browser.pause(700);
      await invoke("frame_all");
    });

    // ── the audit index ────────────────────────────────────────────────────────────────────────
    expect(frames.length).toBeGreaterThanOrEqual(18);
    for (const f of frames) expect(typeof f.shot).toBe("string");
    writeFileSync(
      path.join(shots, "audit.json"),
      JSON.stringify(
        {
          schema: "metrocalk.viewport.stress.v1",
          capturedAt: new Date().toISOString(),
          method:
            "OS-composited window capture (PrintWindow PW_RENDERFULLCONTENT, desktop-DC BitBlt fallback) so the native wgpu surface is in every image",
          frames: frames.length,
          states: frames.map((f) => f.label),
          detail: frames,
        },
        null,
        2,
      ),
      "utf8",
    );
  });
});
