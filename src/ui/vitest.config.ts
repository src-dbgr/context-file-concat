import { defineConfig } from "vitest/config";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import path from "path";

const isTest = !!process.env.VITEST;

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
      all: true,
      include: [
        "src/lib/ipc/**/*.ts",
        "src/lib/utils.ts",
        "src/lib/modules/treeExpansion.ts",
        "src/lib/stores/app.ts",
        "src/lib/stores/toast.ts",
        "src/lib/modules/undo.ts",
        "src/lib/modules/clipboard.ts",
        // added to the Core gate:
        "src/lib/modules/keyboard.ts",
        "src/lib/modules/commands.ts",
        "src/lib/i18n/index.ts",
      ],
      exclude: ["**/*.d.ts"],
      thresholds: { lines: 80, statements: 80, functions: 80, branches: 70 },
      reportOnFailure: true,
    },
  },
  resolve: {
    conditions: ["browser", "svelte"],
    alias: {
      $lib: path.resolve(__dirname, "./src/lib"),
      ...(isTest
        ? {
            // map monaco-editor to local stub in vitest
            "monaco-editor": path.resolve(
              __dirname,
              "./src/tests/__mocks__/monaco-editor.ts"
            ),
          }
        : {}),
    },
  },
});
