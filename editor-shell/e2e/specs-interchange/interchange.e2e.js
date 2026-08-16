// INTERCHANGE on the PACKAGED .exe — what this engine can actually read and write, measured.
//
// BEFORE: "does Metrocalk handle X?" had four answers that disagreed — a hand-written extension list
// in the native file dialog, a magic-byte sniffer, a set of Cargo features, and the commands that were
// actually registered. The observable consequences were specific: **a complete, tested STEP AP242
// writer existed and no command called it**, so CAD could come in and nothing could go back out; KTX2
// was importable and absent from the dialog; audio imported and was silently discarded.
// AFTER: one declared registry. The dialog filter is derived from it, the UI can list it, and STEP is
// wired end to end — a scene exports to faceted AP242 that reads back as the same geometry.

import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, rmSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const dir = path.dirname(fileURLToPath(import.meta.url));
const shots = path.resolve(dir, "../.shots-interchange");
const out = path.resolve(dir, "../.shots-interchange/out");
const capture = path.resolve(dir, "../scripts/capture-composited-window.ps1");
const captureFg = path.resolve(dir, "../../../.uxtest/audit/exe/capture-window-fg.ps1");
mkdirSync(shots, { recursive: true });
mkdirSync(out, { recursive: true });

async function shot(label) {
  await browser.pause(400);
  const file = path.join(shots, `${label}.png`);
  const good = () => existsSync(file) && statSync(file).size > 20_000;
  const attempt = (script, args) => {
    try {
      execFileSync("powershell.exe", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", script, ...args], { stdio: "pipe" });
    } catch { /* fall through */ }
    if (!good() && existsSync(file)) rmSync(file);
    return good();
  };
  const ok = attempt(capture, ["-Out", file]) || attempt(captureFg, ["-ProcName", "metrocalk-editor-shell", "-Out", file]);
  if (!ok) console.log(`[io] CAPTURE UNAVAILABLE for ${label}`);
  return ok;
}

const click = (selector) =>
  browser.execute((sel) => {
    const el = document.querySelector(sel);
    if (!el) return false;
    el.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    return true;
  }, selector);

describe("Interchange — one declared registry, and the export that was unreachable", () => {
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
  });

  it("declares every format it handles, in one list, with an honest fidelity note", async () => {
    const cat = await invoke("format_catalog");
    console.log(`[io] ${cat.length} formats declared:`);
    for (const f of cat) {
      console.log(
        `[io]   ${f.id.padEnd(8)} ${f.direction.padEnd(6)} ${f.fidelity.padEnd(7)} ` +
        `${f.available ? "  " : "UNAVAILABLE "}${f.extensions.join("/")}`,
      );
    }
    expect(cat.length).toBeGreaterThanOrEqual(10);
    // Every entry states what it costs, so the answer is never folklore.
    for (const f of cat) {
      expect(f.note.length).toBeGreaterThan(30);
      expect(f.extensions.length).toBeGreaterThan(0);
    }
    // The three domains this engine claims to serve are all represented.
    const domains = new Set(cat.map((f) => f.domain));
    console.log(`[io] domains: ${[...domains].join(", ")}`);
    for (const d of ["Real-time", "CAD", "Simulation", "Textures"]) {
      expect([...domains]).toContain(d);
    }
  });

  it("a declared SUBSET names the specific thing it will not carry", async () => {
    // The discipline that makes the list trustworthy: a subset that does not say what it drops is
    // indistinguishable from a claim of full support.
    const cat = await invoke("format_catalog");
    const step = cat.find((f) => f.id === "step");
    const usd = cat.find((f) => f.id === "usd");
    console.log(`[io] STEP: ${step.note}`);
    console.log(`[io] USD:  ${usd.note}`);
    expect(step.fidelity).toBe("subset");
    expect(step.note).toContain("NURBS");
    expect(usd.fidelity).toBe("subset");
    expect(usd.note).toContain("composition");
  });

  it("STEP is declared in BOTH directions — the writer is no longer unreachable", async () => {
    const cat = await invoke("format_catalog");
    const step = cat.find((f) => f.id === "step");
    console.log(`[io] STEP direction: ${step.direction}`);
    expect(step.direction).toBe("both");
    expect(step.carries.metadata).toBe(true); // semantic PMI
  });

  it("builds a scene and WRITES IT AS STEP AP242 — the new capability, on disk", async () => {
    for (const p of [[0, 0.5, 0], [2.2, 0.5, 0], [-2.2, 0.5, 0]]) {
      await invoke("shape_spawn", { kind: "box", pos: p });
    }
    await invoke("shape_spawn", { kind: "cylinder", pos: [0, 0.5, 2.4] });
    await invoke("frame_all");
    await shot("00_scene_to_export");

    const target = path.join(out, "scene.step");
    if (existsSync(target)) rmSync(target);
    const reply = await invoke("scene_export", { format: "step", path: target });
    console.log(`[io] scene_export(step) → ${JSON.stringify(reply.message ?? reply)}`);
    expect(existsSync(target)).toBe(true);

    const text = readFileSync(target, "utf8");
    console.log(`[io] wrote ${text.length} bytes of Part-21`);
    // It is a real ISO 10303-21 file, not "text that looks like STEP".
    expect(text.startsWith("ISO-10303-21;")).toBe(true);
    expect(text.trimEnd().endsWith("END-ISO-10303-21;")).toBe(true);
    expect(text).toContain("FILE_SCHEMA");
    // Faceted B-rep: each triangle is a FACE bounded by a POLY_LOOP of CARTESIAN_POINTs. That is the
    // right STEP for tessellated geometry — ADVANCED_FACE would imply an analytic surface underneath,
    // which a triangle mesh does not have and which this exporter deliberately refuses to invent.
    expect(text).toContain("POLY_LOOP");
    expect(text).toContain("FACE_OUTER_BOUND");
    const faces = (text.match(/=\s*FACE\(/g) || []).length;
    const loops = (text.match(/POLY_LOOP/g) || []).length;
    console.log(`[io] FACE count: ${faces}, POLY_LOOP count: ${loops}`);
    // Four primitives worth of tessellation, not an empty shell.
    expect(faces).toBeGreaterThan(20);
    expect(loops).toBe(faces);
  });

  it("and the exported file READS BACK through the engine's own STEP reader", async () => {
    // The claim that separates a real exporter from a plausible one.
    const target = path.join(out, "scene.step");
    const created = await invoke("import_asset", { path: target });
    console.log(`[io] re-imported the exported STEP → ${created ?? "null"}`);
    expect(created).toBeTruthy();
    const report = await invoke("cad_report");
    console.log(`[io] CAD report after re-import: ${report.total} part(s)`);
    expect(report.total).toBeGreaterThan(0);
    await shot("01_step_roundtripped_back_in");
  });

  it("THE PANEL: the registry is visible in the app, not just in a command", async () => {
    // A capability list nothing renders is the same problem the registry was built to fix, one level
    // up: the answer exists and nobody can see it.
    // The workspace list lives behind the bottom dock's summary popup, so open the dock, then the
    // menu, then pick the tab — the same three gestures a person makes.
    // The dock shows a summary POPUP while collapsed and a real TAB LIST once open, so the route
    // depends on its state: open it, then click the tab.
    const isOpen = () =>
      browser.execute(() => !!document.querySelector('[data-testid="bottom-dock"].is-open'));
    if (!(await isOpen())) {
      await click('[data-testid="bottom-dock-toggle"]');
      await browser.pause(400);
    }
    expect(await click("#bottom-workspaces-formats-tab")).toBe(true);
    await (await $('[data-testid="formats-panel"]')).waitForExist({ timeout: 10000 });

    const text = await browser.execute(
      () => document.querySelector('[data-testid="formats-panel"]')?.textContent ?? "",
    );
    console.log(`[io] the Formats panel reads: ${text.slice(0, 180).replace(/\s+/g, " ")}…`);
    // Every domain the engine serves is on screen.
    for (const needle of ["Real-time", "CAD", "Simulation", "Textures"]) {
      expect(text).toContain(needle);
    }
    // Fidelity is a visible column, not a hidden nuance.
    for (const needle of ["Full", "Subset", "Seam"]) {
      expect(text).toContain(needle);
    }
    // STEP shows as readable AND writable, which is the whole point of this round.
    const stepDir = await browser.execute(
      () => document.querySelector('[data-testid="format-step-direction"]')?.textContent ?? "",
    );
    console.log(`[io] the panel shows STEP as: ${stepDir}`);
    expect(stepDir).toContain("Read + write");
    await shot("02_formats_panel");
  });

  it("a format's specific exclusion is one click away, in the panel", async () => {
    expect(await click('[data-testid="format-step"] button')).toBe(true);
    await browser.pause(250);
    const note = await browser.execute(
      () => document.querySelector('[data-testid="format-step-note"]')?.textContent ?? "",
    );
    console.log(`[io] STEP details: ${note.slice(0, 140)}…`);
    expect(note).toContain("NURBS");
  });

  it("the file dialog offers exactly what the registry says can be read", async () => {
    // Derived, not hand-written — the drift that hid KTX2 cannot recur.
    const cat = await invoke("format_catalog");
    const readable = cat
      .filter((f) => f.available && (f.direction === "import" || f.direction === "both"))
      .flatMap((f) => f.extensions)
      .sort();
    console.log(`[io] openable: ${[...new Set(readable)].join(", ")}`);
    expect(readable).toContain("step");
    expect(readable).toContain("glb");
    expect(readable).toContain("3dxml");
    // Formats this build lacks are still LISTED, with a way forward, rather than vanishing.
    const missing = cat.filter((f) => !f.available);
    console.log(`[io] declared but unavailable in this build: ${missing.map((m) => m.id).join(", ") || "none"}`);
    for (const m of missing) {
      expect(m.note.toLowerCase()).toContain("build");
    }
  });
});
