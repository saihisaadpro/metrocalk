//! ADR-163 on the packaged `.exe`, against the real 275 MB STEP assembly — because the vitest suite
//! and the 48 `shots` scenes both drive a fixture, and the claim being made here is about 15,711 parts
//! that only the real shell can produce.
//!
//! DRIVEN THROUGH THE UI. The only command this spec invokes that changes anything is `import_asset`,
//! which is the setup; everything after it is a click or a keystroke on the real WebView2 composite,
//! and the two reads it uses (`gizmo_selected`, `cad_report`) are reads. A spec that reached for
//! `cad_report_page` directly would prove the command works and nothing at all about whether a person
//! can get to it — which is the entire defect this ADR closes.

import { browser } from "@wdio/globals";
import { execFileSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const dir = path.dirname(fileURLToPath(import.meta.url));
const shotDir = process.env.MTK_SHOT_DIR || path.resolve(dir, "../.shots-importsearch");
const capture = path.resolve(dir, "../scripts/capture-composited-window.ps1");
const step = "X:\\Work\\Metrocalk\\Games Projects\\Unreal\\Skid Weld Line A.1\\Skid Weld Line A.1_(1).stp";
mkdirSync(shotDir, { recursive: true });

const invoke = (command, args = {}) =>
  browser.execute((c, a) => window.__TAURI__.core.invoke(c, a), command, args);

/** Fire and forget: a 275 MB import takes minutes and the WebView2 renderer caps an awaited invoke at
 *  30 s, so the call is kicked off and the SCENE is polled instead. */
const kick = (command, args = {}) =>
  browser.execute((c, a) => {
    window.__TAURI__.core.invoke(c, a).catch(() => {});
    return true;
  }, command, args);

const entityCount = () =>
  browser.execute(() => {
    const match = document.getElementById("count")?.textContent?.match(/\d+/);
    return match ? Number.parseInt(match[0], 10) : 0;
  });

/** The panel's own structured signals, read off the DOM — never the prose. */
const panelState = () =>
  browser.execute(() => {
    const el = document.querySelector("[data-testid='import-report']");
    if (!el) return null;
    const showing = document.querySelector("[data-testid='import-showing']");
    const rows = [...document.querySelectorAll("[data-testid='import-row']")];
    return {
      total: Number(el.getAttribute("data-total")),
      matched: Number(el.getAttribute("data-matched")),
      offset: Number(el.getAttribute("data-offset")),
      shown: Number(el.getAttribute("data-shown")),
      rows: rows.length,
      firstName: rows[0]?.querySelector("span")?.textContent ?? null,
      showing: showing?.textContent ?? null,
      empty: !!document.querySelector("[data-testid='import-empty']"),
      nextDisabled: document.querySelector("[data-testid='import-next']")?.disabled ?? null,
      prevDisabled: document.querySelector("[data-testid='import-prev']")?.disabled ?? null,
      prevTitle: document.querySelector("[data-testid='import-prev']")?.getAttribute("title") ?? null,
    };
  });

const shot = async (name) => {
  await browser.pause(600);
  execFileSync(
    "powershell",
    ["-ExecutionPolicy", "Bypass", "-File", capture, "-ProcName", "metrocalk-editor-shell", "-Out", path.join(shotDir, `${name}.png`)],
    { stdio: "ignore" },
  );
};

/** Type into the search field the way a person does — one `input` event per character, so the panel's
 *  settle timer is exercised rather than routed around by a single programmatic set. */
const type = async (text) => {
  const field = await $("[data-testid='import-search']");
  await field.click();
  await browser.keys(text.split(""));
};

const clearSearch = async () => {
  // Select-all then delete, rather than counting backspaces: a click puts the caret where it lands, so
  // a fixed number of backspaces left a stub of the old query in front of the new one and the run's own
  // log read "021NDzzzznotapart" - evidence that says something slightly untrue about what was typed.
  const field = await $("[data-testid='import-search']");
  await field.click();
  await browser.keys(["", "a"]); // Ctrl held, then A
  await browser.keys([""]); // NULL releases every held modifier
  await browser.keys([""]); // Backspace
};

describe("finding one part in a 15,711-part assembly (ADR-163)", () => {
  let imported = 0;

  it("imports the real STEP assembly and opens the Import workspace from the dock", async () => {
    await browser.waitUntil(async () => (await browser.execute(() => !!window.__TAURI__)) === true, {
      timeout: 60000,
      timeoutMsg: "the Tauri bridge never appeared",
    });

    await kick("import_asset", { path: step });
    await browser.waitUntil(async () => (await entityCount()) >= 10000, {
      timeout: 900000,
      interval: 2000,
      timeoutMsg: "the STEP assembly did not land in the scene",
    });
    imported = await entityCount();
    if (imported < 10000) throw new Error(`only ${imported} entities landed`);

    // Through the dock, as a person reaches it — and it is TWO gestures by design, not one: the
    // collapsed dock's picker chooses which workspace is current and deliberately leaves the dock
    // shut (`BottomDock.test.tsx`: "keeps selection separate from explicit expansion"), so the
    // chevron is the second step. A spec that assumed one gesture is what found that out.
    const summary = await $("[data-testid='bottom-workspace-summary']");
    if (await summary.isExisting()) {
      await summary.click();
      await (await $("[data-testid='bottom-workspace-option-import']")).click();
      await browser.pause(300);
      const toggle = await $("[data-testid='bottom-dock-toggle']");
      if ((await toggle.getAttribute("aria-expanded")) !== "true") await toggle.click();
    } else {
      await (await $("#bottom-workspaces-import-tab")).click();
    }
    await browser.waitUntil(async () => (await panelState()) !== null, {
      timeout: 60000,
      timeoutMsg: "the import report never rendered",
    });
    await shot("01-import-report-open");
  });

  it("shows a bounded page of a very large report, and SAYS which rows those are", async () => {
    const s = await panelState();
    console.log("panel on open:", JSON.stringify(s));
    if (s.total < 10000) throw new Error(`the report describes only ${s.total} parts`);
    // The defect, as an assertion: the list is a page, the panel knows it is a page, and the sentence
    // beside it names the page. Before ADR-163 `shown` was 500 with nothing anywhere saying so.
    if (s.matched !== s.total) throw new Error(`no filter is applied, so matched must equal total (${s.matched} vs ${s.total})`);
    if (s.shown >= s.matched) throw new Error(`a ${s.matched}-part report was not paged (shown ${s.shown})`);
    if (s.rows !== s.shown) throw new Error(`the DOM holds ${s.rows} rows but the panel claims ${s.shown}`);
    if (!/Showing 1–\d[\d,]* of [\d,]+ parts/.test(s.showing)) throw new Error(`the showing line does not name the page: ${s.showing}`);
    if (s.prevDisabled !== true) throw new Error("Previous must refuse on the first page");
    if (!/first page/i.test(s.prevTitle ?? "")) throw new Error(`a refusing control must say why, got ${s.prevTitle}`);
    if (s.nextDisabled !== false) throw new Error("Next must be live when there are more pages");
  });

  it("fits the dock it actually ships in, rather than pushing it off its own edge", async () => {
    // Found by LOOKING at the first capture of this suite: the summary read "235 tessell", the third
    // class chip and the pager were off the right edge with the row's Frame control, and a horizontal
    // scrollbar had appeared under the dock. The Import workspace is a two-column split whose minima
    // were 280px + 320px = 601px, and the dock is not the window — it is the stage column, ~510px on
    // this 1296px window with both side docks open. The single-column fallback keys on a 760px WINDOW,
    // which is the wrong axis, so it never fired. This is that observation as an assertion.
    const fit = await browser.execute(() => {
      const split = document.querySelector(".mtk-bottom-workspace--split");
      const panel = document.querySelector("[data-testid='import-report']");
      if (!split || !panel) return null;
      return {
        splitClientWidth: split.clientWidth,
        splitScrollWidth: split.scrollWidth,
        panelWidth: Math.round(panel.getBoundingClientRect().width),
        panelRight: Math.round(panel.getBoundingClientRect().right),
        splitRight: Math.round(split.getBoundingClientRect().right),
      };
    });
    console.log("dock fit:", JSON.stringify(fit));
    if (!fit) throw new Error("the split workspace was not on screen");
    if (fit.splitScrollWidth > fit.splitClientWidth + 1) {
      throw new Error(`the Import workspace is ${fit.splitScrollWidth}px wide inside a ${fit.splitClientWidth}px dock — it scrolls sideways`);
    }
    if (fit.panelRight > fit.splitRight + 1) {
      throw new Error(`the report is painted ${fit.panelRight - fit.splitRight}px past the dock's right edge`);
    }
  });

  it("reaches a part past the first page — the thing the old panel could not do at all", async () => {
    const before = await panelState();
    await (await $("[data-testid='import-next']")).click();
    await browser.waitUntil(async () => (await panelState()).offset > 0, {
      timeout: 30000,
      timeoutMsg: "Next did not advance the page",
    });
    const after = await panelState();
    console.log("page 2:", JSON.stringify({ offset: after.offset, shown: after.shown, first: after.firstName }));
    if (after.offset !== 200) throw new Error(`expected offset 200, got ${after.offset}`);
    if (after.firstName === before.firstName) throw new Error("page 2 shows the same first row as page 1");
    if (after.prevDisabled !== false) throw new Error("Previous must be live on page 2");
    if (!after.showing.includes("Showing 201–")) throw new Error(`the sentence did not follow the page: ${after.showing}`);
    await shot("02-page-two");
    await (await $("[data-testid='import-prev']")).click();
    await browser.waitUntil(async () => (await panelState()).offset === 0, { timeout: 30000, timeoutMsg: "Previous did not go back" });
  });

  it("finds a named part by typing, and the counts follow the search", async () => {
    const all = await panelState();
    // A needle taken from the assembly ITSELF — the 201st row's own name, so the query is real content
    // rather than a word chosen to succeed.
    await (await $("[data-testid='import-next']")).click();
    await browser.waitUntil(async () => (await panelState()).offset === 200, { timeout: 30000, timeoutMsg: "no page 2" });
    const needleRow = (await panelState()).firstName ?? "";
    await (await $("[data-testid='import-prev']")).click();
    await browser.waitUntil(async () => (await panelState()).offset === 0, { timeout: 30000, timeoutMsg: "no page 1" });

    // The WHOLE name, not a prefix: CAD part names in a real assembly share long leading runs
    // ("Skid Weld Line A.1 - ..."), so a 12-character prefix can match every row and prove nothing.
    // Punctuation the key sender would have to escape is dropped rather than typed.
    const needle = needleRow.trim().replace(/[^A-Za-z0-9 ._-]/g, " ").replace(/\s+/g, " ").trim().slice(0, 28);
    if (needle.length < 4) throw new Error(`the assembly produced no usable part name (${JSON.stringify(needleRow)})`);
    await type(needle);
    await browser.waitUntil(async () => (await panelState()).total < all.total, {
      timeout: 30000,
      timeoutMsg: `typing "${needle}" never narrowed the report`,
    });
    const found = await panelState();
    console.log("search:", JSON.stringify({ needle, total: found.total, matched: found.matched, rows: found.rows, showing: found.showing }));
    if (found.rows === 0) throw new Error(`"${needle}" is a name from this very assembly and matched nothing`);
    if (found.matched !== found.total) throw new Error("no class filter is applied, so matched must equal total");
    if (!found.showing.includes(needle.slice(0, 6))) throw new Error(`the sentence does not name the search: ${found.showing}`);
    await shot("03-search-narrowed");

    // The 200-row page is now a handful of rows, and the row is REACHABLE — which is the point.
    if (found.rows > 200) throw new Error("a search result must still be a page");
  });

  it("says so when nothing matches, instead of showing a blank list", async () => {
    await clearSearch();
    await browser.waitUntil(async () => (await panelState()).rows > 1, { timeout: 30000, timeoutMsg: "clearing the search did not restore the list" });
    await type("zzzznotapart");
    await browser.waitUntil(async () => (await panelState()).empty === true, {
      timeout: 30000,
      timeoutMsg: "an empty result rendered no explanation at all — the ADR-163 defect",
    });
    const s = await panelState();
    console.log("empty:", JSON.stringify({ showing: s.showing, rows: s.rows }));
    if (s.rows !== 0) throw new Error("rows were rendered for a query that matched nothing");
    if (!s.showing.includes("zzzznotapart")) throw new Error(`the sentence does not name what was searched for: ${s.showing}`);
    await shot("04-nothing-matches");
    // And the way out is a control, not a suggestion.
    await (await $("[data-testid='import-clear']")).click();
    await browser.waitUntil(async () => (await panelState()).rows > 1, { timeout: 30000, timeoutMsg: "Clear did not restore the list" });
  });

  it("selecting a row moves the ENGINE's selection, and Frame moves the camera to it", async () => {
    const rows = await $$("[data-testid='import-row']");
    const target = rows[3];
    const id = await target.getAttribute("data-id");
    await (await target.$("[data-testid='import-select']")).click();
    await browser.waitUntil(async () => (await invoke("gizmo_selected")) === id, {
      timeout: 30000,
      timeoutMsg: "clicking a row left the engine's selection where it was — the cross-panel desync",
    });

    const before = await invoke("camera_debug").catch(() => null);
    await (await target.$("[data-testid='import-frame']")).click();
    await browser.pause(1200);
    const after = await invoke("camera_debug").catch(() => null);
    if (before && after && JSON.stringify(before) === JSON.stringify(after)) {
      throw new Error("Frame did not move the camera");
    }
    console.log("camera_debug before/after Frame:", JSON.stringify(before), JSON.stringify(after));
    await shot("05-framed-part");
  });

  it("the whole-scene report and the paged one agree about the assembly", async () => {
    const whole = await invoke("cad_report");
    const shown = await panelState();
    console.log("cad_report:", JSON.stringify({ total: whole.total, exact: whole.exactBrep, tess: whole.tessellationOnly, proxy: whole.proxy, failed: whole.failed, parts: whole.parts.length }));
    if (whole.total !== shown.total) throw new Error(`the panel says ${shown.total} and the shell says ${whole.total}`);
    if (whole.matched !== whole.total) throw new Error("an unfiltered report must have matched === total");
    if (whole.parts.length > whole.total) throw new Error("the page cannot be longer than the report");
  });
});
