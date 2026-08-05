import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const dir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repository = path.resolve(dir, "../..");
const userProfile = process.env.USERPROFILE ?? "";
const application = process.env.MTK_EXE
  ? path.resolve(process.env.MTK_EXE)
  : path.resolve(dir, "../src-tauri/target/release/metrocalk-editor-shell.exe");

const requirements = [
  ["WebdriverIO CLI", path.join(dir, "node_modules/@wdio/cli/package.json")],
  ["axe WebdriverIO integration", path.join(dir, "node_modules/@axe-core/webdriverio/package.json")],
  ["packaged editor candidate", application],
  ["tauri-driver", path.join(userProfile, ".cargo/bin/tauri-driver.exe")],
  ["matching Edge driver", path.join(dir, ".driver/msedgedriver.exe")],
];
const candidate = fs.existsSync(application)
  ? {
      bytes: fs.statSync(application).size,
      sha256: createHash("sha256").update(fs.readFileSync(application)).digest("hex"),
    }
  : null;

const missing = requirements.filter(([, requiredPath]) => !fs.existsSync(requiredPath));
if (missing.length > 0) {
  const details = missing.map(([label, requiredPath]) => `  - ${label}: ${requiredPath}`).join("\n");
  console.error(
    [
      "Accessibility preflight failed. Missing release-test prerequisites:",
      details,
      "",
      ...(candidate ? [`Candidate present but not tested: sha256:${candidate.sha256}; ${candidate.bytes} bytes.`, ""] : []),
      "On a connected build machine run `node bootstrap.mjs` once. A later disconnected run is fully local",
      "because the pinned packages, matching driver, and packaged .exe are then present. Do not replace axe",
      "with a smaller DOM heuristic and call it WCAG certification.",
    ].join("\n"),
  );
  process.exit(1);
}

const driverCheck = spawnSync(process.execPath, [path.join(dir, "bootstrap.mjs"), "--check"], {
  cwd: dir,
  encoding: "utf8",
  windowsHide: true,
});
if (driverCheck.status !== 0) {
  process.stderr.write(driverCheck.stderr || driverCheck.stdout || "Driver compatibility check failed.\n");
  process.exit(driverCheck.status ?? 1);
}

const packageJson = JSON.parse(fs.readFileSync(path.join(dir, "node_modules/@axe-core/webdriverio/package.json"), "utf8"));
const harnessPackage = JSON.parse(fs.readFileSync(path.join(dir, "package.json"), "utf8"));
const pinnedAxeVersion = harnessPackage.devDependencies?.["@axe-core/webdriverio"];
if (!pinnedAxeVersion || pinnedAxeVersion !== packageJson.version) {
  console.error(`Accessibility preflight failed. Expected pinned axe ${pinnedAxeVersion ?? "<missing>"}, found ${packageJson.version}. Run npm ci; do not test with an unrecorded scanner version.`);
  process.exit(1);
}
const applicationBytes = candidate?.bytes ?? fs.statSync(application).size;
if (applicationBytes <= 0) {
  console.error(`Accessibility preflight failed. Packaged candidate is empty: ${application}`);
  process.exit(1);
}
const applicationSha256 = candidate?.sha256 ?? createHash("sha256").update(fs.readFileSync(application)).digest("hex");
console.log(
  `accessibility preflight passed: axe ${packageJson.version}; candidate sha256:${applicationSha256}; ${applicationBytes} bytes; ${path.relative(repository, application)}`,
);
