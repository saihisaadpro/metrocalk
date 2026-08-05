// M14.2 / M14.3 convergence (ADR-058 / ADR-059) — the `.exe` gates that close the owed-convergence on the
// REAL /core: the flagship live per-entity thumbnail (the GPU RTT, real pixels via the `thumbnail` readback —
// NOT a DOM screenshot), 0-IPC/frame during orbit WITH thumbnails active (invariant 4), the thumbnail budget,
// and C6 (requirers/diagnostics + the inspector + a field edit = one undoable transaction) against the real
// /core (not MockCore). Local-only (needs the GUI + WebView2-matched msedgedriver). Run with a SMALL scene so
// the seeded HealthBar requirer is present:  set MTK_SCENE_N=16 & node ...wdio.js run wdio.acceptance.conf.js
//   --spec ./specs/acceptance/m14-thumbnails.acceptance.e2e.js

import { browser, expect, $ } from "@wdio/globals";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { page } from "../../pages/scaffold.js";
import {
  invoke,
  ipcPerFrame,
  captureBudget,
  scoreBudget,
  loadBaseline,
  installConsoleGuard,
  consoleErrors,
  clearConsole,
  report,
} from "../../lib/acceptance.js";

const ui = page();
const dir = path.dirname(fileURLToPath(import.meta.url));
const asset = (name) => path.resolve(dir, "../../../assets", name); // editor-shell/assets/<name>
const REAL_GLB = ["prop.glb", "sphere.glb", "multi_material_quad.glb", "dense_sphere.glb"];
const baseline = loadBaseline();

/** Decode a `data:image/png;base64,…` thumbnail and read its objective facts (a REAL GPU render, not a DOM
 *  screenshot of an empty <img>). The visual "matches the viewport exactly" judgment stays a human handoff. */
function pngInfo(dataUrl) {
  expect(typeof dataUrl).toBe("string");
  expect(dataUrl.startsWith("data:image/png;base64,")).toBe(true);
  const buf = Buffer.from(dataUrl.slice("data:image/png;base64,".length), "base64");
  const magic = buf.length > 24 && buf[0] === 0x89 && buf[1] === 0x50 && buf[2] === 0x4e && buf[3] === 0x47;
  return { buf, magic, w: buf.readUInt32BE(16), h: buf.readUInt32BE(20), bytes: buf.length };
}

/** Render a thumbnail, retrying until the async GPU RTT lands a result (the command can briefly return null
 *  under a queued burst — the render produces the pixels reliably, this just polls for them, the standard
 *  async-result pattern). The feature's correctness is the PNG; this only removes a load-timing race. */
async function thumbOf(id, size = 112) {
  let url = null;
  await browser.waitUntil(
    async () => {
      url = await invoke("thumbnail", { id, size });
      return typeof url === "string";
    },
    { timeout: 10000, interval: 400, timeoutMsg: `thumbnail(${id}) never rendered` },
  );
  return url;
}

describe("M14.2/M14.3 — live thumbnails · 0-IPC-with-thumbnails · C6 + scrub on the real /core", () => {
  const ids = []; // imported real-mesh entities

  before(async () => {
    // Gentle, execute-based connect wait (rapid `getText` element-command polling intermittently ECONNRESETs
    // the WebView2 channel under the heavier React renderer; a single `browser.execute` JS read is reliable).
    await browser.waitUntil(
      async () => {
        const c = await browser.execute(() => document.querySelector("#count")?.textContent || "");
        return /\d+ entities/.test(c);
      },
      { timeout: 40000, interval: 1500, timeoutMsg: "editor never showed an entity count (no /core connection?)" },
    );
    await installConsoleGuard();
    await clearConsole();
  });

  it("imports ≥3 real .glb meshes and renders a REAL per-entity thumbnail for each (the GPU RTT)", async () => {
    for (const name of REAL_GLB) {
      const id = await invoke("import_asset", { path: asset(name) });
      if (id) ids.push({ id, name });
    }
    expect(ids.length).toBeGreaterThanOrEqual(3); // ≥3 diverse real assets imported + placed
    await browser.pause(900); // let the imports re-project + the background thumbnail pump's queue drain

    const thumbs = [];
    for (const { id, name } of ids) {
      const url = await thumbOf(id);
      const info = pngInfo(url);
      expect(info.magic).toBe(true); // a valid PNG
      expect(info.w).toBe(112);
      expect(info.h).toBe(112);
      expect(info.bytes).toBeGreaterThan(700); // non-trivial → a real render, not a flat clear (a 112² clear ≈ 300 B)
      thumbs.push({ name, hex: info.buf.toString("hex") });
    }
    // Entity-specific: distinct meshes produce DISTINCT thumbnails (a constant placeholder would be equal).
    const unique = new Set(thumbs.map((t) => t.hex));
    expect(unique.size).toBe(thumbs.length);

    report.workflow("M14.2/thumbnail-rtt-real-pixels", { functional: true, inv1: true, inv4: true, clean: true }, { commands: ["import_asset", "thumbnail"], evidence: `${ids.length} real .glb → ${unique.size} distinct ${112}px PNGs` });
  });

  it("the thumbnail is DETERMINISTIC, and a material edit RE-renders it (silhouette/material change)", async () => {
    const { id } = ids[0];
    const a = pngInfo(await thumbOf(id));
    const b = pngInfo(await thumbOf(id));
    expect(a.buf.equals(b.buf)).toBe(true); // same scene state → byte-identical (determinism-safe presentation)

    await invoke("ai_edit", { id, material: "gold" }); // a validated, undoable material patch
    // Poll until the re-projected material reaches the render + the thumbnail re-renders (the edit→render
    // settle time varies with the profile; the change itself is the assertion, not a fixed pause).
    let c;
    await browser.waitUntil(
      async () => {
        c = pngInfo(await thumbOf(id));
        return !c.buf.equals(a.buf);
      },
      { timeout: 8000, interval: 500, timeoutMsg: "the material patch never changed the thumbnail" },
    );
    expect(c.buf.equals(a.buf)).toBe(false); // the material change is reflected in the live thumbnail
    report.workflow("M14.2/thumbnail-deterministic+reactive", { functional: true, inv3: true, clean: true }, { commands: ["thumbnail", "ai_edit"], evidence: "identical on re-render; differs after a material patch" });
  });

  it("invariant 4: 0 IPC/frame during orbit WITH thumbnails active", async () => {
    await invoke("thumbnail", { id: ids[0].id, size: 112 }); // a thumbnail is live/cached
    const perFrame = await ipcPerFrame(async () => {
      await ui.orbit(120, 60); // a real right-drag orbit gesture (native, render-loop polled)
    }, 500);
    expect(perFrame).toBeLessThan(1); // ≈0 — the RTT is discrete + dirty-only, never on the per-frame path
    report.workflow("M14.2/inv4-orbit-with-thumbnails", { functional: true, inv4: true, clean: true }, { commands: ["thumbnail", "drag_start", "drag_end", "ipc_count"], evidence: `${perFrame.toFixed(3)} IPC/frame during orbit` });
  });

  it("the thumbnail budget holds (a discrete off-frame op, measured)", async () => {
    const s = await captureBudget("thumbnail", "thumbnail", () => ({ id: ids[0].id, size: 112 }), { n: 20, warmup: 3 });
    const scored = await scoreBudget(s, baseline, { perFrame: false, recapture: () => captureBudget("thumbnail", "thumbnail", () => ({ id: ids[0].id, size: 112 }), { n: 20, warmup: 3 }) });
    report.budget(scored);
    // eslint-disable-next-line no-console
    console.log(`[M14.2] thumbnail (${report.profile}) p50 ${s.p50.toFixed(2)} ms / p99 ${s.p99.toFixed(2)} ms`);
    expect(scored.verdict).not.toBe("fail"); // gated only vs its own baseline (a GPU readback is a one-shot heavy op)
  });

  it("C6: requirers surface against the REAL /core; the inspector + diagnostics read real truth", async () => {
    const reqs = await ui.requirers();
    expect(reqs.length).toBeGreaterThan(0); // the C6 fix: requirers actually surface (was 0 against real /core)
    const requirerId = await reqs[0].getAttribute("data-id");

    const reveal = await invoke("reveal_targets", { id: requirerId });
    expect(reveal.required.length).toBeGreaterThan(0); // it needs a capability (the real (Requires,cap) pair)
    expect(reveal.compatible.length + reveal.greyed.length).toBeGreaterThan(0); // ranked targets / explained "no"s

    await ui.selectRequirer(0); // select it → the inspector + diagnostics populate
    const diag = await $("#diagnostics");
    expect(await diag.isExisting()).toBe(true);
    const diagRow = await $('[data-testid="diag-row"]');
    expect(await diagRow.isExisting()).toBe(true);
    expect(await diagRow.getAttribute("data-severity")).toBe("error"); // a structured, actionable diagnostic

    report.workflow("M14.3/C6-requirers+diagnostics-real-core", { functional: true, inv1: true, p1_explained: true, clean: true }, { commands: ["reveal_targets", "entity_details"], evidence: `requirer ${requirerId} needs ${reveal.required.join(",")}; diagnostic severity=error` });
  });

  it("the inspector field edit is ONE undoable transaction (the scrub's commit path)", async () => {
    // Select the imported mesh entity in the UI (the React store drives the inspector) → its Transform renders
    // real, editable props. (gizmo_select sets only the ENGINE selection; the inspector reads the React store.)
    await browser.execute((id) => document.querySelector(`[data-testid="hrow"][data-id="${id}"]`)?.click(), ids[0].id);
    await browser.pause(400);
    const before = await invoke("read_transform", { id: ids[0].id }); // [x,y,z,qx,qy,qz,qw,scale]
    const target = Math.round((Number(before[0]) || 0) + 5);
    // Edit Transform.x via the NumericField commit path. Split set-value (→ React re-renders the field's local
    // text) from the blur (→ commitTyped reads the UPDATED text → onCommit → client.setField, the ADR-010 tx).
    const present = await browser.execute((val) => {
      const input = document.querySelector('[data-testid="num-Transform.x"]');
      if (!input) return false;
      input.focus(); // must be focused so the later blur fires onBlur → commitTyped → the commit
      const setter = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(input), "value").set;
      setter.call(input, String(val));
      input.dispatchEvent(new Event("input", { bubbles: true }));
      return true;
    }, target);
    expect(present).toBe(true); // the inspector rendered the real Transform.x field for the selected entity
    await browser.pause(150); // let React re-render the field's local text before committing on blur
    await browser.execute(() => {
      const input = document.querySelector('[data-testid="num-Transform.x"]');
      if (input) input.blur();
    });
    await browser.pause(350);
    const after = await invoke("read_transform", { id: ids[0].id });
    expect(Math.round(after[0])).toBe(target); // the edit committed (the optimistic transaction reached the core)

    await browser.execute(() => document.activeElement && document.activeElement.blur());
    await browser.keys(["Control", "z"]); // ONE undo step reverts the whole edit (not the input's text undo)
    await browser.pause(350);
    const reverted = await invoke("read_transform", { id: ids[0].id });
    expect(Math.round(reverted[0])).toBe(Math.round(before[0])); // one undo restored it — a single coalesced tx
    report.workflow("M14.3/inspector-edit-one-undoable-tx", { functional: true, inv1: true, inv3: true, clean: true }, { commands: ["submit_edit", "read_transform", "undo"], evidence: `Transform.x ${Math.round(before[0])} → ${Math.round(after[0])} → ${Math.round(reverted[0])} (one undo)` });
  });

  it("no console errors / unhandled rejections across the M14 convergence run", async () => {
    const errs = await consoleErrors();
    report.consoleErrorCount += errs.length;
    expect(errs).toEqual([]);
  });
});
