import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Editor front-end. Tests run under jsdom (Vitest) with globals on, so the projection-store /
// transport / reconciliation logic and component render-counts are testable headlessly.
export default defineConfig(({ command }) => ({
  plugins: [react()],

  // MAY THIS BUNDLE CONSTRUCT A FAKE CORE? One flag, decided at build time, and the reason it is its
  // own flag rather than `import.meta.env.DEV` is that the shots harness disproved that shorthand
  // within a minute of it existing: the harness is a PRODUCTION build (`vite build`, so `DEV` is
  // false) that legitimately renders the whole shell against the MockCore, and every one of its 15
  // shell scenes went black with `NoCoreError` — a capture painting one distinct colour, which is the
  // gate doing its job. "React's production mode" and "there is a real engine behind this window" are
  // simply different facts, and only the second one may delete the mock.
  //
  // `command !== "build"` covers `vite dev` and Vitest (which resolves this config in serve mode);
  // `vite build` here is the app bundle that becomes the packaged shell's `frontendDist`, and that is
  // the one bundle in the repository that must not contain a core it can answer with. Substituting a
  // literal `false` is also what lets Rollup drop `MockClient`, `MockCore`, `DeltaClient` and the
  // sample scene: 76,736 bytes, a third of the old entry chunk.
  define: { __MTK_MOCK_CORE__: JSON.stringify(command !== "build") },

  build: {
    // Dynamic workspace imports keep the viewport shell immediate. Stable vendor boundaries prevent
    // graph and schema-form libraries from being folded back into the entry chunk.
    rollupOptions: {
      output: {
        manualChunks(id) {
          if (!id.includes("node_modules")) return undefined;
          if (id.includes("@xyflow")) return "vendor-graph";
          if (id.includes("@jsonforms")) return "vendor-forms";
          if (id.includes("zustand")) return "vendor-state";
          if (id.includes("react") || id.includes("scheduler")) return "vendor-react";
          return "vendor-support";
        },
      },
    },
  },
  test: {
    globals: true,
    environment: "jsdom",
    setupFiles: ["./src/test-setup.ts"],
    // Vitest's default per-test budget is 5 s, which is below the 15 s a single `waitFor` on a lazy
    // workspace is now allowed (see `test-setup.ts` for why that number exists) — leaving it would
    // just move the flake from the wait to the test around it. One number, one place; the shell
    // tests that used to carry their own `10_000` no longer state it a second time.
    testTimeout: 30_000,
  },
}));
