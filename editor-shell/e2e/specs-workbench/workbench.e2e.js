import { browser, expect, $ } from "@wdio/globals";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const evidenceDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../evidence/workbench-redesign");
fs.mkdirSync(evidenceDir, { recursive: true });

const shown = async (selector) => {
  const element = await $(selector);
  return (await element.isExisting()) && (await element.isDisplayed());
};

const revealPopupControl = async (triggerSelector, controlSelector) => {
  if (!(await shown(controlSelector))) await $(triggerSelector).click();
  await browser.waitUntil(() => shown(controlSelector), {
    timeout: 5000,
    timeoutMsg: `${controlSelector} was not revealed by ${triggerSelector}`,
  });
  return $(controlSelector);
};

const controlsAreUnobstructed = (selectors) => browser.execute((targets) =>
  targets.every((selector) => {
    const control = document.querySelector(selector);
    if (!(control instanceof HTMLElement)) return false;
    const rect = control.getBoundingClientRect();
    const hit = document.elementFromPoint(rect.left + rect.width / 2, rect.top + rect.height / 2);
    return hit === control || (hit instanceof Node && control.contains(hit));
  }), selectors);

const entityCount = async () => {
  const count = await $("#count");
  if (!(await count.isExisting())) return Number.NaN;
  const match = (await count.getText()).match(/(\d+)\s+entities/i);
  return match ? Number(match[1]) : Number.NaN;
};

const waitConnected = async () => {
  try {
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
  } catch (error) {
    const diagnostic = await browser.execute(() => ({
      url: location.href,
      title: document.title,
      body: document.body?.innerText?.slice(0, 800) ?? "",
      rootChildren: document.querySelector("#root")?.childElementCount ?? -1,
    }));
    throw new Error(`${error.message}; page=${JSON.stringify(diagnostic)}`);
  }
};

describe("packaged editor / redesigned workbench", () => {
  before(async () => {
    await browser.setTimeout({ script: 180_000 });
    await browser.setWindowSize(1280, 800);
    await waitConnected();
  });

  after(async () => {
    await browser.setWindowSize(1280, 800);
  });

  it("keeps primary capabilities discoverable in predictable scene, inspector, and task workspaces", async () => {
    expect(await shown('[data-testid="editor-header"]')).toBe(true);
    expect(await shown('[data-testid="left-dock"]')).toBe(true);
    expect(await shown('[data-testid="inspector-dock"]')).toBe(true);

    const sceneTab = await $("#left-workspaces-scene-tab");
    const createTab = await $("#left-workspaces-create-tab");
    expect(await sceneTab.getAttribute("aria-selected")).toBe("true");
    await createTab.click();
    expect(await createTab.getAttribute("aria-selected")).toBe("true");
    expect(await shown('[data-testid="create-pipe"]')).toBe(true);
    expect(await shown('[data-testid="create-import"]')).toBe(true);
    expect(await shown("#assetbrowser")).toBe(true);

    const relationsTab = await $("#inspector-workspaces-relations-tab");
    await relationsTab.click();
    expect(await relationsTab.getAttribute("aria-selected")).toBe("true");
    expect(await shown("#inspector-workspaces-relations-panel")).toBe(true);
    expect((await $("#inspector-workspaces-relations-panel").getText()).toLowerCase()).toContain("select an object");

    await (await revealPopupControl('[data-testid="header-workspaces"]', '[data-testid="header-logic"]')).click();
    expect(await (await $('[data-testid="bottom-dock"]')).getAttribute("class")).toContain("is-open");
    expect(await (await $("#bottom-workspaces-logic-tab")).getAttribute("aria-selected")).toBe("true");
    expect(await shown("#logic-workspaces")).toBe(true);
    await browser.saveScreenshot(path.join(evidenceDir, "workbench-wide.png"));
  });

  it("opens the command palette from the keyboard, focuses search, and runs the selected real action", async () => {
    await browser.keys(["Control", "k"]);
    await browser.waitUntil(() => shown('[data-testid="command-palette"]'), {
      timeout: 5000,
      timeoutMsg: "Ctrl+K did not open the command palette",
    });

    const search = await $('input[role="combobox"][aria-label="Search commands"]');
    expect(await search.isFocused()).toBe(true);
    await search.setValue("Open Physics");
    expect(await shown('button[data-command-id="workspace-physics"]')).toBe(true);
    await browser.saveScreenshot(path.join(evidenceDir, "command-palette.png"));
    await browser.keys(["Enter"]);

    await browser.waitUntil(async () => !(await shown('[data-testid="command-palette"]')), {
      timeout: 5000,
      timeoutMsg: "running a palette command did not close the palette",
    });
    expect(await (await $("#inspector-workspaces-physics-tab")).getAttribute("aria-selected")).toBe("true");
    await browser.waitUntil(() => shown('[data-testid="dropBall"]'), {
      timeout: 10_000,
      timeoutMsg: "the code-split Physics workspace did not finish loading",
    });
  });

  it("wires the high-frequency authoring, viewport, history, review, and Play controls to live engine state", async () => {
    await $("#left-workspaces-scene-tab").click();
    const before = await entityCount();
    const create = await revealPopupControl("#authAdd", "#authCreate");
    await browser.waitUntil(async () => (await create.getAttribute("aria-disabled")) !== "true", {
      timeout: 10000,
      timeoutMsg: "Create Entity was not reachable in the Scene workspace",
    });
    const ipcBeforeCreate = await invoke("ipc_count");
    const createHitTarget = await browser.execute(() => {
      const element = document.querySelector("#authCreate");
      if (!(element instanceof HTMLElement)) return null;
      const rect = element.getBoundingClientRect();
      const hit = document.elementFromPoint(rect.left + rect.width / 2, rect.top + rect.height / 2);
      return {
        elementTag: element.tagName,
        hitId: hit instanceof HTMLElement ? hit.id : null,
        hitTag: hit instanceof HTMLElement ? hit.tagName : null,
        pointerEvents: getComputedStyle(element).pointerEvents,
      };
    });
    // A genuine WebDriver activation crosses the same accessibility/button path as a user click and reaches
    // React's delegated handler in the page world (execute-script DOM clicks may run in an isolated world).
    await create.click();
    await browser.pause(750);
    const immediateCreateState = await browser.execute(() => ({
      focused: document.activeElement?.id ?? null,
      status: document.querySelector("#status")?.textContent ?? null,
    }));
    const ipcAfterCreate = await invoke("ipc_count");
    try {
      await browser.waitUntil(async () => (await entityCount()) === before + 1, {
        timeout: 15000,
        timeoutMsg: "Create Entity did not update the live scene",
      });
    } catch (error) {
      const diagnostic = await browser.execute(() => ({
        count: document.querySelector("#count")?.textContent ?? null,
        status: document.querySelector("#status")?.textContent ?? null,
        createAriaDisabled: document.querySelector("#authCreate")?.getAttribute("aria-disabled") ?? null,
      }));
      throw new Error(
        `${error.message}; before=${before}; ipcDelta=${ipcAfterCreate - ipcBeforeCreate}; ` +
          `hit=${JSON.stringify(createHitTarget)}; immediate=${JSON.stringify(immediateCreateState)}; ` +
          `state=${JSON.stringify(diagnostic)}`,
      );
    }
    expect(typeof (await invoke("gizmo_selected"))).toBe("string");

    expect(await controlsAreUnobstructed(["#vpMove", "#vpRotate", "#vpScale"])).toBe(true);

    for (const [selector, mode] of [["#vpMove", "translate"], ["#vpRotate", "rotate"], ["#vpScale", "scale"]]) {
      await $(selector).click();
      await browser.waitUntil(async () => (await invoke("gizmo_debug"))[0] === mode, {
        timeout: 5000,
        timeoutMsg: `${selector} did not update the native gizmo`,
      });
    }

    const spaceBefore = (await invoke("gizmo_debug"))[3];
    const spaceControl = await revealPopupControl("#vpTransform", "#vpSpace");
    expect(await controlsAreUnobstructed(["#vpSpace", "#vpPivot", "#vpSnap"])).toBe(true);
    await spaceControl.click();
    await browser.waitUntil(async () => (await invoke("gizmo_debug"))[3] !== spaceBefore, { timeout: 5000 });
    await (await revealPopupControl("#vpView", "#vpFrameAll")).click();
    for (const selector of ["#vpTop", "#vpFront", "#vpSide", "#vpPersp"]) {
      await (await revealPopupControl("#vpView", selector)).click();
    }
    await (await revealPopupControl("#vpTransform", "#vpSnap")).click();

    await (await revealPopupControl('[data-testid="header-workspaces"]', '[data-testid="header-review"]')).click();
    expect(await (await $("#bottom-workspaces-import-tab")).getAttribute("aria-selected")).toBe("true");
    expect(await shown("#bottom-workspaces-import-panel")).toBe(true);

    await $('[data-testid="header-undo"]').click();
    await browser.waitUntil(async () => (await entityCount()) === before, { timeout: 10000 });
    await $('[data-testid="header-redo"]').click();
    await browser.waitUntil(async () => (await entityCount()) === before + 1, { timeout: 10000 });

    await $("#play").click();
    await browser.waitUntil(() => shown('[data-testid="playStageBadge"]'), { timeout: 10000 });
    expect(await shown('[data-testid="stageStop"]')).toBe(true);
    await $('[data-testid="stageStop"]').click();
    await browser.waitUntil(async () => !(await shown('[data-testid="playStageBadge"]')), { timeout: 10000 });
    await browser.saveScreenshot(path.join(evidenceDir, "workbench-controls.png"));
  });

  it("preserves the viewport at phone width and exposes both docks as focus-managed keyboard drawers", async () => {
    await browser.setWindowSize(560, 760);
    await browser.waitUntil(async () => !(await shown('[data-testid="left-dock"]')), {
      timeout: 5000,
      timeoutMsg: "desktop docks did not yield to the phone-width stage",
    });
    expect(await shown("#viewport")).toBe(true);

    await $('[data-testid="header-scene"]').click();
    await browser.waitUntil(() => shown('[data-testid="drawer-left"]'), {
      timeout: 5000,
      timeoutMsg: "Scene drawer did not open",
    });
    expect(
      await browser.execute(() => document.querySelector('[data-testid="drawer-left"]')?.contains(document.activeElement) ?? false),
    ).toBe(true);
    await browser.saveScreenshot(path.join(evidenceDir, "responsive-scene-drawer.png"));
    await browser.keys(["Escape"]);
    await browser.waitUntil(async () => !(await shown('[data-testid="drawer-left"]')), { timeout: 5000 });

    await $('[data-testid="header-inspector"]').click();
    await browser.waitUntil(() => shown('[data-testid="drawer-right"]'), {
      timeout: 5000,
      timeoutMsg: "Inspector drawer did not open",
    });
    expect(
      await browser.execute(() => document.querySelector('[data-testid="drawer-right"]')?.contains(document.activeElement) ?? false),
    ).toBe(true);
    await browser.saveScreenshot(path.join(evidenceDir, "responsive-inspector-drawer.png"));
    await browser.keys(["Escape"]);
    await browser.waitUntil(async () => !(await shown('[data-testid="drawer-right"]')), { timeout: 5000 });

    // The engine remains connected after two responsive mode transitions, rather than remounting a mock UI.
    expect((await invoke("camera_debug")).length).toBe(6);
  });
});
