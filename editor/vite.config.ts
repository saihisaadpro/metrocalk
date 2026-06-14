import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Editor front-end. Tests run under jsdom (Vitest) with globals on, so the projection-store /
// transport / reconciliation logic and component render-counts are testable headlessly.
export default defineConfig({
  plugins: [react()],
  test: {
    globals: true,
    environment: "jsdom",
    setupFiles: ["./src/test-setup.ts"],
  },
});
