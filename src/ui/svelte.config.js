// Svelte-Config (Legacy-Reaktivität erlaubt, Runes erst in späterer Stufe aktivieren)
import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";

/** @type {import('svelte/compiler').Config} */
const config = {
  extensions: [".svelte"],
  preprocess: vitePreprocess(),
  compilerOptions: {
    // ⚠️ Runes vorerst AUS, weil der Code noch $:-Reaktivität nutzt.
    // Wird in einer späteren Migrationsstufe eingeschaltet.
    // runes: true
  },
};

export default config;
