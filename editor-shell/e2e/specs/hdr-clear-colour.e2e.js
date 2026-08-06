// M15.11 — the clear colour, on an EMPTY scene, measured rather than eyeballed.
//
// Run with `MTK_SKY=off`: the sky otherwise covers every background pixel, so this is the only
// configuration in which the cleared HDR target is what reaches the screen. With the scene emptied too,
// the whole viewport IS the clear colour, which makes the assertion exact instead of "find a patch of
// background between the objects".
//
// What it proves: an authored sRGB constant, converted once on the way INTO a linear-HDR target, cleared
// at 16-bit float, and brought back out through the single final resolve (exposure → tone curve → OETF →
// dither), lands on the byte the author typed. Every stage of the pipeline is in that round trip, so a
// double encode, a missing encode, a raw-linear write or a mis-anchored conversion all fail it — and each
// fails it in a recognisably different direction, which the failure message spells out.

import { execFileSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const dir = path.dirname(fileURLToPath(import.meta.url));
const shots = path.resolve(dir, "../.shots-hdr/clear-colour");
const capture = path.resolve(dir, "../scripts/capture-composited-window.ps1");
const probe = path.resolve(dir, "../scripts/probe-pixels.ps1");

mkdirSync(shots, { recursive: true });

/** `render::CLEAR_COLOR_SRGB`, as 8-bit. Keep in step with render.rs. */
const AUTHORED = { r: 0.04 * 255, g: 0.05 * 255, b: 0.08 * 255 };

describe("HDR pipeline — the clear colour on an empty scene", () => {
  it("renders the authored background, byte-accurate, through the whole chain", async () => {
    expect(process.env.MTK_SKY).toBe("off"); // otherwise the sky covers the background and this proves nothing

    // Empty the scene, then frame it: `frame_all` is a no-op with nothing to frame, which is itself the
    // path worth exercising — it is where the camera used to fall back to a hard-coded distance.
    await invoke("new_project");
    // `viewport_pick` returns the nearest entity and is documented to return null ONLY when the scene is
    // empty, which makes it the exact emptiness probe (and it is native, so it cannot lag the outliner).
    await browser.waitUntil(async () => (await invoke("viewport_pick", { x: 0.5, y: 0.5 })) === null, {
      timeout: 20000,
      timeoutMsg: "the scene never emptied",
      interval: 400,
    });
    await invoke("view_preset", { preset: "persp" });
    await invoke("frame_all");
    await browser.pause(700);

    const out = path.join(shots, "01-empty-scene-clear-colour.png");
    execFileSync("powershell.exe", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", capture, "-Out", out], {
      stdio: "pipe",
    });

    // Sample the band ABOVE the horizon. The ground quad and the grid are drawn unconditionally (they are
    // not entities), so even an empty scene shows them; the cleared target is what is left above them.
    // x=400 keeps clear of the floating "Transform · World" pill, which sits at the top centre.
    const raw = execFileSync(
      "powershell.exe",
      ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", probe, "-Path", out, "-X", "400", "-Y", "115", "-Radius", "10"],
      { stdio: "pipe" },
    ).toString();
    const measured = JSON.parse(raw);
    // eslint-disable-next-line no-console
    console.log(`[hdr-clear-colour] measured ${JSON.stringify(measured)} vs authored ${JSON.stringify(AUTHORED)}`);

    // Uniform: the clear is a constant, so anything but dither-level variation means geometry or a panel
    // leaked into the patch and the mean below would be measuring the wrong pixels. `fs_resolve`'s dither
    // is ±0.5/255, so exactly one code of spread is expected and more is a mis-aimed sample.
    expect(measured.spread).toBeLessThanOrEqual(2);

    // Within one 8-bit code of the authored colour, per channel. The dither in `fs_resolve` is ±0.5/255,
    // so a tolerance of 1 code is as tight as this can honestly be asserted.
    //
    // The DIRECTION of a failure is diagnostic, so report it rather than just the numbers: far brighter
    // means the OETF ran twice (or the scene reached the swapchain raw-linear); far darker means it never
    // ran at all. Both are single-line fixes once you know which.
    const off = ["r", "g", "b"]
      .map((ch) => ({ ch, delta: measured[ch] - AUTHORED[ch] }))
      .filter(({ delta }) => Math.abs(delta) > 1.0);
    if (off.length > 0) {
      const detail = off
        .map(({ ch, delta }) => `${ch}: ${measured[ch].toFixed(2)} vs ${AUTHORED[ch].toFixed(2)} (${delta > 0 ? "+" : ""}${delta.toFixed(2)})`)
        .join("; ");
      const direction = off[0].delta > 0 ? "TOO BRIGHT (double display encode, or raw linear)" : "TOO DARK (no display encode)";
      throw new Error(`clear colour is ${direction} — ${detail}`);
    }
  });
});
