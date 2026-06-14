// Headless WebGPU bench driver via the Chrome DevTools Protocol (no npm deps; Node 21+ globals).
// Launches a Chromium browser headless with WebGPU enabled, loads the served render spike with a
// `?n=&secs=` bench query, waits for the bench to finish, then reads:
//   - window.crossOriginIsolated / navigator.gpu
//   - globalThis.__benchresult (the frame-time table) and globalThis.__spikelog (adapter/limits)
//   - a PNG screenshot (render proof)
// Usage: node verify-browser.mjs --browser "C:\path\chrome.exe" --n 5000 --secs 20 --out chrome-5k
import { spawn } from "node:child_process";
import { writeFileSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const args = Object.fromEntries(
  process.argv.slice(2).reduce((a, v, i, arr) => (v.startsWith("--") ? [...a, [v.slice(2), arr[i + 1]]] : a), [])
);
const BROWSER = args.browser;
const N = Number(args.n ?? 5000);
const SECS = Number(args.secs ?? 20);
const PORT_HTTP = Number(args.port ?? 8080);
const OUT = args.out ?? `browser-${N}`;
const URL = args.url ?? `http://localhost:${PORT_HTTP}/?n=${N}&secs=${SECS}`;
const DBG = 9222 + Math.floor(Math.random() * 500);
const profile = mkdtempSync(join(tmpdir(), "render-verify-"));

const flags = [
  "--headless=new",
  "--no-sandbox",
  "--no-first-run",
  "--enable-unsafe-webgpu",
  "--enable-features=Vulkan,WebGPU",
  "--use-angle=default",
  "--window-size=980,620",
  "--hide-scrollbars",
  `--user-data-dir=${profile}`,
  `--remote-debugging-port=${DBG}`,
  URL,
];

const child = spawn(BROWSER, flags, { stdio: "ignore" });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function cdp() {
  let pageWs;
  for (let i = 0; i < 60; i++) {
    try {
      const list = await (await fetch(`http://127.0.0.1:${DBG}/json`)).json();
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
    ws.onerror = () => rej(new Error("ws error"));
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

  // Poll for the bench result (init + GPU warmup + SECS of measurement + margin).
  const deadline = Date.now() + (SECS + 25) * 1000;
  let bench = null;
  while (Date.now() < deadline) {
    bench = await evalJs("globalThis.__benchresult || null");
    if (bench) break;
    await sleep(1000);
  }

  const result = await evalJs(
    `(() => {
        const c = document.querySelector('canvas');
        let size = null, distinct = 0;
        if (c) {
          size = [c.width, c.height];
          try {
            const t = document.createElement('canvas');
            t.width = c.width; t.height = c.height;
            const g = t.getContext('2d');
            g.drawImage(c, 0, 0);
            const seen = new Set();
            for (let y = 0; y < c.height; y += 16)
              for (let x = 0; x < c.width; x += 16) {
                const d = g.getImageData(x, y, 1, 1).data;
                seen.add(d[0] + ',' + d[1] + ',' + d[2]);
              }
            distinct = seen.size;
          } catch (e) { distinct = -1; }
        }
        return JSON.stringify({
          crossOriginIsolated: self.crossOriginIsolated,
          gpu: !!navigator.gpu,
          canvasSize: size,
          distinctColors: distinct,
          benchresult: globalThis.__benchresult || null,
          fallback: (document.getElementById('fallback')||{}).textContent || null,
          log: (globalThis.__spikelog || '')
        });
     })()`
  );

  const shot = await send("Page.captureScreenshot", { format: "png" });
  if (shot.result?.data) writeFileSync(`${OUT}.png`, Buffer.from(shot.result.data, "base64"));
  ws.close();
  return JSON.parse(result);
}

try {
  const r = await cdp();
  console.log(`\n===== ${OUT} (n=${N}, secs=${SECS}) =====`);
  console.log("crossOriginIsolated:", r.crossOriginIsolated, "| navigator.gpu:", r.gpu);
  console.log("canvas:", r.canvasSize, "| distinct colors on grid:", r.distinctColors);
  if (r.fallback) console.log("FALLBACK (error):", r.fallback);
  console.log("BENCH:", r.benchresult || "(none — bench did not finish)");
  console.log("--- wasm log ---\n" + (r.log || "(empty)"));
  console.log(`--- screenshot: ${OUT}.png ---`);
} catch (e) {
  console.error(`VERIFY FAILED (${OUT}):`, e.message);
} finally {
  child.kill();
  process.exit(0);
}
