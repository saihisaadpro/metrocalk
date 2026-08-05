// User-perspective imported-animation gate against the real packaged editor:
// import a drawable two-node rigid GLB twice -> select one instance -> map both imported source nodes to
// distinct visible scene targets -> audition without authoring -> scrub a native pose -> stop and restore ->
// save -> wire the ready source into the graph -> compile -> play. Every important state is captured from
// the final OS-composited window, including wgpu.

import { $, $$, browser, expect } from "@wdio/globals";
import { execFileSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const dir = path.dirname(fileURLToPath(import.meta.url));
const SHOT_DIR =
  process.env.MTK_SHOT_DIR ||
  path.resolve(dir, "../../evidence/animation-clip-instancing/native-e2e");
const SHOT_PS1 = path.resolve(dir, "../scripts/capture-composited-window.ps1");
const BASE_CUBE = path.resolve(dir, "../../assets/cube.glb");
const PROC = "metrocalk-editor-shell";
const FIXTURE_ROOT = mkdtempSync(path.join(os.tmpdir(), "metrocalk-animation-e2e-"));
const FIXTURE = path.join(FIXTURE_ROOT, "live-two-node-showcase.glb");
const SHOT_LABELS = [
  "01_imported_setup",
  "02_unsaved_preview_midpoint",
  "03_preview_stopped_restored",
  "04_ready_graph_compiled",
  "05_graph_live_playback",
];

mkdirSync(SHOT_DIR, { recursive: true });

function parseGlb(bytes) {
  if (bytes.subarray(0, 4).toString("ascii") !== "glTF") {
    throw new Error("base fixture is not a GLB");
  }
  let json = null;
  let bin = null;
  for (let offset = 12; offset < bytes.length;) {
    const length = bytes.readUInt32LE(offset);
    const kind = bytes.readUInt32LE(offset + 4);
    const chunk = bytes.subarray(offset + 8, offset + 8 + length);
    if (kind === 0x4e4f534a) {
      json = JSON.parse(chunk.toString("utf8").replace(/\0+$/u, "").trim());
    } else if (kind === 0x004e4942) {
      bin = Buffer.from(chunk);
    }
    offset += 8 + length;
  }
  if (!json || !bin) throw new Error("base GLB must contain JSON and BIN chunks");
  return { json, bin };
}

function f32Buffer(values) {
  const bytes = Buffer.alloc(values.length * 4);
  values.forEach((value, index) => bytes.writeFloatLE(value, index * 4));
  return bytes;
}

function padded(bytes, fill = 0) {
  const padding = (4 - (bytes.length % 4)) % 4;
  return padding === 0 ? bytes : Buffer.concat([bytes, Buffer.alloc(padding, fill)]);
}

function buildTwoNodeAnimatedCubes() {
  const { json, bin: baseBin } = parseGlb(readFileSync(BASE_CUBE));
  const additions = [
    f32Buffer([0, 2]),
    // Root source: translate, rotate and scale.
    f32Buffer([-1.5, 0, 0, -0.5, 0.6, 0]),
    f32Buffer([0, 0, 0, 1, 0, Math.SQRT1_2, 0, Math.SQRT1_2]),
    f32Buffer([1, 1, 1, 1.25, 1.25, 1.25]),
    // Child source: an opposing, equally visible motion. Although this node is a child in the GLB,
    // explicit clip instancing intentionally maps its local typed channels to a separate live entity.
    f32Buffer([1.5, 0, 0, 2.5, 0.6, 0]),
    f32Buffer([0, 0, 0, 1, 0, -Math.SQRT1_2, 0, Math.SQRT1_2]),
    f32Buffer([1, 1, 1, 0.78, 0.78, 0.78]),
  ];
  const views = [];
  let byteOffset = padded(baseBin).length;
  for (const addition of additions) {
    views.push({
      buffer: 0,
      byteOffset,
      byteLength: addition.length,
    });
    byteOffset += padded(addition).length;
  }
  const bin = Buffer.concat([padded(baseBin), ...additions.map((bytes) => padded(bytes))]);
  const firstView = json.bufferViews.length;
  const firstAccessor = json.accessors.length;
  json.bufferViews.push(...views);
  json.accessors.push(
    {
      bufferView: firstView,
      componentType: 5126,
      count: 2,
      type: "SCALAR",
      min: [0],
      max: [2],
    },
    { bufferView: firstView + 1, componentType: 5126, count: 2, type: "VEC3" },
    { bufferView: firstView + 2, componentType: 5126, count: 2, type: "VEC4" },
    { bufferView: firstView + 3, componentType: 5126, count: 2, type: "VEC3" },
    { bufferView: firstView + 4, componentType: 5126, count: 2, type: "VEC3" },
    { bufferView: firstView + 5, componentType: 5126, count: 2, type: "VEC4" },
    { bufferView: firstView + 6, componentType: 5126, count: 2, type: "VEC3" },
  );
  json.buffers[0].byteLength = bin.length;
  json.meshes[0].name = "Live two-node cube";
  json.nodes = [
    {
      ...json.nodes[0],
      name: "Showcase root",
      children: [1],
    },
    {
      mesh: 0,
      name: "Showcase child",
      translation: [3, 0, 0],
    },
  ];
  json.scenes[json.scene ?? 0].nodes = [0];
  json.materials[0].pbrMetallicRoughness = {
    baseColorFactor: [0.08, 0.42, 0.85, 1],
    metallicFactor: 0.2,
    roughnessFactor: 0.24,
  };
  json.animations = [{
    name: "Live two-node showcase",
    samplers: [
      { input: firstAccessor, output: firstAccessor + 1, interpolation: "LINEAR" },
      { input: firstAccessor, output: firstAccessor + 2, interpolation: "LINEAR" },
      { input: firstAccessor, output: firstAccessor + 3, interpolation: "LINEAR" },
      { input: firstAccessor, output: firstAccessor + 4, interpolation: "LINEAR" },
      { input: firstAccessor, output: firstAccessor + 5, interpolation: "LINEAR" },
      { input: firstAccessor, output: firstAccessor + 6, interpolation: "LINEAR" },
    ],
    channels: [
      { sampler: 0, target: { node: 0, path: "translation" } },
      { sampler: 1, target: { node: 0, path: "rotation" } },
      { sampler: 2, target: { node: 0, path: "scale" } },
      { sampler: 3, target: { node: 1, path: "translation" } },
      { sampler: 4, target: { node: 1, path: "rotation" } },
      { sampler: 5, target: { node: 1, path: "scale" } },
    ],
  }];

  const jsonChunk = padded(Buffer.from(JSON.stringify(json), "utf8"), 0x20);
  const binChunk = padded(bin);
  const totalLength = 12 + 8 + jsonChunk.length + 8 + binChunk.length;
  const header = Buffer.alloc(12);
  header.write("glTF", 0, 4, "ascii");
  header.writeUInt32LE(2, 4);
  header.writeUInt32LE(totalLength, 8);
  const jsonHeader = Buffer.alloc(8);
  jsonHeader.writeUInt32LE(jsonChunk.length, 0);
  jsonHeader.writeUInt32LE(0x4e4f534a, 4);
  const binHeader = Buffer.alloc(8);
  binHeader.writeUInt32LE(binChunk.length, 0);
  binHeader.writeUInt32LE(0x004e4942, 4);
  return Buffer.concat([header, jsonHeader, jsonChunk, binHeader, binChunk]);
}

const invoke = (command, args = {}) =>
  browser.execute(async (name, payload) => window.__TAURI__.core.invoke(name, payload), command, args);

// There is intentionally no separate `animation_playback_state` Tauri command. The production
// EditorClient implements that read through the registered `animation_transport` command's non-mutating
// `status` action; keep the native E2E on exactly the same contract.
const playbackState = () => invoke("animation_transport", {
  action: "status",
  tick: null,
  loopPolicy: null,
});

async function shot(label) {
  await browser.pause(350);
  const output = path.join(SHOT_DIR, `${label}.png`);
  execFileSync("powershell.exe", [
    "-NoProfile",
    "-ExecutionPolicy",
    "Bypass",
    "-File",
    SHOT_PS1,
    "-Out",
    output,
    "-ProcName",
    PROC,
    "-X",
    "40",
    "-Y",
    "40",
  ], { stdio: "pipe" });
  if (!existsSync(output) || statSync(output).size <= 1024) {
    throw new Error(`composited capture was not written correctly: ${output}`);
  }
  console.log("  composited shot", output);
  return output;
}

async function openAnimationWorkspace() {
  const tab = await $("#bottom-workspaces-animation-tab");
  await tab.waitForExist({ timeout: 30000 });
  const dock = await $('[data-testid="bottom-dock"]');
  const open = (await dock.getAttribute("class")).includes("is-open");
  const selected = (await tab.getAttribute("aria-selected")) === "true";
  if (!open || !selected) await tab.click();
  await $('[data-testid="animation-workspace"]').waitForDisplayed({ timeout: 30000 });
}

async function openAssetReadiness() {
  const toggle = await $('//button[.//span[normalize-space(.)="Asset readiness"]]');
  await toggle.waitForExist({ timeout: 30000 });
  if ((await toggle.getAttribute("aria-expanded")) !== "true") await toggle.click();
  await $('[data-testid="animation-imported-clip"]').waitForDisplayed({ timeout: 30000 });
}

async function setRangeValue(selector, value) {
  await browser.execute((inputSelector, nextValue) => {
    const input = document.querySelector(inputSelector);
    if (!(input instanceof HTMLInputElement)) {
      throw new Error(`range input is missing: ${inputSelector}`);
    }
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
    if (!setter) throw new Error("HTMLInputElement value setter is unavailable");
    setter.call(input, String(nextValue));
    input.dispatchEvent(new Event("input", { bubbles: true }));
    input.dispatchEvent(new Event("change", { bubbles: true }));
  }, selector, value);
}

async function enabledGraphSource() {
  await browser.waitUntil(async () => {
    const buttons = await $$('[aria-label="Animation graph sources"] button');
    for (const button of buttons) {
      if (!(await button.isEnabled())) continue;
      const text = (await button.getText()).toLocaleLowerCase();
      if (text.includes("two-node") && text.includes("clip instance")) return true;
    }
    return false;
  }, {
    timeout: 30000,
    interval: 200,
    timeoutMsg: "the persisted two-node clip instance never became an enabled graph source",
  });
  const buttons = await $$('[aria-label="Animation graph sources"] button');
  for (const button of buttons) {
    if (!(await button.isEnabled())) continue;
    const text = (await button.getText()).toLocaleLowerCase();
    if (text.includes("two-node") && text.includes("clip instance")) return button;
  }
  throw new Error("the two-node clip-instance graph source disappeared");
}

describe("native imported clip -> preview -> graph playback", () => {
  let entityId;
  let secondaryEntityId;
  let sequenceId;
  let authoredTransform;
  let secondaryAuthoredTransform;
  let initialClipRevision;

  before(async () => {
    // A partial failed run must never leave later screenshots that could be mistaken for current proof.
    for (const label of SHOT_LABELS) {
      rmSync(path.join(SHOT_DIR, `${label}.png`), { force: true });
    }
    writeFileSync(FIXTURE, buildTwoNodeAnimatedCubes());
    await browser.waitUntil(async () => {
      try {
        return await browser.execute(() => Boolean(window.__TAURI__?.core?.invoke));
      } catch {
        return false;
      }
    }, {
      timeout: 30000,
      interval: 200,
      timeoutMsg: "the Tauri bridge never became available",
    });
    await browser.execute(() => {
      window.__animationE2eErrors = [];
      window.__animationE2eResizeObserverDiagnostics = 0;
      window.addEventListener("error", (event) => {
        const message = String(event.message || event.error);
        // Chromium reports ResizeObserver's spec-defined deferred-delivery condition through the same
        // window "error" channel as real exceptions, but without an Error object or application stack.
        // React Flow legitimately resizes its measured graph surface while the bottom dock opens. Classify
        // that browser diagnostic separately so the hygiene gate still fails every application exception,
        // rejection, and console.error instead of producing a false positive.
        if (
          event.error == null
          && message === "ResizeObserver loop completed with undelivered notifications."
        ) {
          window.__animationE2eResizeObserverDiagnostics += 1;
          event.preventDefault();
          return;
        }
        window.__animationE2eErrors.push(message);
      });
      window.addEventListener("unhandledrejection", (event) => {
        window.__animationE2eErrors.push(`unhandledrejection: ${String(event.reason)}`);
      });
      const originalError = console.error.bind(console);
      console.error = (...values) => {
        window.__animationE2eErrors.push(`console.error: ${values.map(String).join(" ")}`);
        originalError(...values);
      };
      localStorage.setItem("mtk.onboarded.v1", "1");
      for (const button of document.querySelectorAll("button")) {
        if (/skip|got it/i.test(button.textContent || "")) {
          button.click();
          break;
        }
      }
    });
    await invoke("new_project");
    await browser.pause(400);
  });

  it("auditions without authoring, restores, persists an instance, compiles a graph and plays it", async () => {
    entityId = await invoke("import_asset", { path: FIXTURE });
    expect(typeof entityId).toBe("string");
    expect(entityId.length).toBeGreaterThan(0);
    secondaryEntityId = await invoke("import_asset", { path: FIXTURE });
    expect(typeof secondaryEntityId).toBe("string");
    expect(secondaryEntityId.length).toBeGreaterThan(0);
    expect(secondaryEntityId).not.toBe(entityId);
    expect(await invoke("multi_edit", {
      ids: [secondaryEntityId],
      component: "Transform",
      field: "x",
      value: 3,
    })).toBe(true);

    const row = await $(`[data-testid="hrow"][data-id="${entityId}"]`);
    const secondaryRow = await $(`[data-testid="hrow"][data-id="${secondaryEntityId}"]`);
    await row.waitForDisplayed({ timeout: 30000 });
    await secondaryRow.waitForExist({ timeout: 30000 });
    await row.scrollIntoView({ block: "center" });
    await row.click();
    await browser.waitUntil(
      async () => (await row.getAttribute("aria-selected")) === "true",
      { timeout: 10000, timeoutMsg: "the imported asset never became the selected entity" },
    );
    await invoke("focus_entity", { id: entityId });

    await browser.waitUntil(async () => {
      const state = await invoke("animation_state", { id: entityId });
      return state.asset?.clips?.some((clip) =>
        clip.readiness === "explicit_mapping_required" &&
        clip.sourceTargets?.length === 2 &&
        clip.sourceTargetIds?.length === 2 &&
        clip.sourceBindings?.length === 6
      ) || false;
    }, {
      timeout: 30000,
      interval: 250,
      timeoutMsg: "the imported two-node clip never exposed stable, typed explicit mapping",
    });
    const initial = await invoke("animation_state", { id: entityId });
    const importedClip = initial.asset.clips.find((clip) => clip.name === "Live two-node showcase");
    expect(importedClip).toBeDefined();
    expect(importedClip.readiness).toBe("explicit_mapping_required");
    expect(importedClip.sourceTargets).toHaveLength(2);
    expect(importedClip.sourceTargetIds).toHaveLength(2);
    expect(new Set(importedClip.sourceTargetIds).size).toBe(2);
    expect(importedClip.sourceBindings).toHaveLength(6);
    expect(initial.ticksPerSecond).toBe(60_000);
    expect(importedClip.durationTick).toBe(120_000);
    sequenceId = initial.sequenceId;
    authoredTransform = await invoke("read_transform", { id: entityId });
    secondaryAuthoredTransform = await invoke("read_transform", { id: secondaryEntityId });
    initialClipRevision = initial.asset.clipInstanceRevision;

    await openAnimationWorkspace();
    await openAssetReadiness();
    const setup = await $('[data-testid^="animation-clip-map-"]');
    await setup.waitForDisplayed({ timeout: 10000 });
    await setup.click();
    const dialog = await $('[data-testid="animation-clip-setup-dialog"]');
    await dialog.waitForDisplayed({ timeout: 10000 });
    expect((await dialog.getText()).toLocaleLowerCase()).toContain("map targets for live two-node showcase");
    const mappingRows = await $$('[data-testid="animation-clip-source-mapping"]');
    expect(mappingRows).toHaveLength(2);
    const rootTarget = await $('[data-testid="animation-clip-target-0"]');
    const childTarget = await $('[data-testid="animation-clip-target-1"]');
    expect(await rootTarget.getValue()).toBe(entityId);
    expect(await childTarget.getValue()).toBe("");
    const preview = await $('[data-testid="animation-clip-preview"]');
    const confirm = await $('[data-testid="animation-clip-confirm"]');
    expect(await preview.isEnabled()).toBe(false);
    expect(await confirm.isEnabled()).toBe(false);
    await childTarget.selectByAttribute("value", secondaryEntityId);
    await browser.waitUntil(async () =>
      (await rootTarget.getValue()) === entityId &&
      (await childTarget.getValue()) === secondaryEntityId &&
      (await preview.isEnabled()) &&
      (await confirm.isEnabled()), {
      timeout: 10000,
      interval: 100,
      timeoutMsg: "the explicit one-to-one two-node mapping never became previewable",
    });
    const mappingText = (await dialog.getText()).toLocaleLowerCase();
    expect(mappingText).toContain("2.00s");
    expect(mappingText).toContain("transform.translation");
    expect(mappingText).toContain("transform.rotation");
    expect(mappingText).toContain("transform.scale");
    await shot("01_imported_setup");

    await preview.click();
    await browser.waitUntil(
      async () => (await preview.getText()).trim() === "Stop preview",
      { timeout: 10000, timeoutMsg: "the unsaved clip audition never started" },
    );
    const previewStatus = await $('[data-testid="status"]');
    await previewStatus.waitForDisplayed({ timeout: 10000 });
    await browser.waitUntil(
      async () => (await previewStatus.getText()).includes("across 6 native channel(s)"),
      {
        timeout: 10000,
        interval: 100,
        timeoutMsg: "the visible status never confirmed that all six native channels were applied",
      },
    );
    const activePreview = await playbackState();
    expect(activePreview.importedClipPreviewActive).toBe(true);
    // The immutable glTF source retains its nanosecond clock, but every admitted graph/runtime source is
    // normalized onto the editor's shared 60 kHz playhead. Two source seconds must therefore be 120,000
    // runtime ticks—not the 2,000,000,000 source ticks that previously made playback take ~9 hours.
    expect(activePreview.durationTick).toBe(120_000);
    if (activePreview.playing) {
      const transportToggle = await $('[data-testid="animation-clip-audition-play"]');
      await transportToggle.click();
      await browser.waitUntil(async () => {
        const status = await playbackState();
        return status.playing === false && status.importedClipPreviewActive === true;
      }, {
        timeout: 10000,
        interval: 50,
        timeoutMsg: "the native unsaved preview did not pause while retaining audition authority",
      });
    }
    const midTick = activePreview.durationTick / 2;
    await setRangeValue('[data-testid="animation-clip-audition-scrub"]', midTick);
    await browser.waitUntil(async () => {
      const status = await playbackState();
      return status.currentTick === midTick && status.playing === false && status.evaluatedTracks === 6;
    }, {
      timeout: 10000,
      interval: 100,
      timeoutMsg: "the native two-target unsaved preview never reached its deterministic midpoint",
    });
    await browser.waitUntil(
      async () => (await $('[data-testid="animation-time"]').getText()).includes(`${midTick}t`),
      { timeout: 10000, timeoutMsg: "the visible transport did not reflect the native scrub tick" },
    );
    expect(await invoke("read_transform", { id: entityId })).toEqual(authoredTransform);
    expect(await invoke("read_transform", { id: secondaryEntityId })).toEqual(secondaryAuthoredTransform);
    const auditionState = await invoke("animation_state", { id: entityId });
    expect(auditionState.asset.clipInstanceRevision).toBe(initialClipRevision);
    expect(auditionState.asset.clips.some((clip) => clip.readiness === "ready")).toBe(false);
    await shot("02_unsaved_preview_midpoint");

    await preview.click();
    await browser.waitUntil(
      async () => (await preview.getText()).trim() === "Preview",
      { timeout: 10000, timeoutMsg: "Stop preview did not restore the setup state" },
    );
    const restoredTransport = await playbackState();
    expect(restoredTransport.playing).toBe(false);
    expect(restoredTransport.currentTick).toBe(0);
    expect(await invoke("read_transform", { id: entityId })).toEqual(authoredTransform);
    expect(await invoke("read_transform", { id: secondaryEntityId })).toEqual(secondaryAuthoredTransform);
    const restoredState = await invoke("animation_state", { id: entityId });
    expect(restoredState.asset.clipInstanceRevision).toBe(initialClipRevision);
    await shot("03_preview_stopped_restored");

    await confirm.click();
    const ready = await $('[data-testid="animation-clip-ready"]');
    await ready.waitForDisplayed({ timeout: 30000 });
    expect(await ready.getText()).toContain("Ready for Graph");
    const saved = await invoke("animation_state", { id: entityId });
    expect(saved.asset.clipInstanceRevision).not.toBe(initialClipRevision);
    const savedClip = saved.asset.clips.find((clip) => clip.name === "Live two-node showcase");
    expect(savedClip.readiness).toBe("ready");
    expect(savedClip.instanceId).toBeTruthy();

    await $("#animation-surfaces-graph-tab").click();
    await $('[data-testid="animation-graph-editor"]').waitForDisplayed({ timeout: 30000 });
    const emptyGraph = await $('//button[normalize-space(.)="Empty graph"]');
    await emptyGraph.waitForDisplayed({ timeout: 10000 });
    await emptyGraph.click();
    const paletteToggle = await $('[aria-controls="animation-graph-palette"]');
    if ((await paletteToggle.getAttribute("aria-expanded")) !== "true") {
      await paletteToggle.click();
    }
    await $('[aria-label="Animation graph node palette"]').waitForDisplayed({ timeout: 10000 });

    const source = await enabledGraphSource();
    expect((await source.getText()).toLowerCase()).toContain("two-node");
    const sourceName = (await source.$("span").getText()).trim();
    expect(sourceName.length).toBeGreaterThan(0);
    await source.click();
    const keyboardView = await $('//summary[normalize-space(.)="Keyboard/list view"]');
    await keyboardView.waitForDisplayed({ timeout: 10000 });
    await keyboardView.click();

    const sourceSelect = await $('[aria-label="Connection source node"]');
    const targetSelect = await $('[aria-label="Connection target node"]');
    // Empty Graph starts with only `Final output`; addSourceNode preserves the exact palette source name.
    // Select by those authored labels instead of relying on option order.
    await browser.waitUntil(async () => {
      const options = await sourceSelect.$$("option");
      for (const option of options) {
        if ((await option.getText()).trim() === sourceName) return true;
      }
      return false;
    }, {
      timeout: 10000,
      timeoutMsg: `the newly added graph node '${sourceName}' never reached the connection editor`,
    });
    await sourceSelect.selectByVisibleText(sourceName);
    await targetSelect.selectByVisibleText("Final output");
    await browser.waitUntil(async () =>
      Boolean(await sourceSelect.getValue()) && (await targetSelect.getValue()) === "output", {
      timeout: 10000,
      timeoutMsg: "the keyboard connection editor did not retain its explicit source and Final output target",
    });
    const connectionEditor = await $('[aria-label="Keyboard connection editor"]');
    await connectionEditor.$('.//button[normalize-space(.)="Add connection"]').click();
    await $('[aria-label="Animation graph connections"] li').waitForExist({ timeout: 10000 });

    const apply = await $('[data-testid="animation-graph-apply"]');
    await browser.waitUntil(
      async () => await apply.isEnabled(),
      { timeout: 10000, timeoutMsg: "the valid graph draft never became applicable" },
    );
    await apply.click();
    await browser.waitUntil(async () => {
      const state = await invoke("animation_graph_state", { sequenceId });
      return state.compile?.state === "ready";
    }, {
      timeout: 30000,
      interval: 200,
      timeoutMsg: "the native animation graph never reached ready",
    });
    await browser.waitUntil(
      async () => (await $(".animation-graph-toolbar-status").getText()).toLowerCase().includes("ready"),
      { timeout: 10000, timeoutMsg: "the graph UI did not refresh to native ready state" },
    );
    await shot("04_ready_graph_compiled");

    const loopPolicy = await $('[aria-label="Loop policy"]');
    await loopPolicy.selectByAttribute("value", "loop");
    await browser.waitUntil(
      async () => (await loopPolicy.getValue()) === "loop",
      { timeout: 10000, timeoutMsg: "loop playback policy was not selected" },
    );
    await $('[data-testid="animation-play"]').click();
    await browser.waitUntil(async () => {
      const status = await playbackState();
      return status.playing === true && status.evaluatedTracks >= 6;
    }, {
      timeout: 10000,
      interval: 100,
      timeoutMsg: "the compiled graph never began native playback",
    });
    const playbackBefore = await playbackState();
    await browser.pause(250);
    const playbackAfter = await playbackState();
    expect(playbackAfter.playing).toBe(true);
    const progressed =
      (playbackAfter.currentTick - playbackBefore.currentTick + playbackAfter.durationTick) %
      playbackAfter.durationTick;
    expect(progressed).toBeGreaterThanOrEqual(4_000);
    expect(progressed).toBeLessThanOrEqual(60_000);
    await shot("05_graph_live_playback");
    await $('[data-testid="animation-stop"]').click();

    const hygiene = await browser.execute(() => ({
      errors: window.__animationE2eErrors || [],
      resizeObserverDiagnostics: window.__animationE2eResizeObserverDiagnostics || 0,
    }));
    expect(hygiene.errors).toEqual([]);
    expect(hygiene.resizeObserverDiagnostics).toBeLessThan(256);
  });

  after(async () => {
    try {
      await invoke("animation_transport", {
        action: "stop",
        tick: null,
        loopPolicy: null,
      });
    } catch {
      // The WebDriver session may already be closing after an earlier fatal failure.
    } finally {
      rmSync(FIXTURE_ROOT, { recursive: true, force: true });
    }
  });
});
