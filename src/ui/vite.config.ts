import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import path from "path";

/**
 * Where the e2e build goes. It contains the `window.__e2e` test hook, so it
 * must never land in `dist`, which is what the application ships.
 */
const E2E_OUT_DIR = "node_modules/.cfc-e2e-dist";

/**
 * The build Playwright tests run against: `--mode e2e`, or anything run by
 * the npm script `preview:e2e` (its `vite build` and its `vite preview`).
 */
function isE2EBuild(mode: string): boolean {
  return mode === "e2e" || process.env.npm_lifecycle_event === "preview:e2e";
}

// https://vitejs.dev/config/
export default defineConfig(({ mode }) => {
  const e2e = isE2EBuild(mode);
  return {
    plugins: [svelte()],
    define: {
      // Test hooks (`window.__e2e`) exist in dev and e2e builds only. A
      // build-time constant, so a production bundle has the code removed.
      __CFC_TEST_HOOKS__: JSON.stringify(e2e || mode !== "production"),
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
