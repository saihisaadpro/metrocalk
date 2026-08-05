// LIVE proof that an imported CAD assembly PRESERVES its original hierarchy / names / grouping / structure
// (the user's ask). Imports a small nested 3DXML (Skid Weld Line A › Robot Cell 1|2 › named parts) into the
// packaged .exe, then asserts the outliner shows the exact NAMED TREE — group folders + nested, named parts,
// not a flat pile of hex ids — and OS-captures the composited window for a visual record.

import { browser } from "@wdio/globals";
import { execSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const dir = path.dirname(fileURLToPath(import.meta.url));
const SHOT_DIR = process.env.MTK_SHOT_DIR || path.resolve(dir, "../.shots-cadtree");
const SHOT_PS1 = path.resolve(dir, "../../../.uxtest/audit/exe/capture-window-fg.ps1");
const PROC = "metrocalk-editor-shell";
// The nested fixture: Skid Weld Line A › (Robot Cell 1 › Weld Gun, Part Fixture) + (Robot Cell 2 › Weld Gun, Conveyor Segment).
const FIXTURE =
  process.env.MTK_CAD_FIXTURE ||
  "C:\\Users\\saihi\\AppData\\Local\\Temp\\claude\\x--Dev-Research---Projects-Metrocalk\\7aaa1487-a438-4be3-81f1-3b40cf62f867\\scratchpad\\NestedAssembly.3dxml";
mkdirSync(SHOT_DIR, { recursive: true });

const shot = async (label) => {
  await browser.pause(600);
  const out = path.join(SHOT_DIR, `${label}.png`);
  try {
    execSync(`powershell -ExecutionPolicy Bypass -File "${SHOT_PS1}" -ProcName "${PROC}" -Out "${out}"`, { stdio: "ignore" });
    console.log("  shot", out);
  } catch (e) {
    console.error("shot failed", label, String(e));
  }
};

describe("Imported CAD preserves its source hierarchy (named tree, grouping, structure)", () => {
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

  it("imports a nested 3DXML and shows the exact named assembly TREE (folders + nested parts), not a flat pile", async () => {
    await shot("00_before_import");

    // Land the CAD import via the real backend command (the same path the File menu / drag-drop use).
    const key = await browser.execute(async (p) => {
      try {
        return await window.__TAURI__.core.invoke("import_asset", { path: p });
      } catch (e) {
        return { error: String(e) };
      }
    }, FIXTURE);
    console.log("import_asset →", JSON.stringify(key));

    // Wait for the scene to populate (the tree lands as one commit → the hierarchy rows appear).
    await browser.waitUntil(
      async () => (await browser.execute(() => document.querySelectorAll('[data-testid="hrow"]').length)) >= 7,
      { timeout: 60000, timeoutMsg: "the imported hierarchy rows never appeared" },
    );
    await browser.pause(400);

    // Read the outliner rows: id, kind, name, and nesting DEPTH (encoded in the row's left padding: 8 + depth*12).
    const report = await browser.execute(() => {
      const rows = [...document.querySelectorAll('[data-testid="hrow"]')].map((r) => {
        const padLeft = parseFloat(getComputedStyle(r).paddingLeft) || 0;
        // The row's Thumbnail span carries `title = (named ? name : id)` — the reliable name source (the
        // visible name span sits next to it; the leading "▣"/"◆" is just the type-icon glyph, not the name).
        const thumb = r.querySelector('[data-testid="thumb"]');
        const name = (thumb?.getAttribute("title") || thumb?.nextElementSibling?.textContent || "").trim();
        return {
          id: r.getAttribute("data-id"),
          kind: r.getAttribute("data-kind"),
          name,
          depth: Math.round((padLeft - 8) / 12),
        };
      });
      const countEl = document.getElementById("count");
      return { rows, count: countEl ? countEl.textContent : "" };
    });
    console.log("hierarchy report:", JSON.stringify(report, null, 2));

    await shot("01_hierarchy_tree");

    const { rows } = report;
    const named = (n) => rows.find((r) => r.name === n);

    // (1) The source's GROUPING survives as named folder containers — each assembly is present AND classified
    //     as a group folder (kind="group" → the reserved is_group), not flattened away.
    for (const g of ["Skid Weld Line A", "Robot Cell 1", "Robot Cell 2"]) {
      const row = named(g);
      if (!row) throw new Error(`group "${g}" is missing from the tree`);
      if (row.kind !== "group") throw new Error(`"${g}" should be a group folder, got kind=${row.kind}`);
    }

    // (2) The source part NAMES survive (no hex-id pile) — each authored part is present, named, NOT a group.
    for (const nm of ["Weld Gun", "Part Fixture", "Conveyor Segment"]) {
      const row = named(nm);
      if (!row) throw new Error(`part name "${nm}" is missing from the tree`);
      if (row.kind === "group") throw new Error(`part "${nm}" was misclassified as a group`);
    }

    // (3) The source's NESTING / STRUCTURE survives — root at depth 0, cells nested at depth 1, parts at depth 2.
    if (named("Skid Weld Line A").depth !== 0) throw new Error(`root should be depth 0, got ${named("Skid Weld Line A").depth}`);
    for (const cell of ["Robot Cell 1", "Robot Cell 2"]) {
      if (named(cell).depth !== 1) throw new Error(`${cell} should nest at depth 1 (under the root), got ${named(cell).depth}`);
    }
    for (const nm of ["Weld Gun", "Part Fixture", "Conveyor Segment"]) {
      if (named(nm).depth !== 2) throw new Error(`part "${nm}" should nest at depth 2 (cell › part), got ${named(nm).depth}`);
    }

    // (4) The rows READ as a contiguous tree (PRE-ORDER), not scrambled by storage order — filtering out the
    //     empty-scene seed entities (named "1_N"), the CAD rows must be: root(0), then each cell(1) IMMEDIATELY
    //     followed by its two parts(2) — depth sequence [0,1,2,2,1,2,2]. A scrambled order fails this.
    const cadDepths = rows.filter((r) => !/^1_\d+$/.test(r.name)).map((r) => r.depth);
    const expectSeq = [0, 1, 2, 2, 1, 2, 2];
    if (JSON.stringify(cadDepths) !== JSON.stringify(expectSeq)) {
      throw new Error(`the tree is not contiguous pre-order — expected depth sequence ${JSON.stringify(expectSeq)} (root › cell › its parts › cell › its parts), got ${JSON.stringify(cadDepths)}`);
    }

    console.log("✓ imported CAD preserved its hierarchy: named group folders (Skid Weld Line A › Robot Cell 1|2) with named parts nested exactly as the source, contiguous pre-order (root › cell › part).");
  });
});
