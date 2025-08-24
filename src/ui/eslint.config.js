// ESLint 9 Flat Config – Svelte 5 + TypeScript (non-typed, CI-friendly)
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
      // Skip config files (avoid unnecessary parser work)
      "eslint.config.js",
      "svelte.config.js",
    ],
  },

  // Base JS rules
  js.configs.recommended,

  // TypeScript – not typed (fast CI defaults)
  ...tseslint.configs.recommended,

  // Svelte Flat Configs
  ...svelte.configs["flat/recommended"],
  // Prettier compatibility
  ...svelte.configs["flat/prettier"],

  // Project-wide options/rules
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
      // Until typed linting is re-enabled:
      "@typescript-eslint/consistent-type-imports": "off",
      "@typescript-eslint/triple-slash-reference": "off",
    },
  },

  // Svelte-specific overrides
  {
    files: ["**/*.svelte"],
    languageOptions: {
      // svelte-eslint-parser uses this parser for <script lang="ts">
      parserOptions: { parser: tseslint.parser },
    },
    rules: {
      "svelte/valid-compile": ["error", { ignoreWarnings: false }],

      // ❌ Forbid legacy reactive statements ($:)
      // Use $derived/$effect instead.
      "no-restricted-syntax": [
        "error",
        {
          selector: "SvelteReactiveStatement",
          message:
            "Legacy reactive statements ($:) are forbidden. Use $derived/$effect.",
        },
      ],

      // ❌ In Svelte components do not import `get` from 'svelte/store'.
      // Read stores via $store or derive via $derived/$effect.
      "no-restricted-imports": [
        "error",
        {
          paths: [
            {
              name: "svelte/store",
              importNames: ["get"],
              message:
                "Do not use get() inside Svelte components. Use $store or $derived/$effect instead.",
            },
          ],
        },
      ],
    },
  },
];
