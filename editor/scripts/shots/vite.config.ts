import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// The visual-acceptance harness, built as a static bundle. `base: "./"` is what makes the assets
// resolve over `file://`, which is how `shoot.mjs` opens it — no dev server, no port, nothing to
// leave running. Its own config rather than a mode of the app's, so a change to the app's build
// cannot silently stop the harness from building.
export default defineConfig({
  root: __dirname,
  plugins: [react()],
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
