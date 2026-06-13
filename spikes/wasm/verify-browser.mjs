// Headless WebGPU verification via the Chrome DevTools Protocol (no npm deps; Node 21+ global
// WebSocket/fetch). Launches a Chromium browser headless with WebGPU enabled, loads the served
// page, waits for the triangle to render, then reads:
//   - window.crossOriginIsolated and navigator.gpu
//   - globalThis.__spikelog (the wasm's adapter/limits/TTFF log)
//   - a PNG screenshot (proof of render)
// Usage: node verify-browser.mjs --browser "C:\path\chrome.exe" --url http://localhost:8080 --out chrome
import { spawn } from "node:child_process";
import { writeFileSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const args = Object.fromEntries(
  process.argv.slice(2).reduce((a, v, i, arr) => (v.startsWith("--") ? [...a, [v.slice(2), arr[i + 1]]] : a), [])
);
const BROWSER = args.browser;
const URL = args.url ?? "http://localhost:8080";
const OUT = args.out ?? "browser";
const PORT = 9222 + Math.floor(Math.random() * 500);
const profile = mkdtempSync(join(tmpdir(), "coi-verify-"));

const flags = [
  "--headless=new",
  "--no-sandbox",
  "--no-first-run",
  "--enable-unsafe-webgpu",
  "--enable-features=Vulkan,WebGPU",
  "--use-angle=default",
  "--window-size=520,520",
  "--hide-scrollbars",
  `--user-data-dir=${profile}`,
  `--remote-debugging-port=${PORT}`,
  URL,
];

const child = spawn(BROWSER, flags, { stdio: "ignore" });

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function cdp() {
  // find the page target's websocket
  let pageWs;
  for (let i = 0; i < 50; i++) {
    try {
      const list = await (await fetch(`http://127.0.0.1:${PORT}/json`)).json();
      const page = list.find((t) => t.type === "page" && t.webSocketDebuggerUrl);
      if (page) {
        pageWs = page.webSocketDebuggerUrl;
        break;
      }
    } catch {}
    await sleep(200);
  }
  if (!pageWs) throw new Error("no CDP page target — browser didn't expose a debugger");

  const ws = new WebSocket(pageWs);
  await new Promise((res, rej) => {
    ws.onopen = res;
    ws.onerror = (e) => rej(new Error("ws error"));
  });
  let id = 0;
  const pending = new Map();
  ws.onmessage = (m) => {
    const msg = JSON.parse(m.data);
    if (msg.id && pending.has(msg.id)) {
      pending.get(msg.id)(msg);
      pending.delete(msg.id);
    }
  };
  const send = (method, params = {}) =>
    new Promise((res) => {
      const myId = ++id;
      pending.set(myId, res);
      ws.send(JSON.stringify({ id: myId, method, params }));
    });

  const evalJs = async (expr) => {
    const r = await send("Runtime.evaluate", { expression: expr, returnByValue: true, awaitPromise: true });
    return r.result?.result?.value;
  };

  // Let the wasm boot + render several frames (real GPU init can take a couple seconds).
  await sleep(7000);

  const result = await evalJs(
    `(() => {
        const c = document.querySelector('canvas');
        let px = null, size = null, distinct = 0;
        if (c) {
          size = [c.width, c.height];
          try {
            // Draw the WebGPU canvas into a 2D canvas and sample pixels (render proof).
            const t = document.createElement('canvas');
            t.width = c.width; t.height = c.height;
            const g = t.getContext('2d');
            g.drawImage(c, 0, 0);
            const cx = c.width >> 1, cy = c.height >> 1;
            px = Array.from(g.getImageData(cx, cy, 1, 1).data);
            // count distinct colors over a coarse grid → >1 means something was drawn
            const seen = new Set();
            for (let y = 0; y < c.height; y += 16)
              for (let x = 0; x < c.width; x += 16) {
                const d = g.getImageData(x, y, 1, 1).data;
                seen.add(d[0] + ',' + d[1] + ',' + d[2]);
              }
            distinct = seen.size;
          } catch (e) { px = 'readback-failed: ' + e; }
        }
        return JSON.stringify({
          crossOriginIsolated: self.crossOriginIsolated,
          gpu: !!navigator.gpu,
          title: document.title,
          canvases: document.querySelectorAll('canvas').length,
          canvasSize: size,
          centerPixel: px,
          distinctColors: distinct,
          coiBanner: (document.getElementById('coi')||{}).textContent || null,
          overlay: (document.getElementById('overlay')||{}).textContent || null,
          fallback: (document.getElementById('fallback')||{}).textContent || null,
          log: (globalThis.__spikelog || '')
        });
     })()`
  );

  const shot = await send("Page.captureScreenshot", { format: "png" });
  if (shot.result?.data) {
    writeFileSync(`${OUT}.png`, Buffer.from(shot.result.data, "base64"));
  }
  ws.close();
  return JSON.parse(result);
}

try {
  const r = await cdp();
  console.log(`\n===== ${OUT} =====`);
  console.log("crossOriginIsolated:", r.crossOriginIsolated);
  console.log("navigator.gpu present:", r.gpu);
  console.log("canvas elements:", r.canvases, "size:", r.canvasSize);
  console.log("center pixel (rgba):", r.centerPixel, "distinct colors on grid:", r.distinctColors);
  console.log("COI banner:", r.coiBanner);
  console.log("overlay:", r.overlay);
  if (r.fallback) console.log("FALLBACK (error):", r.fallback);
  console.log("--- wasm log ---\n" + (r.log || "(empty)"));
  console.log(`--- screenshot written: ${OUT}.png ---`);
} catch (e) {
  console.error(`VERIFY FAILED (${OUT}):`, e.message);
} finally {
  child.kill();
  process.exit(0);
}
