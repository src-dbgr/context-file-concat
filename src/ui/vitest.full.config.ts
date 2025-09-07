// context-file-concat/src/ui/vitest.full.config.ts
import { defineConfig } from "vitest/config";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import path from "node:path";

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
      // eigener Ordner, damit es sich nicht mit "core" mischt
      reportsDirectory: "coverage-full",
      // Full-Modus: alle Dateien in "src/**" in die Messung aufnehmen,
      // auch wenn sie im Testlauf nicht importiert werden:
      all: true,
      include: ["src/**"],
      // Keine Schwellen -> Full-Report schlägt nicht fehl
      thresholds: undefined as unknown as never,
    },
  },
  resolve: {
    // wichtig: in Tests NICHT den Server-Build von Svelte nehmen
    conditions: ["browser", "svelte"],
    alias: {
      $lib: path.resolve(__dirname, "./src/lib"),
    },
  },
});
