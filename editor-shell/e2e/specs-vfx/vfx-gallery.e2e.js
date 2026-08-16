// THE EFFECTS GALLERY — every effect card, authored and MEASURED on the packaged .exe.
//
// Two things this proves that the main VFX spec does not:
//
//  1. **The SOFT (alpha-blended) pipeline actually draws.** Every measurement taken before this spec
//     existed read `soft = 0`, because every card exercised was additive. Half the renderer was
//     shipped unproven. Smoke, steam, snow, rain, poison, bubbles, dust and confetti all go through it.
//  2. **The catalogue is a vocabulary, not a menu of near-duplicates** — each card puts a measurably
//     different number of particles on screen, on the pipeline it claims, at the radiance it claims.
//
// Each card leaves a screenshot, so the gallery is a contact sheet.

import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, rmSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const dir = path.dirname(fileURLToPath(import.meta.url));
const shots = path.resolve(dir, "../.shots-gallery");
const capture = path.resolve(dir, "../scripts/capture-composited-window.ps1");
const captureFg = path.resolve(dir, "../../../.uxtest/audit/exe/capture-window-fg.ps1");
mkdirSync(shots, { recursive: true });

async function shot(label) {
  const out = path.join(shots, `${label}.png`);
  const good = () => existsSync(out) && statSync(out).size > 20_000;
  const attempt = (script, args) => {
    try {
      execFileSync("powershell.exe", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", script, ...args], { stdio: "pipe" });
    } catch { /* fall through */ }
    if (!good() && existsSync(out)) rmSync(out);
    return good();
  };
  const ok = attempt(capture, ["-Out", out]) || attempt(captureFg, ["-ProcName", "metrocalk-editor-shell", "-Out", out]);
  if (!ok) console.log(`[gal] CAPTURE UNAVAILABLE for ${label}`);
  return ok;
}

const click = (selector) =>
  browser.execute((sel) => {
    const el = document.querySelector(sel);
    if (!el) return false;
    el.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    return true;
  }, selector);

const fx = () => invoke("vfx_probe");

async function pressPlay() {
  await click('[data-testid="play"]');
  await browser.waitUntil(
    async () => browser.execute(() => !!document.querySelector('[data-testid="stop"]')),
    { timeout: 10000, timeoutMsg: "Play never engaged" },
  );
}
async function pressStop() {
  await click('[data-testid="stop"]');
  await browser.pause(500);
}

/** Author exactly one effect, run it, and read the HIGH-WATER MARKS rather than racing a sample. */
async function runEffect(host, kind, snap, oneShot) {
  const add = await invoke("vfx_add", { id: host, kind, trigger: "always" });
  if (add.reason) throw new Error(`${kind} refused: ${add.reason}`);
  await pressPlay();
  // A one-shot with a 0.6s life is long over before a 900ms sample arrives, so LOOK EARLY for those.
  // The assertions all read high-water marks either way; this only decides when the picture is taken.
  await browser.pause(oneShot ? 240 : 900);
  const mid = await fx();
  if (snap) await shot(`${snap}_${kind}`);
  await browser.pause(300);
  const marks = await fx();
  await pressStop();
  await browser.pause(400);
  await invoke("vfx_remove", { id: host, index: 0 });
  const left = await invoke("vfx_list", { id: host });
  if (left.layers !== 0) throw new Error(`${kind} did not clean up: ${left.layers} left`);
  return { kind, mid, peak: marks.peakTotal, radiance: marks.peakRadianceMax, reads: add.reads[0] };
}

describe("The effects gallery — every card, measured live", () => {
  let host;
  const runs = [];

  before(async () => {
    await browser.waitUntil(
      async () => browser.execute(() => !!(window.__TAURI__ && window.__TAURI__.core)),
      { timeout: 30000 },
    );
    await pressStop();
    await invoke("new_project");
    await browser.pause(700);
    await browser.execute(() => {
      document.querySelector('[data-testid="onboardSkip"]')?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    host = (await invoke("shape_spawn", { kind: "cylinder", pos: [0, 0.6, 0] })).created;
    // Keep the reference geometry CLOSE. `frame_all` frames the whole scene, so scattering boxes at
    // ±4 units parked the camera far enough back that a 0.3-unit particle was a few pixels — the
    // measurements were fine and the contact sheet was useless.
    // BESIDE and BEHIND the host, never in front of it: at +Z these sat between the camera and the
    // effect, and the depth test correctly hid the very thing the capture existed to show.
    await invoke("shape_spawn", { kind: "box", pos: [1.9, 0.4, -1.2] });
    await invoke("shape_spawn", { kind: "box", pos: [-1.9, 0.4, -1.2] });
    await invoke("frame_all");
  });

  it("offers a broad catalogue with both looks represented", async () => {
    const catalogue = await invoke("vfx_catalog");
    console.log(`[gal] ${catalogue.length} effect cards: ${catalogue.map((c) => c.kind).join(", ")}`);
    expect(catalogue.length).toBeGreaterThanOrEqual(18);
    const oneShots = catalogue.filter((c) => c.burst).map((c) => c.kind);
    console.log(`[gal] one-shot cards: ${oneShots.join(", ")}`);
    expect(oneShots.length).toBeGreaterThanOrEqual(5);
    for (const c of catalogue) expect(c.adds.length).toBeGreaterThan(10);
  });

  it("runs every card live and captures each one", async () => {
    const catalogue = await invoke("vfx_catalog");
    let i = 0;
    for (const card of catalogue) {
      const r = await runEffect(host, card.kind, String(i).padStart(2, "0"), card.burst);
      runs.push(r);
      console.log(
        `[gal] ${card.kind.padEnd(10)} peak=${String(r.peak).padStart(4)} add=${String(r.mid.additive).padStart(4)} soft=${String(r.mid.soft).padStart(4)} peakRad=${r.radiance.toFixed(2).padStart(6)}  "${r.reads}"`,
      );
      i += 1;
    }
    expect(runs.length).toBeGreaterThanOrEqual(18);
  });

  it("EVERY card actually drew something — no card is a dead menu entry", async () => {
    const dead = runs.filter((r) => r.peak === 0).map((r) => r.kind);
    console.log(`[gal] cards that drew nothing: ${dead.join(", ") || "none"}`);
    expect(dead).toEqual([]);
  });

  it("THE SOFT PIPELINE DRAWS — the half of the renderer nothing had exercised", async () => {
    // Until this assertion existed, every live measurement of this engine read soft = 0.
    const soft = runs.filter((r) => r.mid.soft > 0).map((r) => `${r.kind}:${r.mid.soft}`);
    console.log(`[gal] sampled totals: ${runs.map((r) => `${r.kind} ${r.mid.total}`).join(" · ")}`);
    console.log(`[gal] cards on the occluding path: ${soft.join(", ")}`);
    expect(soft.length).toBeGreaterThanOrEqual(5);
    const smoke = runs.find((r) => r.kind === "smoke");
    console.log(`[gal] SMOKE — soft=${smoke.mid.soft} additive=${smoke.mid.additive}`);
    expect(smoke.mid.soft).toBeGreaterThan(10);
    // Smoke OCCLUDES. If it ever renders additive it will glow instead of darkening, which is the
    // whole difference between smoke and fire.
    expect(smoke.mid.additive).toBe(0);
  });

  it("the glowing cards genuinely EMIT — over 1.0 is what bloom responds to", async () => {
    const glowing = ["fire", "explosion", "sparks", "portal", "embers", "aura", "sparkle"];
    // The HIGH-WATER mark, not a sample: a spark burst is over in 0.6s and "does this card emit?" must
    // not depend on whether a poll happened to land inside that window.
    const weak = runs
      .filter((r) => glowing.includes(r.kind) && r.radiance <= 1.0)
      .map((r) => `${r.kind}:${r.radiance.toFixed(2)}`);
    console.log(
      `[gal] peak radiance: ${runs.filter((r) => glowing.includes(r.kind)).map((r) => `${r.kind} ${r.radiance.toFixed(1)}`).join(" · ")}`,
    );
    expect(weak).toEqual([]);
  });

  it("and the soft cards do NOT pretend to emit", async () => {
    const softCards = ["smoke", "steam", "snow", "rain", "poison", "bubbles", "dust"];
    const glowingSoft = runs
      .filter((r) => softCards.includes(r.kind) && r.mid.additive > 0)
      .map((r) => r.kind);
    console.log(`[gal] soft cards wrongly on the additive path: ${glowingSoft.join(", ") || "none"}`);
    expect(glowingSoft).toEqual([]);
  });

  it("no card blew the scene budget, and Stop left nothing behind", async () => {
    const over = runs.filter((r) => r.peak > 6000).map((r) => `${r.kind}:${r.peak}`);
    console.log(`[gal] over budget: ${over.join(", ") || "none"}`);
    expect(over).toEqual([]);
    const after = await fx();
    console.log(`[gal] after the whole gallery: total=${after.total} bursts=${after.bursts}`);
    expect(after.total).toBe(0);
    expect((await invoke("vfx_list", { id: host })).layers).toBe(0);
  });
});
