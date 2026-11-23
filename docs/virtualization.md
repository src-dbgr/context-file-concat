# Virtualization Patterns

> The tree must stay smooth with hundreds of thousands of files, also when all of them are expanded. One idea keeps the work proportional to what is on screen instead of to the size of the project: **the host owns the list of rows, and the UI sees a window of it**. The host sends only the rows around the viewport; the UI renders only the rows inside it.

## Goals

- **O(window)** payload and UI work per state update, regardless of the size of the project and of how much is expanded.
- **O(viewport)** DOM nodes.
- Fast keyboard/mouse interactions (selection, expand/collapse, scrolling) at any size.
- One source of truth: the host. The UI holds no tree, no expansion state and no list of rows.
- Predictable memory use; no accidental retention of large snapshots.

## The rows and their window

| What                         | Where                                              | Bounded by                             |
| ---------------------------- | -------------------------------------------------- | -------------------------------------- |
| The list of all rows         | Host, `src/app/view_model/rows.rs` (`VisibleRows`) | the expanded part of the tree (cached) |
| The window sent to the UI    | Host, `rows.rs` (`Viewport`, `tree_snapshot`)      | the viewport: at most 600 rows         |
| Which window the UI asks for | UI, `src/ui/src/lib/modules/treeWindow.ts`         | the viewport plus a buffer             |
| The rendered rows            | UI, `src/ui/src/lib/components/FileTree.svelte`    | the viewport plus `OVERSCAN`           |

### 1. The list of rows (host)

A **row** is an item of the filtered file list whose ancestors are all expanded. The list of all rows, in display order, is `VisibleRows`: per row the item and its nesting level, plus the row of each item for lookups. It is derived from the cached hierarchy (`TreeIndex`) and `AppState::expanded_dirs` and is itself cached:

- It is rebuilt when the filtered file list changes (scan, lazy load, filter, ignore pattern) or a directory is expanded or collapsed. `expanded_dirs` is a `Tracked` set: borrowing it mutably gives it a new revision, which is part of the cache key.
- It is **not** rebuilt for a change of the selection, the previewed file, the lazy-load status, a row lookup, or the viewport. `rows_revision` in the snapshot says which list a snapshot was cut from.

What a row shows beyond its position (selection, match, preview, sizes) is filled in for the rows of the window only, on every snapshot. The one of these that would need the whole tree, the selection state of a directory (it is made of all files below it), is answered from counts the host keeps and updates per click (`SelectionCountsCache` in `src/app/selection.rs`).

### 2. The window (host ↔ UI)

Every state update is a complete snapshot (`window.render(uiState)`). Of the tree it carries `rows_total` (the length of the list) and the rows of the window the UI has asked for with `setViewport { start, count }`, starting at `rows_start`. The details of the contract (the default and the largest window, what happens at the end of the list, when the window returns to the top) are in [IPC Contracts](./ipc.md#the-rows-of-the-tree).

Scrolling is therefore a round trip: the UI reports the viewport, the host answers with a snapshot that holds the rows. The host needs some 60 µs for it at any size of the project (see [Measured](#measured)); the transport coalesces snapshots the UI has not got to yet, so fast scrolling does not queue up stale windows.

### 3. Which window the UI asks for

`treeWindow.ts` is the arithmetic, without state:

- The UI asks for the rows of the viewport plus a **buffer** of one viewport of rows (at least `MIN_BUFFER_ROWS = 30`) above and below.
- It asks again when less than half of that buffer is left in the direction of travel. Scrolling by a few rows sends nothing; a page of keyboard navigation stays inside the window; a jump (scrollbar drag, `End`) is one request and one answer.
- The request depends on the first visible row and the height of the viewport only, and it is not repeated while its answer is under way (`FileTree.svelte` remembers the snapshot it asked for). A window that is exactly what the host makes of the request is never asked for again, so nothing can loop.
- It never asks for more than the host sends (`MAX_WINDOW_ROWS = 600`).

### 4. Rendering (UI)

`FileTree.svelte` renders the rows of the snapshot that are inside the viewport plus a small overscan:

- The scroller holds a sizer of `rows_total × ITEM_HEIGHT` px; rows are absolutely positioned by their **index in the list** (`rows_start + offset`), not by their place in the window.
- DOM rows carry the stable class `.virtual-scroll-item` (asserted in tests) and `data-index`, the index in the list.
- `TreeItem.svelte` is a pure row: it receives a row and posts commands, nothing else. The indentation comes from the row's `depth`.
- Rows are keyed by `path` (no index keys).
- A row that is inside the viewport but has not arrived yet (right after a jump) is rendered as a **placeholder** of the height of a row at the position of the row (`.virtual-scroll-item.pending`, hidden from assistive technology; the tree is `aria-busy` meanwhile). Nothing moves when the row arrives, and the sizer keeps its height, so the scrollbar does not move either.
- If the viewport is at least as tall as the list, all rows are rendered.

There is no flatten step and no tree in the UI: the rows arrive in order, with their level.

## Expansion

The host is the single source of truth for expansion; the UI stores none. Expanding or collapsing posts a command (`setExpansion`, `loadDirectoryLevel` for directories that are not loaded yet, `expandCollapseAll`) and the next snapshot is cut from the new list. See [IPC Contracts](./ipc.md#expansion-one-source-of-truth).

Consequences for UI code:

- Never assume a row's neighbours are in the window, and never walk the rows to learn something about the tree. What a row needs to know about what is below it is part of the row (`has_children`, `children_loaded`, `selection_state`).
- Never derive counts from `rows`; use `rows_total` for the list and the `tree_*` counters for the tree.
- "Expand all" makes the list as long as the tree. That costs the host one rebuild of the list (2 ms per 500,000 rows); the snapshot stays a window.

## Keyboard and focus

The focused row is known by its **index in the list** and, once the row has arrived, by its **path**.

- **The tree is one stop for Tab** (roving tabindex): the focused row has `tabindex="0"`, every other row and every control inside a row (chevron, checkbox, name, ignore button) has `tabindex="-1"`. While the focused row is not rendered (scrolled away, or on its way from the host) the stop is the tree element itself, so the tree can always be reached and left with one `Tab`. The controls of a row are for the pointer; each has its key on the row:

  | Key on a row | Does | Pointer |
  | --- | --- | --- |
  | `Enter` | toggles a directory, opens a file in the preview | chevron, name |
  | `ArrowRight` / `ArrowLeft` | expands / collapses a directory, moves to the first child / the parent; `ArrowRight` on a file opens it | chevron |
  | `Space` | toggles the selection | checkbox |
  | `Delete` | adds the row to the ignore patterns (`addIgnorePath`) | ignore button (`aria-keyshortcuts="Delete"`) |
  | a letter | type-ahead: moves to the next row whose name starts with what was typed | |

  `Delete` only, not `Backspace`: that key belongs to correcting what was typed. A control that got the focus by a click still acts on its own keys (`Space` on the checkbox, `Enter`/`Space` on the ignore button); every other key there is the tree's.

- `ArrowUp`/`ArrowDown`, `PageUp`/`PageDown`, `Home`/`End` compute the new index (`rows_total` is known), scroll it into view and focus the row when it is rendered. If the row is outside the window, the scroll reports the viewport and the focus follows as soon as the row has arrived.
- **No key is lost while a row is under way.** The element of a row exists only while the row is rendered. When the row that has the DOM focus is about to leave the DOM (scrolled away, or the focus has moved to a row that has not arrived), the focus is parked on the tree element itself instead of falling to the body; the key handler lives there and works with the focused index, so `End` followed by `ArrowUp` ends one row above the last. The focus returns to the row as soon as it is rendered. A key that acts on a row (`Space`, `Enter`, `ArrowLeft`/`ArrowRight`) and is pressed before that row has arrived is kept and carried out on that row when it is there (`End`, `Space` selects the last file). A key pressed while the host is still looking for the row the focus is to move to (type-ahead, the parent) is kept as well and acts from the row that is found (typing a name and then `ArrowUp` ends one row above the match).
- **A kept key is never carried out late.** It is an intention of the moment it was pressed. It is dropped when it is older than one second (`KEPT_KEY_MS`), when the user takes over with the pointer or the wheel, when the list becomes another one (the position then means another row), and when a snapshot arrives without its row while the viewport is no longer where that row is (the answer was overtaken by another viewport report). So a selection is never toggled seconds later, when the row happens to scroll into view.
- `ArrowRight` on an expanded directory moves to the next row (its first child); `ArrowLeft` moves to the nearest row above that is less deep. If the window does not reach that far up, the UI asks the host: `locateRow { target: { kind: "parent_of", path } }`.
- Typing a name (type-ahead) is a search over all rows, so the host does it: `locateRow { target: { kind: "typeahead", prefix, from } }`.
- When the list changes (`rows_revision`), the focus stays on its item: if its path is in the window, the index is taken from there; if not, the UI asks where it went (`{ kind: "path", path }`). An item that is no row any more leaves the focus at its position.

The answer to `locateRow` is `located` of the snapshot; see [Looking up a row](./ipc.md#looking-up-a-row).

## Parameters & Knobs

| Knob                         | Value | Where                                                             |
| ---------------------------- | ----- | ----------------------------------------------------------------- |
| Row height                   | 28 px | `ITEM_HEIGHT`, `treeWindow.ts`                                    |
| Rendered beyond the viewport | 5     | `OVERSCAN`, `treeWindow.ts`                                       |
| Least buffer per side        | 30    | `MIN_BUFFER_ROWS`, `treeWindow.ts`                                |
| Window before the UI reports | 100   | `Viewport::DEFAULT_ROWS`, `rows.rs`                               |
| Largest window               | 600   | `Viewport::MAX_ROWS`, `rows.rs` (and `MAX_WINDOW_ROWS` in the UI) |
| Auto-expansion per filter    | 500   | `AUTO_EXPAND_LIMIT`, `filtering.rs`                               |
| Max rendered rows            | ≤ 600 | asserted in tests for the 25k dataset                             |

A viewport of 1080 px shows 39 rows and asks for about 120 (some 35 kB of JSON); 600 rows cover a viewport of more than 5000 px.

## Filtering & Search

- Filters are applied on the host **before** the rows are listed; the UI never filters.
- A directory is kept if any descendant matches.
- The host expands the directories above matches (`auto_expand_for_matches`), so matches are rows. `is_match` marks the matching rows.
- **The expansion is limited** to the first 500 directories in tree order per application of the filters (`AUTO_EXPAND_LIMIT`). A broad filter would otherwise expand every directory of a large project; the rest of the matches stay listed and counted behind collapsed directories. See [IPC Contracts](./ipc.md#expansion-one-source-of-truth) for the rule and the numbers.
- Changed filter texts show the new list from the top, on both sides: the UI scrolls up, the host's window starts at 0.
- Selection invariants hold across filters: a directory is `"full"` only if all files below it in the filtered list are selected.

## Measured

Host side, release build, a synthetic project with **everything expanded**, a window of 200 rows (`cargo test --release snapshot_of_a_huge -- --nocapture`; the test in `src/app/view_model/windowing_tests.rs` asserts the size and a generous time budget in every build):

| Files (directories) | Rows    | Snapshot before | Snapshot now | Click on a checkbox | Scroll | Expand/collapse one directory |
| ------------------- | ------- | --------------- | ------------ | ------------------- | ------ | ----------------------------- |
| 25,000 (1,300)      | 26,300  | 6.6 MB, 6.5 ms  | 55 kB        | 65 µs               | 65 µs  | 0.17 ms                       |
| 100,000 (5,050)     | 105,050 | 26.3 MB, 25 ms  | 55 kB        | 68 µs               | 66 µs  | 0.48 ms                       |
| 500,000 (25,050)    | 525,050 | 131 MB, 136 ms  | 55 kB        | 59 µs               | 64 µs  | 2.3 ms                        |

Times are building plus serializing the snapshot. Before, that was the cost of **every** update in this state, a click on a checkbox included, and the WebView then had to parse, validate and flatten the same megabytes. What still depends on the size of the project is the first snapshot of a new filtered list, which builds the hierarchy, the list of rows and the selection counts once (6 ms / 22 ms / 113 ms), and the filtering itself.

Auto-expansion of a filter that matches every file: 8 ms → 0.3 ms (25,000 files), 32 ms → 0.8 ms (100,000), 160 ms → 6 ms (500,000).

## Measurement, not sleeps

No artificial delays. Performance is measured via `performance.mark/measure` (see `$lib/dev/budget.ts`) and asserted in E2E `benchmarks.spec.ts`.

## Benchmarks

We ship three benches in `src/ui/e2e/benchmarks.spec.ts`:

- **Apply 25k** — bound the first render of a 25k list under `BMARK_FLATTEN_MS` (default 1800 ms; the name is from the time the UI flattened a tree).
- **Filter (\~1k subset)** — apply a representative filter under `BMARK_FILTER_MS` (default 600 ms).
- **Scroll jump to end** — a programmatic jump to the end, including the round trip for the window, under `BMARK_SCROLL_MS` (default 300 ms).

There is no host in E2E tests. The benches and `e2e/tree-window.spec.ts` (scrolling, keyboard and type-ahead across the edge of the window, in Chromium and WebKit) hand the rows to the stand-in host of the E2E bridge (`window.__e2e.host.setTree`), which answers `setViewport` and `locateRow` asynchronously with windows, as the real host does: it is the model in `$lib/dev/treeHostModel.ts`, which a contract test replays against a scenario recorded from the host (see [IPC Contracts](./ipc.md#contract-tests)), and it delivers its snapshots through `window.render`. `tree-window.spec.ts` also runs with a host that answers late (`host.setLatency`): 40 times `PageDown`, `End` + `ArrowUp`, `End` + `Space`. Where a test needs several steps to happen before any answer (keys that are overtaken, the measure of a placeholder) it does not race a timer: `host.hold()` keeps the answers back until `host.release()`. Run via `npm run bench` (or the VS Code test runner); `npm run bench:strict` uses 1500/500/200 ms. Thresholds are env-driven.

## Anti-Patterns

- Rendering the full list (no virtualization).
- Holding rows in the UI beyond the last snapshot, or stitching windows together: a snapshot is the truth, the one before it is not.
- Walking `rows` to find parents, children or totals instead of using the row's own fields, the counters, or `locateRow`.
- Keeping expansion state in the UI.
- Addressing a row by its place in the window instead of its index in the list.
- Using array index as `key`.
- Triggering layout thrash (reading/writing layout in the same frame).
- Doing heavy work in `onMount` of each row.
