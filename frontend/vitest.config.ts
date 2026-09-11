import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// Frontend unit tests. The app itself has no test-only build concerns — this
// exists so the shared control primitives (timing/coalescing logic that every
// surface depends on) are verified rather than eyeballed.
export default defineConfig({
  plugins: [react()],
  test: {
    environment: "jsdom",
    setupFiles: ["src/test-setup.ts"],
    include: ["src/**/*.test.ts", "src/**/*.test.tsx"],
  },
});
