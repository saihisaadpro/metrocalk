// Self-installing bootstrap (deliverable 1) — takes a fresh checkout to a runnable acceptance harness with
// one idempotent command, installing online what's missing:
//
//   node bootstrap.mjs          # install/verify everything, then print READY
//   node bootstrap.mjs --check  # ONLY verify the msedgedriver ↔ WebView2 match (the mandatory gate) and exit
//   node bootstrap.mjs --driver-only # install/verify only the matching browser driver
//
//  1. detect the installed WebView2 runtime version → download the EXACTLY-matching msedgedriver into
//     ./.driver/ (a mismatch HANGS the session — the match is mandatory and verified here);
//  2. cargo install tauri-driver --locked;
//  3. npm install (in this dir);
//  4. cargo build --release with Tauri's custom-protocol asset embedding enabled.
//
// Re-running is safe: each step is skipped when its artifact is present + matched. Windows-only (Tauri
// WebDriver + WebView2); the web-content fetch restriction does not apply to package managers / the driver.

import { execSync, spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const dir = path.dirname(fileURLToPath(import.meta.url));
const driverDir = path.join(dir, ".driver");
const driverExe = path.join(driverDir, "msedgedriver.exe");
const appExe = path.resolve(dir, "../src-tauri/target/release/metrocalk-editor-shell.exe");
const frontendDir = path.resolve(dir, "../../editor");
const log = (m) => console.log(`[bootstrap] ${m}`);
const fail = (m) => {
  console.error(`[bootstrap] FAIL: ${m}`);
  process.exit(1);
};

// WebView2 Evergreen runtime version from the registry (the client GUID is Microsoft's fixed Edge WebView2).
function webview2Version() {
  // -ErrorAction SilentlyContinue → a missing key/property yields empty output (clean null + our own
  // message), not a raw PowerShell error dump. Try the per-user path too (some installs register there).
  const key = "Microsoft\\EdgeUpdate\\Clients\\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
  const ps = [
    `$p=(Get-ItemProperty -LiteralPath 'HKLM:\\SOFTWARE\\WOW6432Node\\${key}' -ErrorAction SilentlyContinue).pv`,
    `if(-not $p){$p=(Get-ItemProperty -LiteralPath 'HKLM:\\SOFTWARE\\${key}' -ErrorAction SilentlyContinue).pv}`,
    `if(-not $p){$p=(Get-ItemProperty -LiteralPath 'HKCU:\\SOFTWARE\\${key}' -ErrorAction SilentlyContinue).pv}`,
    // Registry-less/per-machine Evergreen installs still expose their version as the application folder.
    `if(-not $p){$p=(Get-ChildItem -LiteralPath 'C:\\Program Files (x86)\\Microsoft\\EdgeWebView\\Application' -Directory -ErrorAction SilentlyContinue | Where-Object Name -Match '^\\d+\\.\\d+\\.\\d+\\.\\d+$' | Sort-Object {[version]$_.Name} -Descending | Select-Object -First 1 -ExpandProperty Name)}`,
    `if($p){$p}`,
  ].join(";");
  try {
    // Pass the script as argv. Going through cmd.exe corrupts `$p`/quotes and used to make every healthy
    // Evergreen install look missing.
    const result = spawnSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", ps], {
      encoding: "utf8",
      windowsHide: true,
    });
    const out = (result.stdout || "").trim();
    return out || null;
  } catch {
    return null;
  }
}

function driverVersion() {
  if (!fs.existsSync(driverExe)) return null;
  const r = spawnSync(driverExe, ["--version"], { encoding: "utf8" });
  // "Microsoft Edge WebDriver 149.0.xxxx.x (...)" → the version token.
  const m = (r.stdout || "").match(/(\d+\.\d+\.\d+\.\d+)/);
  return m ? m[1] : null;
}

// The match rule: the driver's MAJOR must equal the WebView2 runtime's major (Edge guarantees driver↔runtime
// compatibility within a major; an exact full-version match is best but the major is the hang-or-not line).
function matches(driverV, runtimeV) {
  if (!driverV || !runtimeV) return false;
  return driverV.split(".")[0] === runtimeV.split(".")[0];
}

function downloadDriver(version) {
  fs.mkdirSync(driverDir, { recursive: true });
  const url = `https://msedgedriver.microsoft.com/${version}/edgedriver_win64.zip`;
  const zip = path.join(driverDir, "edgedriver.zip");
  log(`downloading msedgedriver ${version} …`);
  const ps = `Invoke-WebRequest '${url}' -OutFile '${zip}'; Expand-Archive -Force '${zip}' '${driverDir}'; Remove-Item -LiteralPath '${zip}' -Force`;
  const result = spawnSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", ps], {
    stdio: "inherit",
    windowsHide: true,
  });
  if (result.status !== 0) fail(`could not download/extract ${url}`);
  if (!fs.existsSync(driverExe)) fail("driver extracted but msedgedriver.exe is missing");
}

function verifyDriverMatch() {
  const rt = webview2Version();
  if (!rt) fail("could not read the WebView2 runtime version (is the Evergreen runtime installed?)");
  const dv = driverVersion();
  if (!matches(dv, rt)) {
    fail(`msedgedriver (${dv ?? "absent"}) does NOT match WebView2 runtime (${rt}) — the session would hang. Run without --check to download the match.`);
  }
  log(`driver ↔ WebView2 match OK (driver ${dv}, runtime ${rt})`);
}

function ensureDriver() {
  const rt = webview2Version();
  if (!rt) fail("could not read the WebView2 runtime version (is the Evergreen runtime installed?)");
  if (matches(driverVersion(), rt)) {
    log(`msedgedriver already matches WebView2 ${rt} — skip`);
    return;
  }
  downloadDriver(rt);
  if (!matches(driverVersion(), rt)) fail("downloaded driver still does not match the runtime");
  log("msedgedriver installed + matched");
}

function ensureTauriDriver() {
  const home = process.env.USERPROFILE || process.env.HOME;
  if (home && fs.existsSync(path.join(home, ".cargo", "bin", "tauri-driver.exe"))) {
    log("tauri-driver already installed — skip");
    return;
  }
  log("cargo install tauri-driver --locked …");
  execSync("cargo install tauri-driver --locked", { stdio: "inherit" });
}

function ensureNpm() {
  const required = [
    path.join(dir, "node_modules", "@wdio", "cli", "package.json"),
    path.join(dir, "node_modules", "@axe-core", "webdriverio", "package.json"),
  ];
  if (required.every((dependency) => fs.existsSync(dependency))) {
    log("npm deps already installed — skip");
    return;
  }
  log("npm ci — restore the lockfile-pinned WebDriver + axe accessibility toolchain …");
  execSync("npm ci", { cwd: dir, stdio: "inherit" });
}

function ensureFrontendNpm() {
  if (fs.existsSync(path.join(frontendDir, "node_modules", "vite"))) {
    log("editor frontend deps already installed — skip");
    return;
  }
  log("npm install (editor frontend) …");
  execSync("npm install", { cwd: frontendDir, stdio: "inherit" });
}

function buildApp() {
  // Tauri's frontendDist embeds editor/dist at compile time. Cargo alone can otherwise package stale UI
  // while the harness claims it tested the current source tree.
  log("npm run build (the current editor UI) …");
  execSync("npm run build", { cwd: frontendDir, stdio: "inherit" });
  log("cargo build --release + tauri/custom-protocol (embed the app under test) …");
  execSync(`cargo build --release --features tauri/custom-protocol --manifest-path "${path.resolve(dir, "../src-tauri/Cargo.toml")}"`, {
    stdio: "inherit",
  });
  if (!fs.existsSync(appExe)) fail("release build finished but the app .exe is missing");
}

function main() {
  if (process.argv.includes("--check")) {
    verifyDriverMatch();
    log("CHECK OK");
    return;
  }
  ensureDriver();
  if (process.argv.includes("--driver-only")) {
    verifyDriverMatch();
    log("DRIVER READY");
    return;
  }
  ensureTauriDriver();
  ensureNpm();
  ensureFrontendNpm();
  buildApp();
  verifyDriverMatch(); // final guard — never leave a mismatched driver that would hang the run
  log("READY — run: node \"node_modules\\@wdio\\cli\\bin\\wdio.js\" run wdio.conf.js");
}

main();
