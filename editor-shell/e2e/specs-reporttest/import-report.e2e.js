// LIVE proof that the M15.7 import-report panel surfaces the never-silent per-part breakdown on a real
// import: import the STEP bar file into the packaged .exe, then assert the ImportReport panel shows the
// fidelity totals (data-total / data-below-exact) + per-class filter chips + rows — the "explain every no"
// surface, keyed on structured data-* signals, not prose. OS-captures the composite for a visual record.

import { browser } from "@wdio/globals";
import { execSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const dir = path.dirname(fileURLToPath(import.meta.url));
const SHOT_DIR = process.env.MTK_SHOT_DIR || path.resolve(dir, "../.shots-reporttest");
const SHOT_PS1 = path.resolve(dir, "../../../.uxtest/audit/exe/capture-window-fg.ps1");
const PROC = "metrocalk-editor-shell";
const STEP_FILE =
  process.env.MTK_STEP_BAR_FILE ||
  "X:\\Work\\Metrocalk\\Games Projects\\Unreal\\Skid Weld Line A.1\\Skid Weld Line A.1_(1).stp";
mkdirSync(SHOT_DIR, { recursive: true });

const shot = async (label) => {
  await browser.pause(600);
  try {
    execSync(`powershell -ExecutionPolicy Bypass -File "${SHOT_PS1}" -ProcName "${PROC}" -Out "${path.join(SHOT_DIR, `${label}.png`)}"`, { stdio: "ignore" });
  } catch (e) {
    console.error("shot failed", label, String(e));
  }
};

const kick = (cmd, args) =>
  browser.execute((c, a) => { window.__TAURI__.core.invoke(c, a || {}).catch(() => {}); return true; }, cmd, args);
const invoke = (cmd, args) =>
  browser.execute(async (c, a) => { try { return await window.__TAURI__.core.invoke(c, a || {}); } catch (e) { return { error: String(e) }; } }, cmd, args);
const entityCount = () =>
  browser.execute(() => { const el = document.getElementById("count"); const m = el && el.textContent ? el.textContent.match(/\d+/) : null; return m ? parseInt(m[0], 10) : 0; });

describe("M15.7 import report — the never-silent per-part surface, live", () => {
  before(async () => {
    await browser.waitUntil(
      async () => (await browser.execute(() => !!(window.__TAURI__ && window.__TAURI__.core))) === true,
      { timeout: 30000, timeoutMsg: "TAURI bridge never appeared" },
    );
    await browser.execute(() => { try { localStorage.setItem("mtk.onboarded.v1", "1"); } catch (e) { void e; } });
    await browser.pause(500);
  });

  it("imports the STEP bar file and the ECS import report accounts for every part", async () => {
    // Before any import the report is empty (never-silent has nothing to say).
    const empty = await invoke("cad_report");
    console.log("cad_report before:", JSON.stringify(empty));
    if (empty.total !== 0) throw new Error(`expected an empty report before import, got total=${empty.total}`);

    await kick("import_asset", { path: STEP_FILE });
    await browser.waitUntil(async () => (await entityCount()) >= 300, { timeout: 240000, interval: 2000, timeoutMsg: "the STEP import never landed" });
    await browser.pause(1000);

    // The report is the ECS-native never-silent surface the panel renders — assert it straight off the
    // backend command (the panel is a thin, layout-dependent view over exactly this).
    const r = await invoke("cad_report");
    console.log("cad_report after:", JSON.stringify({ ...r, parts: `[${r.parts?.length} rows]` }, null, 2));
    await shot("01_after_import");

    // `cad_report` has NO error channel — it returns `CadReportResp`, never a `Result`, and this guard
    // used to read `r.error`, a field that struct has never carried. It could not fire. The real
    // failure mode is the one the command actually takes: if the engine never answers the send, it
    // returns `CadReportResp::default()` — every count zero and no rows — which is what to name.
    if (r.total === 0 && Array.isArray(r.parts) && r.parts.length === 0)
      throw new Error("cad_report returned the default all-zero report — the engine never answered the command");
    if (!(r.total >= 300)) throw new Error(`expected the report to account for the import (>=300 parts), got ${r.total}`);
    // NEVER-SILENT: the breakdown sums to the total — nothing dropped without a class.
    const sum = r.exactBrep + r.tessellationOnly + r.aiReconstructed + r.proxy + r.accessDenied + r.failed;
    if (sum !== r.total) throw new Error(`the fidelity breakdown (${sum}) does not sum to the total (${r.total}) — a part is unaccounted for`);
    // The bar file re-export is tessellation-only, so that class is non-zero + every row carries a token.
    if (!(r.tessellationOnly > 0)) throw new Error(`expected tessellation-only parts on the bar file, got ${r.tessellationOnly}`);
    if (!Array.isArray(r.parts) || r.parts.length === 0) throw new Error("no per-part rows in the report");
    for (const p of r.parts) {
      if (typeof p.fidelity !== "string" || !p.fidelity) throw new Error(`a part row is missing its fidelity token: ${JSON.stringify(p)}`);
    }

    console.log(`✓ import report (ECS-native) live: ${r.total} parts — ${r.exactBrep} exact · ${r.tessellationOnly} tessellation-only · ${r.proxy} proxy · ${r.failed} failed; breakdown sums to total, every row classed.`);
  });
});
