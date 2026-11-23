// src/ui/src/tests/contracts/factories.ts
// Typed, deterministic factories for Zod contract tests (no external deps)

import type { z } from "zod";
import {
  UiConfigSchema,
  TreeRowSchema,
  UiStateSchema,
  type CommandName,
} from "$lib/ipc/schema";

/** Valid path sample (POSIX-like) */
export const VALID_PATH = "/repo/src/index.ts";

/** Build a fully valid config as the host sends it (UiConfigSchema). */
export function makeValidConfig(): z.input<typeof UiConfigSchema> {
  return {
    ignore_patterns: [],
    output_directory: null,
    output_filename: "output.txt",
    case_sensitive_search: false,
    include_tree_by_default: false,
    use_relative_paths: false,
    remove_empty_directories: false,
  };
}

/** Minimal row of a file that satisfies the runtime schema. */
export function makeValidTreeRow(
  path = "/repo/README.md",
  overrides: Partial<z.input<typeof TreeRowSchema>> = {},
) {
  // Let Zod derive the correct output shape; we construct an input matching its requirements.
  const input: z.input<typeof TreeRowSchema> = {
    path,
    name: path.split(/[\\/]/).pop() ?? path,
    depth: 0,
    is_directory: false,
    is_expanded: false,
    is_binary: false,
    is_match: true,
    is_previewed: false,
    selection_state: "none",
    size: 256,
    has_children: false,
    children_loaded: true,
    ...overrides,
  };
  // Ensure it conforms – throws if invalid
  return TreeRowSchema.parse(input);
}

/** Valid rows: one expanded directory and its two files. */
export function makeValidRows() {
  return [
    makeValidTreeRow("/repo/src", {
      is_directory: true,
      is_expanded: true,
      size: 0,
      has_children: true,
    }),
    makeValidTreeRow("/repo/src/index.ts", { depth: 1 }),
    makeValidTreeRow("/repo/src/util.ts", { depth: 1 }),
  ];
}

/**
 * The row of a collapsed directory: its children are no rows, the flags say
 * what the row needs to render and to request them.
 */
export function makeCollapsedDirRow(
  path = "/repo/docs",
  overrides: Partial<z.input<typeof TreeRowSchema>> = {},
) {
  return makeValidTreeRow(path, {
    is_directory: true,
    is_match: false,
    size: 0,
    has_children: true,
    ...overrides,
  });
}

/** Fully valid UiState payload per UiStateSchema. */
export function makeValidUiState(): z.input<typeof UiStateSchema> {
  return {
    config: makeValidConfig(),
    current_path: "/repo",
    rows: makeValidRows(),
    rows_start: 0,
    rows_total: 3,
    rows_revision: 1,
    located: null,
    total_files_found: 3,
    visible_files_count: 3,
    tree_files_count: 2,
    tree_dirs_count: 1,
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
    case "cancelGeneration":
    case "clearPreviewState":
    case "pickOutputDirectory":
    case "exportConfig":
    case "importConfig":
    case "clearIgnorePatterns":
      return null;

    case "loadDirectoryLevel":
    case "loadFilePreview":
    case "toggleSelection":
    case "toggleDirectorySelection":
    case "addIgnorePath":
      return VALID_PATH;

    case "setExpansion":
      return { path: VALID_PATH, expanded: true };

    case "expandCollapseAll":
      return true;

    case "setViewport":
      return { start: 0, count: 90 };

    case "locateRow":
      return { request: 1, target: { kind: "path", path: VALID_PATH } };

    case "saveFile":
      return { kind: "edited", content: "// content to save" };

    case "updateFilters":
      // Provide empty object to test default injection
      return {};

    case "addIgnorePattern":
    case "removeIgnorePattern":
      return { pattern: "*.log" };

    case "setConfigOption":
      return { option: "case_sensitive_search", value: true };
  }
}
