import { browser } from "@wdio/globals";
import { execFileSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const dir = path.dirname(fileURLToPath(import.meta.url));
const shotDir = process.env.MTK_SHOT_DIR || path.resolve(dir, "../.shots-cadfix");
const capture = path.resolve(dir, "../../../.uxtest/audit/exe/capture-window-fg.ps1");
const step = "X:\\Work\\Metrocalk\\Games Projects\\Unreal\\Skid Weld Line A.1\\Skid Weld Line A.1_(1).stp";
const xml = "X:\\Work\\Metrocalk\\Games Projects\\Unreal\\Skid Weld Line A.1\\Skid Weld Line A.1.3dxml";
mkdirSync(shotDir, { recursive: true });

const invoke = (command, args = {}) =>
  browser.execute((c, a) => window.__TAURI__.core.invoke(c, a), command, args);

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

const shot = async (name) => {
  await browser.pause(700);
  execFileSync(
    "powershell",
    ["-ExecutionPolicy", "Bypass", "-File", capture, "-ProcName", "metrocalk-editor-shell", "-Out", path.join(shotDir, `${name}.png`)],
    { stdio: "ignore" },
  );
};

const assertResolved = (report, label) => {
  if (report.total < 10000) throw new Error(`${label}: only ${report.total} parts were reported`);
  if (report.proxy !== 0 || report.failed !== 0 || report.accessDenied !== 0) {
    throw new Error(`${label}: unresolved import classes ${JSON.stringify(report)}`);
  }
  if (report.exactBrep + report.tessellationOnly !== report.total) {
    throw new Error(`${label}: fidelity totals do not account for every part`);
  }
};

const importAndWait = async (source) => {
  const started = Date.now();
  await kick("import_asset", { path: source });
  await browser.waitUntil(async () => (await entityCount()) >= 10000, {
    timeout: 300000,
    interval: 1000,
    timeoutMsg: `${source} did not land in the scene`,
  });
  return (Date.now() - started) / 1000;
};

const thumbnailOf = async (id, size = 160) => {
  let url = null;
  await browser.waitUntil(
    async () => {
      url = await invoke("thumbnail", { id, size });
      return typeof url === "string";
    },
    { timeout: 15000, interval: 500, timeoutMsg: `thumbnail(${id}) never rendered` },
  );
  return url;
};

describe("real CAD pair — resolved geometry, hierarchy, framing, and honest reporting", () => {
  before(async () => {
    await browser.waitUntil(
      () => browser.execute(() => Boolean(window.__TAURI__?.core)),
      { timeout: 30000, timeoutMsg: "Tauri bridge never appeared" },
    );
    await browser.execute(() => localStorage.setItem("mtk.onboarded.v1", "1"));
    const layout = await browser.execute(() => {
      const root = document.getElementById("root")?.getBoundingClientRect();
      const shell = document.querySelector(".mtk-editor-root")?.getBoundingClientRect();
      return {
        dpr: window.devicePixelRatio,
        inner: [window.innerWidth, window.innerHeight],
        outer: [window.outerWidth, window.outerHeight],
        screen: [window.screen.width, window.screen.height],
        root: root ? [root.left, root.top, root.right, root.bottom] : null,
        shell: shell ? [shell.left, shell.top, shell.right, shell.bottom] : null,
      };
    });
    if (!layout.root || Math.abs(layout.root[0]) > 0.5 || Math.abs(layout.root[2] - layout.inner[0]) > 0.5) {
      throw new Error(`WebView root leaves an uncovered horizontal gutter: ${JSON.stringify(layout)}`);
    }
    console.log("render layout", JSON.stringify(layout));
  });

  it("resolves the proprietary 3DXML through its matching AP242 companion with no proxy blocks", async () => {
    const seconds = await importAndWait(xml);
    const report = await invoke("cad_report");
    assertResolved(report, "3DXML companion import");
    if (!report.parts.some((part) => part.sourceFormat?.includes("companion"))) {
      throw new Error("3DXML report did not disclose companion geometry resolution");
    }
    console.log(`3DXML landed ${report.total} parts in ${seconds.toFixed(1)}s`, JSON.stringify(report).slice(0, 300));

    await invoke("frame_all");
    await shot("01_3dxml_resolved_perspective");
    await invoke("view_preset", { preset: "top" });
    await shot("02_3dxml_resolved_top");
    await invoke("view_preset", { preset: "front" });
    await shot("03_3dxml_resolved_front");

    await kick("undo");
    await browser.waitUntil(async () => (await entityCount()) < 10, {
      timeout: 180000,
      interval: 1000,
      timeoutMsg: "one undo did not peel the complete 3DXML import",
    });
  });

  it("imports the AP242 source directly with the same resolved fidelity and usable names", async () => {
    const seconds = await importAndWait(step);
    const report = await invoke("cad_report");
    assertResolved(report, "STEP import");
    if (report.parts.some((part) => /^solid #\d+$/i.test(part.name))) {
      throw new Error("generic STEP entity numbers leaked into the reported component names");
    }
    console.log(`STEP landed ${report.total} parts in ${seconds.toFixed(1)}s`, JSON.stringify(report).slice(0, 300));

    await invoke("view_preset", { preset: "persp" });
    await invoke("frame_all");
    await shot("04_step_resolved_perspective");
    await invoke("view_preset", { preset: "side" });
    await shot("05_step_resolved_side");

    // A real millimetre-authored CAD body exercises the native portrait path that previously replaced its
    // 0.001 instance scale with 0.1 and produced clipped/blank thumbnails. Preserve the PNG as visual evidence
    // and assert its intrinsic dimensions without trusting CSS layout.
    const exactPart = report.parts.find((part) => part.fidelity === "exact-brep");
    const dataUrl = await thumbnailOf(exactPart.id, 160);
    const png = Buffer.from(dataUrl.split(",", 2)[1], "base64");
    if (!png.subarray(1, 4).equals(Buffer.from("PNG")) || png.readUInt32BE(16) !== 160 || png.readUInt32BE(20) !== 160) {
      throw new Error("CAD thumbnail is not a valid 160×160 PNG");
    }
    writeFileSync(path.join(shotDir, "06_step_exact_part_thumbnail.png"), png);
    if (png.length < 600) throw new Error(`CAD thumbnail is suspiciously empty (${png.length} bytes)`);
  });
});
