# Virtualization Patterns

> Our tree must remain smooth at \~25k files. We use windowed rendering, stable keys, and carefully bounded work per frame. Benchmarks enforce this.

## Goals

- **O(visible)** DOM nodes regardless of total items.
- Fast keyboard/mouse interactions (selection, expand/collapse).
- Predictable memory use; no accidental retention of large snapshots.

## Core Ideas

1. **Windowed list**
   Only render the visible slice + a small **overscan** on top/bottom.

   - DOM rows carry a stable class `.virtual-scroll-item` (asserted in tests).
   - Keep the row component pure; no cross-row effects.

2. **Flattened view model**
   Keep the expensive bits out of the hot path:

   - Build a **flattened array** of visible rows (directories expanded → preorder).
   - Recompute the flattened array **incrementally** on state changes (expansion, filter).
   - Avoid deep recursion in the render; do it once in a derived step.

3. **Immutable updates**
   Tree state should be updated immutably so the view can diff cheap slices and avoid tearing.

4. **Measurement not sleeps**
   No artificial delays. Performance is measured via `performance.mark/measure` (see `$lib/dev/budget.ts`) and asserted in E2E `benchmarks.spec.ts`.

## Recommended Component Shape (Svelte)

```svelte
<script lang="ts">
  import { onMount } from "svelte";
  import { appState } from "$lib/stores/app";

  // derived: flatten( appState.tree, appState.filters )
  // keep the derived calculation in a store or a $derived() (Svelte 5)
</script>

<div
  class="virtual-scroll-viewport"
  on:scroll={/* update start index */}
>
  <div class="spacer" style="height:{totalHeight}px"></div>
  {#each visibleRows as row (row.key)}
    <Row class="virtual-scroll-item" {row} style={`transform: translateY(${row.y}px)`}/>
  {/each}
</div>
```

### Parameters & Knobs

- **Row height**: fixed or measured once per density. Avoid per-row measurement at 25k.
- **Overscan**: default 6–10 rows. Increase for fast wheels; decrease for low-end devices.
- **Key**: use a stable, unique path (no index keys).
- **Max rendered rows**: assert upper bound in tests (`≤ 600` for 25k dataset).

## Filtering & Search

- Apply filter **before** flatten.
- For directory nodes: keep a parent if any descendant matches.
- Maintain selection invariants when filter toggles:

  - A directory is `"full"` only if all descendants are selected.
  - When pruning the view for filter, selection applies to **actual** descendants.

## Benchmarks

We ship three benches in `src/ui/e2e/benchmarks.spec.ts`:

- **Flatten/Apply 25k** — bound initial apply under `BMARK_FLATTEN_MS` (default 1500ms).
- **Filter (\~1k subset)** — apply a representative filter under `BMARK_FILTER_MS` (default 500ms).
- **Scroll jump to end** — simulate a programmatic jump to the end under `BMARK_SCROLL_MS` (default 200ms).

Run via `npm run e2e:bench` (or the VS Code test runner). Thresholds are env-driven.

## Anti-Patterns

- Rendering the full tree (no virtualization).
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
