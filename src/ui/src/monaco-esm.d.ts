/**
 * Entry points of monaco-editor that ship without type declarations.
 * `edcore.main` is the editor API plus the editor contributions.
 */
declare module "monaco-editor/esm/vs/editor/edcore.main" {
  export * from "monaco-editor/esm/vs/editor/editor.api";
}

/** Registers the Monarch grammars; imported for its side effect only. */
declare module "monaco-editor/esm/vs/basic-languages/monaco.contribution";
