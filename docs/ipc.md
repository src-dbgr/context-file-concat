# IPC Contracts

> The UI and the host talk over two one-way channels: the UI **posts commands**, the host **calls `window.*` handlers**. There are no request/response pairs and no promises: a command never returns a value, its effect arrives as the next state snapshot.

## Overview

```text
UI  ──  window.ipc.postMessage('{"command": "...", "payload": ...}')  ──▶  Host
UI  ◀──  window.render(uiState) / window.showPreviewContent(...) / ...  ──  Host
```

| Concern                         | Where                                                             |
| ------------------------------- | ----------------------------------------------------------------- |
| Sending commands                | `src/ui/src/lib/services/backend.ts` (`post`)                     |
| Command and state schemas (Zod) | `src/ui/src/lib/ipc/schema.ts`                                    |
| Receiving host calls            | `src/ui/src/lib/ipc/handlers.ts` (`installWindowIPCHandlers`)     |
| Frontend types                  | `src/ui/src/lib/types.ts`                                         |
| Command dispatch (host)         | `src/app/mod.rs` (`dispatch_ipc_message`) → `src/app/commands.rs`   |
| Command ordering (host)         | `src/app/dispatch.rs` (`CommandDispatcher`)                       |
| Host → UI events                | `src/app/events.rs` (`UserEvent`), `src/app/mod.rs` (`handle_user_event`) |
| State snapshot (host)           | `src/app/view_model/` (`generate_ui_state`, `UiState`, `TreeNode`) |
| Delivery / coalescing           | `src/app/transport.rs`                                            |

## Principles

- **Commands are fire-and-forget.** `post()` returns `void`. The host mutates its `AppState` and answers with a state snapshot (and, where applicable, one of the other host calls).
- **Commands take effect in the order they were posted.** The host executes them one after the other, see [Command ordering](#command-ordering).
- **Every state update is a complete snapshot.** `window.render(uiState)` replaces the UI's `appState` store wholesale. There are no diffs and no patches; a snapshot never depends on an earlier one. This is what allows the transport to drop a pending snapshot when a newer one is already available (latest wins, see `transport.rs`).
- **The host owns the state, the UI owns none of it.** In particular the host is the single source of truth for selection and for **expansion** (see below). The UI renders what it receives.
- **Snapshots are pruned to what is visible.** The tree only contains the children of expanded directories; totals are sent as counters.
- **Validation at the edge.** Outgoing commands are validated against `CommandSchemas` in DEV. Incoming calls are validated in `handlers.ts`: `render()` with the full `UiStateSchema` in DEV and tests, and with the envelope-only `UiStateEnvelopeSchema` in production (the recursive tree walk stays off the hot path). Invalid payloads are ignored with a console warning.
- **No untyped `any` at the boundary.**

## Commands (UI → Host)

The single source of truth is `CommandSchemas` in `schema.ts`; names and payloads must match `run_command` / `DialogCommand` in `src/app/mod.rs`. The wire format is `{"command": <name>, "payload": <payload | null>}`.

| Command                    | Payload                                                    | Effect                                                                 |
| -------------------------- | ---------------------------------------------------------- | ---------------------------------------------------------------------- |
| `initialize`               | `null`                                                     | Sends the current snapshot                                              |
| `selectDirectory`          | `null`                                                     | Opens the directory dialog and scans the choice                        |
| `rescanDirectory`          | `null`                                                     | Rescans the current directory, keeping selection and expansion         |
| `clearDirectory`           | `null`                                                     | Resets all directory-related state                                     |
| `cancelScan`               | `null`                                                     | Cancels the running scan                                               |
| `loadDirectoryLevel`       | `string` (path)                                            | Scans one directory level, marks it loaded **and expands it**          |
| `setExpansion`             | `{ path: string, expanded: boolean }` (no other fields)    | Expands or collapses one directory. Idempotent; always answered with a snapshot |
| `toggleExpansion`          | `string` (path)                                            | Flips one directory. Not idempotent and not used by the UI, see [Expansion](#expansion-one-source-of-truth) |
| `expandCollapseAll`        | `boolean`                                                  | Expands (`true`) or collapses (`false`) every listed directory         |
| `expandAllFully`           | `null`                                                     | Expands everything; only after the full scan                           |
| `toggleSelection`          | `string` (path)                                            | Selects/deselects one file                                             |
| `toggleDirectorySelection` | `string` (path)                                            | Selects/deselects all files below a directory                          |
| `selectAll` / `deselectAll`| `null`                                                     | Selects all listed files / clears the selection                        |
| `selectAllFully`           | `null`                                                     | Selects everything; only after the full scan                           |
| `updateFilters`            | `{ searchQuery, extensionFilter, contentSearchQuery }`     | Applies the filters; expands the ancestors of matches                  |
| `loadFilePreview`          | `string` (path)                                            | Answers with `showPreviewContent`                                      |
| `clearPreviewState`        | `null`                                                     | Clears the previewed file                                              |
| `generatePreview`          | `null`                                                     | Concatenates the selection; answers with `showGeneratedContent`        |
| `cancelGeneration`         | `null`                                                     | Cancels the running generation                                         |
| `saveFile`                 | `string` (content)                                         | Opens the save dialog; answers with `fileSaveStatus`                   |
| `pickOutputDirectory`      | `null`                                                     | Opens the dialog and stores the choice in the config                   |
| `updateConfig`             | full config (`ConfigSchema`)                               | Stores the config, rescans if necessary                                |
| `importConfig` / `exportConfig` | `null`                                                | Config import/export via dialog                                        |
| `addIgnorePath`            | `string` (path)                                            | Adds an ignore pattern for the path and rescans                        |

```ts
import { post } from "$lib/services/backend";

post("selectAll"); // nullary command
post("setExpansion", { path: "/repo/src", expanded: true }); // payload is type-checked per command
```

`post` is overloaded so that the payload type follows from the command name. It does not return anything: do not `await` it.

### Command ordering

`dispatch_ipc_message` does not execute anything itself. It parses the message and queues the command on a `CommandDispatcher` (`src/app/dispatch.rs`), which has three lanes:

| Lane         | Commands                                                                   | Guarantee                                                                                         |
| ------------ | -------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------- |
| Ordered      | Everything except the dialog commands                                      | Commands start in arrival order; a command runs without interleaving until it finishes or suspends |
| Continuation | The rest of a command that suspended (`updateFilters` during a content search) | Finished one at a time, in the order the commands were posted                                     |
| Blocking     | `selectDirectory`, `saveFile`, `pickOutputDirectory`, `importConfig`, `exportConfig` | One at a time, in arrival order, on the blocking thread pool                                      |

What follows from that:

- Two commands on the ordered lane never race. `setExpansion(true)` followed by `setExpansion(false)` ends collapsed, two `toggleSelection` cancel out, and of two `updateFilters` the later one determines the filters.
- Long-running work does not occupy the ordered lane. Scans and generations are started as tasks of their own by their commands. A content search suspends `updateFilters`; the part up to the suspension (storing the new filters) has run in order, the rest (applying the search result) moves to the continuation lane. Results are applied in posting order, so an older search cannot overwrite a newer one, and commands posted meanwhile are executed immediately.
- A native dialog blocks its thread for as long as the user takes. Dialog commands therefore run **outside** the ordered lane: a command that arrives while a dialog is open is executed right away, and what the dialog command does after the dialog has been closed (start the scan, store the output directory, import the config) happens after it. Dialog commands are ordered among themselves, so only one dialog is open at a time; a second one opens after the first has been closed.
- A panicking command is logged and dropped; the lanes keep running.

## Host calls (Host → UI)

Each `UserEvent` is turned into exactly one call of a `window.*` function installed by `installWindowIPCHandlers()`:

| `UserEvent`            | Call                                                        | Schema                     |
| ---------------------- | ----------------------------------------------------------- | -------------------------- |
| `StateUpdate`          | `window.render(uiState)`                                    | `UiStateSchema`            |
| `ScanProgress`         | `window.updateScanProgress({ files_scanned, large_files_skipped, current_scanning_path })` | `ScanProgressSchema` |
| `ShowFilePreview`      | `window.showPreviewContent(content, language, searchTerm, path)` | `ShowPreviewArgsSchema` |
| `ShowGeneratedContent` | `window.showGeneratedContent(content, tokenCount)`          | `ShowGeneratedArgsSchema`  |
| `ShowError`            | `window.showError(message)`                                 | `StatusMessageSchema`      |
| `ConfigExported`       | `window.showStatus(message)`                                | `StatusMessageSchema`      |
| `SaveComplete`         | `window.fileSaveStatus(success, path)`                      | `FileSaveStatusArgsSchema` |
| `DragStateChanged`     | `window.setDragState(isDragging)`                           | `DragStateSchema`          |

Errors are reported as plain strings through `showError`; there is no error-code protocol.

## The state snapshot (`UiState`)

Produced by `generate_ui_state` (`src/app/view_model/mod.rs`), validated by `UiStateSchema`.

| Field                                                         | Meaning                                                                    |
| ------------------------------------------------------------- | -------------------------------------------------------------------------- |
| `config`                                                      | The application config                                                     |
| `current_path`                                                | Scanned root directory (`""` if none)                                      |
| `tree`                                                        | **Pruned** tree of the filtered file list, see below                       |
| `total_files_found`                                           | Items (files and directories) in the unfiltered list                       |
| `visible_files_count`                                         | Items (files and directories) in the filtered list                         |
| `tree_files_count`                                            | Files in the complete filtered tree, including the ones not sent           |
| `tree_dirs_count`                                             | Directories in the complete filtered tree, including the ones not sent     |
| `selected_files_count`                                        | Selected files                                                             |
| `is_scanning`, `is_generating`, `is_fully_scanned`            | Activity flags                                                             |
| `status_message`, `scan_progress`                             | Status line and scan progress                                              |
| `search_query`, `extension_filter`, `content_search_query`    | Active filters                                                             |
| `current_config_filename`                                     | Name of the imported config, or `null`                                     |
| `active_ignore_patterns`, `patterns_need_rescan`              | Ignore pattern bookkeeping                                                 |

### The pruned tree

Sending the complete tree on every update costs ~14 MB of JSON at 50,000 files, for every click. The snapshot therefore contains **only what the UI can show**:

- The builder descends into a directory **only if it is expanded** (`AppState::expanded_dirs`).
- A collapsed directory is sent with `children: []`. Everything its row needs is part of the node.
- A directory that is expanded below a collapsed ancestor is not sent at all; its expansion is remembered by the host and shows up when the ancestor is expanded.

`TreeNode`:

| Field             | Meaning                                                                                                              |
| ----------------- | -------------------------------------------------------------------------------------------------------------------- |
| `name`, `path`    | Display name and absolute path (the node's identity)                                                                 |
| `is_directory`, `is_binary`, `size` | Kind and size                                                                                      |
| `children`        | Child nodes, directories first, then by name. **Empty for files and for collapsed directories**                      |
| `has_children`    | **Required.** The directory has children in the filtered list, whether or not they were sent. `false` for files and for directories that are empty or not loaded yet |
| `is_expanded`     | The directory is expanded. Authoritative; `children` is filled exactly for expanded directories                      |
| `children_loaded` | **Required.** `false` for a directory whose content has not been scanned yet (lazy loading). Always `true` for files               |
| `selection_state` | `"none" \| "partial" \| "full"`. For directories it is computed from **all** files below it, also the ones not sent |
| `is_match`        | The node matches the filename, extension or content filter                                                           |
| `is_previewed`    | The file is shown in the preview panel                                                                               |

Because the tree is partial, the UI must not derive totals from it. `tree_files_count` and `tree_dirs_count` describe the complete filtered tree and are independent of what is expanded. (`visible_files_count` is the raw length of the filtered list and counts directories and items outside the tree as well; the file tree header uses the `tree_*` counters.)

Measured for a synthetic project of 50,000 files in 550 directories (`payload_of_a_large_project_is_proportional_to_what_is_expanded` in `src/app/view_model/pruning_tests.rs`):

| Snapshot                                           | JSON size   |
| -------------------------------------------------- | ----------- |
| Complete tree (previous behaviour, any update)     | 13.9 MB     |
| Everything collapsed (50 rows)                     | 12.6 kB     |
| One directory chain expanded (160 rows)            | 42.5 kB     |

"Expand all" still produces the complete tree; that is inherent to showing every row.

### Expansion: one source of truth

`AppState::expanded_dirs` on the host is the only place that knows which directories are expanded. The UI keeps **no** expansion state of its own (there used to be an "expansion memory" that overrode `is_expanded` on arrival; with a pruned tree a directory that is expanded only in the UI would have no children to show, and the memory also undid the host's search auto-expansion).

Every way of changing the expansion is therefore a command, and the result is visible with the next snapshot:

| Gesture                                                    | Command                                   |
| ---------------------------------------------------------- | ----------------------------------------- |
| Click on chevron or directory name, `Enter`                | `setExpansion` with the opposite of the rendered `is_expanded` |
| `ArrowRight` on a collapsed directory                      | `setExpansion { expanded: true }`         |
| `ArrowLeft` on an expanded directory                       | `setExpansion { expanded: false }`        |
| Any of the above on a directory with `children_loaded: false` (no scan running) | `loadDirectoryLevel`; the host loads the level, marks it loaded and expands it |
| "Expand all" / "Collapse all"                              | `expandCollapseAll`                       |
| Typing into a filter                                       | `updateFilters`; the host expands the ancestors of all matches |

The mapping from gesture to command lives in `src/ui/src/lib/modules/treeExpansion.ts`. While a scan is running the host refuses lazy loads (the scan delivers the children itself), so the UI sends `setExpansion` in that case.

There is no optimistic update: a row changes when the snapshot arrives. Snapshots are cheap enough for that, and it rules out the UI and the host disagreeing.

**Gestures name their target state.** Because there is no optimistic update, a gesture can fire a second time before the snapshot answering the first one has been rendered: key repeat on `ArrowRight`, a double click on the chevron. The second event still sees the old row. If the UI sent "flip it" (`toggleExpansion`), the second event would undo the first and the directory would end up collapsed again. The UI therefore always sends the state the gesture should lead to, derived from the row the user acted on, and `setExpansion` is idempotent on the host. This also holds for the gestures that read as "toggle" (chevron, name, `Enter`): the user acts on what is on screen ("open this collapsed directory"), so two activations of the same rendered row ask for the same state twice. `toggleExpansion` stays in the contract for callers that have no rendered state to derive a target from; the UI does not use it.

## Contract tests

- **Rust → TypeScript fixture.** `src/app/view_model/contract_tests.rs` serializes a representative pruned `UiState` and compares it with `src/ui/src/tests/contracts/fixtures/ui_state_pruned.json`. `src/ui/src/tests/contracts/rust-fixture.test.ts` parses that exact file with the full `UiStateSchema` and checks that no field is unknown to the schema. A change on either side that the other does not follow fails CI. Regenerate the fixture after an intended change with

  ```sh
  UPDATE_CONTRACT_FIXTURES=1 cargo test contract_fixture
  ```

- **Schema tests.** `src/ui/src/tests/contracts/` covers the command union, the state schema, property tests and JSON-schema snapshots (`npm run test:contracts`).
- **Pruning.** `src/app/view_model/pruning_tests.rs` and the property tests in `cache_tests.rs` compare the snapshot with the original full-tree algorithm, pruned by the same rule.

## Changing the contract

1. **Host:** add the command to `commands.rs` and to `run_command` in `src/app/mod.rs` (or, if it waits for a native dialog, to `DialogCommand`), or the field to `UiState` / `TreeNode`.
2. **UI:** extend `CommandSchemas` / `UiStateSchema` in `schema.ts` and the types in `types.ts`. Prefer additive changes.
3. Regenerate the fixture (see above) and update the factories in `src/ui/src/tests/contracts/`.
4. Keep the snapshot self-contained: never send something that is only meaningful relative to an earlier update.
5. Update this document.

## Transport notes

The host serializes each `UserEvent` on the UI thread and evaluates the resulting `window.*` call in the WebView. Consecutive undelivered state updates are coalesced to the latest one; the order relative to all other events is preserved (`src/app/transport.rs`).

In E2E tests there is no host. `$lib/dev/e2eBridge.ts` (installed only when `$lib/dev/e2eShim.ts` allows it) exposes `window.__e2e.store.setAppState`, which sets the store directly. States seeded this way have to follow the same rules as real snapshots: children only below expanded directories, counters filled in.
