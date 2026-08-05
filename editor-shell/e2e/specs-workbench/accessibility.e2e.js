import { browser, expect, $ } from "@wdio/globals";
import AxeBuilder from "@axe-core/webdriverio";
import { createHash, randomUUID } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const specDir = path.dirname(fileURLToPath(import.meta.url));
const e2eDir = path.resolve(specDir, "..");
const evidenceDir = path.resolve(specDir, "../../evidence/accessibility");
const renderQualityDir = path.resolve(specDir, "../../evidence/render-quality-qc");
const application = process.env.MTK_EXE || path.resolve(e2eDir, "../src-tauri/target/release/metrocalk-editor-shell.exe");
const applicationBytes = fs.statSync(application).size;
const applicationSha256 = createHash("sha256").update(fs.readFileSync(application)).digest("hex");
const buildId = `sha256:${applicationSha256}`;
const runId = randomUUID();
const startedAt = new Date().toISOString();
const expectedAudits = [
  "wide-editor",
  "command-palette",
  "command-palette-empty",
  "reflow-scene-drawer",
  "reflow-inspector-drawer",
];
fs.mkdirSync(evidenceDir, { recursive: true });
fs.mkdirSync(renderQualityDir, { recursive: true });
const completedAudits = [];
let dpr2Context = null;

function clearStaleEvidence() {
  for (const label of expectedAudits) {
    fs.rmSync(path.join(evidenceDir, `${label}.axe.json`), { force: true });
    fs.rmSync(path.join(evidenceDir, `${label}.png`), { force: true });
  }
  for (const file of ["run-context.json", "accessibility-summary.json", "accessibility-summary.md"]) {
    fs.rmSync(path.join(evidenceDir, file), { force: true });
  }
  fs.rmSync(path.join(renderQualityDir, "runtime-quality.json"), { force: true });
}

function writeRunContext(completedAt = null) {
  const capabilities = browser.capabilities ?? {};
  fs.writeFileSync(path.join(evidenceDir, "run-context.json"), JSON.stringify({
    schemaVersion: 1,
    runId,
    startedAt,
    completedAt,
    target: "packaged Metrocalk editor",
    standard: "WCAG 2.2 Level AA",
    audits: completedAudits,
    expectedAudits,
    automatedEvidenceOnly: true,
    certificationStatus: "pending required human assistive-technology execution and independent review",
    buildId,
    application: {
      path: application,
      bytes: applicationBytes,
      sha256: applicationSha256,
    },
    platformName: capabilities.platformName ?? null,
    browserName: capabilities.browserName ?? null,
    browserVersion: capabilities.browserVersion ?? null,
  }, null, 2));
}

const shown = async (selector) => {
  const element = await $(selector);
  return (await element.isExisting()) && (await element.isDisplayed());
};

async function waitConnected() {
  await browser.waitUntil(
    async () => {
      try {
        return (await invoke("camera_debug")).length === 6;
      } catch {
        return false;
      }
    },
    { timeout: 30000, timeoutMsg: "the packaged editor never connected to its engine" },
  );
}

async function reviewIncompleteChecks(checks) {
  const entries = checks.flatMap((check) =>
    check.nodes.map((node) => ({
      ruleId: check.id,
      target: node.target,
      failureSummary: node.failureSummary ?? null,
      selector: node.target.find((part) => typeof part === "string") ?? null,
    })),
  );
  const selectors = entries.map((entry) => entry.selector);
  const measurements = await browser.execute((targets) => {
    function rgba(value) {
      const match = value.match(/rgba?\(([^)]+)\)/i);
      if (!match) return null;
      const parts = match[1].split(/[\s,\/]+/).filter(Boolean).map(Number);
      if (parts.length < 3 || parts.some((part) => !Number.isFinite(part))) return null;
      return { r: parts[0], g: parts[1], b: parts[2], a: parts[3] ?? 1 };
    }
    function luminance(color) {
      const channel = (value) => {
        const normalized = value / 255;
        return normalized <= 0.04045 ? normalized / 12.92 : ((normalized + 0.055) / 1.055) ** 2.4;
      };
      return 0.2126 * channel(color.r) + 0.7152 * channel(color.g) + 0.0722 * channel(color.b);
    }
    return targets.map((selector) => {
      if (!selector) return { error: "axe target does not contain a CSS selector" };
      const element = document.querySelector(selector);
      if (!(element instanceof Element)) return { error: `target not found: ${selector}` };
      const style = getComputedStyle(element);
      const foreground = rgba(style.color);
      if (!foreground) return { error: `unsupported foreground color: ${style.color}` };
      let opacity = foreground.a;
      let background = null;
      let cursor = element;
      while (cursor) {
        const cursorStyle = getComputedStyle(cursor);
        const ownOpacity = Number.parseFloat(cursorStyle.opacity);
        if (Number.isFinite(ownOpacity)) opacity *= ownOpacity;
        const candidate = rgba(cursorStyle.backgroundColor);
        if (candidate && candidate.a > 0) {
          background = candidate;
          break;
        }
        cursor = cursor.parentElement;
      }
      if (!background) return { error: `no opaque UI background found for ${selector}` };
      const effectiveForeground = {
        r: foreground.r * opacity + background.r * (1 - opacity),
        g: foreground.g * opacity + background.g * (1 - opacity),
        b: foreground.b * opacity + background.b * (1 - opacity),
      };
      const light = Math.max(luminance(effectiveForeground), luminance(background));
      const dark = Math.min(luminance(effectiveForeground), luminance(background));
      const ratio = (light + 0.05) / (dark + 0.05);
      const fontSize = Number.parseFloat(style.fontSize) || 0;
      const parsedWeight = Number.parseInt(style.fontWeight, 10);
      const fontWeight = Number.isFinite(parsedWeight) ? parsedWeight : (style.fontWeight === "bold" ? 700 : 400);
      const threshold = fontSize >= 24 || (fontSize >= 18.66 && fontWeight >= 700) ? 3 : 4.5;
      return {
        selector,
        foreground: style.color,
        background: `rgb(${background.r}, ${background.g}, ${background.b})`,
        effectiveOpacity: Number(opacity.toFixed(4)),
        ratio: Number(ratio.toFixed(3)),
        threshold,
        fontSize,
        fontWeight,
      };
    });
  }, selectors);

  const reviewed = [];
  const unresolved = [];
  entries.forEach((entry, index) => {
    const measurement = measurements[index];
    const record = { ...entry, measurement };
    if (entry.ruleId === "color-contrast" && measurement?.ratio >= measurement?.threshold) {
      reviewed.push({ ...record, disposition: "computed contrast meets the WCAG AA text threshold" });
    } else {
      unresolved.push(record);
    }
  });
  return { reviewed, unresolved };
}

async function audit(label) {
  const result = await new AxeBuilder({ client: browser })
    .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22a", "wcag22aa"])
    .analyze();
  const incompleteReview = await reviewIncompleteChecks(result.incomplete);
  const context = await browser.execute(() => ({
    title: document.title,
    url: location.href,
    innerWidth: window.innerWidth,
    innerHeight: window.innerHeight,
    devicePixelRatio: window.devicePixelRatio,
    rootZoom: getComputedStyle(document.documentElement).zoom,
  }));
  fs.writeFileSync(
    path.join(evidenceDir, `${label}.axe.json`),
    JSON.stringify({
      schemaVersion: 1,
      label,
      runId,
      buildId,
      target: "WCAG 2.2 Level AA automated rules",
      completedAt: new Date().toISOString(),
      tool: { name: result.testEngine?.name ?? "axe-core", version: result.testEngine?.version ?? null },
      context,
      incompleteReview,
      results: result,
    }, null, 2),
  );
  await browser.saveScreenshot(path.join(evidenceDir, `${label}.png`));
  completedAudits.push(label);
  const summary = result.violations.map((violation) => ({
    id: violation.id,
    impact: violation.impact,
    help: violation.help,
    nodes: violation.nodes.map((node) => node.target),
  }));
  if (summary.length > 0) {
    throw new Error(`${label} accessibility violations:\n${JSON.stringify(summary, null, 2)}`);
  }
  if (incompleteReview.unresolved.length > 0) {
    throw new Error(`${label} axe checks still need review:\n${JSON.stringify(incompleteReview.unresolved, null, 2)}`);
  }
}

async function focusBeforeFirstControl() {
  await browser.execute(() => {
    if (document.activeElement instanceof HTMLElement) document.activeElement.blur();
    document.body.tabIndex = -1;
    document.body.focus();
    document.body.removeAttribute("tabindex");
  });
}

async function activateFromKeyboard(element) {
  await browser.execute((node) => node.focus(), element);
  expect(await element.isFocused()).toBe(true);
  await browser.keys(["Enter"]);
}

async function expectNoPageOverflow(label) {
  const metrics = await browser.execute(() => {
    const root = document.querySelector(".mtk-editor-root");
    return {
      viewportWidth: document.documentElement.clientWidth,
      documentWidth: document.documentElement.scrollWidth,
      appClientWidth: root?.clientWidth ?? null,
      appScrollWidth: root?.scrollWidth ?? null,
    };
  });
  if (metrics.documentWidth > metrics.viewportWidth + 1) {
    throw new Error(`${label}: document overflow ${JSON.stringify(metrics)}`);
  }
  if (metrics.appClientWidth != null && metrics.appScrollWidth != null) {
    if (metrics.appScrollWidth > metrics.appClientWidth + 1) {
      throw new Error(`${label}: editor overflow ${JSON.stringify(metrics)}`);
    }
  }
}

describe("packaged editor / WCAG 2.2 AA engineering gate", () => {
  before(async () => {
    clearStaleEvidence();
    await browser.setTimeout({ script: 180_000 });
    await browser.setWindowSize(1280, 800);
    await waitConnected();
    await browser.execute(() => {
      const metrics = { errors: [], unhandledRejections: [], longTasks: [], layoutShifts: [] };
      window.__metrocalkRenderQc = metrics;
      window.addEventListener("error", (event) => metrics.errors.push({ message: event.message, source: event.filename, line: event.lineno }));
      window.addEventListener("unhandledrejection", (event) => metrics.unhandledRejections.push(String(event.reason)));
      if (typeof PerformanceObserver === "function") {
        if (PerformanceObserver.supportedEntryTypes.includes("longtask")) {
          new PerformanceObserver((list) => {
            for (const entry of list.getEntries()) metrics.longTasks.push({ startTime: entry.startTime, duration: entry.duration });
          }).observe({ type: "longtask", buffered: true });
        }
        if (PerformanceObserver.supportedEntryTypes.includes("layout-shift")) {
          new PerformanceObserver((list) => {
            for (const entry of list.getEntries()) {
              if (!entry.hadRecentInput) metrics.layoutShifts.push({ startTime: entry.startTime, value: entry.value });
            }
          }).observe({ type: "layout-shift", buffered: true });
        }
      }
    });
    writeRunContext();
  });

  after(async () => {
    const runtime = await browser.execute(() => {
      const resources = performance.getEntriesByType("resource").map((entry) => ({
        name: entry.name,
        initiatorType: entry.initiatorType,
        durationMs: Number(entry.duration.toFixed(3)),
        transferBytes: entry.transferSize ?? null,
        decodedBytes: entry.decodedBodySize ?? null,
        responseStatus: entry.responseStatus ?? null,
        contentType: entry.contentType ?? null,
      }));
      const isLocalIpc = (resource) => {
        try { return new URL(resource.name).hostname === "ipc.localhost"; } catch { return false; }
      };
      const localIpcResources = resources.filter(isLocalIpc);
      const assetResources = resources.filter((resource) => !isLocalIpc(resource));
      const counts = new Map();
      for (const resource of assetResources) counts.set(resource.name, (counts.get(resource.name) ?? 0) + 1);
      const origin = location.origin;
      const remoteResources = assetResources.filter((resource) => {
        try { return new URL(resource.name).origin !== origin; } catch { return false; }
      });
      const failedResources = assetResources.filter((resource) => Number.isFinite(resource.responseStatus) && resource.responseStatus >= 400);
      const oversizedResources = assetResources.filter((resource) => (resource.transferBytes ?? 0) > 1_048_576);
      const slowResources = assetResources.filter((resource) => resource.durationMs > 2_000);
      const duplicates = Array.from(counts.entries()).filter(([, count]) => count > 1).map(([name, count]) => ({ name, count }));
      const collected = window.__metrocalkRenderQc ?? { errors: [], unhandledRejections: [], longTasks: [], layoutShifts: [] };
      const layoutShiftScore = collected.layoutShifts.reduce((sum, entry) => sum + entry.value, 0);
      const memory = performance.memory ? {
        usedJsHeapBytes: performance.memory.usedJSHeapSize,
        totalJsHeapBytes: performance.memory.totalJSHeapSize,
        jsHeapLimitBytes: performance.memory.jsHeapSizeLimit,
      } : null;
      return {
        capturedAt: new Date().toISOString(),
        page: { origin, devicePixelRatio, innerWidth, innerHeight, rootZoom: getComputedStyle(document.documentElement).zoom },
        navigation: performance.getEntriesByType("navigation").map((entry) => ({ durationMs: entry.duration, domContentLoadedMs: entry.domContentLoadedEventEnd, loadMs: entry.loadEventEnd })),
        resources,
        summary: {
          resourceCount: resources.length,
          assetResourceCount: assetResources.length,
          localIpcRequestCount: localIpcResources.length,
          totalTransferBytes: resources.reduce((sum, resource) => sum + (resource.transferBytes ?? 0), 0),
          totalDecodedBytes: resources.reduce((sum, resource) => sum + (resource.decodedBytes ?? 0), 0),
          remoteResourceCount: remoteResources.length,
          failedResourceCount: failedResources.length,
          oversizedResourceCount: oversizedResources.length,
          slowResourceCount: slowResources.length,
          duplicateResourceCount: duplicates.length,
          runtimeErrorCount: collected.errors.length,
          unhandledRejectionCount: collected.unhandledRejections.length,
          longTaskCount: collected.longTasks.length,
          maxLongTaskMs: collected.longTasks.reduce((max, entry) => Math.max(max, entry.duration), 0),
          layoutShiftScore: Number(layoutShiftScore.toFixed(6)),
        },
        remoteResources,
        failedResources,
        oversizedResources,
        slowResources,
        duplicateResources: duplicates,
        runtimeErrors: collected.errors,
        unhandledRejections: collected.unhandledRejections,
        longTasks: collected.longTasks,
        layoutShifts: collected.layoutShifts,
        memory,
      };
    });
    let browserLogs = [];
    let browserLogError = null;
    try {
      browserLogs = typeof browser.getLogs === "function" ? await browser.getLogs("browser") : [];
    } catch (error) {
      browserLogError = error instanceof Error ? error.message : String(error);
    }
    const actionableLogs = browserLogs.filter((entry) => ["WARNING", "SEVERE"].includes(entry.level) && entry.source !== "deprecation");
    const runtimeEvidence = { schemaVersion: 1, buildId, ...runtime, dpr2Context, browserLogs, browserLogError, actionableBrowserLogCount: actionableLogs.length };
    fs.writeFileSync(path.join(renderQualityDir, "runtime-quality.json"), JSON.stringify(runtimeEvidence, null, 2));
    await browser.execute(() => { document.documentElement.style.zoom = ""; });
    await browser.setWindowSize(1280, 800);
    writeRunContext(new Date().toISOString());
    const failures = {
      runtimeErrors: runtime.summary.runtimeErrorCount,
      unhandledRejections: runtime.summary.unhandledRejectionCount,
      failedResources: runtime.summary.failedResourceCount,
      remoteResources: runtime.summary.remoteResourceCount,
      oversizedResources: runtime.summary.oversizedResourceCount,
      slowResources: runtime.summary.slowResourceCount,
      actionableBrowserLogs: actionableLogs.length,
    };
    if (Object.values(failures).some((count) => count > 0)) {
      throw new Error(`runtime quality evidence failed: ${JSON.stringify(failures)}`);
    }
  });

  it("has no automated WCAG A/AA violations in the primary editor shell", async () => {
    await audit("wide-editor");
  });

  it("preserves the shell layout under a 2x device-pixel-ratio viewport", async () => {
    try {
      await browser.sendCommand("Emulation.setDeviceMetricsOverride", {
        width: 640,
        height: 400,
        deviceScaleFactor: 2,
        mobile: false,
        screenWidth: 1280,
        screenHeight: 800,
      });
      await browser.execute(() => window.dispatchEvent(new Event("resize")));
      dpr2Context = await browser.execute(() => {
        const root = document.querySelector(".mtk-editor-root");
        const bounds = root?.getBoundingClientRect();
        return {
          devicePixelRatio,
          innerWidth,
          innerHeight,
          rootWidth: bounds?.width ?? null,
          rootHeight: bounds?.height ?? null,
          documentWidth: document.documentElement.scrollWidth,
          viewportWidth: document.documentElement.clientWidth,
        };
      });
      expect(dpr2Context.devicePixelRatio).toBe(2);
      expect(dpr2Context.rootWidth).toBe(640);
      expect(dpr2Context.documentWidth).toBeLessThanOrEqual(dpr2Context.viewportWidth + 1);
      expect(await $("#viewport").isDisplayed()).toBe(true);
    } finally {
      await browser.sendCommand("Emulation.clearDeviceMetricsOverride", {});
      await browser.setWindowSize(1280, 800);
      await browser.execute(() => window.dispatchEvent(new Event("resize")));
    }
  });

  it("supports skip navigation, visible focus, keyboard tabs, modal containment, Escape, and focus return", async () => {
    await focusBeforeFirstControl();
    await browser.keys(["Tab"]);
    const skipLink = await $(".mtk-skip-link");
    expect(await skipLink.isFocused()).toBe(true);
    expect(await skipLink.isDisplayed()).toBe(true);
    const focusAppearance = await browser.execute(() => {
      const element = document.querySelector(".mtk-skip-link");
      const style = element ? getComputedStyle(element) : null;
      return style ? { outlineStyle: style.outlineStyle, outlineWidth: style.outlineWidth, top: element.getBoundingClientRect().top } : null;
    });
    expect(focusAppearance?.outlineStyle).not.toBe("none");
    expect(Number.parseFloat(focusAppearance?.outlineWidth ?? "0")).toBeGreaterThanOrEqual(2);
    expect(focusAppearance?.top ?? -1).toBeGreaterThanOrEqual(0);
    await browser.keys(["Enter"]);
    expect(await $("#viewport").isFocused()).toBe(true);

    const workspaceTrigger = await $('[data-testid="header-workspaces"]');
    await activateFromKeyboard(workspaceTrigger);
    await browser.waitUntil(() => shown('[data-testid="header-logic"]'), {
      timeout: 5000,
      timeoutMsg: "the keyboard-opened Workspaces menu did not reveal Logic",
    });
    const createWorkspace = await $('[data-testid="header-create"]');
    await browser.waitUntil(() => createWorkspace.isFocused(), {
      timeout: 5000,
      timeoutMsg: "the Workspaces menu did not focus its first command",
    });
    await browser.keys(["ArrowDown"]);
    const logicTrigger = await $('[data-testid="header-logic"]');
    expect(await logicTrigger.isFocused()).toBe(true);
    await browser.keys(["Enter"]);
    const assetTab = await $("#bottom-workspaces-asset-tab");
    await activateFromKeyboard(assetTab);
    await browser.keys(["ArrowRight"]);
    expect(await $("#bottom-workspaces-problems-tab").getAttribute("aria-selected")).toBe("true");

    const paletteTrigger = await $('[data-testid="command-palette-trigger"]');
    await activateFromKeyboard(paletteTrigger);
    await browser.waitUntil(() => shown('[data-testid="command-palette"]'), { timeout: 5000 });
    const search = await $('input[aria-label="Search commands"]');
    expect(await search.isFocused()).toBe(true);
    await audit("command-palette");

    await search.setValue("no-such-editor-command");
    expect(await $('[data-testid="command-palette"] [role="status"]').getText()).toContain("No matching commands");
    await audit("command-palette-empty");
    await search.clearValue();

    for (let index = 0; index < 12; index += 1) {
      await browser.keys(["Tab"]);
      const contained = await browser.execute(() =>
        document.querySelector('[data-testid="command-palette"]')?.contains(document.activeElement) ?? false,
      );
      expect(contained).toBe(true);
    }
    await browser.keys(["Escape"]);
    await browser.waitUntil(async () => !(await shown('[data-testid="command-palette"]')), { timeout: 5000 });
    expect(await paletteTrigger.isFocused()).toBe(true);
  });

  it("reflows at a 320-CSS-pixel equivalent and keeps both dock workflows accessible", async () => {
    // A 640-pixel WebView at 200% page zoom provides a deterministic 320 CSS-pixel-equivalent canvas. This
    // exercises the WCAG 1.4.10 desktop reflow condition without relying on vendor-specific DevTools APIs.
    await browser.setWindowSize(640, 800);
    await browser.execute(() => {
      document.documentElement.style.zoom = "2";
      // Browser page zoom fires resize; CSS zoom is the deterministic WebView substitute, so mirror that signal.
      window.dispatchEvent(new Event("resize"));
    });
    expect(await shown("#viewport")).toBe(true);
    expect(await shown('[data-testid="left-dock"]')).toBe(false);
    await expectNoPageOverflow("reflow editor");

    const sceneTrigger = await $('[data-testid="header-scene"]');
    await activateFromKeyboard(sceneTrigger);
    await browser.waitUntil(() => shown('[data-testid="drawer-left"]'), { timeout: 5000 });
    expect(await browser.execute(() => document.querySelector('[data-testid="drawer-left"]')?.contains(document.activeElement) ?? false)).toBe(true);
    await expectNoPageOverflow("reflow scene drawer");
    await audit("reflow-scene-drawer");
    await browser.keys(["Escape"]);
    expect(await sceneTrigger.isFocused()).toBe(true);

    const inspectorTrigger = await $('[data-testid="header-inspector"]');
    await activateFromKeyboard(inspectorTrigger);
    await browser.waitUntil(() => shown('[data-testid="drawer-right"]'), { timeout: 5000 });
    expect(await browser.execute(() => document.querySelector('[data-testid="drawer-right"]')?.contains(document.activeElement) ?? false)).toBe(true);
    await expectNoPageOverflow("reflow inspector drawer");
    await audit("reflow-inspector-drawer");
    await browser.keys(["Escape"]);
    expect(await inspectorTrigger.isFocused()).toBe(true);
    expect((await invoke("camera_debug")).length).toBe(6);
  });
});
