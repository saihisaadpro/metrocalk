// COLOUR MANAGEMENT on the packaged .exe — declared, inspectable, and provably reaching the pixels.
//
// BEFORE: the pipeline's colour behaviour was correct BY CONVENTION and by nothing else. Scene-linear
// `Rgba16Float`, one tone curve at the resolve, and base-colour uploaded as sRGB while roughness went up
// as raw data — all right, and all decided at call sites. Nothing declared a texture's colour space,
// nothing could report the working space, and the two tone curves the renderer already ran were filed
// under "presentation profile" rather than named as display transforms. A pipeline that is correct by
// habit cannot be audited by the person whose shot looks wrong.
// AFTER: `colour_status` reports the working space, the LIVE view transform, and a per-capability list
// that shows what is NOT wired as prominently as what is.
//
// THE PROOF THAT MATTERS is not that the command returns a shape. It is that the view transform changes
// rendered pixels. This measures that IN-ENGINE via the thumbnail readback, which runs the same final
// resolve as the viewport (same exposure, same presentation profile) — so, unlike an OS screenshot, the
// desktop cannot refuse it and it is not a proxy for what the renderer does. It IS what the renderer did.

import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const dir = path.dirname(fileURLToPath(import.meta.url));
const shots = path.resolve(dir, "../.shots-colour");
const probe = path.resolve(dir, "../scripts/probe-pixels.ps1");
mkdirSync(shots, { recursive: true });

/**
 * Ask for a thumbnail, retrying briefly.
 *
 * The render thread services only a few thumbnail requests per frame, so a single call can legitimately
 * come back null while the queue drains. Letting that null fail the test would report a scheduling
 * detail as "the view transform does nothing" — which is the opposite of what is true, and the most
 * misleading way this test could fail.
 */
async function thumbnailOf(id, size) {
  for (let attempt = 0; attempt < 6; attempt += 1) {
    const url = await invoke("thumbnail", { id: String(id), size });
    if (url) return url;
    await browser.pause(400);
  }
  return null;
}

/** Write a `data:image/png;base64,…` thumbnail to disk and read its mean RGB over the centre. */
function meanOfThumbnail(dataUrl, label, size) {
  const b64 = dataUrl.replace(/^data:image\/png;base64,/, "");
  const file = path.join(shots, `${label}.png`);
  writeFileSync(file, Buffer.from(b64, "base64"));
  const out = execFileSync(
    "powershell.exe",
    ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", probe,
      "-Path", file, "-X", String(Math.floor(size / 2)), "-Y", String(Math.floor(size / 2)),
      "-Radius", String(Math.floor(size / 4))],
    { encoding: "utf8" },
  );
  return { ...JSON.parse(out), file };
}

const capture = path.resolve(dir, "../scripts/capture-composited-window.ps1");
const captureFg = path.resolve(dir, "../../../.uxtest/audit/exe/capture-window-fg.ps1");

/** OS-capture the live window. Returns the path, or null if the desktop refused. */
async function shot(label) {
  await browser.pause(600);
  const file = path.join(shots, `${label}.png`);
  const good = () => existsSync(file) && statSync(file).size > 20_000;
  const attempt = (script, args) => {
    try {
      execFileSync("powershell.exe", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", script, ...args], {
        encoding: "utf8",
      });
    } catch { /* fall through to the next strategy */ }
    if (!good() && existsSync(file)) rmSync(file);
    return good();
  };
  const ok =
    attempt(capture, ["-Out", file]) ||
    attempt(captureFg, ["-ProcName", "metrocalk-editor-shell", "-Out", file]);
  if (!ok) {
    console.log(`[colour] CAPTURE UNAVAILABLE for ${label}`);
    return null;
  }
  console.log(`[colour] captured ${file}`);
  return file;
}

const click = (selector) =>
  browser.execute((sel) => {
    const el = document.querySelector(sel);
    if (!el) return false;
    el.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    return true;
  }, selector);

describe("Colour — declared, live, and reaching the pixels", () => {
  const SIZE = 96;
  let sphere = null;

  before(async () => {
    await browser.waitUntil(
      async () => browser.execute(() => !!(window.__TAURI__ && window.__TAURI__.core)),
      { timeout: 30000 },
    );
    await click('[data-testid="stop"]');
    await invoke("new_project");
    await browser.pause(700);
    await browser.execute(() => {
      document.querySelector('[data-testid="onboardSkip"]')?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    // `shape_spawn` answers with a ShapeReply, not a bare id — the entity is in `.created`.
    const spawned = await invoke("shape_spawn", { kind: "sphere", pos: [0, 1, 0] });
    console.log(`[colour] spawned: ${JSON.stringify(spawned)}`);
    sphere = spawned.created;
    expect(sphere).toBeTruthy();
    await invoke("frame_all");
    await browser.pause(600);
  });

  it("reports the working space and which view transform is RUNNING", async () => {
    const c = await invoke("colour_status");
    console.log(`[colour] working=${c.working.label} active=${c.activeViewLabel}`);
    expect(c.working.current).toBe("linearRec709");
    expect(c.working.wired).toBe(true);
    // A status that lists possibilities without saying which is in force is not an inspector.
    expect(["acesFit", "pbrNeutral"]).toContain(c.activeView);
    expect(c.views.length).toBe(2);
    expect(c.spaces.length).toBeGreaterThanOrEqual(6);
  });

  it("names raw data as a NON-colour space — the one distinction that must exist", async () => {
    const c = await invoke("colour_status");
    const data = c.spaces.find((s) => s.id === "data");
    console.log(`[colour] data space: ${data.label}, isColour=${data.isColour}`);
    // Applying a transfer function to a roughness map is the classic error. Naming the non-colour case
    // is how it stays impossible rather than merely unlikely.
    expect(data).toBeTruthy();
    expect(data.isColour).toBe(false);
    const srgb = c.spaces.find((s) => s.id === "srgb");
    expect(srgb.isColour).toBe(true);
    expect(srgb.isLinear).toBe(false);
  });

  it("declares what is NOT wired, not only what is", async () => {
    const c = await invoke("colour_status");
    const caps = c.capabilities;
    console.log(`[colour] capabilities: ${JSON.stringify(caps)}`);
    // The true ones.
    expect(caps.sceneLinearWorkingSpace).toBe(true);
    expect(caps.dataMapsBypassColourTransform).toBe(true);
    expect(caps.singleToneMapAtResolve).toBe(true);
    // The role -> colour-space decision has ONE home now, and the uploader calls it.
    expect(caps.colourSpaceDerivedFromOnePolicy).toBe(true);
    // And the false ones, present rather than omitted. A colour report listing only its successes is
    // exactly the report that lets someone assume ACES is end-to-end when it is not.
    expect(caps).toHaveProperty("acesCgWorkingSpaceSelectable");
    expect(caps.acesCgWorkingSpaceSelectable).toBe(false);
    expect(caps).toHaveProperty("ocioConfigLoading");
    // Deriving the space from one policy and letting a person OVERRIDE it are different capabilities.
    // Reporting them as one would let a reader assume the second from the first.
    expect(caps.perAssetColourSpaceOverride).toBe(false);
    const scope = c.notes.join(" ");
    expect(scope).toContain("ACEScg");
    expect(scope.toLowerCase()).toContain("not wired");
  });

  it("the LIVE reading follows the renderer, not a stored copy of it", async () => {
    await invoke("set_render_profile", { profile: "cad" });
    await browser.pause(250);
    let c = await invoke("colour_status");
    console.log(`[colour] after set cad: ${c.activeView}`);
    expect(c.activeView).toBe("pbrNeutral");

    await invoke("set_render_profile", { profile: "cinematic" });
    await browser.pause(250);
    c = await invoke("colour_status");
    console.log(`[colour] after set cinematic: ${c.activeView}`);
    expect(c.activeView).toBe("acesFit");
  });

  it("THE PIXELS CHANGE: the two view transforms grade the same scene differently", async () => {
    // Measured through the thumbnail readback, which runs the SAME final resolve as the viewport.
    //
    // WAIT FIRST. A freshly created scene keeps darkening for a second or two as the renderer settles,
    // and an earlier version of this test read three samples straight off and "proved" a difference of
    // 16.7 that was mostly that drift. A/B on a moving observable measures the movement. So: hold the
    // profile fixed and sample until two consecutive readings agree, and only then compare transforms.
    await invoke("set_render_profile", { profile: "cinematic" });
    let settled = null;
    let previous = null;
    for (let i = 0; i < 25; i += 1) {
      await browser.pause(400);
      const now = meanOfThumbnail(await thumbnailOf(sphere, SIZE), `settle_${i}`, SIZE);
      if (previous) {
        const move = Math.abs(now.r - previous.r) + Math.abs(now.g - previous.g) + Math.abs(now.b - previous.b);
        if (move < 1.5) {
          settled = now;
          console.log(`[colour] settled after ${i + 1} samples (last movement ${move.toFixed(2)})`);
          break;
        }
      }
      previous = now;
    }
    expect(settled).toBeTruthy();
    const filmic = settled;
    console.log(`[colour] filmic mean RGB: ${filmic.r},${filmic.g},${filmic.b}`);
    await shot("10_window_filmic");

    await invoke("set_render_profile", { profile: "cad" });
    await browser.pause(500);
    const neutral = meanOfThumbnail(await thumbnailOf(sphere, SIZE), "01_neutral", SIZE);
    console.log(`[colour] neutral mean RGB: ${neutral.r},${neutral.g},${neutral.b}`);
    await shot("11_window_neutral");

    const delta =
      Math.abs(filmic.r - neutral.r) + Math.abs(filmic.g - neutral.g) + Math.abs(filmic.b - neutral.b);
    console.log(`[colour] channel delta between the two transforms: ${delta.toFixed(2)}`);

    // PROVE WE SAMPLED LIT GEOMETRY, not the background.
    //
    // `render_profile` drives the viewport CLEAR COLOUR as well as the tone curve, so a mean taken over
    // flat background would move when the profile changed and the test would "pass" while proving
    // nothing about the transform. The probe already returns `spread` — the max per-channel range over
    // the patch — and a flat fill has a spread near zero. A shaded sphere does not.
    console.log(`[colour] patch spread: filmic ${filmic.spread}, neutral ${neutral.spread}`);
    expect(filmic.spread).toBeGreaterThan(20);
    expect(neutral.spread).toBeGreaterThan(20);

    // Back to filmic. On a settled scene this must return to where it was, and THAT is what makes the
    // A-vs-B number attributable to the transform rather than to whatever else was still moving.
    await invoke("set_render_profile", { profile: "cinematic" });
    await browser.pause(500);
    const again = meanOfThumbnail(await thumbnailOf(sphere, SIZE), "02_filmic_again", SIZE);
    const drift = Math.abs(again.r - filmic.r) + Math.abs(again.g - filmic.g) + Math.abs(again.b - filmic.b);
    console.log(`[colour] filmic again: ${again.r},${again.g},${again.b} (drift ${drift.toFixed(2)})`);

    // The scene is stable, so the A/B/A shape is the whole argument: switching moved the image, and
    // switching back undid it. If the transform were cosmetic labelling, delta would be ~0.
    expect(drift).toBeLessThan(3);
    expect(delta).toBeGreaterThan(3 * drift + 3);
  });

  it("the colour card is on the Formats tab, showing the live transform", async () => {
    // The bottom dock renders a summary POPUP while collapsed and a real TAB LIST once open — so this
    // must open the dock first and click the tab, not the popup option (which stops existing the
    // moment the dock opens).
    await click('[data-testid="bottom-dock-toggle"]');
    await browser.pause(400);
    const clicked = await click("#bottom-workspaces-formats-tab");
    expect(clicked).toBe(true);
    await browser.pause(600);

    const card = await browser.execute(() => {
      const el = document.querySelector('[data-testid="colour-card"]');
      if (!el) return null;
      const cap = document.querySelector('[data-testid="colour-cap-acesCgWorkingSpaceSelectable"]');
      return {
        active: el.getAttribute("data-active-view"),
        working: document.querySelector('[data-testid="colour-working"]')?.textContent ?? "",
        notWiredShown: !!cap,
        notWiredText: cap?.textContent ?? "",
      };
    });
    console.log(`[colour] card: ${JSON.stringify(card)}`);
    await shot("12_formats_colour_card");
    expect(card).toBeTruthy();
    expect(card.working).toContain("Rec.709");
    expect(card.active).toBe("acesFit");
    // The panel must show the gap, not hide it.
    expect(card.notWiredShown).toBe(true);
    expect(card.notWiredText).toContain("not wired");
  });
});
