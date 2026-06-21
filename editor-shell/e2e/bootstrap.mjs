// Self-installing bootstrap (deliverable 1) — takes a fresh checkout to a runnable acceptance harness with
// one idempotent command, installing online what's missing:
//
//   node bootstrap.mjs          # install/verify everything, then print READY
//   node bootstrap.mjs --check  # ONLY verify the msedgedriver ↔ WebView2 match (the mandatory gate) and exit
//
//  1. detect the installed WebView2 runtime version → download the EXACTLY-matching msedgedriver into
//     ./.driver/ (a mismatch HANGS the session — the match is mandatory and verified here);
//  2. cargo install tauri-driver --locked;
//  3. npm install (in this dir);
//  4. cargo build --release the app under test.
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
  const ps =
    `$p=(Get-ItemProperty "HKLM:\\SOFTWARE\\WOW6432Node\\${key}" -ErrorAction SilentlyContinue).pv;` +
    `if(-not $p){$p=(Get-ItemProperty "HKCU:\\SOFTWARE\\${key}" -ErrorAction SilentlyContinue).pv};` +
    `if($p){$p}`;
  try {
    const out = execSync(`powershell -NoProfile -Command "${ps}"`, { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
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
  execSync(`powershell -NoProfile -Command "Invoke-WebRequest '${url}' -OutFile '${zip}'"`, { stdio: "inherit" });
  execSync(`powershell -NoProfile -Command "Expand-Archive -Force '${zip}' '${driverDir}'"`, { stdio: "inherit" });
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
  if (fs.existsSync(path.join(dir, "node_modules", "@wdio", "cli"))) {
    log("npm deps already installed — skip");
    return;
  }
  log("npm install …");
  execSync("npm install", { cwd: dir, stdio: "inherit" });
}

function buildApp() {
  log("cargo build --release (the app under test) …");
  execSync(`cargo build --release --manifest-path "${path.resolve(dir, "../src-tauri/Cargo.toml")}"`, {
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
  ensureTauriDriver();
  ensureNpm();
  buildApp();
  verifyDriverMatch(); // final guard — never leave a mismatched driver that would hang the run
  log("READY — run: node \"node_modules\\@wdio\\cli\\bin\\wdio.js\" run wdio.conf.js");
}

main();
