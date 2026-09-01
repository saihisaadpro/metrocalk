import { browser, expect, $ } from "@wdio/globals";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const here = path.dirname(fileURLToPath(import.meta.url));
const evidenceDir = path.resolve(here, "../../evidence/workbench-redesign");
// Shared, checked-in helper used by the practical asset-production specs. Unlike WebDriver screenshots,
// this captures the final OS-composited window and therefore retains the native wgpu viewport pixels.
const compositedCapture = path.resolve(here, "../scripts/capture-composited-window.ps1");
fs.mkdirSync(evidenceDir, { recursive: true });

function captureCompositedWindow(name) {
  const output = path.join(evidenceDir, name);
  const result = execFileSync("powershell.exe", [
    "-ExecutionPolicy", "Bypass", "-File", compositedCapture,
    "-ProcName", "metrocalk-editor-shell", "-Out", output,
  ], { encoding: "utf8" });
  if (!fs.existsSync(output) || fs.statSync(output).size < 10_000) {
    throw new Error(`OS-composited Pipe Forge capture was not retained: ${result.trim()}`);
  }
  console.log(`[pipe-forge] composited native viewport capture ${output} (${fs.statSync(output).size}B)`);
}

const shown = async (selector) => {
  const element = await $(selector);
  return (await element.isExisting()) && (await element.isDisplayed());
};

const entityCount = async () => {
  const count = await $("#count");
  if (!(await count.isExisting())) return Number.NaN;
  // Read the semantic DOM token. WebDriver's rendered-text projection can briefly return an empty string
  // while the left dock reflows after a selected asset is replaced in place.
  const copy = await browser.execute(() => document.querySelector("#count")?.textContent ?? "");
  const match = copy.match(/(\d+)\s+entities/i);
  return match ? Number(match[1]) : Number.NaN;
};

async function setNumeric(selector, value) {
  const control = await $(selector);
  await control.scrollIntoView({ block: "center" });
  await control.click();
  await browser.keys(["Control", "a"]);
  await browser.keys(String(value));
  await browser.keys(["Enter"]);
}

// The three sections are `DisclosureSection`s, not raw `<details>`: a real button with `aria-expanded`
// and a `data-state` on the section, instead of a `<summary>` and an `open` attribute. The swap is what
// makes the panel visible to the screenshot gate at all — Chrome gives a closed `<details>` body
// `content-visibility: hidden`, so every descendant reports a collapsed rect and the geometry
// invariants report confident nonsense about controls that are not on screen (ADR-160).
async function openDetails(testId) {
  const section = await $(`[data-testid="${testId}"]`);
  if ((await section.getAttribute("data-state")) !== "open") {
    await section.$(".mtk-disclosure__toggle").click();
  }
  expect(await section.getAttribute("data-state")).toBe("open");
}

async function exposeSceneCount() {
  if (Number.isFinite(await entityCount())) return;
  for (const selector of ['[data-testid="rail-left"]', '[data-testid="header-scene"]']) {
    const trigger = await $(selector);
    if ((await trigger.isExisting()) && (await trigger.isDisplayed())) {
      await trigger.click();
      break;
    }
  }
  await browser.waitUntil(async () => Number.isFinite(await entityCount()), {
    timeout: 10000,
    timeoutMsg: `the connected editor did not expose its stable scene count at ${JSON.stringify(await browser.getWindowSize())}`,
  });
  // A narrow-layout drawer would cover the viewport. Wide layouts have no drawer and Escape is harmless.
  if (await shown('[data-testid="drawer-left"]')) await browser.keys(["Escape"]);
}

const waitConnected = async () => {
  await browser.waitUntil(
    async () => {
      const viewport = await $("#viewport");
      if (!(await viewport.isExisting())) return false;
      try {
        return (await invoke("camera_debug")).length === 6;
      } catch {
        return false;
      }
    },
    { timeout: 30000, timeoutMsg: "the packaged editor never connected to its engine" },
  );
};

async function clickViewportAt(xFraction, yFraction) {
  // Dispatch on the actual viewport surface with real client coordinates. This exercises the React
  // viewport event → Tauri pipe_forge_point → native camera ray/work-plane path, not a command-only seam.
  await browser.execute(
    (x, y) => {
      const viewport = document.querySelector("#viewport");
      if (!(viewport instanceof HTMLElement)) throw new Error("viewport is missing");
      const rect = viewport.getBoundingClientRect();
      viewport.dispatchEvent(new MouseEvent("click", {
        bubbles: true,
        cancelable: true,
        button: 0,
        clientX: rect.left + rect.width * x,
        clientY: rect.top + rect.height * y,
      }));
    },
    xFraction,
    yFraction,
  );
}

describe("packaged editor / Pipe Forge direct viewport asset", () => {
  before(async () => {
    await browser.setTimeout({ script: 180_000 });
    await browser.setWindowSize(1280, 800);
    await waitConnected();
    await exposeSceneCount();
    await invoke("pipe_forge_cancel");
  });

  it("draws a route in the viewport, bakes it, lands one selected editable asset, and reports production outputs", async () => {
    const before = await entityCount();
    const pipeTool = await $('[data-tool="pipe"]');
    expect(await pipeTool.isEnabled()).toBe(true);
    await pipeTool.click();
    await browser.waitUntil(() => shown('[data-testid="pipe-forge-setup"]'), {
      timeout: 10_000,
      timeoutMsg: "the code-split Pipe Forge workspace did not finish loading",
    });

    await $('[data-testid="pipe-forge-kit"]').selectByAttribute("value", "copper");
    await $('[data-testid="pipe-forge-quality"]').selectByAttribute("value", "production");
    const diameter = await $('[data-testid="pipe-forge-diameter"]');
    await diameter.click();
    await browser.keys(["Control", "a"]);
    await browser.keys("7.5");
    await browser.keys(["Enter"]);
    expect(await diameter.getValue()).toBe("7.5");
    const collars = await $('[data-testid="pipe-forge-auto-fittings"]');
    const collarsBefore = await collars.getAttribute("aria-pressed");
    await collars.click();
    expect(await collars.getAttribute("aria-pressed")).not.toBe(collarsBefore);
    await collars.click();
    expect(await collars.getAttribute("aria-pressed")).toBe(collarsBefore);
    await $('[data-testid="pipe-forge-start"]').click();
    await browser.waitUntil(async () => (await invoke("pipe_forge_status")).active === true, {
      timeout: 10000,
      timeoutMsg: "Pipe Forge did not enter drawing mode",
    });

    await clickViewportAt(0.42, 0.62);
    await browser.waitUntil(async () => (await invoke("pipe_forge_status")).points >= 1, {
      timeout: 10000,
      timeoutMsg: "the first viewport click did not place a pipe point",
    });
    await clickViewportAt(0.60, 0.62);
    await browser.waitUntil(async () => (await invoke("pipe_forge_status")).points >= 2, {
      timeout: 10000,
      timeoutMsg: "the second viewport click did not extend the pipe route",
    });
    await clickViewportAt(0.69, 0.54);
    await browser.waitUntil(async () => (await invoke("pipe_forge_status")).points >= 3, { timeout: 10000 });
    await $('[data-testid="pipe-forge-undo"]').click();
    await browser.waitUntil(async () => (await invoke("pipe_forge_status")).points === 2, {
      timeout: 10000,
      timeoutMsg: "Undo point did not remove exactly the latest route point",
    });

    const active = await invoke("pipe_forge_status");
    expect(active.canBake).toBe(true);
    expect(active.previewTriangles).toBeGreaterThan(0);
    const pointCopy = await $('[data-testid="pipe-forge-points"]').getText();
    expect(Number(pointCopy.match(/\d+/)?.[0] ?? Number.NaN)).toBeGreaterThanOrEqual(2);
    expect(await $('[data-testid="pipe-forge-bake"]').isEnabled()).toBe(true);
    await browser.saveScreenshot(path.join(evidenceDir, "pipe-route-ready.png"));

    await $('[data-testid="pipe-forge-bake"]').click();
    await browser.waitUntil(() => shown('[data-testid="pipe-forge-report"]'), {
      timeout: 60000,
      timeoutMsg: "the viewport-authored route never produced a bake report",
    });
    await browser.waitUntil(async () => (await entityCount()) === before + 1, {
      timeout: 20000,
      timeoutMsg: "the successful bake did not land exactly one scene entity",
    });
    await browser.waitUntil(async () => {
      const forge = await $('[data-testid="pipe-forge"]');
      return (await forge.getAttribute("data-active")) === "false";
    }, {
      timeout: 10000,
      timeoutMsg: "Pipe Forge did not enter its completed state after baking",
    });

    expect(await $('[data-testid="pipe-forge-report"]').getText()).toContain("Asset ready");
    const report = await browser.execute(() => document.querySelector('[data-testid="pipe-forge-report"]')?.textContent ?? "");
    expect(report).toContain("PBR");
    expect(report).toContain("LODs");
    expect(report).toContain("Watertight");
    expect(await $('[data-testid="pipe-forge-start"]').getText()).toContain("Draw another pipe");
    await browser.saveScreenshot(path.join(evidenceDir, "pipe-bake-report.png"));

    const selected = await invoke("gizmo_selected");
    expect(typeof selected).toBe("string");
    const details = await invoke("entity_details", { id: selected });
    expect(details.components).toContain("MeshRenderer");
    expect(details.components).toContain("PipeRecipe");
    expect(details.components).toContain("RigidBody");
    expect(details.components).toContain("Collider");
    expect((await invoke("pipe_forge_status")).active).toBe(false);

    // The non-destructive exit path is practical too: start a fresh draft, place one point, then cancel.
    await $('[data-testid="pipe-forge-start"]').click();
    await browser.waitUntil(async () => (await invoke("pipe_forge_status")).active === true, { timeout: 10000 });
    await clickViewportAt(0.50, 0.60);
    await browser.waitUntil(async () => (await invoke("pipe_forge_status")).points === 1, { timeout: 10000 });
    await $('[data-testid="pipe-forge-cancel"]').click();
    await browser.waitUntil(async () => (await invoke("pipe_forge_status")).active === false, { timeout: 10000 });
    expect(await entityCount()).toBe(before + 1);
  });

  it("authors a branched fitted network, rebakes a stable entity through visible handles, and restores it through history", async () => {
    const before = await entityCount();

    // Start from the ready surface left by the simple journey's non-destructive cancel. This is a real
    // visible action and proves a user can author another procedural asset without closing the workspace.
    await $('[data-testid="pipe-forge-start"]').click();
    await browser.waitUntil(async () => (await invoke("pipe_forge_status")).active === true, {
      timeout: 10_000,
      timeoutMsg: "Draw another pipe did not open a fresh native recipe",
    });

    for (const [x, y, expected] of [[0.39, 0.62, 1], [0.54, 0.62, 2], [0.68, 0.62, 3]]) {
      await clickViewportAt(x, y);
      await browser.waitUntil(async () => (await invoke("pipe_forge_status")).points === expected, {
        timeout: 10_000,
        timeoutMsg: `viewport point ${expected} did not become a primary route handle`,
      });
    }

    let status = await invoke("pipe_forge_status");
    const primaryNodeIds = new Set(status.handles.map((handle) => handle.nodeId));
    const junction = status.handles.find((handle) => handle.connectedEdges.length === 2);
    expect(junction).toBeTruthy();

    await openDetails("pipe-forge-network");
    await $('[data-testid="pipe-forge-handle"]').selectByAttribute("value", String(junction.nodeId));
    await setNumeric('[data-testid="pipe-forge-branch-diameter"]', "4.5");
    await $('[data-testid="pipe-forge-begin-branch"]').click();
    await browser.waitUntil(async () => (await invoke("pipe_forge_status")).branchFrom === junction.nodeId, {
      timeout: 10_000,
      timeoutMsg: "Draw branch did not anchor to the selected stable handle",
    });
    expect(await shown('[data-testid="pipe-forge-branch-mode"]')).toBe(true);

    const handlesBeforeBranch = status.handles.length;
    await clickViewportAt(0.54, 0.45);
    await browser.waitUntil(async () => {
      const current = await invoke("pipe_forge_status");
      return current.handles.length === handlesBeforeBranch + 1
        && current.fittings.some((fitting) => fitting.nodeId === junction.nodeId && fitting.kind === "tee" && fitting.automatic);
    }, {
      timeout: 10_000,
      timeoutMsg: "the viewport branch did not create one handle and its semantic automatic tee",
    });
    status = await invoke("pipe_forge_status");
    const branchTip = status.handles.find((handle) => !primaryNodeIds.has(handle.nodeId));
    expect(branchTip).toBeTruthy();
    await $('[data-testid="pipe-forge-end-branch"]').click();
    await browser.waitUntil(async () => (await invoke("pipe_forge_status")).branchFrom === null, {
      timeout: 10_000,
      timeoutMsg: "Finish branch did not leave branch drawing mode",
    });

    // Register a project fitting through the progressively disclosed catalog, then choose that exact style
    // in the visible fitting controls. Its asset handle remains authored even though the compiler deliberately
    // uses a bounded semantic proxy when the external catalog mesh is unavailable.
    await openDetails("pipe-forge-catalog");
    await $('[data-testid="pipe-forge-catalog-label"]').setValue("Packaged isolation valve");
    await $('[data-testid="pipe-forge-catalog-id"]').setValue("packaged-isolation-valve");
    await $('[data-testid="pipe-forge-catalog-kind"]').selectByAttribute("value", "valve");
    await $('[data-testid="pipe-forge-catalog-asset"]').setValue("mtkasset:packaged-isolation-valve");
    await setNumeric('[data-testid="pipe-forge-catalog-diameter-scale"]', "1.15");
    await setNumeric('[data-testid="pipe-forge-catalog-length-scale"]', "1.4");
    await $('[data-testid="pipe-forge-save-catalog"]').click();
    await browser.waitUntil(async () => (await invoke("pipe_forge_status")).fittingCatalog.some((entry) =>
      entry.id === "packaged-isolation-valve"
      && entry.kind === "valve"
      && entry.assetHandle === "mtkasset:packaged-isolation-valve"
      && Math.abs(entry.diameterScale - 1.15) < 0.001
      && Math.abs(entry.lengthScale - 1.4) < 0.001), {
      timeout: 10_000,
      timeoutMsg: "the visible project fitting form did not persist its bounded catalog metadata",
    });

    await openDetails("pipe-forge-network");
    await $('[data-testid="pipe-forge-handle"]').selectByAttribute("value", String(branchTip.nodeId));
    await openDetails("pipe-forge-fittings");
    await $('[data-testid="pipe-forge-fitting-kind"]').selectByAttribute("value", "valve");
    await browser.waitUntil(async () => (await $('[data-testid="pipe-forge-fitting-catalog"] option[value="packaged-isolation-valve"]')).isExisting(), {
      timeout: 5_000,
      timeoutMsg: "the saved valve did not become an immediately usable fitting style",
    });
    await $('[data-testid="pipe-forge-fitting-catalog"]').selectByAttribute("value", "packaged-isolation-valve");
    await $('[data-testid="pipe-forge-place-fitting"]').click();
    await browser.waitUntil(async () => (await invoke("pipe_forge_status")).fittings.some((fitting) =>
      fitting.nodeId === branchTip.nodeId
      && fitting.kind === "valve"
      && fitting.catalogId === "packaged-isolation-valve"
      && !fitting.automatic), {
      timeout: 10_000,
      timeoutMsg: "the custom valve was not attached to the selected branch handle",
    });

    status = await invoke("pipe_forge_status");
    const flangeHandle = status.handles.find((handle) => handle.connectedEdges.length === 1 && handle.nodeId !== branchTip.nodeId);
    expect(flangeHandle).toBeTruthy();
    await $('[data-testid="pipe-forge-handle"]').selectByAttribute("value", String(flangeHandle.nodeId));
    await $('[data-testid="pipe-forge-fitting-kind"]').selectByAttribute("value", "flange");
    await browser.waitUntil(async () => (await $('[data-testid="pipe-forge-fitting-catalog"]').getValue()) === "", {
      timeout: 5_000,
      timeoutMsg: "switching to the built-in flange did not clear the incompatible valve catalog style",
    });
    await $('[data-testid="pipe-forge-place-fitting"]').click();
    await browser.waitUntil(async () => (await invoke("pipe_forge_status")).fittings.some((fitting) =>
      fitting.nodeId === flangeHandle.nodeId && fitting.kind === "flange" && !fitting.automatic), {
      timeout: 10_000,
      timeoutMsg: "the explicit built-in flange was not attached to the selected route end",
    });

    const fitted = await invoke("pipe_forge_status");
    expect(fitted.edges.length).toBe(3);
    expect(fitted.handles.length).toBe(4);
    expect(fitted.fittings.some((fitting) => fitting.kind === "tee" && fitting.automatic)).toBe(true);
    expect(fitted.fittings.some((fitting) => fitting.kind === "valve" && fitting.catalogId === "packaged-isolation-valve")).toBe(true);
    expect(fitted.fittings.some((fitting) => fitting.kind === "flange" && !fitting.automatic)).toBe(true);
    const fittingCopy = await browser.execute(() => document.querySelector('[aria-label="Placed fittings"]')?.textContent ?? "");
    expect(fittingCopy).toContain("Tee");
    expect(fittingCopy).toContain("automatic");
    expect(fittingCopy).toContain("Valve");
    expect(fittingCopy).toContain("Flange");
    await browser.saveScreenshot(path.join(evidenceDir, "pipe-branched-network.png"));

    await $('[data-testid="pipe-forge-bake"]').click();
    await browser.waitUntil(() => shown('[data-testid="pipe-forge-report"]'), {
      timeout: 60_000,
      timeoutMsg: "the branched and fitted network did not produce a cooked asset",
    });
    await browser.waitUntil(async () => (await entityCount()) === before + 1, {
      timeout: 20_000,
      timeoutMsg: "the fitted network did not land exactly one scene entity",
    });
    const pipeEntity = await invoke("gizmo_selected");
    expect(typeof pipeEntity).toBe("string");
    const originalAudit = await invoke("asset_lab_audit", { id: pipeEntity });
    expect(originalAudit.ok).toBe(true);
    expect(typeof originalAudit.sourceHandle).toBe("string");
    expect(originalAudit.sourceHandle).toContain("mtkasset:");
    expect((await browser.execute(() => document.querySelector('[data-testid="pipe-forge-report"]')?.textContent ?? "")).toLowerCase())
      .toContain("proxy");
    await browser.saveScreenshot(path.join(evidenceDir, "pipe-branched-bake-report.png"));
    if (!(await shown("#vpFrameSel"))) await $("#vpView").click();
    await browser.waitUntil(async () => {
      const frameSelected = await $("#vpFrameSel");
      return (await frameSelected.isExisting())
        && (await frameSelected.isDisplayed())
        && (await frameSelected.getAttribute("aria-disabled")) !== "true";
    }, {
      timeout: 5_000,
      timeoutMsg: "the selected fitted asset never enabled the visible Frame selected control",
    });
    await $("#vpFrameSel").click();
    await browser.pause(500);
    captureCompositedWindow("pipe-branched-baked-native.png");

    // Exercise the safety refusal through the shipping Properties input and Edit button: a non-rigid entity
    // cannot expose misleading local handles. Resetting scale through the same control then re-enables editing.
    await $("#inspector-workspaces-properties-tab").click();
    await browser.waitUntil(() => shown('[data-testid="num-Transform.scale"]'), {
      timeout: 10_000,
      timeoutMsg: "the selected pipe's scale control was not available in Properties",
    });
    await setNumeric('[data-testid="num-Transform.scale"]', "1.25");
    await browser.waitUntil(async () => Math.abs((await invoke("read_transform", { id: pipeEntity }))[7] - 1.25) < 0.001, {
      timeout: 10_000,
      timeoutMsg: "the visible scale edit did not reach the selected pipe",
    });
    await $('[data-testid="pipe-forge-edit"]').click();
    await browser.waitUntil(async () => {
      const current = await invoke("pipe_forge_status");
      const visibleMessage = await browser.execute(() =>
        document.querySelector('[data-testid="pipe-forge-message"]')?.textContent ?? "",
      );
      return !current.active && visibleMessage.includes("unscaled, unrotated");
    }, {
      timeout: 10_000,
      timeoutMsg: "a scaled pipe was not safely refused by post-bake route editing",
    });
    expect(await browser.execute(() => document.querySelector('[data-testid="pipe-forge-message"]')?.textContent ?? ""))
      .toContain("unscaled, unrotated");

    await setNumeric('[data-testid="num-Transform.scale"]', "1");
    await browser.waitUntil(async () => Math.abs((await invoke("read_transform", { id: pipeEntity }))[7] - 1) < 0.001, {
      timeout: 10_000,
      timeoutMsg: "the visible scale reset did not restore an editable transform",
    });
    await $('[data-testid="pipe-forge-edit"]').click();
    await browser.waitUntil(async () => {
      const current = await invoke("pipe_forge_status");
      return current.active && current.editingEntity === pipeEntity;
    }, {
      timeout: 10_000,
      timeoutMsg: "Edit selected pipe did not restore the baked graph into route-handle mode",
    });

    status = await invoke("pipe_forge_status");
    const restoredTip = status.handles.find((handle) => handle.nodeId === branchTip.nodeId);
    expect(restoredTip).toBeTruthy();
    const editedY = Number((restoredTip.position[1] + 0.75).toFixed(3));
    await openDetails("pipe-forge-network");
    await $('[data-testid="pipe-forge-handle"]').selectByAttribute("value", String(branchTip.nodeId));
    await setNumeric('[data-testid="pipe-forge-handle-position-y"]', String(editedY));
    await $('[data-testid="pipe-forge-move-handle"]').click();
    await browser.waitUntil(async () => {
      const current = await invoke("pipe_forge_status");
      const tip = current.handles.find((handle) => handle.nodeId === branchTip.nodeId);
      return tip && Math.abs(tip.position[1] - editedY) < 0.001 && current.message.includes("Route handle moved");
    }, {
      timeout: 10_000,
      timeoutMsg: "Apply position did not move the restored stable branch handle",
    });
    await browser.saveScreenshot(path.join(evidenceDir, "pipe-post-bake-handle-edit.png"));

    const countBeforeRebake = await entityCount();
    await $('[data-testid="pipe-forge-bake"]').click();
    await browser.waitUntil(async () => {
      const current = await invoke("asset_lab_audit", { id: pipeEntity });
      return current.ok && current.sourceHandle !== originalAudit.sourceHandle;
    }, {
      timeout: 60_000,
      timeoutMsg: "Rebake asset did not replace the cooked mesh payload",
    });
    const editedAudit = await invoke("asset_lab_audit", { id: pipeEntity });
    expect(await entityCount()).toBe(countBeforeRebake);
    expect(await invoke("gizmo_selected")).toBe(pipeEntity);
    expect(editedAudit.sourceHandle).not.toBe(originalAudit.sourceHandle);
    expect(await browser.execute(() => document.querySelector('[data-testid="pipe-forge-report"]')?.textContent ?? ""))
      .toContain("Asset ready");

    // The replacement is one document transaction. Undo and redo through the editor header must swap the
    // complete cooked/source pair while retaining entity identity and scene count.
    await $('[data-testid="header-undo"]').click();
    await browser.waitUntil(async () => (await invoke("asset_lab_audit", { id: pipeEntity })).sourceHandle === originalAudit.sourceHandle, {
      timeout: 10_000,
      timeoutMsg: "Undo did not atomically restore the pre-edit pipe payload",
    });
    expect(await entityCount()).toBe(countBeforeRebake);
    await $('[data-testid="header-redo"]').click();
    await browser.waitUntil(async () => (await invoke("asset_lab_audit", { id: pipeEntity })).sourceHandle === editedAudit.sourceHandle, {
      timeout: 10_000,
      timeoutMsg: "Redo did not restore the edited pipe payload",
    });
    expect(await entityCount()).toBe(countBeforeRebake);

    // Reopen once more through the visible edit action. This proves the redone document still carries the
    // graph, catalog reference, semantic fittings and moved stable handle, not only a rendered mesh slot.
    await $('[data-testid="pipe-forge-edit"]').click();
    await browser.waitUntil(async () => (await invoke("pipe_forge_status")).editingEntity === pipeEntity, {
      timeout: 10_000,
      timeoutMsg: "the redone pipe no longer exposed its editable recipe",
    });
    const persisted = await invoke("pipe_forge_status");
    expect(persisted.handles.length).toBe(4);
    expect(persisted.edges.length).toBe(3);
    expect(persisted.fittingCatalog.some((entry) => entry.id === "packaged-isolation-valve")).toBe(true);
    expect(persisted.fittings.some((fitting) => fitting.kind === "tee" && fitting.automatic)).toBe(true);
    expect(persisted.fittings.some((fitting) => fitting.kind === "valve" && fitting.catalogId === "packaged-isolation-valve")).toBe(true);
    expect(persisted.fittings.some((fitting) => fitting.kind === "flange")).toBe(true);
    expect(Math.abs(persisted.handles.find((handle) => handle.nodeId === branchTip.nodeId).position[1] - editedY)).toBeLessThan(0.001);
    await $('[data-testid="pipe-forge-cancel"]').click();
    await browser.waitUntil(async () => (await invoke("pipe_forge_status")).active === false, { timeout: 10_000 });
    expect(await entityCount()).toBe(countBeforeRebake);
  });
});
