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
    // **THE PER-TEST BUDGET WAS MEASURING THE MACHINE.** Vitest defaults to 5 s per test, and four
    // `App` cases — the ones that mount the whole shell and then wait for a `React.lazy` panel — take
    // 7 to 16 s here whenever a sibling build holds the cores. They fail together, pass alone, and
    // fail identically on an untouched checkout, which is the definition of a flake rather than a
    // finding (`<test_and_ci_discipline>` 4).
    //
    // Raising the ceiling does not weaken any assertion: nothing in this suite claims a duration, and
    // the tests that DO measure interaction cost measure it directly rather than through a timeout. A
    // wait that genuinely never resolves still fails — it just fails saying what it was waiting for
    // instead of "timed out in 5000ms".
    testTimeout: 30_000,
    hookTimeout: 30_000,
  },
}));
