import { browser, expect, $, $$ } from "@wdio/globals";
import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, "../../..");
const fixture = path.join(repo, "editor-shell/assets/dense_sphere.glb");
const evidenceDir = path.join(repo, "editor-shell/evidence/workbench-redesign");
const exported = path.join(evidenceDir, "asset-lab-dense-sphere.glb");
const compositedBakeEvidence = path.join(evidenceDir, "asset-lab-baked-composited.png");
const captureScript = path.join(repo, "editor-shell/e2e/scripts/capture-composited-window.ps1");
const sceneGlb = path.join(evidenceDir, "asset-lab-complete-scene.glb");
const sceneUsda = path.join(evidenceDir, "asset-lab-complete-scene.usda");
fs.mkdirSync(evidenceDir, { recursive: true });

function captureCompositedWindow(output) {
  fs.rmSync(output, { force: true });
  const result = spawnSync("powershell.exe", [
    "-NoProfile",
    "-ExecutionPolicy", "Bypass",
    "-File", captureScript,
    "-Out", output,
  ], { encoding: "utf8", timeout: 30_000 });
  if (result.status !== 0 || !fs.existsSync(output) || fs.statSync(output).size <= 1_024) {
    throw new Error(
      `OS-composited viewport capture failed (status ${String(result.status)}): `
      + `${result.stderr || result.stdout || "no capture output"}`,
    );
  }
}

const shown = async (selector) => {
  const element = await $(selector);
  return (await element.isExisting()) && (await element.isDisplayed());
};

async function waitConnected() {
  await browser.waitUntil(async () => {
    if (!(await shown("#viewport"))) return false;
    try {
      return (await invoke("camera_debug")).length === 6;
    } catch {
      return false;
    }
  }, { timeout: 30_000, timeoutMsg: "the packaged editor never connected" });
}

async function countEntities() {
  const opened = await exposeSceneWorkspace();
  // Use the semantic text node rather than WebDriver's rendered-text projection: the latter can return
  // an empty string for the dock during its selection/reflow frame even though the counter remains mounted.
  const text = await browser.execute(() => document.querySelector("#count")?.textContent ?? "");
  const count = Number(text.match(/(\d+)\s+entities/i)?.[1] ?? Number.NaN);
  await closeSceneWorkspace(opened);
  return count;
}

async function exposeSceneWorkspace() {
  await browser.waitUntil(async () => {
    if (await (await $('input[aria-label="Search scene objects"]')).isExisting()) return true;
    for (const selector of ['[data-testid="rail-left"]', '[data-testid="header-scene"]']) {
      if (await (await $(selector)).isExisting()) return true;
    }
    return false;
  }, { timeout: 20_000, timeoutMsg: "Scene workspace did not mount in desktop, rail, or drawer layout" });
  if (await (await $('input[aria-label="Search scene objects"]')).isExisting()) return false;
  for (const selector of ['[data-testid="rail-left"]', '[data-testid="header-scene"]']) {
    const trigger = await $(selector);
    if ((await trigger.isExisting()) && (await trigger.isDisplayed())) {
      await browser.execute((target) => document.querySelector(target)?.click(), selector);
      break;
    }
  }
  await browser.waitUntil(async () => await (await $('input[aria-label="Search scene objects"]')).isExisting(), {
    timeout: 10_000,
    timeoutMsg: "Scene drawer did not expose the hierarchy search",
  });
  return await (await $('[data-testid="drawer-left"]')).isExisting();
}

async function closeSceneWorkspace(opened) {
  if (!opened) return;
  const close = await $('button[aria-label="Close Scene workspace"]');
  if (await close.isExisting()) {
    await browser.execute(() => document.querySelector('button[aria-label="Close Scene workspace"]')?.click());
  }
  await browser.waitUntil(async () => !(await (await $('[data-testid="drawer-left"]')).isExisting()), {
    timeout: 10_000,
    timeoutMsg: "Scene drawer did not close before the next workspace action",
  });
}

async function selectRowNamed(fragment) {
  const opened = await exposeSceneWorkspace();
  const search = await $('input[aria-label="Search scene objects"]');
  await search.setValue(fragment);
  await browser.waitUntil(async () => {
    for (const row of await $$('[data-testid="hrow"]')) {
      if ((await row.getText()).includes(fragment)) {
        await row.click();
        await search.setValue("");
        return true;
      }
    }
    return false;
  }, { timeout: 20_000, timeoutMsg: `no hierarchy row contained ${fragment}` });
  await closeSceneWorkspace(opened);
}

async function findRowNamed(fragment) {
  const opened = await exposeSceneWorkspace();
  const search = await $('input[aria-label="Search scene objects"]');
  await search.setValue(fragment);
  let id = null;
  await browser.waitUntil(async () => {
    for (const row of await $$('[data-testid="hrow"]')) {
      if ((await row.getText()).includes(fragment)) {
        id = await row.getAttribute("data-id");
        return typeof id === "string" && id.length > 0;
      }
    }
    return false;
  }, { timeout: 20_000, timeoutMsg: `no hierarchy row contained ${fragment}` });
  await search.setValue("");
  await closeSceneWorkspace(opened);
  return id;
}

async function selectEntity(id) {
  const opened = await exposeSceneWorkspace();
  const search = await $('input[aria-label="Search scene objects"]');
  await search.setValue(id);
  await browser.waitUntil(async () => {
    for (const row of await $$('[data-testid="hrow"]')) {
      if ((await row.getAttribute("data-id")) === id) {
        await row.click();
        await search.setValue("");
        return true;
      }
    }
    return false;
  }, { timeout: 20_000, timeoutMsg: `no hierarchy row represented ${id}` });
  await closeSceneWorkspace(opened);
}

async function waitAudit() {
  await browser.waitUntil(async () => {
    const feedback = await $('[data-testid="asset-lab-operation"]');
    if (!(await feedback.isExisting())) return false;
    const cls = await feedback.getAttribute("class");
    return cls.includes("is-success") && (await shown('[data-testid="asset-lab-metrics"]'));
  }, { timeout: 60_000, timeoutMsg: "Asset Lab did not finish its measured audit" });
}

async function runVariant(button, expectedName) {
  const before = await countEntities();
  const action = await $(button);
  expect(await action.isEnabled()).toBe(true);
  await action.scrollIntoView({ block: "center", inline: "nearest" });
  await browser.waitUntil(async () => {
    if (!(await action.isDisplayed())) return false;
    return browser.execute((selector) => {
      const element = document.querySelector(selector);
      if (!(element instanceof HTMLElement)) return false;
      const rect = element.getBoundingClientRect();
      const hit = document.elementFromPoint(rect.left + rect.width / 2, rect.top + rect.height / 2);
      return hit === element || (hit instanceof Node && element.contains(hit));
    }, button);
  }, { timeout: 10_000, timeoutMsg: `${button} never became an unobstructed visible action` });
  const feedbackBefore = await $('[data-testid="asset-lab-operation"]').getText();
  await action.click();
  await browser.waitUntil(async () => {
    const feedback = await $('[data-testid="asset-lab-operation"]');
    if (!(await feedback.isExisting())) return false;
    return (await feedback.getAttribute("class")).includes("is-busy") || (await feedback.getText()) !== feedbackBefore;
  }, { timeout: 10_000, timeoutMsg: `${button} did not start after an unobstructed WebDriver click` });
  let feedbackText = "";
  let feedbackClass = "";
  try {
    await browser.waitUntil(async () => {
      const feedback = await $('[data-testid="asset-lab-operation"]');
      if (!(await feedback.isExisting())) return false;
      feedbackText = await feedback.getText();
      feedbackClass = await feedback.getAttribute("class");
      return feedbackClass.includes("is-error")
        || (feedbackClass.includes("is-success") && feedbackText.toLowerCase().includes("variant created"))
        || (await countEntities()) === before + 1;
    }, {
      timeout: 120_000,
      timeoutMsg: `${button} did not report a terminal variant result`,
    });
    if (feedbackClass.includes("is-error")) throw new Error(feedbackText);
    let after = Number.NaN;
    await browser.waitUntil(async () => {
      after = await countEntities();
      return after === before + 1;
    }, {
      timeout: 20_000,
      timeoutMsg: `scene count=${after}, expected ${before + 1}`,
    });
  } catch (cause) {
    const feedback = await $('[data-testid="asset-lab-operation"]');
    feedbackText = (await feedback.isExisting()) ? await feedback.getText() : "operation feedback missing";
    feedbackClass = (await feedback.isExisting()) ? await feedback.getAttribute("class") : "missing";
    const selected = await invoke("gizmo_selected");
    throw new Error(
      `${button} did not place exactly one source-preserving variant; selected=${String(selected)}, `
      + `feedback=${JSON.stringify(feedbackText)}, `
      + `class=${JSON.stringify(feedbackClass)}; ${cause instanceof Error ? cause.message : String(cause)}`,
    );
  }
  await browser.waitUntil(async () => {
    const feedback = await $('[data-testid="asset-lab-operation"]');
    return (await feedback.isExisting()) && (await feedback.getAttribute("class")).includes("is-success");
  }, { timeout: 120_000 });
  expect(await $('[data-testid="asset-lab-metrics"]').getText()).toContain("->");
  const createdId = await findRowNamed(expectedName);
  await browser.waitUntil(async () => (await invoke("gizmo_selected")) === createdId, {
    timeout: 10_000,
    timeoutMsg: "Asset Lab selected the derivative in the editor but not in the native viewport",
  });
  await selectEntity(createdId);
  await waitAudit();
}

describe("packaged editor / Asset Lab production workflow", () => {
  before(async () => {
    await browser.setTimeout({ script: 180_000 });
    await browser.setWindowSize(1440, 900);
    await waitConnected();
    for (const artifact of [exported, sceneGlb, sceneUsda]) fs.rmSync(artifact, { force: true });
  });

  it("imports, inspects, repairs, optimizes, prepares UVs/material, adds collision, validates and exports", async () => {
    console.log("[asset-lab] import source");
    const imported = await invoke("import_asset", { path: fixture });
    expect(typeof imported).toBe("string");
    await selectEntity(imported);

    await $("#bottom-workspaces-asset-tab").click();
    expect(await (await $('[data-testid="bottom-dock"]')).getAttribute("class")).toContain("is-open");
    await waitAudit();
    console.log("[asset-lab] inspect complete");
    expect(await shown('[data-testid="asset-lab"]')).toBe(true);
    expect(await $('[data-testid="asset-lab-metrics"]').getText()).toContain("Triangles");
    await browser.saveScreenshot(path.join(evidenceDir, "asset-lab-inspect.png"));

    await $("#asset-lab-stages-repair-tab").click();
    await runVariant('[data-testid="asset-lab-repair"]', "Repaired");
    console.log("[asset-lab] repair complete");

    await $("#asset-lab-stages-optimize-tab").click();
    await $('[aria-label="Optimization preset"]').selectByAttribute("value", "web");
    await runVariant('[data-testid="asset-lab-optimize"]', "Optimized");
    console.log("[asset-lab] optimize complete");

    await $("#asset-lab-stages-uv-tab").click();
    await $('[aria-label="UV method"]').selectByAttribute("value", "chart");
    await $('[aria-label="Atlas resolution"]').selectByAttribute("value", "512");
    await runVariant('[data-testid="asset-lab-uv"]', "Chart UV + MikkTSpace");
    console.log("[asset-lab] UV complete");

    await $("#asset-lab-stages-uv-tab").click();
    await $('[aria-label="Material finish"]').selectByAttribute("value", "brushed-metal");
    await runVariant('[data-testid="asset-lab-material"]', "Brushed Metal");
    console.log("[asset-lab] material complete");
    await browser.saveScreenshot(path.join(evidenceDir, "asset-lab-before-after.png"));

    await $("#asset-lab-stages-bake-tab").click();
    await $('[aria-label="Texture resolution"]').selectByAttribute("value", "512");
    const selfBake = await $('//fieldset[legend[contains(., "High-detail sources")]]//label[contains(., "selected target / self-bake")]//input[@type="checkbox"]');
    if (!(await selfBake.isSelected())) await selfBake.click();
    expect(await selfBake.isSelected()).toBe(true);
    await selfBake.click();
    expect(await selfBake.isSelected()).toBe(false);
    const originalSource = await $(`//fieldset[legend[contains(., "High-detail sources")]]//label[contains(., "${imported}")]//input[@type="checkbox"]`);
    if (await originalSource.isSelected()) await originalSource.click();
    expect(await originalSource.isSelected()).toBe(false);
    await originalSource.click();
    expect(await originalSource.isSelected()).toBe(true);
    const advancedBake = await $('//button[contains(., "Advanced projection controls")]');
    await advancedBake.click();
    expect(await advancedBake.getAttribute("aria-expanded")).toBe("true");
    await $('[aria-label="AO quality"]').selectByAttribute("value", "8");
    for (const label of ["Normal", "Ambient occlusion", "Signed curvature"]) {
      expect(await $(`//div[@role="group" and @aria-label="Bake maps"]//label[contains(., "${label}")]//input`).isSelected()).toBe(true);
    }
    await runVariant('[data-testid="asset-lab-bake"]', "Baked Maps");
    const bakeEvidence = await $('[data-testid="asset-lab-bake-evidence"]');
    expect(await bakeEvidence.isDisplayed()).toBe(true);
    const projectedTexels = Number(await bakeEvidence.getAttribute("data-projected-texels"));
    const coveredTexels = Number(await bakeEvidence.getAttribute("data-covered-texels"));
    const projectionHitRatio = Number(await bakeEvidence.getAttribute("data-projection-hit-ratio"));
    const requiredHitRatio = Number(await bakeEvidence.getAttribute("data-required-hit-ratio"));
    expect(Number.isFinite(projectedTexels) && projectedTexels > 0).toBe(true);
    expect(Number.isFinite(coveredTexels) && coveredTexels >= projectedTexels).toBe(true);
    expect(Number.isFinite(projectionHitRatio) && projectionHitRatio > 0).toBe(true);
    expect(projectionHitRatio).toBeGreaterThanOrEqual(requiredHitRatio);
    expect(await bakeEvidence.getAttribute("data-alignment-policy")).toBe("autoRelated");
    const autoAlignedSources = Number(await bakeEvidence.getAttribute("data-auto-aligned-sources"));
    const worldSpaceSources = Number(await bakeEvidence.getAttribute("data-world-space-sources"));
    expect(autoAlignedSources).toBeGreaterThan(0);
    console.log(
      `[asset-lab] bake evidence covered=${coveredTexels} projected=${projectedTexels} missed=${coveredTexels - projectedTexels} `
      + `hit=${projectionHitRatio} required=${requiredHitRatio} autoAligned=${autoAlignedSources} worldSpace=${worldSpaceSources}`,
    );
    const baked = await invoke("asset_lab_audit", { id: await invoke("gizmo_selected") });
    expect(baked.ok).toBe(true);
    expect(baked.audit.textures.count).toBe(3);
    expect(baked.audit.textures.referenced).toBe(3);
    expect(baked.audit.textures.descriptors.every((texture) => texture.width === 512 && texture.height === 512 && texture.layoutValid)).toBe(true);
    console.log("[asset-lab] normal/AO/curvature bake complete");
    await browser.saveScreenshot(path.join(evidenceDir, "asset-lab-baked-maps.png"));

    await $("#asset-lab-stages-validate-tab").click();
    await $('[data-testid="asset-lab-validate"]').click();
    await waitAudit();
    const selected = await invoke("gizmo_selected");
    expect(typeof selected).toBe("string");
    const collision = await $('[data-testid="asset-lab-collision"]');
    expect(await collision.isEnabled()).toBe(true);
    await collision.click();
    await browser.waitUntil(async () => {
      const details = await invoke("entity_details", { id: selected });
      return details.components.includes("Collider") && details.components.includes("RigidBody");
    }, { timeout: 30_000, timeoutMsg: "collision generation did not land on the selected variant" });
    console.log("[asset-lab] validation and collision complete");

    const beforeRoundTrip = await invoke("asset_lab_audit", { id: selected });
    expect(beforeRoundTrip.ok).toBe(true);
    expect(beforeRoundTrip.audit).not.toBeNull();

    await browser.execute((target) => {
      globalThis.__MTK_ASSET_LAB_EXPORT_PATH__ = target;
    }, exported);
    await $("#asset-lab-stages-export-tab").click();
    await $('[aria-label="Export scope"]').selectByAttribute("value", "asset");
    const exportButton = await $('[data-testid="asset-lab-export"]');
    expect(await exportButton.isEnabled()).toBe(true);
    await exportButton.click();
    await browser.waitUntil(() => fs.existsSync(exported) && fs.statSync(exported).size > 1_000, {
      timeout: 60_000,
      timeoutMsg: "the real Export button did not write a GLB",
    });
    expect(fs.readFileSync(exported).subarray(0, 4).toString("ascii")).toBe("glTF");
    expect((await $('[data-testid="asset-lab-operation"]').getText()).toLowerCase()).toContain("exported glb");
    await browser.saveScreenshot(path.join(evidenceDir, "asset-lab-exported.png"));
    console.log("[asset-lab] export complete");

    const entitiesBeforeRoundTrip = await countEntities();
    const roundTripEntity = await invoke("import_asset", { path: exported });
    expect(typeof roundTripEntity).toBe("string");
    await browser.waitUntil(async () => (await countEntities()) === entitiesBeforeRoundTrip + 1, {
      timeout: 60_000,
      timeoutMsg: "the exported GLB did not re-import as a new viewport entity",
    });
    const afterRoundTrip = await invoke("asset_lab_audit", { id: roundTripEntity });
    expect(afterRoundTrip.ok).toBe(true);
    expect(afterRoundTrip.audit).not.toBeNull();
    expect(afterRoundTrip.audit.vertices).toBe(beforeRoundTrip.audit.vertices);
    expect(afterRoundTrip.audit.triangles).toBe(beforeRoundTrip.audit.triangles);
    expect(afterRoundTrip.audit.materials.slots).toBe(beforeRoundTrip.audit.materials.slots);
    expect(afterRoundTrip.audit.textures.count).toBe(beforeRoundTrip.audit.textures.count);
    await selectEntity(roundTripEntity);
    await $("#asset-lab-stages-inspect-tab").click();
    await waitAudit();
    await browser.saveScreenshot(path.join(evidenceDir, "asset-lab-round-trip.png"));
    console.log("[asset-lab] GLB round trip complete");

    fs.rmSync(sceneGlb, { force: true });
    fs.rmSync(sceneUsda, { force: true });
    await browser.execute((target) => { globalThis.__MTK_SCENE_EXPORT_PATH__ = target; }, sceneGlb);
    await $("#asset-lab-stages-export-tab").click();
    await $('[aria-label="Export scope"]').selectByAttribute("value", "scene");
    await $('[aria-label="Export format"]').selectByAttribute("value", "glb");
    await $('[data-testid="asset-lab-export"]').click();
    await browser.waitUntil(() => fs.existsSync(sceneGlb) && fs.statSync(sceneGlb).size > 1_000, {
      timeout: 60_000,
      timeoutMsg: "the visible complete-scene GLB action did not write a nontrivial file",
    });
    expect((await $('[data-testid="asset-lab-operation"]').getText()).toLowerCase()).toContain("exported");
    // The visible workflow exposes completion and fidelity warnings; query the same native boundary only
    // for structured counts/preserved rows that are not flattened into the compact editor feedback.
    const completeGlb = await invoke("scene_export", { format: "glb", path: sceneGlb });
    expect(completeGlb.ok).toBe(true);
    expect(completeGlb.exportedPath).toBe(sceneGlb);
    expect(completeGlb.nodes).toBeGreaterThan(0);
    expect(completeGlb.meshes).toBeGreaterThan(0);
    expect(completeGlb.fidelity.length).toBeGreaterThan(0);
    expect(fs.statSync(sceneGlb).size).toBeGreaterThan(1_000);
    expect(fs.readFileSync(sceneGlb).subarray(0, 4).toString("ascii")).toBe("glTF");

    await browser.execute((target) => { globalThis.__MTK_SCENE_EXPORT_PATH__ = target; }, sceneUsda);
    await $('[aria-label="Export format"]').selectByAttribute("value", "usda");
    await $('[data-testid="asset-lab-export"]').click();
    await browser.waitUntil(() => fs.existsSync(sceneUsda) && fs.statSync(sceneUsda).size > 1_000, {
      timeout: 60_000,
      timeoutMsg: "the visible complete-scene USDA action did not write a nontrivial file",
    });
    expect((await $('[data-testid="asset-lab-operation"]').getText()).toLowerCase()).toContain("exported");
    const completeUsda = await invoke("scene_export", { format: "usda", path: sceneUsda });
    expect(completeUsda.ok).toBe(true);
    expect(completeUsda.exportedPath).toBe(sceneUsda);
    expect(completeUsda.nodes).toBe(completeGlb.nodes);
    expect(completeUsda.meshes).toBe(completeGlb.meshes);
    expect(completeUsda.fidelity.length).toBeGreaterThan(0);
    expect(fs.statSync(sceneUsda).size).toBeGreaterThan(1_000);
    expect(fs.readFileSync(sceneUsda, "utf8").startsWith("#usda 1.0")).toBe(true);
    console.log(
      `[asset-lab] complete-scene exports GLB=${fs.statSync(sceneGlb).size}B/${completeGlb.fidelity.length} rows `
      + `USDA=${fs.statSync(sceneUsda).size}B/${completeUsda.fidelity.length} rows`,
    );

    // The round-tripped selection is the same measured baked asset. Capture last so interchange and scene
    // export assertions always complete before the separate OS-compositing evidence boundary.
    await browser.setWindowSize(1280, 800);
    await browser.pause(750);
    captureCompositedWindow(compositedBakeEvidence);
    expect(fs.statSync(compositedBakeEvidence).size).toBeGreaterThan(1_024);
    await browser.setWindowSize(1440, 900);
  });
});
