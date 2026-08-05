// LIVE provenance + near-duplicate verification (M11.5 / ADR-044). Drives the packaged .exe over the invoke
// bridge: import textured assets, read `asset_provenance` back, and assert (1) the import path RECORDS a
// provenance record (kind=imported, file-name source, content-address hash, perceptual-hash fingerprint),
// and (2) a perceptual near-duplicate — the SAME ripple texture in a different-geometry glb (distinct bytes,
// so the store's EXACT dedup keeps it as a separate asset) — is HINTED via `nearDuplicateOf`. Writes
// results.json for the record. Backend wiring check (no pixels): there is no provenance UI yet (React lane).

import { browser } from "@wdio/globals";
import { writeFileSync, mkdirSync } from "node:fs";
import path from "node:path";

const ASSET_DIR = process.env.MTK_ASSET_DIR;
const OUT_DIR = process.env.MTK_OUT_DIR;
mkdirSync(OUT_DIR, { recursive: true });

const invoke = (cmd, args = {}) =>
  browser.execute(async (c, a) => window.__TAURI__.core.invoke(c, a), cmd, args);

const countEntities = async () => {
  try {
    const el = await browser.$("#count");
    const m = (await el.getText()).match(/(\d+)\s+entities/);
    return m ? Number(m[1]) : NaN;
  } catch {
    return NaN;
  }
};

const importFile = async (file) => {
  const before = await countEntities();
  const id = await invoke("import_asset", { path: path.join(ASSET_DIR, file) });
  if (typeof id !== "string") throw new Error(`import_asset(${file}) returned ${JSON.stringify(id)}`);
  await browser.waitUntil(async () => (await countEntities()) > before, {
    timeout: 15000,
    timeoutMsg: `${file} did not place into the scene`,
  });
  return id;
};

const results = [];

describe("LIVE provenance + near-duplicate (M11.5)", () => {
  before(async () => {
    await browser.waitUntil(async () => Number.isFinite(await countEntities()), {
      timeout: 30000,
      timeoutMsg: "editor never connected (#count empty)",
    });
    await browser.execute(() => {
      try {
        localStorage.setItem("mtk.onboarded.v1", "1");
      } catch (e) {
        void e;
      }
    });
    await browser.pause(300);
  });

  it("records provenance on import + hints a perceptual near-duplicate", async () => {
    // 1) A structured-texture asset → a real provenance record with a non-zero perceptual fingerprint.
    const idA = await importFile("ripple_quad.glb");
    const provA = await invoke("asset_provenance", { id: idA });
    results.push({ step: "import ripple_quad.glb", id: idA, provenance: provA });
    console.log("provA =", JSON.stringify(provA));
    if (!provA) throw new Error("asset_provenance returned null for the first import");
    if (provA.kind !== "imported") throw new Error(`kind expected 'imported', got '${provA.kind}'`);
    if (provA.source !== "ripple_quad.glb") throw new Error(`source expected file name, got '${provA.source}'`);
    if (!provA.contentHash || provA.contentHash.length < 8) throw new Error(`bad contentHash '${provA.contentHash}'`);
    if (provA.aiGenerated !== false) throw new Error("an imported asset must not be flagged AI-generated");
    if (!provA.perceptualHash || provA.perceptualHash === "0")
      throw new Error(`a structured texture must fingerprint non-zero, got '${provA.perceptualHash}'`);
    if (provA.nearDuplicateOf) throw new Error(`first import should have no near-dup, got '${provA.nearDuplicateOf}'`);

    // 2) The stretched copy: SAME ripple texture, different geometry → distinct content hash (no exact
    //    dedup) but a perceptual match → the near-duplicate hint must name the first asset's source.
    const idB = await importFile("ripple_quad_wide.glb");
    const provB = await invoke("asset_provenance", { id: idB });
    results.push({ step: "import ripple_quad_wide.glb", id: idB, provenance: provB });
    console.log("provB =", JSON.stringify(provB));
    if (!provB) throw new Error("asset_provenance returned null for the second import");
    if (provB.contentHash === provA.contentHash) throw new Error("exact dedup wrongly collapsed two distinct files");
    if (provB.perceptualHash !== provA.perceptualHash)
      throw new Error(`shared texture should hash identically: '${provA.perceptualHash}' vs '${provB.perceptualHash}'`);
    if (provB.nearDuplicateOf !== "ripple_quad.glb")
      throw new Error(`near-dup hint expected 'ripple_quad.glb', got '${provB.nearDuplicateOf}'`);

    console.log("PASS: provenance recorded on import + near-duplicate hinted across distinct bytes.");
  });

  after(async () => {
    writeFileSync(path.join(OUT_DIR, "results.json"), JSON.stringify(results, null, 2));
    console.log(`\nWROTE ${results.length} provenance records → ${path.join(OUT_DIR, "results.json")}`);
  });
});
