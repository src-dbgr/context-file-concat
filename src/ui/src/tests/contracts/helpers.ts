// Strongly typed helpers for IPC contract tests (no `any`, no persisting `unknown`).
// These helpers are kept *framework-agnostic* and only import Zod schemas/types.

import type { z } from "zod";
import {
  AnyCommandMessageSchema,
  ConfigSchema,
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
 * Payload type that matches what the wire/protocol expects for a given command.
 * For most commands this equals `PayloadFor<T>`, except `updateConfig` where the
 * runtime schema expects the full `ConfigSchema` shape.
 */
export type PayloadForWire<T extends CommandName> = T extends "updateConfig"
  ? z.input<typeof ConfigSchema>
  : PayloadFor<T>;

/**
 * Validate a command + payload against the runtime Zod union.
 * Returns a fully typed object when valid; throws (ZodError) otherwise.
 */
export function validateCommand<T extends CommandName>(
  name: T,
  payload: PayloadForWire<T> | null
): IPCCommandMessage {
  const parsed = AnyCommandMessageSchema.parse({
    command: name,
    payload,
  });
  return parsed as unknown as IPCCommandMessage;
}

/**
 * Build a fully valid config payload for `updateConfig` according to the *wire* schema.
 * Note: This uses the *runtime* contract (ConfigSchema), not the UI's narrowed Config type.
 */
export function makeWireConfig(
  overrides: Partial<z.input<typeof ConfigSchema>> = {}
): z.input<typeof ConfigSchema> {
  return {
    ignore_patterns: [],
    tree_ignore_patterns: [],
    last_directory: null,
    output_directory: null,
    output_filename: "output.txt",
    case_sensitive_search: false,
    include_tree_by_default: false,
    use_relative_paths: false,
    remove_empty_directories: false,
    window_size: [1280, 800],
    window_position: [100, 100],
    auto_load_last_directory: false,
    max_file_size_mb: 20,
    scan_chunk_size: 100,
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
    tree: [
      {
        path: "/repo",
        name: "repo",
        is_directory: true,
        is_expanded: true,
        is_binary: false,
        is_match: true,
        is_previewed: false,
        selection_state: "none",
        size: 0,
        children: [],
        // children_loaded is optional on the UI side; omitted here.
      },
    ],
    total_files_found: 1,
    visible_files_count: 1,
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
