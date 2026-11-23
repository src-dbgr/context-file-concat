import type { UiConfig, UiState } from "./ipc/schema";

/**
 * One row of the file tree and the answer to a row lookup, as the host sends
 * them (see `ipc/schema.ts`).
 */
export type { TreeRow, LocatedRow } from "./ipc/schema";

/**
 * The configuration and the application state as the host sends them. Both are
 * derived from the runtime schemas in `ipc/schema.ts`, the single definition
 * of the wire shapes, so the types cannot drift from what is validated.
 */
export type Config = UiConfig;
export type AppState = UiState;

/**
 * Represents the context of the currently focused UI element.
 * Used for determining which keyboard shortcuts should be active.
 */
export interface FocusContext {
  activeEl: HTMLElement | null;
  isEditorFocused: boolean;
  isInNormalInputField: boolean;
}
