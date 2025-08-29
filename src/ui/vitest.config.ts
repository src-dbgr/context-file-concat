// context-file-concat/src/ui/vitest.config.ts
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
