// Does an OS capture actually observe a change that touches ONLY the native viewport?
//
// Every "did the viewport change?" gate in this harness compares two composited-window captures. The
// capture reads the window's own presentation, so it is only evidence if a 3D-only change reliably
// reaches that presentation. If it does not — for instance because nothing forces a recomposite when no
// DOM repaints — then a stale frame reads as "the engine did not move", and a real engine defect and a
// capture artefact become indistinguishable.
//
// This spec settles that with the cheapest possible experiment, deliberately WITHOUT importing anything:
//
//   A. exposure change   — 3D only, no DOM. The discriminator.
//   B. entity selection  — changes the DOM (the Inspector). The positive control for the capture path.
//
// If A is zero and B is large, viewport-delta gates cannot see 3D-only changes on this machine and every
// such measurement must be reported as UNPROVEN rather than failed.

import { browser } from "@wdio/globals";
import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, statSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const e2eDir = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const captureScript = path.join(e2eDir, "scripts", "capture-composited-window.ps1");
const ffmpeg = process.env.MTK_FFMPEG || "ffmpeg";
const outDir = path.resolve(e2eDir, "../evidence/capture-integrity");

function capture(label) {
  mkdirSync(outDir, { recursive: true });
  const output = path.join(outDir, `${label}.png`);
  execFileSync("powershell.exe", [
    "-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
    "-File", captureScript, "-Out", output,
  ], { encoding: "utf8", timeout: 120_000, windowsHide: true });
  if (!existsSync(output) || statSync(output).size <= 1024) {
    throw new Error(`Capture ${label} is missing or implausibly small.`);
  }
  return output;
}

function delta(before, after) {
  const done = spawnSync(ffmpeg, [
    "-hide_banner", "-nostdin",
    "-i", before, "-i", after,
    "-lavfi", "[0][1]blend=all_mode=difference,signalstats,metadata=print",
    "-f", "null", "-",
  ], { encoding: "utf8", timeout: 120_000, windowsHide: true });
  const text = `${done.stdout ?? ""}\n${done.stderr ?? ""}`;
  const read = (key) => {
    const match = new RegExp(`lavfi\\.signalstats\\.${key}=([-0-9.]+)`).exec(text);
    return match ? Number(match[1]) : null;
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

describe("OS capture integrity", () => {
  it("observes a 3D-only change, and says so honestly when it cannot", async () => {
    await browser.waitUntil(
      async () => (await browser.execute(() => !!document.querySelector("#viewport"))) === true,
      { timeout: 60_000, interval: 250, timeoutMsg: "The editor shell never presented a viewport." },
    );
    await browser.pause(2_000);

    // ── A: 3D only, no DOM ─────────────────────────────────────────────────────────────────────
    await invoke("set_exposure", { exposure: 0.35 });
    await browser.pause(1_200);
    const exposureBefore = capture("a1-exposure-035");
    await invoke("set_exposure", { exposure: 2.4 });
    await browser.pause(1_200);
    const exposureAfter = capture("a2-exposure-240");
    const exposureDelta = delta(exposureBefore, exposureAfter);

    // ── B: DOM-changing positive control ───────────────────────────────────────────────────────
    await invoke("set_exposure", { exposure: 0.35 });
    await browser.pause(800);
    const domBefore = capture("b1-dom-before");
    const opened = await browser.execute(() => {
      const dock = document.querySelector('[data-testid="bottom-dock"]');
      const toggle = document.querySelector('[data-testid="bottom-dock-toggle"]');
      if (toggle instanceof HTMLElement) { toggle.click(); return true; }
      return !!dock;
    });
    await browser.pause(1_200);
    const domAfter = capture("b2-dom-after");
    const domDelta = delta(domBefore, domAfter);

    const verdict = {
      threeDOnly: { change: "exposure 0.35 -> 2.4", ...exposureDelta },
      domControl: { change: "toggle bottom dock", opened, ...domDelta },
      capturesObserveThreeDOnlyChanges: (exposureDelta.peakLuma ?? 0) > 0,
    };
    writeFileSync(path.join(outDir, "verdict.json"), `${JSON.stringify(verdict, null, 2)}\n`, "utf8");
    // eslint-disable-next-line no-console
    console.log(`CAPTURE_INTEGRITY_VERDICT ${JSON.stringify(verdict)}`);

    if ((domDelta.peakLuma ?? 0) === 0) {
      throw new Error(`Even a DOM change was not observed; the capture path itself is broken: ${JSON.stringify(verdict)}`);
    }
    if (!verdict.capturesObserveThreeDOnlyChanges) {
      throw new Error(
        "The OS capture does NOT observe 3D-only changes on this machine, so every viewport-delta gate "
        + `is measuring a stale frame: ${JSON.stringify(verdict)}`,
      );
    }
  });
});
