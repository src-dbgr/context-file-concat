import type { UiConfig, UiState } from "./ipc/schema";

/**
 * Defines the structure for a file or directory node in the tree.
 * This is the data contract between the Rust backend and the Svelte frontend.
 *
 * The tree is pruned to what is visible: only expanded directories carry their
 * children. Everything a collapsed row needs is part of the node itself.
 */
export interface TreeNode {
  path: string;
  name: string;
  is_directory: boolean;
  /** Authoritative expansion state, owned by the backend. */
  is_expanded: boolean;
  is_binary: boolean;
  is_match: boolean;
  is_previewed: boolean;
  /** For directories: derived from all descendants, also the ones not sent. */
  selection_state: "none" | "partial" | "full";
  /** Empty for files and for collapsed directories. */
  children: TreeNode[];
  size: number;
  /**
   * Whether the directory has children in the filtered list, even when they
   * are not part of `children` because it is collapsed.
   */
  has_children: boolean;
  /**
   * `false` for a directory whose children have not been scanned yet; they
   * are fetched with `loadDirectoryLevel`. Always `true` for files.
   */
  children_loaded: boolean;
}

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
