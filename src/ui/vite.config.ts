import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import path from "path";

/**
 * Where the e2e build goes. It contains the `window.__e2e` test hook, so it
 * must never land in `dist`, which is what the application ships.
 */
const E2E_OUT_DIR = "node_modules/.cfc-e2e-dist";

// https://vitejs.dev/config/
export default defineConfig(({ mode, command }) => {
  // The build Playwright tests run against (`npm run preview:e2e`).
  const e2e = mode === "e2e";
  return {
    plugins: [svelte()],
    define: {
      // Test hooks (`window.__e2e`) exist in the e2e build and on the dev
      // server only – not in a build of any other mode. A build-time
      // constant, so every other bundle has the code removed.
      __CFC_TEST_HOOKS__: JSON.stringify(e2e || command === "serve"),
    },
    base: "./", // relative URLs
    worker: { format: "iife" as const }, // <-- important for Safari/WKWebView stability
    server: {
      port: 1420,
      strictPort: true,
      hmr: {
        protocol: "ws",
        host: "localhost",
        port: 1421,
      },
    },
    // Add path alias for $lib
    resolve: {
      alias: {
        $lib: path.resolve(__dirname, "./src/lib"),
      },
    },
    // Configures the build output directory for clean integration
    build: {
      outDir: e2e ? E2E_OUT_DIR : "dist",
      emptyOutDir: true,
      rollupOptions: {
        // Name important dynamic chunks for stable caching
        output: {
          // No `manualChunks: { monaco: ["monaco-editor"] }` here: that adds
          // the package's full entry (all language services) to the build
          // whether it is imported or not. The editor is the chunk of the
          // dynamically imported `monacoBundle` module.
          //
          // Vite's helper for dynamic imports gets a chunk of its own.
          // Otherwise it sits in the entry chunk and every lazy chunk (the
          // editor, its grammars) imports the entry – a cycle, and a lazy
          // chunk that cannot load once the entry has failed.
          manualChunks: (id: string) =>
            id.includes("vite/preload-helper") ? "preload-helper" : undefined,
          entryFileNames: `assets/[name].js`,
          chunkFileNames: `assets/[name].js`,
          assetFileNames: `assets/[name].[ext]`,
        },
      },
    },
  };
});
