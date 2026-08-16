// UX review capture: the Terrain sub-engine with a REAL world in it.
//
// Every screenshot taken so far has been of the empty state — the first ten seconds of the product. This
// captures the surface an author actually lives in: a built world, its sections open, the readouts
// populated. WebDriver's own screenshot is used deliberately rather than the composited native capture:
// it cannot see the wgpu viewport (which comes out as a hole), but it renders every panel faithfully and
// can be called many times per session, and the panels are what this review is about.

import { mkdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const dir = path.dirname(fileURLToPath(import.meta.url));
const shots = path.resolve(dir, "../.shots-ux");
mkdirSync(shots, { recursive: true });

const shot = (name) => browser.saveScreenshot(path.join(shots, `${name}.png`));

/** Open a named section in the terrain panel, if it is not already open. */
async function openSection(id) {
  const head = await $(`[data-testid="terrain-section-${id}"]`);
  if (!(await head.isExisting())) return false;
  if ((await head.getAttribute("aria-expanded")) !== "true") {
    await head.click();
    await browser.pause(350);
  }
  return true;
}

describe("terrain UX — the surface an author lives in", () => {
  it("builds a world and shows the authoring surface", async () => {
    await invoke("new_project");
    await (await $('[data-testid="engine-terrain"]')).waitForExist({ timeout: 20000 });
    await (await $('[data-testid="engine-terrain"]')).click();
    await browser.pause(600);
    await shot("01-empty");

    // A real world, through the same command the panel's own button calls.
    const built = await invoke("terrain_describe", {
      text: "a 4 km eroded alpine valley with a river and dense conifer forest",
    });
    console.log(`[ux] built ok=${built.ok} ${built.message ?? ""}`);
    expect(built.ok).toBe(true);
    await browser.pause(4000);
    await shot("02-built");

    // The panel polls stats on an interval; give it one.
    await browser.pause(2500);
    await shot("03-settled");
  });

  it("shows every section of the authoring surface", async () => {
    for (const id of ["describe", "shape", "look", "life", "routes", "perf"]) {
      const opened = await openSection(id);
      console.log(`[ux] section ${id}: ${opened ? "opened" : "absent"}`);
      if (opened) await shot(`10-section-${id}`);
    }
  });

  it("reports what the world costs and what is wrong with it", async () => {
    const stats = await invoke("terrain_stats");
    console.log(
      `[ux] stats active=${stats.active} chunks=${stats.residentChunks} ` +
        `mb=${stats.totalMb?.toFixed?.(1)} problem=${JSON.stringify(stats.problem)}`,
    );
    // The whole point of the problem wire: it exists, and it is quiet when nothing is wrong.
    expect(typeof stats.problem).toBe("string");
    await shot("20-cost");
  });

  it("keeps the surface usable at a narrow window", async () => {
    const before = await browser.getWindowSize();
    await browser.setWindowSize(1100, before.height);
    await browser.pause(900);
    await shot("30-narrow");
    await browser.setWindowSize(before.width, before.height);
    await browser.pause(600);
  });
});
