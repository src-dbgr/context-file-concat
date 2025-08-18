// Runtime schemas for IPC payloads and app state (Zod).
// Keep this file framework-agnostic; only data shapes live here.

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
      // present in backend, not required by UI types:
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

const SelectDirectory = z.object({
  command: z.literal("selectDirectory"),
  payload: NullPayload,
});
const RescanDirectory = z.object({
  command: z.literal("rescanDirectory"),
  payload: NullPayload,
});
const LoadDirectoryLevel = z.object({
  command: z.literal("loadDirectoryLevel"),
  payload: PathPayload,
});
const GeneratePreview = z.object({
  command: z.literal("generatePreview"),
  payload: NullPayload,
});
const ClearDirectory = z.object({
  command: z.literal("clearDirectory"),
  payload: NullPayload,
});
const CancelScan = z.object({
  command: z.literal("cancelScan"),
  payload: NullPayload,
});
const Initialize = z.object({
  command: z.literal("initialize"),
  payload: NullPayload,
});
const LoadFilePreview = z.object({
  command: z.literal("loadFilePreview"),
  payload: PathPayload,
});
const ToggleSelection = z.object({
  command: z.literal("toggleSelection"),
  payload: PathPayload,
});
const ToggleDirectorySelection = z.object({
  command: z.literal("toggleDirectorySelection"),
  payload: PathPayload,
});
const ToggleExpansion = z.object({
  command: z.literal("toggleExpansion"),
  payload: PathPayload,
});
const ExpandCollapseAll = z.object({
  command: z.literal("expandCollapseAll"),
  payload: BoolPayload,
});
const SelectAll = z.object({
  command: z.literal("selectAll"),
  payload: NullPayload,
});
const DeselectAll = z.object({
  command: z.literal("deselectAll"),
  payload: NullPayload,
});
const ExpandAllFully = z.object({
  command: z.literal("expandAllFully"),
  payload: NullPayload,
});
const SelectAllFully = z.object({
  command: z.literal("selectAllFully"),
  payload: NullPayload,
});
const CancelGeneration = z.object({
  command: z.literal("cancelGeneration"),
  payload: NullPayload,
});
const ClearPreviewState = z.object({
  command: z.literal("clearPreviewState"),
  payload: NullPayload,
});
const SaveFile = z.object({
  command: z.literal("saveFile"),
  payload: SaveFilePayload,
});
const PickOutputDirectory = z.object({
  command: z.literal("pickOutputDirectory"),
  payload: NullPayload,
});
const ExportConfig = z.object({
  command: z.literal("exportConfig"),
  payload: NullPayload,
});
const ImportConfig = z.object({
  command: z.literal("importConfig"),
  payload: NullPayload,
});
const UpdateConfig = z.object({
  command: z.literal("updateConfig"),
  payload: UpdateConfigPayload,
});
const UpdateFilters = z.object({
  command: z.literal("updateFilters"),
  payload: UpdateFiltersPayload,
});
const AddIgnorePath = z.object({
  command: z.literal("addIgnorePath"),
  payload: PathPayload,
});

export const AnyCommandMessageSchema = z.union([
  SelectDirectory,
  RescanDirectory,
  LoadDirectoryLevel,
  GeneratePreview,
  ClearDirectory,
  CancelScan,
  Initialize,
  LoadFilePreview,
  ToggleSelection,
  ToggleDirectorySelection,
  ToggleExpansion,
  ExpandCollapseAll,
  SelectAll,
  DeselectAll,
  ExpandAllFully,
  SelectAllFully,
  CancelGeneration,
  ClearPreviewState,
  SaveFile,
  PickOutputDirectory,
  ExportConfig,
  ImportConfig,
  UpdateConfig,
  UpdateFilters,
  AddIgnorePath,
]);

export type AnyCommandMessage = z.infer<typeof AnyCommandMessageSchema>;
export type UiState = z.infer<typeof UiStateSchema>;
