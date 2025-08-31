// vitest.config.ts
import { defineConfig } from "vitest/config";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import path from "path";

export default defineConfig({
  plugins: [svelte()],
  test: {
    environment: "jsdom",
    setupFiles: ["src/tests/setup.ts"],
    include: ["src/tests/**/*.test.ts", "src/**/__tests__/**/*.test.ts"],
    passWithNoTests: false,
    coverage: {
      provider: "v8",
      reporter: ["text", "html"],
      reportsDirectory: "coverage",

      // ➜ Nur die Kernlogik in die Quote nehmen
      all: true,
      include: [
        "src/lib/ipc/**/*.ts", // Contracts/Schemas
        "src/lib/utils.ts", // reine Hilfsfunktionen (hat Tests)
        "src/lib/modules/treeExpansion.ts",
        "src/lib/stores/app.ts", // zentrale Store-Logik (hat Tests)
      ],
      exclude: ["**/*.d.ts"],

      thresholds: {
        lines: 80,
        statements: 80,
        functions: 80,
        branches: 70,
      },
      reportOnFailure: true,
    },
  },
  resolve: {
    // 👇 wichtig: so wird in Tests NICHT der server build von svelte genommen
    conditions: ["browser", "svelte"],
    alias: {
      $lib: path.resolve(__dirname, "./src/lib"),
    },
  },
});
