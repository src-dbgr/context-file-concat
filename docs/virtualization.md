# Virtualization Patterns

> The tree must stay smooth with tens of thousands of files. Two mechanisms keep the work proportional to what is on screen instead of to the size of the project: the host sends a **pruned snapshot**, and the UI renders a **window** of it.

## Goals

- **O(expanded)** payload and UI work per state update, regardless of total items.
- **O(visible)** DOM nodes, regardless of how much is expanded.
- Fast keyboard/mouse interactions (selection, expand/collapse).
- Predictable memory use; no accidental retention of large snapshots.

## Two levels of "only what is visible"

| Level        | Where                                         | Bounds                                               |
| ------------ | --------------------------------------------- | ---------------------------------------------------- |
| Pruning      | Host, `src/app/view_model/tree_builder.rs`    | JSON size, parsing, validation, the flatten step     |
| Windowing    | UI, `src/ui/src/lib/components/FileTree.svelte` | DOM rows                                           |

### 1. Pruned snapshot (host)

Every state update is a complete snapshot (`window.render(uiState)`), but its tree contains only the children of **expanded** directories. A collapsed directory arrives as a single node with `children: []` plus the flags its row needs:

- `has_children` – it has content in the filtered list,
- `children_loaded` – `false` if the content has not been scanned yet (lazy loading),
- `selection_state` – computed on the host from **all** files below it,
- `is_expanded` – authoritative; `children` is filled exactly when this is `true`.

Totals cannot be derived from a partial tree, so the snapshot carries `tree_files_count` and `tree_dirs_count` for the complete filtered tree. The header statistics use these counters.

The structure of the tree (parent/child relations, sibling order, node totals) is cached per filtered list in `TreeIndex`; a click that only changes selection or expansion rebuilds nothing but the visible nodes.

For 50,000 files this reduces a snapshot from ~14 MB to ~13 kB with everything collapsed (see [IPC Contracts](./ipc.md#the-pruned-tree)).

### 2. Windowed list (UI)

`FileTree.svelte` flattens the received tree into rows (preorder, descending only into expanded directories) and renders the rows inside the viewport plus a small overscan:

- Rows are absolutely positioned inside a sizer of `rows × ITEM_HEIGHT` px.
- DOM rows carry the stable class `.virtual-scroll-item` (asserted in tests).
- `TreeItem.svelte` is a pure row: it receives a node and posts commands, nothing else.
- Rows are keyed by `node.path` (no index keys).
- If the viewport is at least as tall as the content, windowing is bypassed and all rows are rendered.

The flatten step is a `$derived` of `$appState.tree`. Because the tree is pruned, its cost is bounded by the number of rows, not by the number of files.

## Expansion

The host is the single source of truth for expansion; the UI stores none. Expanding or collapsing posts a command (`toggleExpansion`, `loadDirectoryLevel` for directories that are not loaded yet, `expandCollapseAll`) and the next snapshot contains the new rows. See [IPC Contracts](./ipc.md#expansion-one-source-of-truth).

Consequences for UI code:

- Never assume the tree has full depth. Code that walks `node.children` only sees what is expanded.
- Never derive counts, selection states or "has content" from `children`; use the counters and node flags.
- Keyboard navigation operates on the flattened rows, which are exactly the rows the host sent.
- "Expand all" produces the complete tree again. Windowing still bounds the DOM, but payload and flatten cost are proportional to the project in that state.

## Parameters & Knobs

- **Row height**: fixed (`ITEM_HEIGHT = 28`). No per-row measurement.
- **Overscan**: `OVERSCAN = 5` rows above and below the viewport.
- **Key**: the node's path.
- **Max rendered rows**: asserted in tests (`≤ 600` for the 25k dataset).

## Filtering & Search

- Filters are applied on the host **before** the tree is built; the UI never filters.
- A directory is kept if any descendant matches.
- The host expands the ancestors of all matches (`auto_expand_for_matches`), so matches are part of the pruned snapshot. `is_match` marks the matching nodes.
- Selection invariants hold across filters: a directory is `"full"` only if all files below it in the filtered list are selected.

## Measurement, not sleeps

No artificial delays. Performance is measured via `performance.mark/measure` (see `$lib/dev/budget.ts`) and asserted in E2E `benchmarks.spec.ts`.

## Benchmarks

We ship three benches in `src/ui/e2e/benchmarks.spec.ts`:

- **Flatten/Apply 25k** — bound initial apply under `BMARK_FLATTEN_MS` (default 1500ms).
- **Filter (\~1k subset)** — apply a representative filter under `BMARK_FILTER_MS` (default 500ms).
- **Scroll jump to end** — simulate a programmatic jump to the end under `BMARK_SCROLL_MS` (default 200ms).

They seed one expanded directory with 25k files, i.e. the worst case of a pruned snapshot. Run via `npm run e2e:bench` (or the VS Code test runner). Thresholds are env-driven.

## Anti-Patterns

- Rendering the full tree (no virtualization).
- Sending or expecting children of collapsed directories.
- Keeping expansion state in the UI.
- Counting or aggregating over `tree` instead of using the host's counters.
- Using array index as `key`.
- Triggering layout thrash (reading/writing layout in the same frame).
- Doing heavy work in `onMount` of each row.
  EOF

cat > "\$DOCS_DIR/svelte-runes-guidelines.md" << 'EOF'

# Svelte Runes Guidelines

> These guidelines read comfortably on Svelte v5 (runes) but remain compatible with v4 by mapping to stores/derived stores.

## Goals

- Keep **global app state** in a single store (`$lib/stores/app.ts`).
- Keep **ephemeral UI state** in the component via runes (or local stores).
- Keep effects explicit and minimal. No hidden global subscriptions.

## Core Rules

1. **One global source of truth**
   `appState` holds everything the renderer must agree on. Avoid scattering redundant slices.

2. **Prefer runes for component local state** (Svelte 5)

   - `$state()` for local mutable state.
   - `$derived()` for pure derivations.
   - `$effect()` for side effects that depend on reactive values.
   - `$props()` for prop mapping.

   **v4 fallback**: use `writable/derived` locally and `$:` reactive statements.

3. **Do not mix transport with business logic**

   - IPC calls live behind `$lib/services/backend.ts`.
   - Components dispatch **intents**, stores handle orchestration.

4. **Effects must be idempotent**

   - Tie effects to the minimal set of dependencies.
   - Clean up on destroy (event listeners, observers).

5. **Performance-aware derivations**

   - Push heavy transforms (e.g., `flatten(tree)`) into a derived store or a memoized function.
   - Avoid recreating large arrays on every keypress; debounce at the intent layer if needed.

## Examples (Svelte v5)

```svelte
<script lang="ts">
  import { post } from "$lib/services/backend";
  import { appState } from "$lib/stores/app";

  // Local runes
  const query = $state("");
  const isSaving = $state(false);

  const visible = $derived(() => {
    const s = $appState;           // assuming appState is exposed via a runes-friendly store wrapper
    return filterAndFlatten(s.tree, query);
  });

  $effect(() => {
    // Only when query changes and tree is ready
    void visible; // consume to establish dependency
  });

  async function onSave() {
    isSaving = true;
    try {
      const res = await post<{ saved_path: string }>("saveOutput");
      // toast, update state, etc.
    } finally {
      isSaving = false;
    }
  }
</script>
```

### v4 Mapping

- `$state` → `let x = ...` + `writable` if it must be shared.
- `$derived` → `derived([...], fn)`.
- `$effect` → `$:` block or `onMount` + `unsubscribe`.

## Component Boundaries

- **Dumb rows**: `Row.svelte` receives a plain `row` object and never reaches into global stores.
- **Containers** assemble data and pass it down. Containers may trigger IPC intents; rows do not.

## Testing Runes-based Components

- Prefer mounting the **container** with a seeded `appState`.
- Interact via DOM, assert via DOM (E2E) or via mocked `post()` calls (unit).
- Avoid asserting internal runes values; treat them as implementation details.

## Common Pitfalls

- Implicit subscriptions to large stores in deep leaf components.
- Reactive blocks that rebuild large arrays on every minor change.
- Effects that depend on unstable references (e.g., inline lambdas in deps).
