/**
 * Loads Monaco on first use. The dynamic import below is what splits the
 * editor (see `monacoBundle`) off the initial bundle.
 */

export type MonacoNS = typeof import("monaco-editor/esm/vs/editor/editor.api");

let pending: Promise<MonacoNS> | null = null;

/** The Monaco namespace; loaded once, a failed load can be retried. */
export function loadMonaco(): Promise<MonacoNS> {
  pending ??= import("./monacoBundle")
    .then((bundle) => bundle.monaco)
    .catch((err: unknown) => {
      pending = null;
      throw err;
    });
  return pending;
}
