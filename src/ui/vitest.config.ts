import { defineConfig } from "vitest/config";
import path from "path";

// Vitest v3 configuration for unit tests.
// - Node environment (no DOM)
// - Include both src/tests and src/**/__tests__ patterns
// - Mirrors the $lib alias used by Vite
export default defineConfig({
  test: {
    environment: "node",
    include: ["src/tests/**/*.test.ts", "src/**/__tests__/**/*.test.ts"],
    passWithNoTests: false,
    coverage: {
      provider: "v8",
      reporter: ["text", "html"],
      reportsDirectory: "coverage",
    },
  },
  resolve: {
    alias: {
      $lib: path.resolve(__dirname, "./src/lib"),
    },
  },
});
