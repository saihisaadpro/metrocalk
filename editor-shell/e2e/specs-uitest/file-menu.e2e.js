// LIVE proof the File-menu overlay fix works in the packaged .exe: open the File menu, assert the dropdown is
// PORTALED to <body> (so the header's `overflow: hidden` can't clip it), fully on-screen, and above the chrome,
// then OS-capture the composited window (the dropdown must be visible, not hidden behind the suggestion bar).

import { browser } from "@wdio/globals";
import { execSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const dir = path.dirname(fileURLToPath(import.meta.url));
const SHOT_DIR = process.env.MTK_SHOT_DIR || path.resolve(dir, "../.shots-uitest");
const SHOT_PS1 = path.resolve(dir, "../../../.uxtest/audit/exe/capture-window-fg.ps1");
const PROC = "metrocalk-editor-shell";
mkdirSync(SHOT_DIR, { recursive: true });

const shot = async (label) => {
  await browser.pause(700);
  const out = path.join(SHOT_DIR, `${label}.png`);
  try {
    execSync(`powershell -ExecutionPolicy Bypass -File "${SHOT_PS1}" -ProcName "${PROC}" -Out "${out}"`, { stdio: "ignore" });
    console.log("  shot", out);
  } catch (e) {
    console.error("shot failed", label, String(e));
  }
};

describe("File menu overlay (portaled, not clipped)", () => {
  before(async () => {
    await browser.waitUntil(
      async () => (await browser.execute(() => !!(window.__TAURI__ && window.__TAURI__.core))) === true,
      { timeout: 30000, timeoutMsg: "TAURI bridge never appeared" },
    );
    await browser.execute(() => {
      try {
        localStorage.setItem("mtk.onboarded.v1", "1");
      } catch (e) {
        void e;
      }
    });
    await browser.pause(500);
  });

  it("opens the File dropdown portaled to <body>, fully on-screen, above the chrome", async () => {
    await shot("00_before_open");

    // Open the File menu.
    await browser.execute(() => document.querySelector('[data-testid="fileMenu"]')?.click());
    await browser.pause(300);

    const report = await browser.execute(() => {
      const panel = document.querySelector('[data-testid="fileMenuPanel"]');
      if (!panel) return { present: false };
      const menuEl = panel.closest('[role="menu"]') || panel;
      const parentIsBody = menuEl.parentElement === document.body;
      const insideHeaderClip = !!panel.closest('#fileMenuRoot'); // if true, it did NOT portal out
      const r = panel.getBoundingClientRect();
      const onScreen = r.top >= 0 && r.left >= 0 && r.bottom <= window.innerHeight + 1 && r.right <= window.innerWidth + 1;
      // What actually paints at the dropdown's own centre? (occlusion check — is the menu on top?)
      const cx = Math.round(r.left + r.width / 2);
      const cy = Math.round(r.top + r.height / 2);
      const topEl = document.elementFromPoint(cx, cy);
      const menuOnTop = !!(topEl && (menuEl.contains(topEl) || topEl === menuEl));
      return { present: true, parentIsBody, insideHeaderClip, onScreen, menuOnTop, rect: { t: Math.round(r.top), l: Math.round(r.left), w: Math.round(r.width), h: Math.round(r.height) }, tag: topEl?.tagName };
    });
    console.log("File-menu overlay report:", JSON.stringify(report));

    await shot("01_file_menu_open");

    // Structured assertions (the regression guards):
    if (!report.present) throw new Error("File dropdown did not open");
    if (!report.parentIsBody) throw new Error("dropdown is NOT portaled to <body> (parent != body)");
    if (report.insideHeaderClip) throw new Error("dropdown is still nested inside #fileMenuRoot → clippable");
    if (!report.onScreen) throw new Error(`dropdown is off-screen: ${JSON.stringify(report.rect)}`);
    if (!report.menuOnTop) throw new Error(`dropdown is OCCLUDED — top element at its centre is <${report.tag}>, not the menu`);

    console.log("✓ File menu opens portaled to <body>, fully on-screen, and paints ON TOP (not clipped/hidden).");
  });
});
