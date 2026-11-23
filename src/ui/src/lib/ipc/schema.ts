// Runtime schemas for IPC payloads and app state (Zod) + typed Command map.
// Keep this file framework-agnostic; only data shapes + type utilities live here.

import { z } from "zod";
import { MAX_WINDOW_ROWS } from "../modules/treeWindow";

// Zod probes with `new Function` whether it may compile its parsers. Under the
// CSP of the app (no `unsafe-eval`) that probe is reported as a violation, so
// it is switched off before the first schema is used. This module is the only
// one that imports zod.
z.config({ jitless: true });

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
const RowIndex = z.number().int().nonnegative();

/**
 * One row of the tree (`TreeRow` in `src/app/view_model/rows.rs`). The tree
 * arrives as a flat list of rows, and of that list only a window, so a row
 * carries everything it shows: its nesting level, and for a directory what it
 * holds (`has_children`, `children_loaded`) and how much of it is selected.
 * The host always sends every field (guarded by the Rust fixture contract
 * test) and they are required here: a row without `children_loaded` would
 * silently never be lazy-loaded.
 */
const TreeRowObject = z.object({
  path: z.string(),
  name: z.string(),
  /** 0 for the items directly below the loaded directory. */
  depth: RowIndex,
  is_directory: z.boolean(),
  /** Authoritative expansion state, owned by the host. */
  is_expanded: z.boolean(),
  is_binary: z.boolean(),
  is_match: z.boolean(),
  is_previewed: z.boolean(),
  /** For directories: made of all files below, wherever they are listed. */
  selection_state: z.enum(["none", "partial", "full"]),
  size: z.number(),
  /** The directory has children in the filtered list, expanded or not. */
  has_children: z.boolean(),
  /**
   * `false` for a directory whose children have not been scanned yet; they
   * are fetched with `loadDirectoryLevel`. Always `true` for files.
   */
  children_loaded: z.boolean(),
});
export const TreeRowSchema = TreeRowObject.passthrough();
export type TreeRow = z.infer<typeof TreeRowObject>;

/** The answer to a `locateRow` request (`LocatedRow` on the host). */
const LocatedRowObject = z.object({
  /** The number the UI gave its request. */
  request: RowIndex,
  /** The row in the list of all rows; `null` if there is no such row. */
  row: RowIndex.nullable(),
});
export type LocatedRow = z.infer<typeof LocatedRowObject>;

/* ------------------------------- Correlation ------------------------------- */
/**
 * The number of a command. The UI numbers the commands it posts, counting
 * from 1 (`post()` in `services/backend.ts`), and the host states in every
 * snapshot the number of the last command the snapshot accounts for
 * (`last_processed_seq`; 0: none yet). See `src/app/correlation.rs`.
 */
export const CommandSeqSchema = z.number().int().nonnegative();
export type CommandSeq = z.infer<typeof CommandSeqSchema>;

/**
 * Identifies the UI that posts: a number it draws when it is loaded, so a UI
 * that is loaded anew is another one. The host states in every snapshot whose
 * command it accounts for last (`last_processed_session`; 0: none yet).
 */
export const SessionIdSchema = z.number().int().nonnegative();
export type SessionId = z.infer<typeof SessionIdSchema>;

/**
 * The name of a command, as it stands in the envelope of its message: who
 * posted it, and which of its commands it is. `post()` returns it.
 */
export type CommandId = { session: SessionId; seq: CommandSeq };

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
  /**
   * The rows of the window the UI asked for (`setViewport`), in order. The
   * host never sends more than its largest window.
   */
  rows: z.array(TreeRowSchema).max(MAX_WINDOW_ROWS),
  /** The position of `rows[0]` in the list of all rows. */
  rows_start: RowIndex,
  /** The number of rows of the tree: what the scrollbar spans. */
  rows_total: RowIndex,
  /** Changes when the list of all rows does, and only then. */
  rows_revision: RowIndex,
  /** The answer to the most recent `locateRow`, `null` if there was none. */
  located: LocatedRowObject.passthrough().nullable(),
  total_files_found: z.number(),
  visible_files_count: z.number(),
  // Totals of the complete filtered tree (a window of rows cannot provide them).
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
  /**
   * The last command this snapshot accounts for: the UI that posted it and
   * its number. The snapshot was sent by that command or after it, so for
   * that UI it is the answer to the command (and to every command it posted
   * before), or something newer. Both are 0 before the first command.
   */
  last_processed_session: SessionIdSchema,
  last_processed_seq: CommandSeqSchema,
});

/**
 * The snapshot of the host. Every row is validated, in production as well: a
 * snapshot holds a window of rows, never the tree, so the cost is bounded.
 * The window has to lie inside the list it is a part of.
 */
export const UiStateSchema = UiStateObject.passthrough().refine(
  (state) => state.rows_start + state.rows.length <= state.rows_total,
  { message: "the window of rows exceeds rows_total", path: ["rows"] },
);
/**
 * The state the UI works with. Derived from the schema, so the type cannot
 * drift from what is validated (unknown extra fields are not part of it).
 */
export type UiState = z.infer<typeof UiStateObject>;

/* ------------------------------ Incoming API ------------------------------- */
export const ShowPreviewArgsSchema = z.tuple([
  z.string(), // content
  z.string(), // language
  z.string().nullable().optional(), // searchTerm
  z.string(), // path
]);

/**
 * The id under which the host keeps a generated document (`GeneratedDocument`
 * in `src/app/state.rs`). The UI gets it with the document and names it when
 * the document is to be saved.
 */
const DocumentId = z.number().int().nonnegative();

export const ShowGeneratedArgsSchema = z.tuple([
  z.string(), // content
  z.number().int().nonnegative(), // tokenCount
  DocumentId, // documentId
]);

export const StatusMessageSchema = z.string();

/**
 * How a save ended (`SaveResult` in `src/app/events.rs`). A failed save names
 * its reason, a cancelled one is a case of its own, and only a successful one
 * has a path. `document_unavailable`: the host does not hold the generated
 * document the save named (any more); nothing was saved, and the UI saves the
 * text it has shown instead (see `saveOutput` in `modules/editor.ts`).
 */
export const SaveResultSchema = z.discriminatedUnion("status", [
  z.object({ status: z.literal("saved"), path: z.string() }).strict(),
  z.object({ status: z.literal("cancelled") }).strict(),
  z.object({ status: z.literal("failed"), error: z.string() }).strict(),
  z
    .object({
      status: z.literal("document_unavailable"),
      document_id: z.number().int().nonnegative(),
    })
    .strict(),
]);
export type SaveResult = z.infer<typeof SaveResultSchema>;

const ByteCount = z.number().int().nonnegative();
const outputSizeNotice = <S extends string>(status: S) =>
  z
    .object({
      status: z.literal(status),
      estimated_bytes: ByteCount,
      limit_bytes: ByteCount,
    })
    .strict();

/**
 * What the size of the output means for a generation that was asked for
 * (`OutputSizeNotice` in `src/app/events.rs`): `large` – it runs, but showing
 * the output takes a moment; `too_large` – it was refused.
 */
export const OutputSizeNoticeSchema = z.discriminatedUnion("status", [
  outputSizeNotice("large"),
  outputSizeNotice("too_large"),
]);
export type OutputSizeNotice = z.infer<typeof OutputSizeNoticeSchema>;

const ignorePathRefusal = <R extends string>(reason: R) =>
  z.object({ reason: z.literal(reason), name: z.string() }).strict();

/**
 * Why the ignore button of a tree row has added no pattern
 * (`IgnorePathRefusal` in `src/app/events.rs`): the entry `name` has a name no
 * ignore pattern can name. `trailing_whitespace` – the file name ends with
 * whitespace outside ASCII; `not_unicode` – the name is not valid Unicode
 * (`name` is written lossily then).
 */
export const IgnorePathRefusalSchema = z.discriminatedUnion("reason", [
  ignorePathRefusal("trailing_whitespace"),
  ignorePathRefusal("not_unicode"),
]);
export type IgnorePathRefusal = z.infer<typeof IgnorePathRefusalSchema>;

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
/**
 * The part of the rows the UI shows, overscan included (`SetViewportPayload`
 * on the host, which rejects unknown fields as well).
 */
const SetViewportPayload = z
  .object({
    start: RowIndex,
    count: z.number().int().positive(),
  })
  .strict();
/** The row a `locateRow` request asks for (`RowTarget` on the host). */
const RowTargetSchema = z.discriminatedUnion("kind", [
  /** The row of the item at `path`. */
  z.object({ kind: z.literal("path"), path: PathString }).strict(),
  /** The row of the directory the item at `path` lies in. */
  z.object({ kind: z.literal("parent_of"), path: PathString }).strict(),
  /**
   * The first row from row `from` on (wrapping around) whose name starts with
   * `prefix`, ignoring case.
   */
  z
    .object({
      kind: z.literal("typeahead"),
      prefix: z.string().min(1),
      from: RowIndex,
    })
    .strict(),
]);
export type RowTarget = z.infer<typeof RowTargetSchema>;
const LocateRowPayload = z
  .object({
    /** Chosen by the UI; the answer (`located`) repeats it. */
    request: RowIndex,
    target: RowTargetSchema,
  })
  .strict();
/**
 * What is to be saved (`SaveFilePayload` in `src/app/commands.rs`): the
 * generated output as the host produced it, named by its id (the host writes
 * its own copy, the text is not sent back), or the text of the editor if the
 * user has changed it.
 */
export const SaveFilePayloadSchema = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("generated"), documentId: DocumentId }).strict(),
  z.object({ kind: z.literal("edited"), content: z.string() }).strict(),
]);
export type SaveFilePayload = z.infer<typeof SaveFilePayloadSchema>;
/** A pattern is set if it is not blank; the host ignores blank ones. */
const AddIgnorePatternPayload = z
  .object({
    pattern: z.string().refine((p) => p.trim().length > 0, "blank pattern"),
  })
  .strict();
/**
 * Any text can be removed: an imported configuration may contain a blank
 * pattern, and its chip has to be removable like every other.
 */
const RemoveIgnorePatternPayload = z.object({ pattern: z.string() }).strict();

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
 * Names and shapes must match the Rust backend exactly (`Command` in
 * `src/app/ipc.rs`). That is enforced: the host writes examples of every
 * command into `tests/contracts/fixtures/commands.json` and
 * `command_messages.json`, and `rust-fixture.test.ts` compares the names with
 * this map, validates every example with it and checks that `post()` writes
 * the messages the host reads.
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
  setViewport: SetViewportPayload,
  locateRow: LocateRowPayload,
  selectAll: NullPayload,
  deselectAll: NullPayload,
  cancelGeneration: NullPayload,
  clearPreviewState: NullPayload,
  saveFile: SaveFilePayloadSchema,
  pickOutputDirectory: NullPayload,
  exportConfig: NullPayload,
  importConfig: NullPayload,
  updateFilters: UpdateFiltersPayload,
  addIgnorePattern: AddIgnorePatternPayload,
  removeIgnorePattern: RemoveIgnorePatternPayload,
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
// A command on the wire: its name and its payload, and nothing else (the host
// refuses a message with anything more). The name of the message (session
// and number) is not part of the command; `post()` puts it next to it.
const _commandObjectSchemas = (
  Object.keys(CommandSchemas) as CommandName[]
).map((name) =>
  z
    .object({
      command: z.literal(name),
      payload: CommandSchemas[name],
    })
    .strict(),
);

type NonEmptyTuple<T> = [T, ...T[]];
const _commandObjectSchemasTuple =
  _commandObjectSchemas as unknown as NonEmptyTuple<z.ZodTypeAny>;

export const AnyCommandMessageSchema = z.union(_commandObjectSchemasTuple);
