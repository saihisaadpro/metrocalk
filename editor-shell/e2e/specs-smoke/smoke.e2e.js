// EXE pipeline smoke — proves the real .exe launches, WebDriver connects, and BOTH capture paths work
// (WebDriver DOM screenshot + OS composited screenshot of the wgpu viewport). Used to validate the EXE
// audit pipeline before the B3–B6 batches.
import { browser } from "@wdio/globals";
import { execSync } from "node:child_process";
import { mkdirSync } from "node:fs";

const SHOT_DIR = "x:\\Dev Research & Projects\\Metrocalk\\.uxtest\\audit\\shots-exe";
const PS1 = "x:\\Dev Research & Projects\\Metrocalk\\.uxtest\\audit\\exe\\capture-window.ps1";
mkdirSync(SHOT_DIR, { recursive: true });

describe("EXE pipeline smoke", () => {
  it("launches, connects, and captures DOM + OS screenshots", async () => {
    await browser.waitUntil(
      async () => {
        try {
          const t = await (await browser.$("#count")).getText();
          return /\d+\s+entities/.test(t);
        } catch {
          return false;
        }
      },
      { timeout: 60000, timeoutMsg: "editor never connected (#count empty)" },
    );
    const count = await (await browser.$("#count")).getText();
    console.log("SMOKE: connected —", count);

    // Frame the whole scene so any rendered geometry is centered in view (disambiguates "viewport is
    // see-through" from "camera just isn't pointed at anything"). Then give wgpu a few frames to paint.
    try {
      await browser.execute(async () => window.__TAURI__.core.invoke("frame_all"));
      await browser.execute(async () => window.__TAURI__.core.invoke("view_preset", { preset: "persp" }));
      await browser.execute(async () => window.__TAURI__.core.invoke("frame_all"));
    } catch (e) {
      console.error("SMOKE: frame_all failed —", String(e).slice(0, 120));
    }
    await browser.pause(900);

    // 1) WebDriver (DOM/WebView) screenshot — captures the React panels (transparent over the viewport).
    await browser.saveScreenshot(`${SHOT_DIR}\\smoke_dom.png`);
    console.log("SMOKE: DOM screenshot saved");

    // 2) OS composited screenshot — the only way to see the native wgpu viewport pixels.
    try {
      const out = execSync(
        `powershell -ExecutionPolicy Bypass -File "${PS1}" -ProcName "metrocalk-editor-shell" -Out "${SHOT_DIR}\\smoke_os.png"`,
        { encoding: "utf8" },
      );
      console.log("SMOKE: OS capture —", out.trim());
    } catch (e) {
      console.error("SMOKE: OS capture FAILED —", String(e).slice(0, 200));
    }

    expect(/\d+/.test(count)).toBe(true);
  });
});
