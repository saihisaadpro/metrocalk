// M15.11 — visual verification of the linear-HDR intermediate pipeline, against the PACKAGED .exe.
//
// Every capture is an OS-composited PrintWindow of the real window, so the native wgpu surface is in the
// image (a WebDriver screenshot shows the React panels and a hole where the 3D is).
//
// The renderer's configuration — MSAA sample count, SSAO, bloom, sky — is read from the environment ONCE
// at device creation, so a configuration cannot be switched inside a session. `scripts/hdr-capture-all.ps1`
// therefore runs this spec once per configuration, each into its own `MTK_SHOT_DIR`, and this file
// captures the states that vary WITHIN a configuration: camera modes, overlays, exposure, resize.
//
// What the set is for: the HDR migration's failure modes are all visual and all specific — near-black raw
// linear output, a washed-out double tone map, overlays that shifted colour, bloom leaking out of ordinary
// midtones, banding in the sky, antialiasing that stopped working because MSAA resolved in the wrong
// space. Each is visible in at least one frame below, in every routing combination.

import { execFileSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const dir = path.dirname(fileURLToPath(import.meta.url));
const label = process.env.MTK_SHOT_DIR || "default";
const shots = path.resolve(dir, `../.shots-hdr/${label}`);
const capture = path.resolve(dir, "../scripts/capture-composited-window.ps1");
const resizer = path.resolve(dir, "../scripts/resize-window.ps1");

mkdirSync(shots, { recursive: true });

const frames = [];
let counter = 0;

/** The renderer configuration this run is exercising, recorded alongside the pixels. */
const configuration = {
  label,
  ssao: process.env.MTK_SSAO ?? "on (default)",
  bloom: process.env.MTK_BLOOM ?? "on (default)",
  msaa: process.env.MTK_MSAA ?? "4 (default)",
  sky: process.env.MTK_SKY ?? "on (default)",
};

function ps(script, args) {
  return execFileSync(
    "powershell.exe",
    ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", script, ...args],
    { stdio: "pipe" },
  )
    .toString()
    .trim();
}

function snap(name) {
  const out = path.join(shots, `${name}.png`);
  let last;
  for (let attempt = 0; attempt < 4; attempt += 1) {
    try {
      ps(capture, ["-Out", out]);
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

async function shot(name, what, act) {
  if (act) await act();
  // Let the render loop present at least one frame in the new state before grabbing pixels.
  await browser.pause(500);
  counter += 1;
  const id = String(counter).padStart(2, "0");
  const cam = await invoke("camera_debug").catch(() => null);
  const file = snap(`${id}-${name}`);
  frames.push({
    n: counter,
    label: name,
    what,
    camera: Array.isArray(cam)
      ? {
          orbit: Number(cam[0].toFixed(3)),
          elevation: Number(cam[1].toFixed(3)),
          distance: Number(cam[2].toFixed(3)),
        }
      : null,
    shot: file,
  });
  return file;
}

describe(`HDR pipeline — ${label}`, () => {
  it("captures the lit scene through every camera mode", async () => {
    // The baseline read: is the frame tone-mapped exactly once? Raw linear output reads near-black and
    // flat; a double tone map reads milky with no black point. Neither is subtle once you look.
    await shot("persp-baseline", "Default scene, perspective, default exposure.", async () => {
      await invoke("view_preset", { preset: "persp" });
      await invoke("frame_all");
    });
    for (const preset of ["top", "front", "side"]) {
      await shot(`ortho-${preset}`, `Orthographic ${preset} view — same colour pipeline.`, async () => {
        await invoke("view_preset", { preset });
        await invoke("frame_all");
      });
    }
    await invoke("view_preset", { preset: "persp" });
    await invoke("frame_all");
  });

  it("captures the exposure sweep — the values that live above 1.0", async () => {
    // Exposure is applied in the final resolve now, against LINEAR radiance. Sweeping it is the direct
    // test that highlights compress rather than clip, and that bloom follows exposure the way a camera
    // does. At 4.0 the sky and speculars are far above 1.0 in scene-linear terms.
    for (const e of [0.15, 0.45, 1.2, 4.0]) {
      await shot(`exposure-${String(e).replace(".", "p")}`, `Exposure ${e}.`, async () => {
        await invoke("set_exposure", { exposure: e });
      });
    }
    await shot("exposure-restored", "Back to the default exposure.", async () => {
      await invoke("set_exposure", { exposure: 0.45 });
    });
  });

  it("captures both presentation profiles", async () => {
    // Two different tone curves through the SAME final resolve. Authored helper colour must survive both.
    for (const profile of ["cad", "cinematic"]) {
      await shot(`profile-${profile}`, `Presentation profile: ${profile}.`, async () => {
        await invoke("set_render_profile", { profile });
      });
    }
  });

  it("captures the overlay layer: grid, lines, gizmo, markers and selection together", async () => {
    // Every unlit authored colour in one frame. These are the pixels that would have gone 25–40% dark if
    // the migration had converted them with a plain srgb_to_linear and let the tone curve have them.
    await shot("overlays-none-selected", "Grid, ground and sky with nothing selected.", async () => {
      await invoke("view_preset", { preset: "persp" });
      await invoke("frame_all");
    });
    // Select through the ENGINE, not the outliner: that list is virtualised, so a DOM row query returns
    // whatever happens to be mounted (zero, before the panel has been scrolled). `viewport_pick` picks the
    // entity nearest the given normalised point and is what a real click routes to anyway — and the render
    // loop builds the gizmo from that same selection, so this frame carries the tint, the rim and the
    // X/Y/Z handles together.
    let picked;
    await shot(
      "overlays-selected-with-gizmo",
      "One object selected: selection tint, rim and the transform gizmo's X/Y/Z axes.",
      async () => {
        picked = await invoke("viewport_pick", { x: 0.5, y: 0.5 });
        await browser.pause(400);
      },
    );
    // Falsy would mean the scene was empty, and the frame above would silently carry no overlay at all.
    expect(picked).toBeTruthy();
    await shot("overlays-close", "Close in on the grid intersections and the gizmo handles.", async () => {
      for (let i = 0; i < 10; i += 1) await invoke("zoom", { delta: -120.0 });
    });
    await shot("overlays-far", "Pulled back: thin lines and high-contrast edges (antialiasing).", async () => {
      for (let i = 0; i < 22; i += 1) await invoke("zoom", { delta: 120.0 });
    });
    await invoke("frame_all");
  });

  it("captures a populated match — meshes, shadows, contact and many actors", async () => {
    const tab = await $("#inspector-workspaces-match-tab");
    await tab.waitForExist({ timeout: 20000 });
    await tab.click();
    await (await $("#inspector-workspaces-match-panel")).waitForDisplayed({ timeout: 20000 });
    const press = async (text) => {
      const panel = await $("#inspector-workspaces-match-panel");
      const button = await panel.$(`button*=${text}`);
      await button.scrollIntoView({ block: "center" });
      await button.waitForClickable({ timeout: 15000 });
      await button.click();
    };
    await shot("scene-meshes-authored", "Real imported meshes, lit and shadowed.", async () => {
      await press("Create a starter match");
      await browser.waitUntil(async () => (await invoke("moba_validate")).ok, { timeout: 20000 });
      await invoke("view_preset", { preset: "persp" });
      await invoke("frame_all");
    });
    await shot("scene-meshes-populated", "After a wave spawns — a mesh-heavy frame.", async () => {
      await press("Start match");
      await browser.waitUntil(async () => (await invoke("moba_status")).running, { timeout: 20000 });
      await press("Step 30");
      await browser.waitUntil(async () => (await invoke("moba_status")).tick >= 30, { timeout: 20000 });
      await invoke("frame_all");
    });
    await shot("scene-meshes-bright", "The same frame at high exposure — highlights and bloom.", async () => {
      await invoke("set_exposure", { exposure: 2.5 });
    });
    await shot("scene-meshes-dark", "The same frame at low exposure — dark gradients and banding.", async () => {
      await invoke("set_exposure", { exposure: 0.12 });
    });
    await invoke("set_exposure", { exposure: 0.45 });
  });

  it("captures a real native resize", async () => {
    // NOT browser.setWindowSize — that does not resize a Tauri window through tauri-driver, which is why
    // the viewport-stress audit recorded its resize block as a known gap. This drives SetWindowPos on the
    // real window, so the render loop genuinely reconfigures the surface and rebuilds the depth, MSAA,
    // HDR scene, SSAO and bloom targets together. A stale view survives here or nowhere.
    await shot("resize-narrow", "Window narrowed to 900x900 — targets rebuilt at the new size.", async () => {
      ps(resizer, ["-Width", "900", "-Height", "900"]);
      await invoke("frame_all");
    });
    await shot("resize-wide", "Window widened to 1600x850.", async () => {
      ps(resizer, ["-Width", "1600", "-Height", "850"]);
      await invoke("frame_all");
    });
    await shot("resize-restored", "Restored to 1400x900.", async () => {
      ps(resizer, ["-Width", "1400", "-Height", "900"]);
      await invoke("frame_all");
    });

    // ── the audit index ────────────────────────────────────────────────────────────────────────
    // 4 camera modes + 5 exposures + 2 profiles + 4 overlay states + 4 scene states + 3 resizes.
    // Asserted, because a spec that silently captured nothing would still look green.
    expect(frames.length).toBe(22);
    for (const f of frames) expect(typeof f.shot).toBe("string");
    writeFileSync(
      path.join(shots, "audit.json"),
      JSON.stringify(
        {
          schema: "metrocalk.hdr.pipeline.v1",
          capturedAt: new Date().toISOString(),
          configuration,
          method:
            "OS-composited window capture (PrintWindow PW_RENDERFULLCONTENT) so the native wgpu surface is in every image; resize driven via SetWindowPos on the real window",
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
