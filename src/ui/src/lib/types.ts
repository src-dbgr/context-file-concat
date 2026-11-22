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
 * Defines the configuration settings for the application.
 */
export interface Config {
  ignore_patterns: string[];
  case_sensitive_search: boolean;
  include_tree_by_default: boolean;
  use_relative_paths: boolean;
  remove_empty_directories: boolean;
  output_directory: string;
  output_filename: string;
}

/**
 * Represents the entire state of the frontend application.
 */
export interface AppState {
  is_scanning: boolean;
  is_generating: boolean;
  is_fully_scanned: boolean;
  patterns_need_rescan: boolean;
  tree: TreeNode[];
  current_path: string | null;
  current_config_filename: string | null;
  status_message: string;
  selected_files_count: number;
  /** Files in the complete filtered tree, including the ones not sent. */
  tree_files_count: number;
  /** Directories in the complete filtered tree, including the ones not sent. */
  tree_dirs_count: number;
  search_query: string;
  extension_filter: string;
  content_search_query: string;
  active_ignore_patterns: string[];
  config: Config;
}

/**
 * Represents the context of the currently focused UI element.
 * Used for determining which keyboard shortcuts should be active.
 */
export interface FocusContext {
  activeEl: HTMLElement | null;
  isEditorFocused: boolean;
  isInNormalInputField: boolean;
}
