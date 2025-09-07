// Svelte config – Svelte 5 with Runes enabled (TypeScript + vitePreprocess)
import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";

/** @type {import('svelte/compiler').Config} */
const config = {
  extensions: [".svelte"],
  preprocess: vitePreprocess(),
  compilerOptions: {
    // ✅ Use the Svelte 5 Runes compiler
    runes: true,
  },
};

export default config;
