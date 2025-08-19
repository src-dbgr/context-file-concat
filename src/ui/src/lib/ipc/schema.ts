// Runtime schemas for IPC payloads and app state (Zod) + typed Command map.
// Keep this file framework-agnostic; only data shapes + type utilities live here.

import { z } from "zod";

/* ---------------------------------- Common --------------------------------- */
export const PathString = z.string().min(1);
/** (number, number) tuple */
const Tuple2 = z.tuple([z.number(), z.number()]);

/* --------------------------------- Config ---------------------------------- */
export const ConfigSchema = z
  .object({
    ignore_patterns: z.array(z.string()),
    tree_ignore_patterns: z.array(z.string()).default([]),
    last_directory: z.string().nullable().optional(),
    output_directory: z.string().nullable().optional(),
    output_filename: z.string(),
    case_sensitive_search: z.boolean(),
    include_tree_by_default: z.boolean(),
    use_relative_paths: z.boolean(),
    remove_empty_directories: z.boolean(),
    window_size: Tuple2,
    window_position: Tuple2,
    auto_load_last_directory: z.boolean(),
    max_file_size_mb: z.number(),
    scan_chunk_size: z.number(),
  })
  .passthrough();

/* ---------------------------------- Tree ----------------------------------- */
export const TreeNodeSchema: z.ZodType<any> = z.lazy(() =>
  z
    .object({
      path: z.string(),
      name: z.string(),
      is_directory: z.boolean(),
      is_expanded: z.boolean(),
      is_binary: z.boolean(),
      is_match: z.boolean(),
      is_previewed: z.boolean(),
      selection_state: z.enum(["none", "partial", "full"]),
      children: z.array(TreeNodeSchema),
      size: z.number(),
      // present in backend, not required by UI:
      children_loaded: z.boolean().optional(),
    })
    .passthrough()
);

/* --------------------------------- UiState --------------------------------- */
export const ScanProgressSchema = z
  .object({
    files_scanned: z.number().int().nonnegative(),
    large_files_skipped: z.number().int().nonnegative(),
    current_scanning_path: z.string(),
  })
  .passthrough();

export const UiStateSchema = z
  .object({
    config: ConfigSchema,
    current_path: z.string(),
    tree: z.array(TreeNodeSchema),
    total_files_found: z.number(),
    visible_files_count: z.number(),
    selected_files_count: z.number(),
    is_scanning: z.boolean(),
    is_generating: z.boolean(),
    is_fully_scanned: z.boolean(),
    status_message: z.string(),
    search_query: z.string(),
    extension_filter: z.string(),
    content_search_query: z.string(),
    current_config_filename: z.string().nullable().optional(),
    scan_progress: ScanProgressSchema,
    active_ignore_patterns: z.array(z.string()),
    patterns_need_rescan: z.boolean(),
  })
  .passthrough();

/* ------------------------------ Incoming API ------------------------------- */
export const ShowPreviewArgsSchema = z.tuple([
  z.string(), // content
  z.string(), // language
  z.string().nullable().optional(), // searchTerm
  z.string(), // path
]);

export const ShowGeneratedArgsSchema = z.tuple([
  z.string(), // content
  z.number().int().nonnegative(), // tokenCount
]);

export const StatusMessageSchema = z.string();
export const FileSaveStatusArgsSchema = z.tuple([z.boolean(), z.string()]);
export const DragStateSchema = z.boolean();

/* ------------------------------ Outgoing IPC ------------------------------- */
/** Individual payload schemas */
const NullPayload = z.null();
const UpdateFiltersPayload = z
  .object({
    searchQuery: z.string().optional().default(""),
    extensionFilter: z.string().optional().default(""),
    contentSearchQuery: z.string().optional().default(""),
  })
  .passthrough();
const PathPayload = PathString;
const BoolPayload = z.boolean();
const SaveFilePayload = z.string();
const UpdateConfigPayload = ConfigSchema;

/**
 * Single source of truth: command → payload schema.
 * ⚠️ Names and shapes must match the Rust backend exactly.
 */
export const CommandSchemas = {
  selectDirectory: NullPayload,
  rescanDirectory: NullPayload,
  loadDirectoryLevel: PathPayload,
  generatePreview: NullPayload,
  clearDirectory: NullPayload,
  cancelScan: NullPayload,
  initialize: NullPayload,
  loadFilePreview: PathPayload,
  toggleSelection: PathPayload,
  toggleDirectorySelection: PathPayload,
  toggleExpansion: PathPayload,
  expandCollapseAll: BoolPayload,
  selectAll: NullPayload,
  deselectAll: NullPayload,
  expandAllFully: NullPayload,
  selectAllFully: NullPayload,
  cancelGeneration: NullPayload,
  clearPreviewState: NullPayload,
  saveFile: SaveFilePayload,
  pickOutputDirectory: NullPayload,
  exportConfig: NullPayload,
  importConfig: NullPayload,
  updateConfig: UpdateConfigPayload,
  updateFilters: UpdateFiltersPayload,
  addIgnorePath: PathPayload,
} as const satisfies Record<string, z.ZodTypeAny>;

export type CommandName = keyof typeof CommandSchemas;

/**
 * Compile-time mapping: command name → payload type accepted at callsite.
 * - For most commands we use `z.input<schema>` (pre-parse type).
 * - Special-case: `updateConfig` should accept the *frontend* Config interface at compile-time,
 *   while still being validated against the full backend `ConfigSchema` at runtime.
 */
type FrontendConfig = import("../types").Config;
type _PayloadForBase<T extends CommandName> = z.input<
  (typeof CommandSchemas)[T]
>;
export type PayloadFor<T extends CommandName> = T extends "updateConfig"
  ? FrontendConfig
  : _PayloadForBase<T>;

/** Helpers for typed overloading in `post()` */
type CommandsByOutput<TOut> = {
  [K in CommandName]: z.output<(typeof CommandSchemas)[K]> extends TOut
    ? K
    : never;
}[CommandName];

export type NullaryCommandName = CommandsByOutput<null>;
export type NonNullCommandName = Exclude<CommandName, NullaryCommandName>;

/* ----------------------- Dev-time validation union ------------------------- */
/**
 * Build the union members programmatically, then assert a non-empty tuple type.
 * svelte-check/TS expects a variadic tuple for `z.union`, not a plain array.
 */
const _commandObjectSchemas = (
  Object.keys(CommandSchemas) as CommandName[]
).map((name) =>
  z.object({
    command: z.literal(name),
    payload: CommandSchemas[name],
  })
);

// Generic non-empty tuple helper
type NonEmptyTuple<T> = [T, ...T[]];
// Assert as a non-empty tuple for z.union()
const _commandObjectSchemasTuple =
  _commandObjectSchemas as unknown as NonEmptyTuple<z.ZodTypeAny>;

export const AnyCommandMessageSchema = z.union(_commandObjectSchemasTuple);
