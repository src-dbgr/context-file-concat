import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import path from "path";

// https://vitejs.dev/config/
export default defineConfig({
  plugins: [svelte()],
  // Ensures the dev server is accessible within the WebView
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
    outDir: "dist",
    rollupOptions: {
      output: {
        entryFileNames: `assets/[name].js`,
        chunkFileNames: `assets/[name].js`,
        assetFileNames: `assets/[name].[ext]`,
      },
    },
  },
});
