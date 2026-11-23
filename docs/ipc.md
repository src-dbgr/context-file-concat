# IPC Contracts

> The UI and the host talk over two one-way channels: the UI **posts commands**, the host **calls `window.*` handlers**. There are no request/response pairs and no promises: a command never returns a value, its effect arrives as the next state snapshot.

## Overview

```text
UI  ──  window.ipc.postMessage('{"command": "...", "payload": ...}')  ──▶  Host
UI  ◀──  window.render(uiState) / window.showPreviewContent(...) / ...  ──  Host
```

| Concern                         | Where                                                                         |
| ------------------------------- | ----------------------------------------------------------------------------- |
| Sending commands                | `src/ui/src/lib/services/backend.ts` (`post`)                                 |
| Command and state schemas (Zod) | `src/ui/src/lib/ipc/schema.ts`                                                |
| Receiving host calls            | `src/ui/src/lib/ipc/handlers.ts` (`installWindowIPCHandlers`)                 |
| Frontend types                  | `src/ui/src/lib/types.ts` (derived from the schemas with `z.infer`)           |
| Command dispatch (host)         | `src/app/mod.rs` (`COMMANDS`, `dispatch_ipc_message`) → `src/app/commands.rs` |
| Command ordering (host)         | `src/app/dispatch.rs` (`CommandDispatcher`)                                   |
| Host → UI events                | `src/app/events.rs` (`UserEvent`), `src/app/mod.rs` (`handle_user_event`)     |
| State snapshot (host)           | `src/app/view_model/` (`generate_ui_state`, `UiState`, `UiConfig`, `TreeRow`) |
| Rows of the tree, their window  | `src/app/view_model/rows.rs` (host), `src/ui/src/lib/modules/treeWindow.ts`   |
| Delivery / coalescing           | `src/app/transport.rs`                                                        |

## Principles

- **Commands are fire-and-forget.** `post()` returns `void`. The host mutates its `AppState` and answers with a state snapshot (and, where applicable, one of the other host calls).
- **Commands take effect in the order they were posted.** The host executes them one after the other, see [Command ordering](#command-ordering).
- **Every state update is a complete snapshot.** `window.render(uiState)` replaces the UI's `appState` store wholesale. There are no diffs and no patches; a snapshot never depends on an earlier one. This is what allows the transport to drop a pending snapshot when a newer one is already available (latest wins, see `transport.rs`).
- **The host owns the state, the UI owns none of it.** In particular the host is the single source of truth for selection, for **expansion** and for the **list of rows** the tree consists of (see below). The UI renders what it receives.
- **Snapshots are bounded by what is on screen.** Of the tree a snapshot carries a window: the rows around the viewport the UI has reported (see [The rows of the tree](#the-rows-of-the-tree)). Its size does not depend on the size of the project or on how much is expanded; totals are sent as counters.
- **Validation at the edge.** Outgoing commands are validated against `CommandSchemas` in DEV. Incoming calls are validated in `handlers.ts`: `render()` with the full `UiStateSchema`, every row included, in every build (a window of rows is small enough for that). Invalid payloads are ignored with a console warning.
- **No untyped `any` at the boundary.**

## Commands (UI → Host)

The single source of truth on the UI side is `CommandSchemas` in `schema.ts`, on the host the routing table `COMMANDS` in `src/app/mod.rs`. The two are compared by the [contract tests](#contract-tests): same command names, and every example payload the host serializes passes the schema of its command. The wire format is `{"command": <name>, "payload": <payload | null>}`.

| Command                         | Payload                                                 | Effect                                                                                                                                                                                                                                              |
| ------------------------------- | ------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `initialize`                    | `null`                                                  | Sends the current snapshot, with the window of rows at the top                                                                                                                                                                                      |
| `selectDirectory`               | `null`                                                  | Opens the directory dialog and [opens](#opening-a-path) the choice                                                                                                                                                                                  |
| `rescanDirectory`               | `null`                                                  | Rescans the current directory, keeping selection and expansion                                                                                                                                                                                      |
| `clearDirectory`                | `null`                                                  | Resets all directory-related state                                                                                                                                                                                                                  |
| `cancelScan`                    | `null`                                                  | Cancels the running scan                                                                                                                                                                                                                            |
| `loadDirectoryLevel`            | `string` (path)                                         | **Expands** the directory right away, then scans its level and marks it loaded                                                                                                                                                                      |
| `setExpansion`                  | `{ path: string, expanded: boolean }` (no other fields) | Expands or collapses one directory. Idempotent; always answered with a snapshot                                                                                                                                                                     |
| `expandCollapseAll`             | `boolean`                                               | Expands (`true`) or collapses (`false`) every directory the filters show                                                                                                                                                                            |
| `setViewport`                   | `{ start: number, count: number }` (no other fields)    | Reports the rows the UI shows, overscan included; the snapshots from then on carry that [window](#the-rows-of-the-tree). Always answered with a snapshot                                                                                            |
| `locateRow`                     | `{ request: number, target }` (no other fields)         | Asks where a row is in the list of all rows, see [Looking up a row](#looking-up-a-row); the answer is `located` of the snapshot                                                                                                                     |
| `toggleSelection`               | `string` (path)                                         | Selects/deselects one file                                                                                                                                                                                                                          |
| `toggleDirectorySelection`      | `string` (path)                                         | Selects/deselects all files below a directory                                                                                                                                                                                                       |
| `selectAll` / `deselectAll`     | `null`                                                  | Selects all listed files / clears the selection                                                                                                                                                                                                     |
| `updateFilters`                 | `{ searchQuery, extensionFilter, contentSearchQuery }`  | Applies the filters; expands the ancestors of matches, up to a [limit](#expansion-one-source-of-truth) (no other fields). Changed texts show the list from the top                                                                                  |
| `loadFilePreview`               | `string` (path)                                         | Marks the file as previewed; `showPreviewContent` follows once it has been read. Only an entry of the loaded list, see [File content](#file-content)                                                                                                |
| `clearPreviewState`             | `null`                                                  | Clears the previewed file; a preview still being read is not shown any more                                                                                                                                                                         |
| `generatePreview`               | `null`                                                  | Concatenates the selection; answers with `showGeneratedContent`, or refuses an output that is too large, see [The generated output](#the-generated-output). A filename `cfc_output_<timestamp>.txt` gets the current time; every other name is kept |
| `cancelGeneration`              | `null`                                                  | Cancels the running generation                                                                                                                                                                                                                      |
| `saveFile`                      | `SaveFilePayload`, see [Saving](#saving)                | Opens the save dialog and writes the output; answers with `saveResult`                                                                                                                                                                              |
| `pickOutputDirectory`           | `null`                                                  | Opens the dialog, stores the choice as output directory and persists it                                                                                                                                                                             |
| `addIgnorePattern`              | `{ pattern: string }` (not blank, no other fields)      | Adds one ignore pattern, see [Config changes](#config-changes)                                                                                                                                                                                      |
| `removeIgnorePattern`           | `{ pattern: string }` (any text, no other fields)       | Removes one ignore pattern                                                                                                                                                                                                                          |
| `clearIgnorePatterns`           | `null`                                                  | Removes all ignore patterns                                                                                                                                                                                                                         |
| `setConfigOption`               | `{ option, value }` (no other fields), see below        | Sets one user-editable option                                                                                                                                                                                                                       |
| `importConfig` / `exportConfig` | `null`                                                  | Config import/export via dialog, see [Config import and export](#config-import-and-export)                                                                                                                                                          |
| `addIgnorePath`                 | `string` (path)                                         | Adds the ignore pattern for exactly that entry of the loaded tree, see [The pattern of `addIgnorePath`](#the-pattern-of-addignorepath)                                                                                                              |

### Filter texts

One rule decides whether a filter text is set, for `searchQuery`, `extensionFilter` and `contentSearchQuery` alike: a text that is empty or consists of whitespace only is **not set**. It is ignored entirely: it does not filter, it starts no content search, and the UI does not show the "no results for filter" message for it. A text that is set is matched exactly as typed; it is not trimmed. The rule lives in `core::is_blank` on the host; the UI mirrors it in `hasActiveFilters` (`FileTree.svelte`).

The payload has a type on the host (`UpdateFiltersPayload` in `commands.rs`): the three texts, each optional (a text that is left out is empty), and nothing else. A malformed payload changes nothing and is answered with the unchanged snapshot.

### Content search

A content query is searched in the background, and a search can take long. `updateFilters` with a changed content query is therefore answered **twice**: with a snapshot right away, which carries the new filter texts while the search is pending (the filter inputs wait for it, see [Text fields](#text-fields)), and with a second one when the matches are there. The command suspends in between (see [Command ordering](#command-ordering)). A search that is repeated for a query the UI already shows (after a lazy load, a scan, a change of the case sensitivity) sends only the snapshot with its matches. What a snapshot shows that is sent while a search is running, the first answer included:

- While the **first** search for a query is running (the query was empty before), there is no content filter yet: the list is shown as it is without it. It is not emptied.
- Once a search has answered, its matches are the filter until the next search answers, also while that next search (a refined query, the repetition after a lazy load or a scan) is running. A query without matches shows an empty tree.
- A search that **fails** (its worker panicked) is reported with `showError("The content search failed: …")`, followed by a snapshot. The matches of the search before it stay; a failed search is not a search without matches.

### Config changes

**The host owns the configuration; the UI edits it with intents.** A snapshot carries the settings the UI displays (`config`, see [The state snapshot](#the-state-snapshot-uistate)), and the UI never sends that copy back. Each edit is a command that names the one thing the user changed, and the host applies it to its own configuration (`change_config` in `commands.rs`). There is deliberately no command that replaces the whole configuration: two quick edits (two clicks on pattern chips before the first snapshot has been rendered) would each be based on the same outdated copy, and the second one would silently revert the first. Loading a whole configuration is done by `importConfig`, on the host.

`setConfigOption` takes one of these payloads (`ConfigOption` in `commands.rs`, `ConfigOptionSchema` in `schema.ts`):

| `option`                   | `value`          | Effect                                                                                                   |
| -------------------------- | ---------------- | -------------------------------------------------------------------------------------------------------- |
| `case_sensitive_search`    | `boolean`        | Filters are re-applied; an active content search is repeated, because its matches depend on the setting. |
| `remove_empty_directories` | `boolean`        | Filters are re-applied.                                                                                  |
| `include_tree_by_default`  | `boolean`        | Used by the next generation.                                                                             |
| `use_relative_paths`       | `boolean`        | Used by the next generation.                                                                             |
| `output_filename`          | `string`         | Suggested by the save dialog.                                                                            |
| `output_directory`         | `string \| null` | Suggested by the save dialog. `null` or `""`: no preset.                                                 |

Everything else in the persisted configuration (`AppConfig`: `window_size`, `window_position`, `geometry_units`, `last_directory`, `max_file_size_mb`, `tree_ignore_patterns`) is not part of the snapshot and cannot be set through IPC. Unknown options, extra fields and wrong value types are rejected.

What a change of the ignore patterns does (`addIgnorePattern`, `removeIgnorePattern`, `clearIgnorePatterns`, `addIgnorePath`):

| Change                                            | Effect                                                                                                                                                                                                                                                                                                                                                                                        |
| ------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Ignore pattern added or removed **during a scan** | The scan is restarted with the new patterns (selection and expansion are kept). A scan works with the patterns it was started with, so its result would otherwise bring ignored entries back. A scan that is still [opening its path](#opening-a-path) has not read the patterns yet; it is left alone and the change is applied to the directory that is still loaded, as in the rows below. |
| Ignore pattern added, no scan running             | Applied to the loaded file list right away; no rescan.                                                                                                                                                                                                                                                                                                                                        |
| Ignore pattern removed, no scan running           | Sets `patterns_need_rescan`; the UI offers `rescanDirectory`.                                                                                                                                                                                                                                                                                                                                 |
| No directory loaded                               | The configuration is changed; there is nothing to apply it to.                                                                                                                                                                                                                                                                                                                                |

#### Ignore pattern semantics

One type decides what the ignore patterns hide: `core::IgnoreRules` (`src/core/ignore_rules.rs`). The scan, the local application of a changed pattern set and the generated tree (`tree_ignore_patterns`) all ask it, so they cannot disagree. The patterns use the `.gitignore` syntax and are anchored at the loaded directory:

| Pattern         | Hides                                                          |
| --------------- | -------------------------------------------------------------- |
| `name`, `*.log` | Files and directories of that name, at any depth               |
| `dir/`          | Directories of that name only                                  |
| `/x`, `a/b`     | The entry at that path relative to the loaded directory only   |
| `!pattern`      | Nothing: a negation **re-includes** what another pattern hides |

- A hidden directory hides everything below it. As in git, a negation cannot re-include an entry below a hidden directory (`out` + `!out/keep.txt` hides the file; `out/*` + `!out/keep.txt` keeps it).
- The patterns are a set without an order, so there is no "last line wins": **a negation takes precedence over every other pattern**, whenever it was added.
- Blank patterns, comments (`#…`) and invalid globs are not rules and hide nothing.
- <a id="the-pattern-of-addignorepath"></a>**The pattern of `addIgnorePath`** (the ignore button of a tree row) hides that one entry and nothing else. It is the path relative to the loaded directory, **anchored with a leading `/`**, the names joined with `/` on every platform, a directory of the loaded list with a trailing `/`: `/README.md`, `/src/gen/`. Characters with a meaning in a pattern are escaped with a backslash (`\`, `*`, `?`, `[`, `]`, `{`, `}`), so `a[1].txt` becomes `/a\[1\].txt`; the leading slash also keeps `!important` and `#notes` from being read as a negation or a comment. The matcher drops the whitespace at the end of a pattern (of any kind, not only spaces), so a file name that ends with ASCII whitespace (space, tab, line feed, vertical tab, form feed, carriage return) has its last character written as a character class: `notes ` becomes `/notes[ ]`, a name that ends with a tab `/name[<TAB>]`. (A pattern typed by hand may also end with `\ `, as in a `.gitignore` file.) A path outside the loaded directory, a name that is not valid Unicode and a file name that ends with whitespace outside ASCII (a no-break space U+00A0, an ideographic space U+3000) have no pattern: nothing is added, and the command is answered with the unchanged snapshot. A directory with such a name has a pattern: the slash at its end protects the name. (Before, the pattern was the bare relative path: `README.md` and `docs/` also hid entries of that name in subdirectories, and special characters were interpreted. Patterns saved by earlier versions keep that meaning; to hide a name at any depth, add it as a pattern with `addIgnorePattern`.)
- `active_ignore_patterns` holds the patterns that decided about at least one entry of the scan: those that hid one and the negations that re-included one. Of several patterns matching the same entry only the deciding one is reported.

The commands are idempotent (adding a pattern that is set, removing one that is not, and setting an option to its current value change nothing), a changed configuration is persisted, and **every config command is answered with a snapshot**, also when nothing changed. The UI relies on that: controls show the host's `config` and do not write to the store. This holds for every way these commands can end (`addIgnorePattern`, `removeIgnorePattern`, `clearIgnorePatterns`, `addIgnorePath`, `setConfigOption`): a payload that is refused (malformed, an unknown option, a blank pattern), `addIgnorePath` for a path outside the loaded directory or without a loaded directory. Refused payloads are logged on the host. The dialog command `pickOutputDirectory` is the exception that follows from its nature: a dismissed dialog sends nothing.

#### Config import and export

`exportConfig` writes the settings a user edits and shares, and nothing else: ignore patterns (also those of the generated tree), the output file name, the search and output options, the size limit (`config::SharedConfig`; the patterns are sorted). What describes the installation is **not** part of the file: `last_directory`, `output_directory` and the window geometry. An exported file can be passed on without telling anything about the machine it comes from. The settings are copied under the state lock and written without it.

`importConfig` accepts such a file as well as a complete configuration file (what versions up to 0.3.1 exported, or a copy of `config.json`). Settings the file does not name get their default. It takes the shared settings from the chosen file, and the `output_directory` if the file names one; without one the current output directory stays. The window geometry of the file is **not** imported; the current values stay. The import is a hard reset of the loaded directory: selection, expansion, filters and preview are cleared and no directory is loaded (answered with a snapshot). If the file names a `last_directory` and that directory exists, it is then [opened](#opening-a-path) like a dropped path, which also records it as `last_directory`. If the file names none, or one that does not exist, nothing is scanned: the UI stays clean, no error is shown and `last_directory` keeps its value. The imported settings are persisted.

#### Persistence

A changed configuration is written to the config file in the background. Every writer goes the same way: a numbered snapshot of the configuration is taken under the state lock, and of several saves under way the most recent snapshot is the one that ends up on disk (`ConfigStore` in `src/config/store.rs`). The window geometry is written with the last save of a session, at exit: the store takes the most recent snapshot, written or not, adds the geometry, writes the result and closes (`ConfigStore::finalize`). For every other save this is a single step, so a save that is still queued can neither overtake it nor run after it, and a change made right before the exit is part of it.

The file is replaced atomically: the configuration is written to a temporary file in the same directory, which is then renamed over the config file (`config::settings::save_config`; the same holds for `exportConfig`). A write that fails or is interrupted leaves the previous file untouched. A symbolic link at the place of the config file is followed, and the permissions of the file are kept. Only if the directory does not allow creating the temporary file is the file written in place.

`window_size` and `window_position` are logical units, marked by `geometry_units: "logical"`. A config file without that mark was written by a version that stored physical pixels (up to 0.3.1); its geometry is converted once at startup, with the scale factor of the primary monitor.

`max_file_size_mb` is the size limit for a single file in MiB (default 20), and the only limit that is configured (the output as a whole has a fixed one, see [The generated output](#the-generated-output)): larger files are not listed by a scan and are replaced by a `[FILE TOO LARGE …]` marker in the generated output. The content search does not read larger files either. The value is not capped: a limit above the default is honoured by all of them, and nothing else depends on the size of a file (whether a file is text or binary is decided by its extension and its first kilobyte, see [File content](#file-content)). The smallest limit is 1: a smaller value in a config file (0 would hide every file that is not empty) is read as 1, on load and on import. It is not exposed in the UI; set it in the config file.

### Text fields

A snapshot replaces the whole store, so a text field bound to the store would be reset by any snapshot that arrives while the user is typing (scan progress, the answer to another command, a snapshot from before the host has seen the latest input). The filter inputs (`Sidebar.svelte`) and the output fields (`HostTextInput.svelte`) therefore show a local draft and ask a `HostDraft` (`src/ui/src/lib/modules/hostDraft.ts`) whether a host value may replace it:

- While the user is editing (changed, not sent yet: the 300 ms debounce of the filters, an output field that still has the focus), the draft stays.
- After the text has been sent, host values that differ from it are skipped until the host reports the sent text (its answer to the command).
- The wait for that answer is limited to 1.5 s, for the case that something else has changed the value in the meantime; the host is the source of truth. What ends the wait is always a host value that is newer than the send, never the time alone:
  - If the host has reported differing values by then (they were skipped), a timer ends the wait and the field takes the latest host value; it does not depend on another snapshot arriving.
  - If the host has reported nothing since the send, the value the field knows is older than the send and says nothing about the host's decision. The draft stays, and the next snapshot that arrives is shown. (Every command that sets a text is answered right away, also `updateFilters` with a content query; this keeps a typed text from being reset should an answer take longer all the same.)
- The filters belong to the directory they were typed for. When `current_path` changes (another directory, or none after `clearDirectory`), a pending debounce is cancelled, the draft is given up and the host's filters are shown.

Changes made from script (word and line deletion with the keyboard, undo/redo, paste) dispatch an `input` event, so they are drafts like typed text.

Checkboxes need no draft; they send their intent on `change` and show `config`.

```ts
import { post } from "$lib/services/backend";

post("selectAll"); // nullary command
post("setExpansion", { path: "/repo/src", expanded: true }); // payload is type-checked per command
```

`post` is overloaded so that the payload type follows from the command name. It does not return anything: do not `await` it.

### Command ordering

`dispatch_ipc_message` does not execute anything itself. It parses the message and queues the command on a `CommandDispatcher` (`src/app/dispatch.rs`), which has three lanes:

| Lane         | Commands                                                                                                               | Guarantee                                                                                                             |
| ------------ | ---------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------- |
| Ordered      | Everything except the dialog commands                                                                                  | Commands start in arrival order; a command runs without interleaving until it finishes or suspends                    |
| Continuation | The rest of a command that suspended (`updateFilters` during a content search, `setConfigOption` while it repeats one) | Finished one at a time, in the order the commands were posted                                                         |
| Blocking     | `selectDirectory`, `saveFile`, `pickOutputDirectory`, `importConfig`, `exportConfig`                                   | One at a time, in arrival order, on the blocking thread pool; each starts after the ordered commands posted before it |

What follows from that:

- Two commands on the ordered lane never race. `setExpansion(true)` followed by `setExpansion(false)` ends collapsed, two `toggleSelection` cancel out, and of two `updateFilters` the later one determines the filters.
- **A command that starts background work does its bookkeeping itself, before it returns.** Starting a scan (`rescanDirectory`, a drop, the scan after a dialog) supersedes the scan that was running, registers the new one and sends the snapshot with `is_scanning: true` synchronously; `loadDirectoryLevel` marks the directory as expanded; `loadFilePreview` records the previewed file; `generatePreview` registers the generation. Only the IO runs in a task. The command that follows therefore always acts on the result: `rescanDirectory` → `cancelScan` ends cancelled, `rescanDirectory` → `clearDirectory` ends with no directory loaded, a drop → `cancelScan` ends cancelled with the previous directory still loaded, a drop → `rescanDirectory` ends with the dropped directory loaded, `loadDirectoryLevel` → `setExpansion(false)` ends collapsed (the load still delivers the children).
- **Results of background work carry a token and are dropped when they are outdated.** A scan that was cancelled or replaced, a lazy load whose file list was replaced, a file preview that was followed by another one or closed, a content search that was followed by a newer one: none of them changes the state or sends anything any more. For previews this means `showPreviewContent` calls arrive in the order of the requests and an older file never replaces a newer one.
- **A dropped path is a command.** The drag & drop handler queues the drop on the ordered lane (`dispatch_dropped_path`), so drops are ordered among themselves and with the IPC commands: of two drops in quick succession the second one is loaded.
- Long-running work and file IO do not occupy the ordered lane; no command of that lane touches the file system. Scans and generations are started as tasks of their own by their commands; so are the read of a previewed file and the look at a path that is to be scanned (on a slow network drive even checking that it exists can take long). `addIgnorePath` takes what it needs to know about its path (is it a directory?) from the loaded file list. The config file is written in the background as well, from a numbered snapshot, so that an older save can never overwrite a newer one (see [Persistence](#persistence)). A content search suspends `updateFilters`; the part up to the suspension (storing the new filters) has run in order, the rest (applying the search result) moves to the continuation lane. Results are applied in posting order, so an older search cannot overwrite a newer one, and commands posted meanwhile are executed immediately.
- A native dialog blocks its thread for as long as the user takes. Dialog commands therefore run **outside** the ordered lane: a command that arrives while a dialog is open is executed right away, and what the dialog command does after the dialog has been closed (start the scan, store the output directory, import the config) happens after it. Dialog commands are ordered among themselves, so only one dialog is open at a time; a second one opens after the first has been closed.
- **A dialog command starts after the commands that were posted before it.** Renaming the output file and pressing "Save" right away opens the save dialog with the new name. The dispatcher puts a marker on the ordered lane when the dialog command arrives and starts the command when the lane has reached the marker, i.e. when every earlier command has finished or suspended. A suspended command (a content search that is still running) has made its own state changes by then and does not hold the dialog back; the lane itself never waits for a dialog.
- A panicking command is logged and dropped; the lanes keep running. This holds in release builds too (the release profile unwinds), and also for a command that panics while it holds the state lock: the lock is taken over by the next command (`SharedState` in `src/app/shared_state.rs`), which works on the state as it was left.

## Host calls (Host → UI)

Each `UserEvent` is turned into exactly one call of a `window.*` function installed by `installWindowIPCHandlers()`:

| `UserEvent`            | Call                                                                                       | Schema                    |
| ---------------------- | ------------------------------------------------------------------------------------------ | ------------------------- |
| `StateUpdate`          | `window.render(uiState)`                                                                   | `UiStateSchema`           |
| `ScanProgress`         | `window.updateScanProgress({ files_scanned, large_files_skipped, current_scanning_path })` | `ScanProgressSchema`      |
| `ShowFilePreview`      | `window.showPreviewContent(content, language, searchTerm, path)`                           | `ShowPreviewArgsSchema`   |
| `ShowGeneratedContent` | `window.showGeneratedContent(content, tokenCount, documentId)`                             | `ShowGeneratedArgsSchema` |
| `OutputSize`           | `window.outputSizeNotice({ status, estimated_bytes, limit_bytes })`                        | `OutputSizeNoticeSchema`  |
| `ShowError`            | `window.showError(message)`                                                                | `StatusMessageSchema`     |
| `ConfigExported`       | `window.showStatus(message)`                                                               | `StatusMessageSchema`     |
| `SaveResult`           | `window.saveResult({ status, … })`, see [Saving](#saving)                                  | `SaveResultSchema`        |
| `DragStateChanged`     | `window.setDragState(isDragging)`                                                          | `DragStateSchema`         |

Errors are reported as plain strings through `showError`; there is no error-code protocol. Two outcomes have a type of their own because the UI words them itself, in its language: how a save ended (`saveResult`) and what the size of an output means for its generation (`outputSizeNotice`).

### File content

Whether a file is text or binary is decided by one rule (`is_binary` in `src/utils/file_detection.rs`), for the scan (`is_binary` of a tree node), the file preview, the content search and the generated output alike, so they never disagree about a file:

- A file is binary if its extension names a format that is certainly binary: images (not SVG), audio and video, archives and packages, executables and other compiler output (`wasm` among them), PDF and the zipped office formats, databases, fonts. The case of the extension does not matter.
- Every other file is what its content says: binary if its first kilobyte holds a NUL byte or, where it is not valid UTF-8, more than 5 % control characters; text otherwise. An empty file is text.

An extension never makes a file text, and extensions that are merely often binary (`dat`, `tmp`, `bak`, `cache`, `d`, `snap`, `raw`, ...) are left to the content. A symbolic link is judged by the name it is listed under and the content of the file it points to; a link that leads to no file is not binary (its preview and its block in the output report it as unreadable).

The text of a file is decoded by one rule as well (`decode_text` in `src/core/file_handler.rs`), for the file preview (`showPreviewContent`), the generated output (`showGeneratedContent`) and the content search:

1. Binary content (by the rule above) is not decoded.
2. Valid UTF-8 is used as it is.
3. Otherwise it is text that is not valid UTF-8 as a whole. If it holds at least one valid multi-byte UTF-8 sequence, and at least as many of them as invalid sequences, it is UTF-8 with damaged places: each invalid sequence becomes U+FFFD (`�`), everything else stays as it is. If not (a file in a legacy single-byte encoding has invalid sequences but practically never a valid multi-byte one), it is decoded as **Windows-1252**, the superset of ISO-8859-1 (Latin-1), which maps every byte to one character.

The generated output is a header (`# CFC Output - Generated: …`, `# Total files: n`), optionally the directory tree, and one block per selected file: the path, `===FILE-START===`, the content, `---FILE-END-----`. Where the content cannot be embedded, the block holds a marker instead; the generation never fails because of a single file:

| Marker                                          | When                                                                                                                                      |
| ----------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------- |
| `[FILE TOO LARGE: <n> bytes - CONTENT SKIPPED]` | The file is larger than `max_file_size_mb`                                                                                                |
| `[BINARY OR NON-UTF8 FILE - CONTENT SKIPPED]`   | The content is binary by rule 1                                                                                                           |
| `[FILE UNREADABLE: <reason> - CONTENT SKIPPED]` | The file could not be read, e.g. it was deleted or lost its permissions since the scan; `<reason>` is the message of the operating system |

### The generated output

`generatePreview` plans the generation while it holds the state lock (`GenerationPlan` in `src/app/tasks.rs`): the selected files in tree order, the options, the file list for the tree (copied only if the output gets a tree) and an **estimate of the size of the output**. The estimate is told from the file sizes the scan has recorded, without opening a file (`FileHandler::estimate_output_size`); for files that have not changed since the scan it is a few bytes per file above the real size. The generation works with this plan: what is selected or deselected while it runs belongs to the next one.

The size decides what happens (`LARGE_OUTPUT_SIZE`, `MAX_OUTPUT_SIZE` in `src/app/tasks.rs`; constants, not configuration):

| Estimated output | What happens                                                                                                                                                                                                                                                                |
| ---------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| up to 20 MiB     | The output is generated                                                                                                                                                                                                                                                     |
| above 20 MiB     | The output is generated; `outputSizeNotice({ status: "large", … })` follows the snapshot with `is_generating: true`. The UI shows a warning toast                                                                                                                           |
| above 200 MiB    | **Refused**: `outputSizeNotice({ status: "too_large", … })` and a snapshot. Nothing changes: a generation that is running keeps running, the output that is shown stays (and can be saved), the output filename is not touched. The UI shows an error toast with both sizes |

Why these numbers: the output is handed to the WebView as one string and shown by the editor as a whole, on the UI thread, where it cannot be cancelled. The editor needs about 23 ms per megabyte to take a text over (measured in Chromium and WebKit: 20 MB 0.5 s, 50 MB 1.2 s, 200 MB 4.7 s). Below 20 MiB a generation is a matter of well under a second; 200 MiB block the UI for about 5 s, are within a factor of 2.5 of what a string can hold in the WebView of Windows (V8: 512 MiB of characters) and amount to roughly 50 million tokens, far more than the output is made for.

The limit is enforced a second time while the document is built: should it grow beyond 200 MiB all the same (files have grown since the scan), the generation ends with `showError("The output would be larger than 200 MiB. Select fewer files.")` (`CoreError::OutputTooLarge`).

The tokens of the output (`tokenCount`, the `cl100k_base` encoding) are counted on the blocking thread pool, from the document itself (it is shared, not copied), in pieces of about 1 MiB on up to four threads (`src/core/token_counter.rs`). The pieces end where the encoding splits the text in any case (behind a line end that is followed by text), so the count is the count of the whole text. Between two pieces the count looks at the cancellation flag of its generation: `cancelGeneration`, a new generation or closing the directory end it within a piece. A piece cannot end inside a line, so a line that is longer than 1 MiB (a minified bundle, a dump on one line) is one piece however long it is: it is counted on one thread and cannot be cancelled before it is done, which takes about 50 ms per megabyte (1 s for a line of 20 MiB, the most a single file holds with the default limit).

**The host keeps the document.** A finished generation stores its document in the state (`AppState::complete_generation`, `GeneratedDocument` in `src/app/state.rs`) under the id of its run and sends it with that id: `showGeneratedContent(content, tokenCount, documentId)`. The document stays until the next one replaces it, also while a newer generation is running or after one was cancelled, because the UI still shows it during that time. It is given up exactly where the UI stops showing it: when a file preview is shown (`showPreviewContent`) and when the preview is cleared (`clearPreviewState`). Opening another directory (a drop, the directory dialog, reopening the same directory, an import) does **not** give it up: the output stays in the editor then, and can still be saved. Closing the directory (`clearDirectory`) empties the editor, and the UI reports that with `clearPreviewState` like any other clearing.

#### Saving

The UI does not send the output back to have it saved. `saveFile` says what is to be written (`SaveFilePayload` in `commands.rs`, `SaveFilePayloadSchema`):

| Payload                               | When                                                                           | What the host writes        |
| ------------------------------------- | ------------------------------------------------------------------------------ | --------------------------- |
| `{ kind: "generated", documentId }`   | The editor shows the output as it came (`outputToSave` in `modules/editor.ts`) | Its own copy, byte for byte |
| `{ kind: "edited", content: string }` | The user has changed the text in the editor                                    | `content`                   |

Whether the text was changed is told by the version of the editor's model: Monaco's alternative version id is recorded when the output is shown; it changes with every edit and returns to the recorded value when the edits are undone, so an output that was edited and restored with undo is saved as `generated` again.

What is saved is decided when the command starts, before the dialog opens; a generation that ends while the user chooses the file does not change it. The command is answered with exactly one `saveResult`:

| Result                        | Meaning                                                                                                                                                                                |
| ----------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `{ status: "saved", path }`   | The file was written                                                                                                                                                                   |
| `{ status: "cancelled" }`     | The dialog was dismissed; nothing was written                                                                                                                                          |
| `{ status: "failed", error }` | Nothing was saved. `error` is the message of the operating system, or the host's own if it does not hold the document that was named (any more); the dialog is not opened in that case |

#### Copying

Copying is done by the UI (`handleCopy` in `modules/clipboard.ts`), and it follows the same rule as saving, so that the clipboard and the saved file hold the same text:

- While the output is unchanged, a copy of the whole output (the Copy button, or a copy with nothing or everything selected) puts the **text the host sent** on the clipboard. The UI keeps that string next to the editor (`unchangedOutput` in `modules/editor.ts`).
- Once the user has edited the output, the editor's text is the one text there is: it is what a copy takes and what a save sends.
- A copy of a selected part is always taken from the editor.

The distinction matters for line ends. The output keeps the line ends of its files, and Monaco gives a text one kind of line end: for an output of files with CRLF and LF line ends, the editor's value (`getValue()`) has all of them rewritten to the more frequent kind. An unedited output is therefore neither saved nor copied through the editor. The first edit makes the editor's text the output, with its uniform line ends, for the copy and the save alike.

A payload that is neither of the two kinds is logged and not answered. The UI shows a toast per result (success, info, error with the reason) in its language.

Measured for a selection of 2,560 files of 20 KiB each, an output of 52.8 MB (`large_output_benchmark` in `src/app/commands.rs`, release build, Apple M-series; the time until the output is shown adds the 1.2 s of the editor in both columns):

|                                                    | Output sent back for saving (before) | Host keeps the document |
| -------------------------------------------------- | ------------------------------------ | ----------------------- |
| Generation and token count                         | 2.6 s                                | 1.2–1.6 s               |
| Script for the WebView                             | 29 ms                                | 37 ms                   |
| Save: message from the UI                          | 53 MB                                | 68 bytes                |
| Save: parsing and writing on the host              | 32 ms                                | 6 ms                    |
| Peak memory of the host: generated / shown / saved | 204 / 255 / 298 MB                   | 107 / 163 / 163 MB      |

For an output of 200 MB (just below the limit) the host needs 4.5 s instead of 10.3 s and 471 MB instead of 1,049 MB.

The preview shows the first 1500 lines, followed by `...` and `[Preview truncated]` if the file has more. It reads only the beginning of the file: it stops once it has these lines and never reads more than 4 MiB, so a file whose first lines are longer than that is cut there (never inside a character). Rule 3 is applied to what was read. Instead of text the preview is `[DIRECTORY]` or `[BINARY FILE]` where that applies.

`loadFilePreview` reads entries of the loaded list only: the path has to be one the scan has listed (the path as listed; it is compared, not resolved, so neither `..` nor a symbolic link leads elsewhere). Any other path, inside or outside the loaded directory, is not read: the command is answered with `ShowError` and a snapshot, and the preview that is shown stays.

### Scan progress

While a scan walks the directory it reports its progress at most every 100 ms. Each report is sent as `ScanProgress` (the live display of the scan panel) and stored in the host's state, so the snapshots sent during a scan carry the progress as of that moment in `scan_progress` and in `status_message` ("Scanning... 40 files processed. 2 large files skipped (/repo/src/main.rs)"). A scan that has not reported anything yet reads "Scanning... 0 files processed. 0 large files skipped."; when a scan has ended, the status is its outcome and `scan_progress` keeps the last counters. Reports of a scan that was cancelled or replaced are neither stored nor sent.

### Snapshots during a scan

A scan publishes what it finds while it runs. After the snapshot with the first level, further snapshots follow with `is_scanning: true`, `is_fully_scanned: false` and a tree that has grown: at most one every 150 ms, less often as the list grows (publishing takes about a tenth of the time of the scan). The UI needs nothing special for them; they are snapshots like any other.

- `children_loaded` of a directory stays `false` until its content is known to be complete: when the scan has ended (then for all directories at once), or when its level was loaded with `loadDirectoryLevel`. An expanded directory can therefore show some of its children with `children_loaded: false` during a scan.
- `loadDirectoryLevel` is answered during a scan like at any other time: the directory is expanded at once, and a snapshot with its complete level follows. The scan goes on and does not list that level a second time.
- No entry is listed twice, and the list of a completed scan is exactly what its walk found, whatever was loaded on the way.
- With an active content query, entries the scan adds are matched when the search is repeated at the end of the scan (or after a `loadDirectoryLevel`); until then they are not among the matches.
- `cancelScan` keeps what has been published: the tree stays as far as the scan got, `is_fully_scanned` stays `false`, and the directories can be loaded on demand.
- A re-scan starts again from the first level: directories that were expanded stay expanded and fill up as the scan gets to them.

### Opening a path

Opening a path (a drop, the choice of the directory dialog, the `last_directory` of an imported config) replaces the loaded directory. Whether the path exists is not known when the command runs, and finding out is IO that does not belong on the command lane. The command therefore only **announces** the scan: the first snapshot has `is_scanning: true`, and everything else is still the directory that was loaded before (`current_path`, tree, selection, filters). The scan task then looks at the path:

- A directory is entered: the previous directory is reset at that moment and `current_path` changes, with a snapshot (`is_scanning: true`, empty tree). Then the directory is scanned.
- For a file (a dropped file) the same happens with the directory the file lies in.
- A path that does not exist ends the scan: `showError("Path does not exist: …")`. **The directory that was loaded stays exactly as it was**, with its list, selection, expansion and filters. Whatever the attempt had superseded when it was announced is taken up again: a scan of the loaded directory that was still running is restarted (announced with a snapshot of its own), and so is the opening of another path that had not been looked at yet (of a drop of an existing directory followed by a drop of a missing path, the directory is loaded). Otherwise a snapshot with `is_scanning: false` follows the error.

Until the path has been looked at, commands act on the directory that is still loaded, and the announced scan is the current one: `cancelScan` cancels it (the loaded directory stays), `clearDirectory` ends with no directory loaded, another drop replaces it. `rescanDirectory` does nothing in that phase (the opening delivers a fresh listing anyway) and is answered with the unchanged snapshot.

`rescanDirectory` itself keeps the loaded directory by definition; if the directory has vanished, the scan ends with the same error and the list stays, as complete as it was (`is_fully_scanned` is untouched, so "remove empty directories" keeps working on it). A re-scan reports `is_fully_scanned: false` from the moment it has found its directory, with a snapshot, until it completes.

A scan task that ends without having finished its scan (a panic at any point, from the first look at the path on) releases the scanning state and sends a snapshot with `is_scanning: false`.

The scan is reported as complete (`is_scanning: false`, status "Indexing complete …") as its very last step. With an active content query that is after the search has been repeated on the complete file list, so the number of visible items in the status is the one of the final list. Until then the snapshot can show `is_fully_scanned: true` together with `is_scanning: true`.

## The state snapshot (`UiState`)

Produced by `generate_ui_state` (`src/app/view_model/mod.rs`), validated by `UiStateSchema`.

| Field                                                      | Meaning                                                                |
| ---------------------------------------------------------- | ---------------------------------------------------------------------- |
| `config`                                                   | The settings the UI displays (`UiConfig`), see below                   |
| `current_path`                                             | Scanned root directory (`""` if none)                                  |
| `rows`                                                     | The rows of the tree inside the window, in order, see below            |
| `rows_start`                                               | The position of `rows[0]` in the list of all rows                      |
| `rows_total`                                               | The number of rows of the tree: what the scrollbar spans               |
| `rows_revision`                                            | Changes when the list of all rows does, and only then                  |
| `located`                                                  | The answer to the last `locateRow`, or `null`                          |
| `total_files_found`                                        | Items (files and directories) in the unfiltered list                   |
| `visible_files_count`                                      | Items (files and directories) in the filtered list                     |
| `tree_files_count`                                         | Files in the complete filtered tree, including the ones not sent       |
| `tree_dirs_count`                                          | Directories in the complete filtered tree, including the ones not sent |
| `selected_files_count`                                     | Selected files                                                         |
| `is_scanning`, `is_generating`, `is_fully_scanned`         | Activity flags                                                         |
| `status_message`, `scan_progress`                          | Status line and scan progress, see [Scan progress](#scan-progress)     |
| `search_query`, `extension_filter`, `content_search_query` | Active filters                                                         |
| `current_config_filename`                                  | Name of the imported config, or `null`                                 |
| `active_ignore_patterns`, `patterns_need_rescan`           | Ignore pattern bookkeeping                                             |

### The config view

`config` is a view of its own (`UiConfig` in `src/app/view_model/ui_config.rs`, `UiConfigSchema`), not the persisted `AppConfig`: `ignore_patterns` (sorted), `case_sensitive_search`, `remove_empty_directories`, `include_tree_by_default`, `use_relative_paths`, `output_directory` (`string | null`) and `output_filename`. It is an allow-list: a field added to `AppConfig` is not sent unless it is added to the view, the schema and the fixture. The UI reads it and edits it with the commands in [Config changes](#config-changes).

### The rows of the tree

The tree is shown as a flat list of **rows**: every item of the filtered file list whose ancestors are all expanded, in display order (directories first, then by name, each directory followed by its content). The host owns that list (`VisibleRows` in `src/app/view_model/rows.rs`); the UI never holds it. A snapshot carries

- `rows_total`, the length of the list (the UI sizes its scroller with it),
- `rows`, the rows of one **window** of the list, and `rows_start`, the position of the first of them.

The window is where the UI looks. The UI reports its viewport with `setViewport { start, count }` (the rows it shows plus a buffer, see [Virtualization Patterns](./virtualization.md)), the host stores it and every snapshot from then on, whatever command it answers, carries the rows `start .. start + count` of the list as it is at that moment. So a snapshot stays self-contained: it never depends on rows sent earlier, and dropping one in favour of a newer one (see `transport.rs`) loses nothing.

- Until the UI has reported a viewport the window is the first 100 rows (`Viewport::DEFAULT_ROWS`).
- `count` is at least 1; a viewport without rows is refused like any malformed payload (answered with the unchanged snapshot), by the host and by the UI's schema alike.
- The host sends at most 600 rows (`Viewport::MAX_ROWS`), whatever `count` says. This is what bounds a snapshot. The UI knows the two limits as constants (`MAX_WINDOW_ROWS`, `DEFAULT_WINDOW_ROWS` in `treeWindow.ts`), its schema rejects a snapshot with more rows, and the [contract tests](#contract-tests) compare the constants with what the host states.
- A window that would start behind the end of the list (a directory was collapsed, a filter applied) is moved up so that it ends with the list. `rows_start` states where the rows really start; the UI must not assume it is what it asked for.
- A new tree is shown from the top: loading or closing a directory, changed filter texts and `initialize` set `start` to 0 (the UI scrolls to the top on the same occasions).
- `rows_start + rows.length <= rows_total` always holds; the schema rejects anything else.

`TreeRow`:

| Field                               | Meaning                                                                                                                                                |
| ----------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `name`, `path`                      | Display name and absolute path (the row's identity). A path that is not valid Unicode is sent lossily (U+FFFD); see below                              |
| `depth`                             | The nesting level, 0 for the items directly below the loaded directory. A row belongs to the nearest row above it that is less deep                    |
| `is_directory`, `is_binary`, `size` | Kind and size                                                                                                                                          |
| `has_children`                      | The directory has children in the filtered list, whether or not it is expanded. `false` for files and for directories that are empty or not loaded yet |
| `is_expanded`                       | The directory is expanded. Authoritative; its children are rows of the list exactly while this is `true` (and all of its ancestors are expanded)       |
| `children_loaded`                   | `false` for a directory whose content has not been scanned yet (lazy loading). Always `true` for files                                                 |
| `selection_state`                   | `"none" \| "partial" \| "full"`. For directories it is made of **all** files below it: also below collapsed directories, also far outside the window   |
| `is_match`                          | The item matches the filename, extension or content filter                                                                                             |
| `is_previewed`                      | The file is shown in the preview panel                                                                                                                 |

Every field is required. A row has no children: what is below a directory are the rows that follow it with a greater `depth`.

Paths are written lossily (`src/app/view_model/lossy_path.rs`; also `config.output_directory` and the path of `showPreviewContent`): serde refuses a path that is not valid Unicode, and a single such file name would otherwise cost the UI the whole snapshot. For every valid path nothing changes. The row of such a file is shown, but the path the UI sends back for it is not the one the host knows, so commands for it have no effect.

Because the UI holds a window, it must not derive anything about the tree from `rows`. `tree_files_count` and `tree_dirs_count` describe the complete filtered tree and are independent of what is expanded or sent. (`visible_files_count` is the raw length of the filtered list and counts directories and items outside the tree as well; the file tree header uses the `tree_*` counters.) `rows_total` counts rows, i.e. depends on what is expanded.

`rows_revision` identifies the list: it changes when the list is built anew (another filtered file list, a directory expanded or collapsed) and stays the same for everything else (selection, preview, scrolling, a row lookup). While it is the same, a position in the list means the same row. It is 0 while no tree is shown.

#### What the host keeps for it

Nothing of this is recomputed per snapshot unless it has changed (measured below):

| Derived data                        | Rebuilt when                                                          | Otherwise                                     |
| ----------------------------------- | --------------------------------------------------------------------- | --------------------------------------------- |
| Hierarchy (`TreeIndex`)             | the filtered file list or the root path changes                       | reused                                        |
| List of rows (`VisibleRows`)        | the hierarchy changes, or a directory is expanded or collapsed        | reused: selection, preview, scrolling, lookup |
| Selected files below each directory | the hierarchy changes, or the selection changes wholesale (see below) | follows single changes                        |
| The rows of the window (`TreeRow`s) | every snapshot (at most 600 rows)                                     |                                               |

The selection counts follow the journal of `SelectedFiles` (`src/app/selection.rs`): selecting or deselecting one file adjusts the directories above it. Changes of up to 16,384 files at once (a click on a directory of that size) are followed file by file; larger ones ("select all", a click on a larger directory) and "deselect all" are counted again: per selected file if the selection is small compared to the list, else in one pass over the tree. These commands walk their files anyway. For 500,000 listed files: selecting or deselecting a directory of 10,000 files 4 ms, "select all" 86 ms (collecting, hashing and counting 500,000 paths; 242 ms before the set was filled in one go and counted without an intermediate journal), "deselect all" 13 ms, a directory of 20,000 files while most of the list is selected 50 ms (the one case that still counts the whole list).

#### Measured

A synthetic project, everything expanded (the worst case), release build, `cargo test --release snapshot_of_a_huge -- --nocapture` (`src/app/view_model/windowing_tests.rs`, which also asserts the size and a generous time budget). Before: the snapshot held every row, nested, for every update. Now: a window of 200 rows.

| Files (directories) | Before: JSON | Before: build + serialize | Now: JSON | Click on a checkbox | Scroll (`setViewport`) | Expand/collapse one directory |
| ------------------- | ------------ | ------------------------- | --------- | ------------------- | ---------------------- | ----------------------------- |
| 25,000 (1,300)      | 6.6 MB       | 1.9 ms + 4.6 ms           | 55 kB     | 26 µs + 39 µs       | 26 µs + 39 µs          | 0.13 ms + 39 µs               |
| 100,000 (5,050)     | 26.3 MB      | 7.6 ms + 17.3 ms          | 55 kB     | 28 µs + 40 µs       | 28 µs + 38 µs          | 0.44 ms + 39 µs               |
| 500,000 (25,050)    | 131 MB       | 48 ms + 88 ms             | 55 kB     | 24 µs + 35 µs       | 29 µs + 35 µs          | 2.3 ms + 36 µs                |

(Build + serialize on the host; parsing, validating and flattening megabytes of JSON in the WebView came on top and is gone as well.) The largest window the host sends, 600 rows, is 163 kB. What still grows with the project is the work after the list itself has changed: the first snapshot for a new filtered list builds the hierarchy, the rows and the counts (6 ms / 22 ms / 113 ms), and expanding or collapsing rebuilds the list of rows (last column).

### Looking up a row

The UI knows the rows of its window. For everything else it asks: `locateRow { request, target }`, where `request` is a number the UI chooses and `target` is one of

| `target`                              | The row that is looked for                                                                    | Used for                                  |
| ------------------------------------- | --------------------------------------------------------------------------------------------- | ----------------------------------------- |
| `{ kind: "path", path }`              | The row of the item at `path`                                                                 | Keeping the focus on its item (see below) |
| `{ kind: "parent_of", path }`         | The row of the directory the item at `path` lies in                                           | `ArrowLeft` when the parent is not held   |
| `{ kind: "typeahead", prefix, from }` | The first row from row `from` on, wrapping around, whose name starts with `prefix` (any case) | Typing a name in the tree                 |

The host resolves the target to a path, remembers it, and **every** snapshot from then on states where that path is in its own list: `located: { request, row }`, with `row: null` if there is no such row (not listed, below a collapsed directory, nothing found). The answer is therefore true for the snapshot it arrives with, also when the list has changed in between or the snapshot that was sent first has been dropped. The UI acts on the answer to the request it is waiting for and ignores every other; `located` is `null` until the first lookup and again after a new tree is shown or the UI has started (`initialize`).

A `typeahead` with an empty `prefix` asks for nothing: the host refuses it like a malformed payload (the snapshot that answers it still carries the previous `located`), as does the UI's schema.

A lookup does not move the window. The UI scrolls to the row (if it wants to show it), which reports the viewport, and the rows arrive with the answer to that.

### Expansion: one source of truth

`AppState::expanded_dirs` on the host is the only place that knows which directories are expanded. The UI keeps **no** expansion state of its own (there used to be an "expansion memory" that overrode `is_expanded` on arrival; the children of a directory are rows only while it is expanded on the host, so a directory that is expanded only in the UI would have nothing to show, and the memory also undid the host's search auto-expansion).

Every way of changing the expansion is therefore a command, and the result is visible with the next snapshot:

| Gesture                                                                    | Command                                                                        |
| -------------------------------------------------------------------------- | ------------------------------------------------------------------------------ |
| Click on chevron or directory name, `Enter`                                | `setExpansion` with the opposite of the rendered `is_expanded`                 |
| `ArrowRight` on a collapsed directory                                      | `setExpansion { expanded: true }`                                              |
| `ArrowLeft` on an expanded directory                                       | `setExpansion { expanded: false }`                                             |
| A gesture that expands a directory with `children_loaded: false` (no scan) | `loadDirectoryLevel`; the host loads the level, marks it loaded and expands it |
| "Expand all" / "Collapse all"                                              | `expandCollapseAll`                                                            |
| Typing into a filter                                                       | `updateFilters`; the host expands the ancestors of the matches, see below      |

The mapping from gesture to command lives in `src/ui/src/lib/modules/treeExpansion.ts`. Expanding a directory whose children are not loaded sends `loadDirectoryLevel`, also while a scan is running: the scan fills the tree piece by piece, the level the user asks for is listed at once (see [Snapshots during a scan](#snapshots-during-a-scan)). Only expanding loads: a directory that is already expanded without loaded children (expanded during a scan that was cancelled) is collapsed with `setExpansion { expanded: false }` like any other, and is loaded when it is expanded the next time.

Keys are handled in one place, the key handler of the tree in `FileTree.svelte` (roving focus): `Enter` toggles a directory or opens a file, `Space` toggles the selection, wherever the focus is inside the row. Moving the focus (`ArrowUp`/`ArrowDown`, `PageUp`/`PageDown`, `Home`/`End`) is arithmetic on positions in the list, so it also reaches rows the UI does not hold: the tree scrolls there, the rows arrive, the row gets the focus. Where a position cannot be computed (the parent of a row, a name typed into the tree, the focused item after the list has changed around it) the UI [asks the host](#looking-up-a-row). The one exception are the keys a control of the row acts on by itself (`activatesNatively` in `src/ui/src/lib/modules/nativeActivation.ts`): `Enter` and `Space` on the ignore button, `Space` on the checkbox. The tree leaves them to the control, whose click handler runs once (`addIgnorePath`, `toggleSelection` / `toggleDirectorySelection`); the global key guard (`keyboard.ts`), which blocks keys nothing handles, lets them through for the same reason. `TreeItem.svelte` handles the pointer only (a key that activates a control arrives there as its `click`).

**"Expand all" and "Collapse all" act on the directories the filters show**, i.e. the directories of the filtered list. Under an active filter, "Expand all" adds them to what is expanded and "Collapse all" removes exactly them; directories the filter hides keep their state, so what was expanded before the filter is still expanded when the filter is cleared. Without a filter both reach every listed directory.

**Automatic expansion and manual collapse.** Whenever the filters are applied with a result that can show matches (typing into a filter, the answer of a content search, its repetition after a lazy load, a change of the case sensitivity or the end of a scan), the host expands the directories above the matches. One rule keeps that from fighting the user (`AppState::reveal_directory`):

> A directory the user collapses while a filter is active (`setExpansion { expanded: false }`, "Collapse all") is not expanded by the host again until the filters are cleared, i.e. until all three filter texts are blank, or until the user expands it (`setExpansion { expanded: true }`, "Expand all", `loadDirectoryLevel`).

So collapsing a directory with matches and typing on leaves it collapsed, as does everything that merely applies the same filters again; a match in a directory the user has not collapsed is still revealed. Loading another directory (which clears the filters) ends the hold as well.

**The automatic expansion is limited.** One application of the filters expands the directories above matches in the order of the tree and stops after 500 of them (`AUTO_EXPAND_LIMIT` in `src/app/filtering.rs`). A filter that matches broadly (an extension most files have) would otherwise expand every directory of the project: a list nobody reads, and an expanded set that stays that large after the filter is gone. The matches further down are still listed, counted (`tree_files_count`) and highlighted; their directories stay collapsed until the user opens them (a click, "Expand all") or narrows the filter. Finding the directories is one pass over the filtered list: for 500,000 matching files in 25,000 directories it takes 6 ms, where expanding all of them took 160 ms (8 ms → 0.3 ms at 25,000 files, 32 ms → 0.8 ms at 100,000).

There is no optimistic update: a row changes when the snapshot arrives. Snapshots are cheap enough for that, and it rules out the UI and the host disagreeing.

**Gestures name their target state.** Because there is no optimistic update, a gesture can fire a second time before the snapshot answering the first one has been rendered: key repeat on `ArrowRight`, a double click on the chevron. The second event still sees the old row. If the UI sent "flip it", the second event would undo the first and the directory would end up collapsed again. The UI therefore always sends the state the gesture should lead to, derived from the row the user acted on, and `setExpansion` is idempotent on the host. This also holds for the gestures that read as "toggle" (chevron, name, `Enter`): the user acts on what is on screen ("open this collapsed directory"), so two activations of the same rendered row ask for the same state twice. There is no "flip it" command in the contract.

## Contract tests

- **Rust → TypeScript fixture.** `src/app/view_model/contract_tests.rs` serializes a representative `UiState` (every kind of row, a window that is not the whole list, an answered row lookup) and compares it with `src/ui/src/tests/contracts/fixtures/ui_state_window.json`. `src/ui/src/tests/contracts/rust-fixture.test.ts` parses that exact file with the full `UiStateSchema` and checks that no field of the state, of its config or of a row is unknown to the schema. A change on either side that the other does not follow fails CI. Regenerate the fixture after an intended change with

  ```sh
  UPDATE_CONTRACT_FIXTURES=1 cargo test contract_fixture
  ```

- **TypeScript → Rust fixture (commands).** `src/app/command_contract_tests.rs` writes the routing table `COMMANDS` with example payloads into `src/ui/src/tests/contracts/fixtures/commands.json`; payloads with a type on the host (`ConfigOption`, `IgnorePatternPayload`, `SaveFilePayload`, `SetExpansionPayload`, `SetViewportPayload`, `LocateRowPayload`, `UpdateFiltersPayload`) are serialized from that type, one example per `ConfigOption` and per kind of `RowTarget` and of `SaveFilePayload`. These types reject unknown fields, and so do their schemas (`.strict()`). `rust-fixture.test.ts` checks that `CommandSchemas` has exactly the same command names, that every example passes the schema of its command unchanged, and that both sides know the same options of `setConfigOption` and the same kinds of row lookup. `COMMANDS` is what the router resolves names with, so a command cannot be routed without being part of the contract. The fixture is regenerated with the same command as above.
- **Rust → TypeScript fixture (host calls of the output).** `src/app/event_contract_tests.rs` builds every form of `ShowGeneratedContent`, `OutputSize` and `SaveResult`, turns each into the script the WebView evaluates and writes the arguments of those calls into `src/ui/src/tests/contracts/fixtures/output_events.json`. `rust-fixture.test.ts` checks that every example passes the schema of its handler unchanged and that both sides know the same statuses. Regenerated with the same command.
- **Schema tests.** `src/ui/src/tests/contracts/` covers the command union, the state schema, property tests and JSON-schema snapshots (`npm run test:contracts`).
- **The window, replayed (Rust → TypeScript).** The UI's tests have no host; what stands in for it (the E2E bridge and the fake host of the component tests) is one model, `src/ui/src/lib/dev/treeHostModel.ts`. `src/app/window_contract_tests.rs` runs a scenario through the real commands and writes what the host answered at every step into `fixtures/tree_window_scenario.json`: `setViewport` (also too large, behind the end, malformed), `locateRow` of every kind (also wrapping around, without a match, malformed), a list that changes, a selection that changes, filters that change, `initialize`; per step `rows_start`, the number of rows, `rows_total`, whether `rows_revision` changed, `located`, the first and last path. `tree-window-scenario.test.ts` replays the file against the model and requires the same answers, and that every snapshot of the model passes `UiStateSchema`. The fixture also states `Viewport::DEFAULT_ROWS` and `Viewport::MAX_ROWS`, which the test compares with the UI's constants and with what the schema accepts.
- **Rows.** `src/app/view_model/pruning_tests.rs` and the property tests in `cache_tests.rs` compare the rows with the original full-tree algorithm, pruned to the expanded part (the rows are put back into nested form for that, `reference::nest`). `windowing_tests.rs` covers the window (a property test: any viewport yields exactly that slice of the list), when the list is rebuilt, the row lookup, and the cost of a snapshot.

## Changing the contract

1. **Host:** add the command to `commands.rs`, to `OrderedCommand` (or, if it waits for a native dialog, to `DialogCommand`) and to `COMMANDS` in `src/app/mod.rs`; the compiler then asks for its handler in `run_command` and for example payloads in `command_contract_tests.rs`. A field goes into `UiState` / `UiConfig` / `TreeRow`.
2. **UI:** extend `CommandSchemas` / `UiStateSchema` in `schema.ts`. The types in `types.ts` are derived from the schemas. Prefer additive changes.
3. Regenerate the fixtures (see above) and update the factories in `src/ui/src/tests/contracts/`.
4. Keep the snapshot self-contained: never send something that is only meaningful relative to an earlier update.
5. Update this document.

## Transport notes

The host takes commands from the UI's own origin only. wry reports the URL of the document a message was posted from; a message from any other document (`about:blank`, a `data:` or `blob:` document, a web page) is logged and dropped before it is parsed (`accepted_message` in `src/platform/ipc.rs`, decided by `AppOrigin::is_own_page`). The origin is `cfc://index.html` in a release build (`http://cfc.index.html` on Windows, where wry rewrites custom schemes) and the Vite dev server `http://localhost:1420` in a debug build. See [Security of the WebView](./architecture.md#security-of-the-webview).

The host serializes each `UserEvent` on the UI thread and evaluates the resulting `window.*` call in the WebView. An event that cannot be serialized is logged and skipped; no call is made for it. Consecutive undelivered state updates are coalesced to the latest one; the order relative to all other events is preserved (`src/app/transport.rs`).

In E2E tests there is no host. `$lib/dev/e2eBridge.ts` (installed only when `$lib/dev/e2eShim.ts` allows it) exposes `window.__e2e.store.setAppState`, which sets the store directly. States seeded this way have to follow the same rules as real snapshots: `rows` is a window of a list of `rows_total` rows (all of it, for a small tree), children are rows only below expanded directories, counters filled in. For a list that is longer than a window the bridge has a stand-in for the host's windowing, `window.__e2e.host.setTree(state, rows)`: the model that the contract test above holds against the host (`treeHostModel.ts`), wired to the IPC. It validates the commands the UI posts with `CommandSchemas`, answers `setViewport`, `locateRow` and `initialize` asynchronously, and delivers its snapshots through `window.render`, so they pass the same validation as snapshots of the host (`e2e/tree-window.spec.ts`, the 25k budgets and benchmarks). `host.setLatency(ms)` makes it answer late, for tests of what the UI does while rows are under way.
