// The acceptance library — the machinery that turns a click into the full ACCEPTANCE-DIMENSION
// CONJUNCTION (functional + invariants 1–4 + principles 1–3 + offline + clean) the gate scores, plus the
// budget capture (idle/warm/p50/p99 + baseline-diff + flake-quarantine) and the machine-readable report +
// COVERAGE.md emitter. Imported by every acceptance spec; UI-agnostic (it talks to commands + the
// page-object, never raw selectors).

import { browser, expect } from "@wdio/globals";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";
// Re-export the page-object's `invoke` so specs can import the whole acceptance toolkit (report · budgets ·
// console guard · `invoke` read-back) from one module — the shape every acceptance spec was authored to.
export { invoke };

const dir = path.dirname(fileURLToPath(import.meta.url));
const E2E = path.resolve(dir, "..");

// ── clean: no JS console errors / unhandled rejections during a workflow ─────────────────────────
// WebView2 + tauri-driver don't reliably expose `browser.getLogs`, so we install our own capture: a
// global error/rejection sink in the page, read back after each workflow.
export async function installConsoleGuard() {
  await browser.execute(() => {
    if (window.__mtkErrors) return;
    window.__mtkErrors = [];
    window.addEventListener("error", (e) => window.__mtkErrors.push(String(e.message || e.error)));
    window.addEventListener("unhandledrejection", (e) =>
      window.__mtkErrors.push("unhandledrejection: " + String(e.reason))
    );
    const err = console.error.bind(console);
    console.error = (...a) => {
      window.__mtkErrors.push("console.error: " + a.map(String).join(" "));
      err(...a);
    };
  });
}
export const consoleErrors = () => browser.execute(() => window.__mtkErrors || []);
export async function clearConsole() {
  await browser.execute(() => {
    window.__mtkErrors = [];
  });
}

// ── invariant 4: the hot path never crosses JS — `render::IPC_CALLS` 0/frame during an interaction ─
// Run `action` for `ms` and assert the IPC counter grew by far less than 1/frame (~60 fps → ~ms/16
// frames). Returns the per-frame IPC estimate.
export async function ipcPerFrame(action, ms = 450) {
  const a = await invoke("ipc_count");
  await action();
  await browser.pause(ms);
  const b = await invoke("ipc_count");
  const frames = ms / 16.7;
  const perFrame = (b - a) / frames;
  return perFrame;
}

// ── principle 2: budgets, measured (idle machine · warm up · median + p50/p99 · N iterations) ─────
// Time a command's round-trip from inside the WebView (incl. IPC) over N iterations after a warm-up.
// Returns { label, n, p50, p99, max }. The caller asserts ≤ 16 ms AND within baseline tolerance.
export async function captureBudget(label, cmd, argsFn, { n = 30, warmup = 5 } = {}) {
  const sample = async () => {
    const args = typeof argsFn === "function" ? argsFn() : argsFn || {};
    return browser.execute(
      async (c, a) => {
        const t = performance.now();
        await window.__TAURI__.core.invoke(c, a);
        return performance.now() - t;
      },
      cmd,
      args
    );
  };
  // A small node-side yield between samples so the back-to-back WebDriver round-trips don't saturate the
  // WebView2 DevTools channel under the heavier React renderer. A tight uninterrupted burst of ~35
  // `execute/sync` invokes intermittently stalled the renderer (a 30s "Timed out receiving message from
  // renderer" → the 120s mocha timeout); spacing the round-trips lets the renderer message pump breathe.
  // It does NOT affect what is measured: each sample independently times its own invoke round-trip.
  const breathe = () => new Promise((r) => setTimeout(r, 5));
  for (let i = 0; i < warmup; i++) {
    await sample(); // warm up — not scored
    await breathe();
  }
  const xs = [];
  for (let i = 0; i < n; i++) {
    xs.push(await sample());
    await breathe();
  }
  xs.sort((x, y) => x - y);
  const q = (p) => xs[Math.min(xs.length - 1, Math.floor(p * xs.length))];
  return { label, n, p50: q(0.5), p99: q(0.99), max: xs[xs.length - 1] };
}

// Baseline (local doc — seeded from progress.md's measured numbers). Missing file ⇒ no regression gate
// (first run records the baseline), only the absolute ≤16 ms per-frame budget applies.
export function loadBaseline() {
  const p = path.join(E2E, "baseline.json");
  try {
    return JSON.parse(fs.readFileSync(p, "utf8"));
  } catch {
    return { ops: {} };
  }
}

// Score a budget sample: the per-frame ops must be ≤ FRAME_MS; a one-shot heavy is checked only against
// its own baseline. A p99 over (factor × baseline) is a REGRESSION — but a single spike under transient
// load is quarantined (re-measured isolated) before failing, distinguishing a real regression from jitter.
const FRAME_MS = 16;
export async function scoreBudget(sample, baseline, { perFrame = true, factor = 1.5, recapture } = {}) {
  const base = baseline.ops?.[sample.label];
  const result = { ...sample, perFrame, baseline: base?.p99 ?? null, verdict: "pass", note: "" };
  if (perFrame && sample.p99 > FRAME_MS) {
    result.verdict = "fail";
    result.note = `p99 ${sample.p99.toFixed(2)} ms > ${FRAME_MS} ms frame budget`;
  }
  if (base && sample.p99 > factor * base.p99) {
    // Flake guard: re-measure ONCE in isolation; only a persistent over-baseline is a real regression.
    if (recapture) {
      const re = await recapture();
      result.requarantined = { p50: re.p50, p99: re.p99 };
      if (re.p99 > factor * base.p99) {
        result.verdict = "fail";
        result.note = `p99 ${re.p99.toFixed(2)} ms > ${factor}× baseline ${base.p99} ms (regression, confirmed isolated)`;
      } else {
        result.note = `p99 spike ${sample.p99.toFixed(2)} ms quarantined → isolated ${re.p99.toFixed(2)} ms within baseline`;
      }
    } else {
      result.verdict = "fail";
      result.note = `p99 ${sample.p99.toFixed(2)} ms > ${factor}× baseline ${base.p99} ms`;
    }
  }
  return result;
}

// ── the report accumulator ───────────────────────────────────────────────────────────────────────
// One per run. Workflows record their dimension conjunction + evidence; ops record budgets; the run
// records console/IPC/offline/min-spec. `writeArtifacts` emits acceptance-report.json (machine-readable,
// the gate) + COVERAGE.md (human, generated from it).
export class Report {
  constructor(profile = "high-end") {
    this.profile = profile;
    this.workflows = [];
    this.budgets = [];
    this.consoleErrorCount = 0;
    this.offline = null;
    this.startedFrame = 0; // a coarse timestamp surrogate (no Date in scripts; harness fills via args)
  }
  workflow(name, dims, { commands = [], evidence = null } = {}) {
    // dims = { functional, inv1, inv2, inv3, inv4, p1_interactions, p1_explained, clean, offline }
    // Each value is true (pass) / false (fail) / null (not-applicable-to-this-workflow).
    const pass = Object.entries(dims).every(([, v]) => v === true || v === null);
    this.workflows.push({ name, dims, pass, commands, evidence });
    return pass;
  }
  budget(scored) {
    this.budgets.push(scored);
  }
  commandsCovered(inventory) {
    const covered = new Set();
    for (const wf of this.workflows) for (const c of wf.commands || []) covered.add(c);
    const declared = new Set(inventory.flatMap((e) => e.command));
    return { covered: [...covered], declared: [...declared] };
  }

  writeArtifacts(inventory, { reportName = "acceptance-report.json", coverageName = "COVERAGE.md" } = {}) {
    const report = {
      profile: this.profile,
      controls: { total: inventory.length, exercised: this.workflows.filter((w) => w.pass).length },
      workflows: this.workflows,
      budgets: this.budgets,
      consoleErrorCount: this.consoleErrorCount,
      offline: this.offline,
    };
    fs.writeFileSync(path.join(E2E, reportName), JSON.stringify(report, null, 2));
    fs.writeFileSync(path.join(E2E, coverageName), renderCoverage(report, inventory));
    return report;
  }
}

// The run-wide singleton every acceptance spec accumulates into; the wdio `after` hook writes its
// artifacts once. The profile (high-end | min-spec) is set via MTK_PROFILE so the same suite runs both.
export const report = new Report(process.env.MTK_PROFILE || "high-end");

function renderCoverage(report, inventory) {
  const L = [];
  L.push("# Build-acceptance coverage matrix (generated — do not edit by hand)");
  L.push("");
  L.push(`Profile: **${report.profile}** · controls ${report.controls.exercised}/${report.controls.total} · console errors ${report.consoleErrorCount}`);
  L.push("");
  L.push("| Control | Commands | Workflow | Result |");
  L.push("|---|---|---|---|");
  const byWf = new Map(report.workflows.map((w) => [w.name, w]));
  for (const e of inventory) {
    const w = byWf.get(e.workflow);
    const res = w ? (w.pass ? "✅ pass" : "❌ FAIL") : "⏳ not-run";
    L.push(`| \`${e.control}\` | ${e.command.join(", ") || "—"} | ${e.workflow} | ${res} |`);
  }
  L.push("");
  L.push("## Budgets (live p50/p99, ms)");
  L.push("| Op | p50 | p99 | baseline p99 | verdict |");
  L.push("|---|---|---|---|---|");
  for (const b of report.budgets) {
    L.push(`| ${b.label} | ${b.p50?.toFixed(3)} | ${b.p99?.toFixed(3)} | ${b.baseline ?? "—"} | ${b.verdict}${b.note ? " — " + b.note : ""} |`);
  }
  return L.join("\n") + "\n";
}
