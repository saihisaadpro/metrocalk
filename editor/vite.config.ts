import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Editor front-end. Tests run under jsdom (Vitest) with globals on, so the projection-store /
// transport / reconciliation logic and component render-counts are testable headlessly.
export default defineConfig({
  plugins: [react()],
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
  },
});
