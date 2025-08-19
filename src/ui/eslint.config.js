// ESLint 9 Flat Config – Svelte 5 + TypeScript (non-typed, CI-freundlich)
import js from "@eslint/js";
import tseslint from "typescript-eslint";
import svelte from "eslint-plugin-svelte";
import globals from "globals";

export default [
  // Global ignores
  {
    ignores: [
      "dist/**",
      "build/**",
      "node_modules/**",
      ".svelte-kit/**",
      ".vite/**",
      ".idea/**",
      ".vscode/**",
      // Konfigurationsdateien überspringen (verhindert ProjectService-Parsing)
      "eslint.config.js",
      "svelte.config.js",
    ],
  },

  // Base JS rules
  js.configs.recommended,

  // TypeScript – Not typed
  ...tseslint.configs.recommended,

  // Svelte Flat Configs
  ...svelte.configs["flat/recommended"],
  // Important: This preset ist an Array → spread!
  ...svelte.configs["flat/prettier"],

  // Projektweite Optionen/Regeln
  {
    languageOptions: {
      parserOptions: {
        ecmaVersion: "latest",
        sourceType: "module",
        extraFileExtensions: [".svelte"],
      },
      globals: { ...globals.browser, ...globals.node },
    },
    rules: {
      "no-debugger": "error",
      "@typescript-eslint/no-unused-vars": [
        "error",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_" },
      ],
      // Bis wir typed linting wieder aktivieren:
      "@typescript-eslint/consistent-type-imports": "off",
      "@typescript-eslint/triple-slash-reference": "off",
    },
  },

  // .svelte-spezifische Ergänzungen
  {
    files: ["**/*.svelte"],
    languageOptions: {
      // Der svelte-eslint-parser nutzt diesen Parser für <script lang="ts">
      parserOptions: { parser: tseslint.parser },
    },
    rules: {
      "svelte/valid-compile": ["error", { ignoreWarnings: false }],
    },
  },
];
