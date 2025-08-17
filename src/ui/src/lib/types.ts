/**
 * Defines the structure for a file or directory node in the tree.
 * This is the data contract between the Rust backend and the Svelte frontend.
 */
export interface TreeNode {
  path: string;
  name: string;
  is_directory: boolean;
  is_expanded: boolean;
  is_binary: boolean;
  is_match: boolean;
  is_previewed: boolean;
  selection_state: "none" | "partial" | "full";
  children: TreeNode[];
  size: number;
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
