# IPC Contracts

> The UI and the host talk over two one-way channels: the UI **posts commands**, the host **calls `window.*` handlers**. There are no request/response pairs and no promises: a command never returns a value, its effect arrives as the next state snapshot.

## Overview

```text
UI  ──  window.ipc.postMessage('{"command": "...", "payload": ...}')  ──▶  Host
UI  ◀──  window.render(uiState) / window.showPreviewContent(...) / ...  ──  Host
```

| Concern                         | Where                                                                          |
| ------------------------------- | ------------------------------------------------------------------------------ |
| Sending commands                | `src/ui/src/lib/services/backend.ts` (`post`)                                  |
| Command and state schemas (Zod) | `src/ui/src/lib/ipc/schema.ts`                                                 |
| Receiving host calls            | `src/ui/src/lib/ipc/handlers.ts` (`installWindowIPCHandlers`)                  |
| Frontend types                  | `src/ui/src/lib/types.ts` (derived from the schemas with `z.infer`)            |
| Command dispatch (host)         | `src/app/mod.rs` (`COMMANDS`, `dispatch_ipc_message`) → `src/app/commands.rs`  |
| Command ordering (host)         | `src/app/dispatch.rs` (`CommandDispatcher`)                                    |
| Host → UI events                | `src/app/events.rs` (`UserEvent`), `src/app/mod.rs` (`handle_user_event`)      |
| State snapshot (host)           | `src/app/view_model/` (`generate_ui_state`, `UiState`, `UiConfig`, `TreeNode`) |
| Delivery / coalescing           | `src/app/transport.rs`                                                         |

## Principles

- **Commands are fire-and-forget.** `post()` returns `void`. The host mutates its `AppState` and answers with a state snapshot (and, where applicable, one of the other host calls).
- **Commands take effect in the order they were posted.** The host executes them one after the other, see [Command ordering](#command-ordering).
- **Every state update is a complete snapshot.** `window.render(uiState)` replaces the UI's `appState` store wholesale. There are no diffs and no patches; a snapshot never depends on an earlier one. This is what allows the transport to drop a pending snapshot when a newer one is already available (latest wins, see `transport.rs`).
- **The host owns the state, the UI owns none of it.** In particular the host is the single source of truth for selection and for **expansion** (see below). The UI renders what it receives.
- **Snapshots are pruned to what is visible.** The tree only contains the children of expanded directories; totals are sent as counters.
- **Validation at the edge.** Outgoing commands are validated against `CommandSchemas` in DEV. Incoming calls are validated in `handlers.ts`: `render()` with the full `UiStateSchema` in DEV and tests, and with the envelope-only `UiStateEnvelopeSchema` in production (the recursive tree walk stays off the hot path). Invalid payloads are ignored with a console warning.
- **No untyped `any` at the boundary.**

## Commands (UI → Host)

The single source of truth on the UI side is `CommandSchemas` in `schema.ts`, on the host the routing table `COMMANDS` in `src/app/mod.rs`. The two are compared by the [contract tests](#contract-tests): same command names, and every example payload the host serializes passes the schema of its command. The wire format is `{"command": <name>, "payload": <payload | null>}`.

| Command                         | Payload                                                 | Effect                                                                          |
| ------------------------------- | ------------------------------------------------------- | ------------------------------------------------------------------------------- |
| `initialize`                    | `null`                                                  | Sends the current snapshot                                                      |
| `selectDirectory`               | `null`                                                  | Opens the directory dialog and [opens](#opening-a-path) the choice              |
| `rescanDirectory`               | `null`                                                  | Rescans the current directory, keeping selection and expansion                  |
| `clearDirectory`                | `null`                                                  | Resets all directory-related state                                              |
| `cancelScan`                    | `null`                                                  | Cancels the running scan                                                        |
| `loadDirectoryLevel`            | `string` (path)                                         | **Expands** the directory right away, then scans its level and marks it loaded  |
| `setExpansion`                  | `{ path: string, expanded: boolean }` (no other fields) | Expands or collapses one directory. Idempotent; always answered with a snapshot |
| `expandCollapseAll`             | `boolean`                                               | Expands (`true`) or collapses (`false`) every listed directory                  |
| `toggleSelection`               | `string` (path)                                         | Selects/deselects one file                                                      |
| `toggleDirectorySelection`      | `string` (path)                                         | Selects/deselects all files below a directory                                   |
| `selectAll` / `deselectAll`     | `null`                                                  | Selects all listed files / clears the selection                                 |
| `updateFilters`                 | `{ searchQuery, extensionFilter, contentSearchQuery }`  | Applies the filters; expands the ancestors of matches                           |
| `loadFilePreview`               | `string` (path)                                         | Marks the file as previewed; `showPreviewContent` follows once it has been read |
| `clearPreviewState`             | `null`                                                  | Clears the previewed file; a preview still being read is not shown any more     |
| `generatePreview`               | `null`                                                  | Concatenates the selection; answers with `showGeneratedContent`                 |
| `cancelGeneration`              | `null`                                                  | Cancels the running generation                                                  |
| `saveFile`                      | `string` (content)                                      | Opens the save dialog; answers with `fileSaveStatus`                            |
| `pickOutputDirectory`           | `null`                                                  | Opens the dialog, stores the choice as output directory and persists it         |
| `addIgnorePattern`              | `{ pattern: string }` (not blank, no other fields)      | Adds one ignore pattern, see [Config changes](#config-changes)                  |
| `removeIgnorePattern`           | `{ pattern: string }` (not blank, no other fields)      | Removes one ignore pattern                                                      |
| `clearIgnorePatterns`           | `null`                                                  | Removes all ignore patterns                                                     |
| `setConfigOption`               | `{ option, value }` (no other fields), see below        | Sets one user-editable option                                                   |
| `importConfig` / `exportConfig` | `null`                                                  | Config import/export via dialog, see [Config import](#config-import)            |
| `addIgnorePath`                 | `string` (path)                                         | Adds the ignore pattern for a path of the loaded tree (relative to the root)    |

### Filter texts

One rule decides whether a filter text is set, for `searchQuery`, `extensionFilter` and `contentSearchQuery` alike: a text that is empty or consists of whitespace only is **not set**. It is ignored entirely: it does not filter, it starts no content search, and the UI does not show the "no results for filter" message for it. A text that is set is matched exactly as typed; it is not trimmed. The rule lives in `core::is_blank` on the host; the UI mirrors it in `hasActiveFilters` (`FileTree.svelte`).

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

The commands are idempotent (adding a pattern that is set, removing one that is not, and setting an option to its current value change nothing), a changed configuration is persisted, and **every config command is answered with a snapshot**, also when nothing changed. The UI relies on that: controls show the host's `config` and do not write to the store.

#### Config import

`importConfig` takes the settings a user edits and shares from the chosen file: ignore patterns (also those of the generated tree), search and output options, the size limit. The window geometry and `last_directory` describe the installation the file was exported from; they are **not** imported, the current values stay. The import is a hard reset of the loaded directory: selection, expansion, filters and preview are cleared (answered with a snapshot), and the directory that was loaded is then [opened](#opening-a-path) again, so it is listed according to the imported settings. Without a loaded directory no scan is started. The imported settings are persisted.

#### Persistence

A changed configuration is written to the config file in the background. Every writer goes the same way: a numbered snapshot of the configuration is taken under the state lock, and of several saves under way the most recent snapshot is the one that ends up on disk (`ConfigStore` in `src/config/store.rs`). The window geometry is written with the last save of a session, at exit, which also goes that way and cannot be overtaken by a save that is still queued.

`window_size` and `window_position` are logical units, marked by `geometry_units: "logical"`. A config file without that mark was written by a version that stored physical pixels (up to 0.3.1); its geometry is converted once at startup, with the scale factor of the primary monitor.

`max_file_size_mb` is the size limit in MiB (default 20): larger files are not listed by a scan and are replaced by a `[FILE TOO LARGE …]` marker in the generated output. The content search does not read larger files either. The value is not capped: a limit above the default is honoured by all of them. It is not exposed in the UI; set it in the config file.

### Text fields

A snapshot replaces the whole store, so a text field bound to the store would be reset by any snapshot that arrives while the user is typing (scan progress, the answer to another command, a snapshot from before the host has seen the latest input). The filter inputs (`Sidebar.svelte`) and the output fields (`HostTextInput.svelte`) therefore show a local draft and ask a `HostDraft` (`src/ui/src/lib/modules/hostDraft.ts`) whether a host value may replace it:

- While the user is editing (changed, not sent yet: the 300 ms debounce of the filters, an output field that still has the focus), the draft stays.
- After the text has been sent, host values that differ from it are skipped until the host reports the sent text (its answer to the command).
- If that answer does not arrive within 1.5 s, the host's value is shown again: the host is the source of truth, and something else has changed the value in the meantime.

Checkboxes need no draft; they send their intent on `change` and show `config`.

```ts
import { post } from "$lib/services/backend";

post("selectAll"); // nullary command
post("setExpansion", { path: "/repo/src", expanded: true }); // payload is type-checked per command
```

`post` is overloaded so that the payload type follows from the command name. It does not return anything: do not `await` it.

### Command ordering

`dispatch_ipc_message` does not execute anything itself. It parses the message and queues the command on a `CommandDispatcher` (`src/app/dispatch.rs`), which has three lanes:

| Lane         | Commands                                                                                                               | Guarantee                                                                                          |
| ------------ | ---------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------- |
| Ordered      | Everything except the dialog commands                                                                                  | Commands start in arrival order; a command runs without interleaving until it finishes or suspends |
| Continuation | The rest of a command that suspended (`updateFilters` during a content search, `setConfigOption` while it repeats one) | Finished one at a time, in the order the commands were posted                                      |
| Blocking     | `selectDirectory`, `saveFile`, `pickOutputDirectory`, `importConfig`, `exportConfig`                                   | One at a time, in arrival order, on the blocking thread pool                                       |

What follows from that:

- Two commands on the ordered lane never race. `setExpansion(true)` followed by `setExpansion(false)` ends collapsed, two `toggleSelection` cancel out, and of two `updateFilters` the later one determines the filters.
- **A command that starts background work does its bookkeeping itself, before it returns.** Starting a scan (`rescanDirectory`, a drop, the scan after a dialog) supersedes the scan that was running, registers the new one and sends the snapshot with `is_scanning: true` synchronously; `loadDirectoryLevel` marks the directory as expanded; `loadFilePreview` records the previewed file; `generatePreview` registers the generation. Only the IO runs in a task. The command that follows therefore always acts on the result: `rescanDirectory` → `cancelScan` ends cancelled, `rescanDirectory` → `clearDirectory` ends with no directory loaded, a drop → `cancelScan` ends cancelled with the previous directory still loaded, a drop → `rescanDirectory` ends with the dropped directory loaded, `loadDirectoryLevel` → `setExpansion(false)` ends collapsed (the load still delivers the children).
- **Results of background work carry a token and are dropped when they are outdated.** A scan that was cancelled or replaced, a lazy load whose file list was replaced, a file preview that was followed by another one or closed, a content search that was followed by a newer one: none of them changes the state or sends anything any more. For previews this means `showPreviewContent` calls arrive in the order of the requests and an older file never replaces a newer one.
- **A dropped path is a command.** The drag & drop handler queues the drop on the ordered lane (`dispatch_dropped_path`), so drops are ordered among themselves and with the IPC commands: of two drops in quick succession the second one is loaded.
- Long-running work and file IO do not occupy the ordered lane. Scans and generations are started as tasks of their own by their commands; so are the read of a previewed file and the look at a path that is to be scanned (on a slow network drive even checking that it exists can take long). The config file is written in the background as well, from a numbered snapshot, so that an older save can never overwrite a newer one (see [Persistence](#persistence)). A content search suspends `updateFilters`; the part up to the suspension (storing the new filters) has run in order, the rest (applying the search result) moves to the continuation lane. Results are applied in posting order, so an older search cannot overwrite a newer one, and commands posted meanwhile are executed immediately.
- A native dialog blocks its thread for as long as the user takes. Dialog commands therefore run **outside** the ordered lane: a command that arrives while a dialog is open is executed right away, and what the dialog command does after the dialog has been closed (start the scan, store the output directory, import the config) happens after it. Dialog commands are ordered among themselves, so only one dialog is open at a time; a second one opens after the first has been closed.
- A panicking command is logged and dropped; the lanes keep running. This holds in release builds too (the release profile unwinds), and also for a command that panics while it holds the state lock: the lock is taken over by the next command (`SharedState` in `src/app/shared_state.rs`), which works on the state as it was left.

## Host calls (Host → UI)

Each `UserEvent` is turned into exactly one call of a `window.*` function installed by `installWindowIPCHandlers()`:

| `UserEvent`            | Call                                                                                       | Schema                     |
| ---------------------- | ------------------------------------------------------------------------------------------ | -------------------------- |
| `StateUpdate`          | `window.render(uiState)`                                                                   | `UiStateSchema`            |
| `ScanProgress`         | `window.updateScanProgress({ files_scanned, large_files_skipped, current_scanning_path })` | `ScanProgressSchema`       |
| `ShowFilePreview`      | `window.showPreviewContent(content, language, searchTerm, path)`                           | `ShowPreviewArgsSchema`    |
| `ShowGeneratedContent` | `window.showGeneratedContent(content, tokenCount)`                                         | `ShowGeneratedArgsSchema`  |
| `ShowError`            | `window.showError(message)`                                                                | `StatusMessageSchema`      |
| `ConfigExported`       | `window.showStatus(message)`                                                               | `StatusMessageSchema`      |
| `SaveComplete`         | `window.fileSaveStatus(success, path)`                                                     | `FileSaveStatusArgsSchema` |
| `DragStateChanged`     | `window.setDragState(isDragging)`                                                          | `DragStateSchema`          |

Errors are reported as plain strings through `showError`; there is no error-code protocol.

### Scan progress

While a scan walks the directory it reports its progress at most every 100 ms. Each report is sent as `ScanProgress` (the live display of the scan panel) and stored in the host's state, so the snapshots sent during a scan carry the progress as of that moment in `scan_progress` and in `status_message` ("Scanning... 40 files processed. 2 large files skipped (/repo/src/main.rs)"). A scan that has not reported anything yet reads "Scanning... 0 files processed. 0 large files skipped."; when a scan has ended, the status is its outcome and `scan_progress` keeps the last counters. Reports of a scan that was cancelled or replaced are neither stored nor sent.

### Opening a path

Opening a path (a drop, the choice of the directory dialog, the reload after a config import) replaces the loaded directory. Whether the path exists is not known when the command runs, and finding out is IO that does not belong on the command lane. The command therefore only **announces** the scan: the first snapshot has `is_scanning: true`, and everything else is still the directory that was loaded before (`current_path`, tree, selection, filters). The scan task then looks at the path:

- A directory is entered: the previous directory is reset at that moment and `current_path` changes, with a snapshot (`is_scanning: true`, empty tree). Then the directory is scanned.
- For a file (a dropped file) the same happens with the directory the file lies in.
- A path that does not exist ends the scan: `showError("Path does not exist: …")`. **The directory that was loaded stays exactly as it was**, with its list, selection, expansion and filters. Whatever the attempt had superseded when it was announced is taken up again: a scan of the loaded directory that was still running is restarted (announced with a snapshot of its own), and so is the opening of another path that had not been looked at yet (of a drop of an existing directory followed by a drop of a missing path, the directory is loaded). Otherwise a snapshot with `is_scanning: false` follows the error.

Until the path has been looked at, commands act on the directory that is still loaded, and the announced scan is the current one: `cancelScan` cancels it (the loaded directory stays), `clearDirectory` ends with no directory loaded, another drop replaces it. `rescanDirectory` does nothing in that phase (the opening delivers a fresh listing anyway) and is answered with the unchanged snapshot.

`rescanDirectory` itself keeps the loaded directory by definition; if the directory has vanished, the scan ends with the same error and the list stays.

The scan is reported as complete (`is_scanning: false`, status "Indexing complete …") as its very last step. With an active content query that is after the search has been repeated on the complete file list, so the number of visible items in the status is the one of the final list. Until then the snapshot can show `is_fully_scanned: true` together with `is_scanning: true`.

## The state snapshot (`UiState`)

Produced by `generate_ui_state` (`src/app/view_model/mod.rs`), validated by `UiStateSchema`.

| Field                                                      | Meaning                                                                |
| ---------------------------------------------------------- | ---------------------------------------------------------------------- |
| `config`                                                   | The settings the UI displays (`UiConfig`), see below                   |
| `current_path`                                             | Scanned root directory (`""` if none)                                  |
| `tree`                                                     | **Pruned** tree of the filtered file list, see below                   |
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

### The pruned tree

Sending the complete tree on every update costs ~14 MB of JSON at 50,000 files, for every click. The snapshot therefore contains **only what the UI can show**:

- The builder descends into a directory **only if it is expanded** (`AppState::expanded_dirs`).
- A collapsed directory is sent with `children: []`. Everything its row needs is part of the node.
- A directory that is expanded below a collapsed ancestor is not sent at all; its expansion is remembered by the host and shows up when the ancestor is expanded.

`TreeNode`:

| Field                               | Meaning                                                                                                                                                              |
| ----------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `name`, `path`                      | Display name and absolute path (the node's identity)                                                                                                                 |
| `is_directory`, `is_binary`, `size` | Kind and size                                                                                                                                                        |
| `children`                          | Child nodes, directories first, then by name. **Empty for files and for collapsed directories**                                                                      |
| `has_children`                      | **Required.** The directory has children in the filtered list, whether or not they were sent. `false` for files and for directories that are empty or not loaded yet |
| `is_expanded`                       | The directory is expanded. Authoritative; `children` is filled exactly for expanded directories                                                                      |
| `children_loaded`                   | **Required.** `false` for a directory whose content has not been scanned yet (lazy loading). Always `true` for files                                                 |
| `selection_state`                   | `"none" \| "partial" \| "full"`. For directories it is computed from **all** files below it, also the ones not sent                                                  |
| `is_match`                          | The node matches the filename, extension or content filter                                                                                                           |
| `is_previewed`                      | The file is shown in the preview panel                                                                                                                               |

Because the tree is partial, the UI must not derive totals from it. `tree_files_count` and `tree_dirs_count` describe the complete filtered tree and are independent of what is expanded. (`visible_files_count` is the raw length of the filtered list and counts directories and items outside the tree as well; the file tree header uses the `tree_*` counters.)

Measured for a synthetic project of 50,000 files in 550 directories (`payload_of_a_large_project_is_proportional_to_what_is_expanded` in `src/app/view_model/pruning_tests.rs`):

| Snapshot                                       | JSON size |
| ---------------------------------------------- | --------- |
| Complete tree (previous behaviour, any update) | 13.9 MB   |
| Everything collapsed (50 rows)                 | 12.6 kB   |
| One directory chain expanded (160 rows)        | 42.5 kB   |

"Expand all" still produces the complete tree; that is inherent to showing every row.

### Expansion: one source of truth

`AppState::expanded_dirs` on the host is the only place that knows which directories are expanded. The UI keeps **no** expansion state of its own (there used to be an "expansion memory" that overrode `is_expanded` on arrival; with a pruned tree a directory that is expanded only in the UI would have no children to show, and the memory also undid the host's search auto-expansion).

Every way of changing the expansion is therefore a command, and the result is visible with the next snapshot:

| Gesture                                                                         | Command                                                                        |
| ------------------------------------------------------------------------------- | ------------------------------------------------------------------------------ |
| Click on chevron or directory name, `Enter`                                     | `setExpansion` with the opposite of the rendered `is_expanded`                 |
| `ArrowRight` on a collapsed directory                                           | `setExpansion { expanded: true }`                                              |
| `ArrowLeft` on an expanded directory                                            | `setExpansion { expanded: false }`                                             |
| Any of the above on a directory with `children_loaded: false` (no scan running) | `loadDirectoryLevel`; the host loads the level, marks it loaded and expands it |
| "Expand all" / "Collapse all"                                                   | `expandCollapseAll`                                                            |
| Typing into a filter                                                            | `updateFilters`; the host expands the ancestors of all matches                 |

The mapping from gesture to command lives in `src/ui/src/lib/modules/treeExpansion.ts`. While a scan is running the host refuses lazy loads (the scan delivers the children itself), so the UI sends `setExpansion` in that case.

There is no optimistic update: a row changes when the snapshot arrives. Snapshots are cheap enough for that, and it rules out the UI and the host disagreeing.

**Gestures name their target state.** Because there is no optimistic update, a gesture can fire a second time before the snapshot answering the first one has been rendered: key repeat on `ArrowRight`, a double click on the chevron. The second event still sees the old row. If the UI sent "flip it", the second event would undo the first and the directory would end up collapsed again. The UI therefore always sends the state the gesture should lead to, derived from the row the user acted on, and `setExpansion` is idempotent on the host. This also holds for the gestures that read as "toggle" (chevron, name, `Enter`): the user acts on what is on screen ("open this collapsed directory"), so two activations of the same rendered row ask for the same state twice. There is no "flip it" command in the contract.

## Contract tests

- **Rust → TypeScript fixture.** `src/app/view_model/contract_tests.rs` serializes a representative pruned `UiState` and compares it with `src/ui/src/tests/contracts/fixtures/ui_state_pruned.json`. `src/ui/src/tests/contracts/rust-fixture.test.ts` parses that exact file with the full `UiStateSchema` and checks that no field is unknown to the schema. A change on either side that the other does not follow fails CI. Regenerate the fixture after an intended change with

  ```sh
  UPDATE_CONTRACT_FIXTURES=1 cargo test contract_fixture
  ```

- **TypeScript → Rust fixture (commands).** `src/app/command_contract_tests.rs` writes the routing table `COMMANDS` with example payloads into `src/ui/src/tests/contracts/fixtures/commands.json`; payloads with a type on the host (`ConfigOption`, `IgnorePatternPayload`, `SetExpansionPayload`) are serialized from that type, one example per `ConfigOption`. `rust-fixture.test.ts` checks that `CommandSchemas` has exactly the same command names, that every example passes the schema of its command unchanged, and that both sides know the same options of `setConfigOption`. `COMMANDS` is what the router resolves names with, so a command cannot be routed without being part of the contract. The fixture is regenerated with the same command as above.
- **Schema tests.** `src/ui/src/tests/contracts/` covers the command union, the state schema, property tests and JSON-schema snapshots (`npm run test:contracts`).
- **Pruning.** `src/app/view_model/pruning_tests.rs` and the property tests in `cache_tests.rs` compare the snapshot with the original full-tree algorithm, pruned by the same rule.

## Changing the contract

1. **Host:** add the command to `commands.rs`, to `OrderedCommand` (or, if it waits for a native dialog, to `DialogCommand`) and to `COMMANDS` in `src/app/mod.rs`; the compiler then asks for its handler in `run_command` and for example payloads in `command_contract_tests.rs`. A field goes into `UiState` / `UiConfig` / `TreeNode`.
2. **UI:** extend `CommandSchemas` / `UiStateSchema` in `schema.ts`. The types in `types.ts` are derived from the schemas. Prefer additive changes.
3. Regenerate the fixtures (see above) and update the factories in `src/ui/src/tests/contracts/`.
4. Keep the snapshot self-contained: never send something that is only meaningful relative to an earlier update.
5. Update this document.

## Transport notes

The host serializes each `UserEvent` on the UI thread and evaluates the resulting `window.*` call in the WebView. Consecutive undelivered state updates are coalesced to the latest one; the order relative to all other events is preserved (`src/app/transport.rs`).

In E2E tests there is no host. `$lib/dev/e2eBridge.ts` (installed only when `$lib/dev/e2eShim.ts` allows it) exposes `window.__e2e.store.setAppState`, which sets the store directly. States seeded this way have to follow the same rules as real snapshots: children only below expanded directories, counters filled in.
