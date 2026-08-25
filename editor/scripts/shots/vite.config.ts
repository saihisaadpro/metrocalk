import { resolve } from "node:path";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { MANIFEST, buildManifest, ownedSource } from "./freshness.mjs";

const REPO_ROOT = resolve(__dirname, "..", "..", "..");

/**
 * Writes `dist/.sources.json` — the fingerprint of every source that went INTO this bundle, so that
 * `shoot.mjs --no-build` can refuse a bundle the tree has moved out from under. See
 * `freshness.mjs` for why this is written from rollup's module graph rather than from a directory
 * walk, and for what it deliberately cannot see.
 *
 * It runs in `generateBundle` and emits the manifest as an asset, so the file lands in `outDir` by
 * the same mechanism as the bundle and cannot end up beside a DIFFERENT build's output — which a
 * post-build `writeFileSync` could, on a machine where the two races.
 */
function sourceFingerprint() {
  return {
    name: "mtk-shots-source-fingerprint",
    generateBundle() {
      const ids = [...this.getModuleIds()];
      const excludedNodeModules = ids.filter(
        (id) => typeof id === "string" && id.split("?")[0].replace(/\\/g, "/").includes("/node_modules/"),
      ).length;
      const manifest = buildManifest({
        root: REPO_ROOT,
        moduleIds: ids,
        // Build INPUTS that are not graph modules. The config decides `define`/`base` and therefore
        // what every scene renders against; the HTML entry is pre-processed by vite rather than
        // imported; the lockfile is the only cheap statement of which node_modules produced this.
        // `freshness.mjs` is deliberately NOT here. It decides how the manifest is COMPUTED, not what
        // went into the bundle, and it was in this list for exactly one commit: the first edit to it
        // — a comment — refused a bundle that was perfectly fresh, which is the cry-wolf failure its
        // own ninth self-check exists to prevent. Its coverage rules are covered mechanically instead,
        // by the `recipe` digest of its marked region.
        extra: [
          resolve(__dirname, "vite.config.ts"),
          resolve(__dirname, "harness.html"),
          resolve(__dirname, "..", "..", "package-lock.json"),
        ],
        excludedNodeModules,
      });
      const owned = ids.filter((id) => ownedSource(id, REPO_ROOT)).length;
      this.info(
        `fingerprinted ${Object.keys(manifest.files).length} source file(s) ` +
          `(${owned} from the module graph, ${excludedNodeModules} node_modules module(s) excluded) → ${MANIFEST}`,
      );
      this.emitFile({ type: "asset", fileName: MANIFEST, source: `${JSON.stringify(manifest, null, 2)}\n` });
    },
  };
}

// The visual-acceptance harness, built as a static bundle. `base: "./"` is what makes the assets
// resolve over `file://`, which is how `shoot.mjs` opens it — no dev server, no port, nothing to
// leave running. Its own config rather than a mode of the app's, so a change to the app's build
// cannot silently stop the harness from building.
export default defineConfig({
  root: __dirname,
  plugins: [react(), sourceFingerprint()],
  base: "./",

  // The harness renders the REAL shell, and the real shell needs a core to render against. This is a
  // `vite build`, so the app config's `command !== "build"` would answer `false` here and every shell
  // scene would capture the `NoCoreError` surface instead of the editor — which is exactly what
  // happened the first time the flag existed, in 15 of 27 scenes. Stated explicitly rather than
  // inherited: this bundle is evidence, never a shipped artifact, and it may have a fake core.
  define: { __MTK_MOCK_CORE__: "true" },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    rollupOptions: { input: `${__dirname}/harness.html` },
  },
});
