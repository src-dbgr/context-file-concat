// Strongly typed helpers for IPC contract tests (no `any`, no persisting `unknown`).
// These helpers are kept *framework-agnostic* and only import Zod schemas/types.

import type { z } from "zod";
import {
  AnyCommandMessageSchema,
  UiConfigSchema,
  UiStateSchema,
  type CommandName,
  type PayloadFor,
} from "$lib/ipc/schema";

/** The parsed/validated IPC command message type (output side of the union schema). */
export type IPCCommandMessage = {
  command: CommandName;
  payload: unknown | null;
};

/**
 * Payload type that the wire/protocol expects for a given command. There is no
 * difference between what `post()` accepts and what is sent anymore; the alias
 * is kept for the readability of the tests.
 */
export type PayloadForWire<T extends CommandName> = PayloadFor<T>;

/**
 * Validate a command + payload against the runtime Zod union.
 * Returns a fully typed object when valid; throws (ZodError) otherwise.
 */
export function validateCommand<T extends CommandName>(
  name: T,
  payload: PayloadForWire<T> | null,
): IPCCommandMessage {
  const parsed = AnyCommandMessageSchema.parse({
    command: name,
    payload,
  });
  return parsed as unknown as IPCCommandMessage;
}

/**
 * Build the config part of a snapshot as the host sends it (`UiConfigSchema`).
 */
export function makeWireConfig(
  overrides: Partial<z.input<typeof UiConfigSchema>> = {},
): z.input<typeof UiConfigSchema> {
  return {
    ignore_patterns: [],
    output_directory: null,
    output_filename: "output.txt",
    case_sensitive_search: false,
    include_tree_by_default: false,
    use_relative_paths: false,
    remove_empty_directories: false,
    ...overrides,
  };
}

/**
 * Build a minimal-yet-valid UiState payload for inbound event validation.
 */
export function makeWireUiState(): z.input<typeof UiStateSchema> {
  return {
    config: makeWireConfig(),
    current_path: "/repo",
    rows: [
      {
        path: "/repo",
        name: "repo",
        depth: 0,
        is_directory: true,
        is_expanded: true,
        is_binary: false,
        is_match: true,
        is_previewed: false,
        selection_state: "none",
        size: 0,
        has_children: false,
        children_loaded: true,
      },
    ],
    rows_start: 0,
    rows_total: 1,
    rows_revision: 1,
    located: null,
    total_files_found: 1,
    visible_files_count: 1,
    tree_files_count: 0,
    tree_dirs_count: 1,
    selected_files_count: 0,
    is_scanning: false,
    is_generating: false,
    is_fully_scanned: true,
    status_message: "Ready.",
    search_query: "",
    extension_filter: "",
    content_search_query: "",
    current_config_filename: null,
    scan_progress: {
      files_scanned: 0,
      large_files_skipped: 0,
      current_scanning_path: "Ready.",
    },
    active_ignore_patterns: [],
    patterns_need_rescan: false,
  };
}
