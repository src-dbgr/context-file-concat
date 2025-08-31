// src/ui/src/tests/contracts/factories.ts
// Typed, deterministic factories for Zod contract tests (no external deps)

import type { z } from "zod";
import {
  ConfigSchema,
  TreeNodeSchema,
  UiStateSchema,
  type CommandName,
} from "$lib/ipc/schema";

/** Valid path sample (POSIX-like) */
export const VALID_PATH = "/repo/src/index.ts";

/** Build a fully valid Config payload per ConfigSchema (backend-facing, richer than frontend's Config). */
export function makeValidConfig(): z.input<typeof ConfigSchema> {
  return {
    ignore_patterns: [],
    tree_ignore_patterns: [], // default exists, but provide explicitly for clarity
    last_directory: null,
    output_directory: "",
    output_filename: "output.txt",
    case_sensitive_search: false,
    include_tree_by_default: false,
    use_relative_paths: false,
    remove_empty_directories: false,
    window_size: [1280, 800],
    window_position: [100, 100],
    auto_load_last_directory: false,
    max_file_size_mb: 20,
    scan_chunk_size: 1024,
  };
}

/** Minimal TreeNode that still satisfies runtime schema. */
export function makeValidTreeNode(path = "/repo/README.md") {
  // Let Zod derive the correct output shape; we construct an input matching its requirements.
  const input: z.input<typeof TreeNodeSchema> = {
    path,
    name: path.split(/[\\/]/).pop() ?? path,
    is_directory: false,
    is_expanded: false,
    is_binary: false,
    is_match: true,
    is_previewed: false,
    selection_state: "none",
    children: [],
    size: 256,
    // children_loaded is optional in UI schema
  };
  // Ensure it conforms – throws if invalid
  return TreeNodeSchema.parse(input);
}

/** Valid UI tree: one directory with two files. */
export function makeValidTree() {
  const dir = {
    path: "/repo/src",
    name: "src",
    is_directory: true,
    is_expanded: true,
    is_binary: false,
    is_match: true,
    is_previewed: false,
    selection_state: "none",
    size: 0,
    children: [
      makeValidTreeNode("/repo/src/index.ts"),
      makeValidTreeNode("/repo/src/util.ts"),
    ],
  } as const;

  // Coerce via schema to ensure shape
  return [TreeNodeSchema.parse(dir)];
}

/** Fully valid UiState payload per UiStateSchema. */
export function makeValidUiState(): z.input<typeof UiStateSchema> {
  return {
    config: makeValidConfig(),
    current_path: "/repo",
    tree: makeValidTree(),
    total_files_found: 3,
    visible_files_count: 3,
    selected_files_count: 1,
    is_scanning: false,
    is_generating: false,
    is_fully_scanned: true,
    status_message: "Ready.",
    search_query: "",
    extension_filter: "",
    content_search_query: "",
    current_config_filename: null,
    scan_progress: {
      files_scanned: 42,
      large_files_skipped: 0,
      current_scanning_path: "Ready.",
    },
    active_ignore_patterns: [],
    patterns_need_rescan: false,
  };
}

/** Returns a valid command payload for a given command name. */
export function makeValidPayloadForCommand(name: CommandName): unknown {
  switch (name) {
    case "selectDirectory":
    case "rescanDirectory":
    case "generatePreview":
    case "clearDirectory":
    case "cancelScan":
    case "initialize":
    case "selectAll":
    case "deselectAll":
    case "expandAllFully":
    case "selectAllFully":
    case "cancelGeneration":
    case "clearPreviewState":
    case "pickOutputDirectory":
    case "exportConfig":
    case "importConfig":
      return null;

    case "loadDirectoryLevel":
    case "loadFilePreview":
    case "toggleSelection":
    case "toggleDirectorySelection":
    case "toggleExpansion":
    case "addIgnorePath":
      return VALID_PATH;

    case "expandCollapseAll":
      return true;

    case "saveFile":
      return "// content to save";

    case "updateFilters":
      // Provide empty object to test default injection
      return {};

    case "updateConfig":
      return makeValidConfig();
  }
}
