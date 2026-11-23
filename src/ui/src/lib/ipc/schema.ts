// Runtime schemas for IPC payloads and app state (Zod) + typed Command map.
// Keep this file framework-agnostic; only data shapes + type utilities live here.

import { z } from "zod";
import type { TreeNode } from "../types";

/* ---------------------------------- Common --------------------------------- */
export const PathString = z.string().min(1);

/* --------------------------------- Config ---------------------------------- */
/**
 * The configuration as the host shows it to the UI (`UiConfig` in
 * `src/app/view_model/ui_config.rs`): exactly the settings the UI displays.
 * The UI never sends it back; every edit is an intent command (see
 * `ConfigOptionSchema` and the ignore pattern commands below).
 */
export const UiConfigSchema = z.object({
  ignore_patterns: z.array(z.string()),
  case_sensitive_search: z.boolean(),
  remove_empty_directories: z.boolean(),
  include_tree_by_default: z.boolean(),
  use_relative_paths: z.boolean(),
  output_directory: z.string().nullable(),
  output_filename: z.string(),
});
export type UiConfig = z.infer<typeof UiConfigSchema>;

/* ---------------------------------- Tree ----------------------------------- */
/**
 * The tree is a pruned snapshot: `children` is only filled for expanded
 * directories, so `has_children` and `children_loaded` are the only way to
 * tell what a collapsed directory holds. The backend always sends them
 * (guarded by the Rust fixture contract test) and they are required here: a
 * node without `children_loaded` would silently never be lazy-loaded.
 */
export const TreeNodeSchema: z.ZodType<TreeNode> = z.lazy(() =>
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
      has_children: z.boolean(),
      children_loaded: z.boolean(),
    })
    .passthrough(),
);

/* --------------------------------- UiState --------------------------------- */
export const ScanProgressSchema = z
  .object({
    files_scanned: z.number().int().nonnegative(),
    large_files_skipped: z.number().int().nonnegative(),
    current_scanning_path: z.string(),
  })
  .passthrough();

const UiStateObject = z.object({
  config: UiConfigSchema,
  current_path: z.string(),
  tree: z.array(TreeNodeSchema),
  total_files_found: z.number(),
  visible_files_count: z.number(),
  // Totals of the complete filtered tree (the pruned `tree` cannot provide them).
  tree_files_count: z.number().int().nonnegative(),
  tree_dirs_count: z.number().int().nonnegative(),
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
});

export const UiStateSchema = UiStateObject.passthrough();
/**
 * The state the UI works with. Derived from the schema, so the type cannot
 * drift from what is validated (unknown extra fields are not part of it).
 */
export type UiState = z.infer<typeof UiStateObject>;

/**
 * Envelope-only variant of {@link UiStateSchema}: every top-level field is
 * validated exactly as in the full schema, but `tree` is only checked to be an
 * array of objects — no recursion into the nodes. The deep tree walk dominates
 * validation cost on large trees, so production validates the envelope only,
 * while DEV and tests keep the full schema.
 */
const ShallowTreeNodeSchema = z.custom<TreeNode>(
  (value) =>
    typeof value === "object" && value !== null && !Array.isArray(value),
);

export const UiStateEnvelopeSchema = UiStateSchema.extend({
  tree: z.array(ShallowTreeNodeSchema),
});

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
const NullPayload = z.null();
/** Strict like the backend (`UpdateFiltersPayload`), which rejects unknown fields. */
const UpdateFiltersPayload = z
  .object({
    searchQuery: z.string().optional().default(""),
    extensionFilter: z.string().optional().default(""),
    contentSearchQuery: z.string().optional().default(""),
  })
  .strict();
const PathPayload = PathString;
const BoolPayload = z.boolean();
/** Strict like the backend, which rejects unknown fields for this command. */
const SetExpansionPayload = z
  .object({
    path: PathString,
    expanded: z.boolean(),
  })
  .strict();
const SaveFilePayload = z.string();
/** A pattern is set if it is not blank; the host ignores blank ones. */
const IgnorePatternPayload = z
  .object({
    pattern: z.string().refine((p) => p.trim().length > 0, "blank pattern"),
  })
  .strict();

const configOption = <N extends string, V extends z.ZodTypeAny>(
  option: N,
  value: V,
) => z.object({ option: z.literal(option), value }).strict();

/**
 * One user-editable option together with its new value (`ConfigOption` in
 * `src/app/commands.rs`). These are all the options the UI can set; the rest
 * of the configuration belongs to the host.
 */
export const ConfigOptionSchema = z.discriminatedUnion("option", [
  configOption("case_sensitive_search", z.boolean()),
  configOption("remove_empty_directories", z.boolean()),
  configOption("include_tree_by_default", z.boolean()),
  configOption("use_relative_paths", z.boolean()),
  configOption("output_filename", z.string()),
  /** `null` or an empty text: no output directory is preset. */
  configOption("output_directory", z.string().nullable()),
]);
export type ConfigOption = z.infer<typeof ConfigOptionSchema>;

/**
 * Single source of truth: command → payload schema.
 * Names and shapes must match the Rust backend exactly. That is enforced: the
 * host writes its routing table with example payloads into
 * `tests/contracts/fixtures/commands.json`, and `rust-fixture.test.ts`
 * compares the names with this map and validates every example with it.
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
  setExpansion: SetExpansionPayload,
  expandCollapseAll: BoolPayload,
  selectAll: NullPayload,
  deselectAll: NullPayload,
  cancelGeneration: NullPayload,
  clearPreviewState: NullPayload,
  saveFile: SaveFilePayload,
  pickOutputDirectory: NullPayload,
  exportConfig: NullPayload,
  importConfig: NullPayload,
  updateFilters: UpdateFiltersPayload,
  addIgnorePattern: IgnorePatternPayload,
  removeIgnorePattern: IgnorePatternPayload,
  clearIgnorePatterns: NullPayload,
  addIgnorePath: PathPayload,
  setConfigOption: ConfigOptionSchema,
} as const satisfies Record<string, z.ZodTypeAny>;

export type CommandName = keyof typeof CommandSchemas;

/** What `post()` accepts for a command: the input type of its schema. */
export type PayloadFor<T extends CommandName> = z.input<
  (typeof CommandSchemas)[T]
>;

type CommandsByOutput<TOut> = {
  [K in CommandName]: z.output<(typeof CommandSchemas)[K]> extends TOut
    ? K
    : never;
}[CommandName];

export type NullaryCommandName = CommandsByOutput<null>;
export type NonNullCommandName = Exclude<CommandName, NullaryCommandName>;

/* ----------------------- Dev-time validation union ------------------------- */
const _commandObjectSchemas = (
  Object.keys(CommandSchemas) as CommandName[]
).map((name) =>
  z.object({
    command: z.literal(name),
    payload: CommandSchemas[name],
  }),
);

type NonEmptyTuple<T> = [T, ...T[]];
const _commandObjectSchemasTuple =
  _commandObjectSchemas as unknown as NonEmptyTuple<z.ZodTypeAny>;

export const AnyCommandMessageSchema = z.union(_commandObjectSchemasTuple);
