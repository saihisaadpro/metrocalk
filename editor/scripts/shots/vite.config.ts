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
  build: {
    outDir: "dist",
    emptyOutDir: true,
    rollupOptions: { input: `${__dirname}/harness.html` },
  },
});
